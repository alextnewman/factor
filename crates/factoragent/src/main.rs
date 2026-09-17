//! factoragent: the session-owning engine process (1:1:1).
//!
//! One process per session: owns the session lock, the SQLite DB, the pwsh
//! host, and the agent loop. Serves a JSON-RPC socket for frontends (`fa32`).
//! The lock is a kernel object (flock here; named mutex on Windows) —
//! no stale locks, no PID-reuse hazards.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use fa_bridge::{HostBridge, SessionEnv};
use fa_core::approver::{ApprovalDecision, ApprovalRequest, Approver, ApproverRef, AutoApprover};
use fa_core::backend::{LlamaCppBackend, LlmBackend, MockBackend};
use fa_core::executor::{ErrorMode, Executor};
use fa_core::manifest::load_manifest;
use fa_core::prompt::{build_block_a, SessionFacts};
use fa_core::rpc::{RpcHandler, RpcPeer, RpcServer};
use fa_core::session::SessionDb;
use fa_core::{agent::AgentLoop, agent::LoopEvent};
use serde_json::{json, Value};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BackendKind {
    Mock,
    Llamacpp,
}

#[derive(Parser)]
#[command(name = "factoragent", about = "FactorAgent session engine")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Own a session: lock, DB, host, loop; serve the frontend socket.
    Serve {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        state_dir: PathBuf,
        #[arg(long)]
        module_dir: PathBuf,
        #[arg(long, value_enum, default_value = "mock")]
        backend: BackendKind,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        llm_url: String,
        #[arg(long, default_value = "default")]
        model: String,
        #[arg(long, default_value_t = false)]
        auto_approve: bool,
        #[arg(long, default_value = "stop")]
        error_mode: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

#[allow(clippy::too_many_lines)]
async fn async_main(cli: Cli) -> Result<()> {
    let Cmd::Serve {
        session_id,
        state_dir,
        module_dir,
        backend,
        llm_url,
        model,
        auto_approve,
        error_mode,
        cwd,
    } = cli.cmd;

    let session_id = session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let session_dir = state_dir.join("sessions").join(&session_id);
    std::fs::create_dir_all(&session_dir)?;
    let lock_path = session_dir.join("session.lock");
    let db_path = session_dir.join("session.db");
    let sock_path = session_dir.join("session.sock");

    // Session lock: kernel-reaped on crash. A second owner fails loudly.
    // (The file stays open for the process lifetime; the lock dies with it.)
    let _lock_guard = {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false) // lock file: never truncate, flock is the mutex
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("open {}", lock_path.display()))?;
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&f);
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("session {session_id} is already owned by another factoragent process");
        }
        f
    };
    tracing::info!(session_id, "session lock acquired");

    let cwd = cwd.unwrap_or_else(|| std::env::current_dir().unwrap());
    let error_mode = ErrorMode::parse(&error_mode);
    let db = Arc::new(SessionDb::open(&db_path)?);
    db.create_session(
        &session_id,
        error_mode.as_str(),
        "host",
        "0.1.0",
        &cwd.to_string_lossy(),
    )?;
    db.set_scope(&session_id, "cwd", &cwd.to_string_lossy())?;
    db.set_scope(&session_id, "note", "prototype session")?;
    db.append_event(
        &session_id,
        "session.started",
        &json!({"backend": format!("{backend:?}")}),
    )?;

    // Spawn the host.
    let bridge_ps1 = module_dir.join("bridge.ps1");
    let session_json = json!({"id": session_id, "mode": "managed", "backend": "host"}).to_string();
    let (mut bridge, cold_start) = HostBridge::spawn(
        &bridge_ps1.to_string_lossy(),
        &SessionEnv {
            session_json,
            error_action: match error_mode {
                ErrorMode::StopAndReport => "Stop".into(),
                ErrorMode::HealAndContinue => "Continue".into(),
            },
            max_terminals: 8,
            cwd: cwd.to_string_lossy().into(),
        },
    )
    .await?;
    tracing::info!("host cold start: {cold_start:?}");

    // The manifest IS the module: reflect once per session, freeze Block A.
    let manifest_value = bridge
        .call("Get-FAToolManifest", &serde_json::Map::new())
        .await?;
    let manifest_str = match &manifest_value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other)?,
    };
    let schemas = load_manifest(&manifest_str)?;
    tracing::info!("manifest: {} tools", schemas.len());
    let block_a = build_block_a(&schemas);

    let backend: Arc<dyn LlmBackend> = match backend {
        BackendKind::Mock => Arc::new(MockBackend::new(vec![])),
        BackendKind::Llamacpp => Arc::new(LlamaCppBackend::new(&llm_url)?),
    };

    // Approval path: auto-approve flag, or ask the connected frontend.
    let socket_approver = Arc::new(SocketApprover::new());
    let approver: ApproverRef = if auto_approve {
        Arc::new(AutoApprover)
    } else {
        socket_approver.clone()
    };
    let executor = Executor::new(
        bridge,
        db.clone(),
        session_id.clone(),
        approver,
        auto_approve,
        error_mode,
    );
    let facts = SessionFacts {
        session_id: session_id.clone(),
        cwd: cwd.to_string_lossy().into(),
        error_mode: error_mode.as_str().into(),
        backend: "host".into(),
        manifest_version: "0.1.0".into(),
        scope_notes: db.scope_all(&session_id)?,
        terminals: vec![],
    };
    let agent = Arc::new(tokio::sync::Mutex::new(AgentLoop::new(
        backend,
        model,
        executor,
        db.clone(),
        session_id.clone(),
        block_a,
        facts,
    )));

    let stopping = Arc::new(AtomicBool::new(false));
    let handler = SessionHandler {
        agent,
        socket_approver,
        db,
        session_id: session_id.clone(),
        stopping: stopping.clone(),
    };
    let server = RpcServer::bind(&sock_path, handler).await?;
    tracing::info!("serving {}", sock_path.display());
    server
        .serve(move || stopping.load(Ordering::SeqCst))
        .await?;
    tracing::info!("session {session_id} shutting down");
    Ok(())
}

/// Approver that asks the connected frontend over the socket.
/// Bound to one connection for the duration of a prompt.
struct SocketApprover {
    binding: Mutex<Option<ApproverBinding>>,
}

#[derive(Clone)]
struct ApproverBinding {
    peer: RpcPeer,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>>,
}

impl SocketApprover {
    fn new() -> Self {
        Self {
            binding: Mutex::new(None),
        }
    }

    fn bind(&self, binding: ApproverBinding) {
        *self.binding.lock().unwrap() = Some(binding);
    }

    fn unbind(&self) {
        *self.binding.lock().unwrap() = None;
    }

    fn resolve(&self, approval_id: &str, decision: ApprovalDecision) {
        let binding = self.binding.lock().unwrap().clone();
        if let Some(b) = binding {
            if let Some(tx) = b.pending.lock().unwrap().remove(approval_id) {
                let _ = tx.send(decision);
            }
        }
    }
}

impl Approver for SocketApprover {
    fn decide<'a>(
        &'a self,
        req: &'a ApprovalRequest,
    ) -> Pin<Box<dyn Future<Output = fa_core::Result<ApprovalDecision>> + Send + 'a>> {
        Box::pin(async move {
            let binding = self.binding.lock().unwrap().clone().ok_or_else(|| {
                fa_core::FaError::Other("no frontend connected for approval".into())
            })?;
            let approval_id = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = oneshot::channel();
            binding
                .pending
                .lock()
                .unwrap()
                .insert(approval_id.clone(), tx);
            let chain: Vec<Value> = req
                .chain
                .iter()
                .map(|c| json!({"name": c.name, "args": c.args}))
                .collect();
            binding
                .peer
                .notify(
                    "event.approval_requested",
                    json!({
                        "approvalId": approval_id,
                        "chain": chain,
                        "previews": req.previews,
                    }),
                )
                .await?;
            // No timeout: blocking over deadlines. The frontend resolves.
            rx.await
                .map_err(|_| fa_core::FaError::Rpc("frontend disconnected during approval".into()))
        })
    }
}

struct SessionHandler {
    agent: Arc<tokio::sync::Mutex<AgentLoop>>,
    socket_approver: Arc<SocketApprover>,
    db: Arc<SessionDb>,
    session_id: String,
    stopping: Arc<AtomicBool>,
}

impl RpcHandler for SessionHandler {
    fn on_request<'a>(
        &'a self,
        method: &'a str,
        params: Value,
        peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Value, String>> + Send + 'a>> {
        let method = method.to_string();
        let peer = peer.clone();
        let agent = self.agent.clone();
        let socket_approver = self.socket_approver.clone();
        let stopping = self.stopping.clone();
        let db = self.db.clone();
        let session_id = self.session_id.clone();
        Box::pin(async move {
            match method.as_str() {
                "session.prompt" => {
                    let text = params
                        .get("text")
                        .and_then(|t| t.as_str())
                        .ok_or("session.prompt needs {text}")?
                        .to_string();
                    // Bind this connection as the approval frontend for the prompt.
                    let pending: Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>> =
                        Arc::new(Mutex::new(HashMap::new()));
                    socket_approver.bind(ApproverBinding {
                        peer: peer.clone(),
                        pending,
                    });
                    let outcome = {
                        let mut agent = agent.lock().await;
                        let r = agent
                            .run_prompt(&text, &|ev| emit_to_peer(&peer, ev))
                            .await
                            .map_err(|e| e.to_string());
                        socket_approver.unbind();
                        r?
                    };
                    db.append_event(
                        &session_id,
                        "prompt.served",
                        &json!({"turnsUsed": outcome.turns_used}),
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(json!({"text": outcome.final_text, "turnsUsed": outcome.turns_used}))
                }
                "session.shutdown" => {
                    stopping.store(true, Ordering::SeqCst);
                    Ok(json!({}))
                }
                _ => Err(format!("unknown method {method}")),
            }
        })
    }

    fn on_notification<'a>(
        &'a self,
        method: &'a str,
        params: Value,
        _peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let method = method.to_string();
        let approver = self.socket_approver.clone();
        Box::pin(async move {
            if method == "event.approval_resolved" {
                let id = params
                    .get("approvalId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let decision = match params.get("decision").and_then(|v| v.as_str()) {
                    Some("deny") => ApprovalDecision::Deny,
                    Some("edit") => {
                        let args = params
                            .get("args")
                            .and_then(|v| v.as_object())
                            .cloned()
                            .unwrap_or_default();
                        ApprovalDecision::Edit(args)
                    }
                    _ => ApprovalDecision::Approve,
                };
                approver.resolve(&id, decision);
            }
        })
    }
}

fn emit_to_peer(peer: &RpcPeer, ev: LoopEvent) {
    let peer = peer.clone();
    tokio::spawn(async move {
        let (method, params) = match ev {
            LoopEvent::AgentText(t) => ("event.agent_text", json!({"text": t})),
            LoopEvent::ToolCalls(calls) => (
                "event.tool_call",
                json!({"calls": calls.iter().map(|c| json!({"name": c.name, "args": c.args})).collect::<Vec<_>>()}),
            ),
            LoopEvent::ToolResult(r) => (
                "event.tool_result",
                json!({"name": r.call.name, "ok": r.ok, "error": r.error, "duration_ms": r.duration_ms}),
            ),
            LoopEvent::Warning(w) => ("event.warning", json!({"text": w})),
            LoopEvent::ModelUsage {
                prompt_tokens,
                completion_tokens,
                cached_tokens,
                latency_ms,
                server_prompt_ms,
            } => (
                "event.model_usage",
                json!({"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens,
                       "cached_tokens": cached_tokens, "latency_ms": latency_ms,
                       "server_prompt_ms": server_prompt_ms}),
            ),
        };
        let _ = peer.notify(method, params).await;
    });
}
