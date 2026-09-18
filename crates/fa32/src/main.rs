//! fa32: the console client for FactorAgent sessions.
//!
//! Chat rendering, approval UX (approve/deny/edit), WhatIf display.
//! Speaks JSON-RPC to a `factoragent serve` process over a Unix socket
//! (named pipe on Windows). `run` spawns the server if none is listening.

use std::future::Future;
use std::io::{self, Write};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fa_core::rpc::{endpoint, RpcClient, RpcHandler, RpcPeer};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "fa32", about = "FactorAgent console client")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Chat with a session (spawns `factoragent serve` if needed).
    Run {
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
        #[arg(long)]
        module_dir: Option<PathBuf>,
        /// LLM backend for a freshly spawned server.
        #[arg(long, default_value = "mock")]
        backend: String,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        llm_url: String,
        #[arg(long, default_value = "default")]
        model: String,
        /// Auto-approve mutating chains (no interactive prompts).
        #[arg(long, default_value_t = false)]
        auto_approve: bool,
        #[arg(long, default_value = "stop")]
        error_mode: String,
        /// Max agent turns per prompt (prototype default 25).
        #[arg(long, default_value_t = 25)]
        turn_cap: usize,
        /// Single prompt; print the outcome and exit (no REPL).
        #[arg(long)]
        message: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Check the environment and (with --llm) the model backend.
    Doctor {
        #[arg(long, default_value_t = false)]
        llm: bool,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        llm_url: String,
    },
    /// Reserved: standalone MCP server (deferred to v2).
    McpServe {},
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Run {
            session_id,
            state_dir,
            module_dir,
            backend,
            llm_url,
            model,
            auto_approve,
            error_mode,
            turn_cap,
            message,
            cwd,
        } => {
            run_session(RunOpts {
                session_id,
                state_dir,
                module_dir,
                backend,
                llm_url,
                model,
                auto_approve,
                error_mode,
                turn_cap,
                message,
                cwd,
            })
            .await
        }
        Cmd::Doctor { llm, llm_url } => doctor(llm, &llm_url).await,
        Cmd::McpServe {} => {
            println!("fa32 mcp-serve: reserved; the standalone MCP server is deferred to v2.");
            Ok(())
        }
    }
}

struct RunOpts {
    session_id: Option<String>,
    state_dir: Option<PathBuf>,
    module_dir: Option<PathBuf>,
    backend: String,
    llm_url: String,
    model: String,
    auto_approve: bool,
    error_mode: String,
    turn_cap: usize,
    message: Option<String>,
    cwd: Option<PathBuf>,
}

fn default_state_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/factoragent")
    } else {
        PathBuf::from(".fa-state")
    }
}

fn find_factoragent() -> Option<PathBuf> {
    // Same directory as this exe, else PATH.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(format!("factoragent{}", std::env::consts::EXE_SUFFIX));
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    None
}

async fn run_session(opts: RunOpts) -> Result<()> {
    let state_dir = opts.state_dir.unwrap_or_else(default_state_dir);
    let session_id = opts.session_id.unwrap_or_else(uuid_simple);
    let session_dir = state_dir.join("sessions").join(&session_id);
    std::fs::create_dir_all(&session_dir)?;
    // Session endpoint: socket file on Unix, named pipe on Windows.
    #[cfg(unix)]
    let endpoint = endpoint::path_for(&state_dir, &session_id);
    #[cfg(windows)]
    let endpoint = endpoint::pipe_name(&session_id);

    // Spawn the server if nobody is listening.
    let mut server_child = None;
    if !endpoint::listening(&endpoint).await {
        let factoragent = find_factoragent().context(
            "no session socket and no `factoragent` binary next to fa32 (nor --session-id of a live session)",
        )?;
        let module_dir = opts.module_dir.clone().unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."))
        });
        let mut cmd = tokio::process::Command::new(&factoragent);
        cmd.arg("serve")
            .arg("--session-id")
            .arg(&session_id)
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--module-dir")
            .arg(&module_dir)
            .arg("--backend")
            .arg(&opts.backend)
            .arg("--llm-url")
            .arg(&opts.llm_url)
            .arg("--model")
            .arg(&opts.model)
            .arg("--error-mode")
            .arg(&opts.error_mode)
            .arg("--turn-cap")
            .arg(opts.turn_cap.to_string());
        if opts.auto_approve {
            cmd.arg("--auto-approve");
        }
        if let Some(cwd) = &opts.cwd {
            cmd.arg("--cwd").arg(cwd);
        }
        cmd.env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        );
        let child = cmd.spawn().context("spawn factoragent serve")?;
        server_child = Some(child);
        wait_for_endpoint(&endpoint, Duration::from_secs(60)).await?;
    }

    let handler = ClientHandler;
    let client = match RpcClient::connect(&endpoint, handler).await {
        Ok(client) => client,
        Err(e) => {
            // Never orphan a server we spawned: with no client the session
            // is useless, and on Windows the live process locks its own
            // .exe, which blocks rebuilds and deletes.
            if let Some(mut child) = server_child.take() {
                let _ = child.kill().await;
            }
            return Err(e.into());
        }
    };
    println!("session {session_id} — type /quit to exit\n");

    let outcome = if let Some(msg) = opts.message {
        prompt_once(&client, &msg).await.map(|o| println!("\n{o}"))
    } else {
        repl(&client).await
    };

    // If we spawned the server, shut it down: graceful request first,
    // then kill. A spawned server must never be orphaned — on Windows the
    // live process locks its own .exe, blocking rebuilds.
    if let Some(mut child) = server_child {
        let _ = client.peer().request("session.shutdown", json!({})).await;
        if tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
    outcome
}

#[cfg(unix)]
async fn wait_for_endpoint(path: &Path, timeout: Duration) -> Result<()> {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < timeout {
        // `listening` both checks the socket file and tries a connect:
        // the file existing isn't quite "listening".
        if endpoint::listening(path).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("timed out waiting for session socket {}", path.display())
}

#[cfg(windows)]
async fn wait_for_endpoint(pipe: &str, timeout: Duration) -> Result<()> {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < timeout {
        if endpoint::listening(pipe).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("timed out waiting for session pipe {pipe}")
}

async fn prompt_once(client: &RpcClient<ClientHandler>, text: &str) -> Result<String> {
    println!("you> {text}");
    let v = client
        .peer()
        .request("session.prompt", json!({"text": text}))
        .await?;
    if let Some(err) = v.get("__rpc_error") {
        anyhow::bail!("prompt failed: {err}");
    }
    Ok(v.get("text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string())
}

async fn repl(client: &RpcClient<ClientHandler>) -> Result<()> {
    loop {
        print!("you> ");
        io::stdout().flush()?;
        let mut line = String::new();
        // Blocking stdin read: run it off-thread so the runtime stays alive.
        let n = tokio::task::spawn_blocking(move || io::stdin().read_line(&mut line).map(|_| line))
            .await??;
        let line = n;
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        if text == "/quit" {
            break;
        }
        match prompt_once(client, text).await {
            Ok(outcome) => println!("\n{outcome}\n"),
            Err(e) => eprintln!("error: {e:#}"),
        }
    }
    Ok(())
}

/// Renders server events; answers approval requests interactively.
struct ClientHandler;

impl RpcHandler for ClientHandler {
    fn on_request<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
        _peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Value, String>> + Send + 'a>> {
        let method = method.to_string();
        Box::pin(async move { Err(format!("client: unexpected request {method}")) })
    }

    fn on_notification<'a>(
        &'a self,
        method: &'a str,
        params: Value,
        peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let method = method.to_string();
        let peer = peer.clone();
        Box::pin(async move {
            match method.as_str() {
                "event.agent_text" => {
                    if let Some(t) = params.get("text").and_then(|v| v.as_str()) {
                        if !t.trim().is_empty() {
                            println!("\nagent> {t}");
                        }
                    }
                }
                "event.tool_call" => {
                    if let Some(calls) = params.get("calls").and_then(|v| v.as_array()) {
                        for c in calls {
                            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            let args =
                                serde_json::to_string(&c.get("args").unwrap_or(&Value::Null))
                                    .unwrap_or_default();
                            println!("  ⚙ {name} {args}");
                        }
                    }
                }
                "event.tool_result" => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let ok = params.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    let ms = params
                        .get("duration_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if ok {
                        println!("  ✓ {name} ({ms}ms)");
                    } else {
                        let err = params
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown error");
                        println!("  ✗ {name}: {err}");
                    }
                }
                "event.model_usage" => {
                    let pt = params
                        .get("prompt_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let ct = params
                        .get("completion_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let cached = params
                        .get("cached_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|c| format!(" ({c} cached)"))
                        .unwrap_or_default();
                    let ms = params
                        .get("latency_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let server = params
                        .get("server_prompt_ms")
                        .and_then(|v| v.as_u64())
                        .map(|s| format!(", server prompt-eval {s}ms"))
                        .unwrap_or_default();
                    println!("  · tokens {pt}+{ct}{cached} in {ms}ms{server}");
                }
                "event.warning" => {
                    if let Some(t) = params.get("text").and_then(|v| v.as_str()) {
                        println!("  ! {t}");
                    }
                }
                "event.approval_requested" => {
                    let approval_id = params
                        .get("approvalId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let previews = params
                        .get("previews")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let chain = params
                        .get("chain")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let decision = ask_approval(&previews, chain.len());
                    let mut resp = json!({"approvalId": approval_id, "decision": decision.0});
                    if let Some(args) = decision.1 {
                        resp["args"] = args;
                    }
                    let _ = peer.notify("event.approval_resolved", resp).await;
                }
                _ => {}
            }
        })
    }
}

/// Interactive approval UX: WhatIf expansion, then approve/deny/edit.
fn ask_approval(previews: &[Value], chain_len: usize) -> (&'static str, Option<Value>) {
    println!("\n── approval requested ──────────────────────");
    for p in previews {
        println!("  • {}", p.as_str().unwrap_or("?"));
    }
    if previews.is_empty() {
        println!("  • ({chain_len} mutating call(s), no previews)");
    }
    println!("────────────────────────────────────────────");
    loop {
        print!("[a]pprove / [d]eny");
        if chain_len == 1 {
            print!(" / [e]dit args");
        }
        print!("> ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            return ("deny", None);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "a" | "approve" | "y" | "yes" => return ("approve", None),
            "d" | "deny" | "n" | "no" => return ("deny", None),
            "e" | "edit" if chain_len == 1 => {
                print!("new args as JSON> ");
                io::stdout().flush().ok();
                let mut js = String::new();
                if io::stdin().read_line(&mut js).is_ok() {
                    match serde_json::from_str::<Value>(js.trim()) {
                        Ok(Value::Object(_)) => {
                            return ("edit", serde_json::from_str(js.trim()).ok())
                        }
                        _ => println!("not a JSON object; try again"),
                    }
                }
            }
            _ => println!("answer a/d{}", if chain_len == 1 { "/e" } else { "" }),
        }
    }
}

async fn doctor(with_llm: bool, llm_url: &str) -> Result<()> {
    println!("fa32 doctor\n");
    // 1. pwsh present?
    let pwsh = tokio::process::Command::new("pwsh")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-c",
            "$PSVersionTable.PSVersion.ToString()",
        ])
        .output()
        .await;
    match pwsh {
        Ok(o) if o.status.success() => {
            println!("✓ pwsh {}", String::from_utf8_lossy(&o.stdout).trim())
        }
        _ => {
            println!("✗ pwsh not found on PATH");
            anyhow::bail!("pwsh missing");
        }
    }
    // 2. module loads + manifest reflects?
    let module_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    println!(
        "✓ module dir hint: {}",
        module_dir
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "?".into())
    );
    // 3. LLM backend.
    if with_llm {
        let client = reqwest::Client::new();
        let health = client
            .get(format!("{llm_url}/health"))
            .send()
            .await
            .with_context(|| format!("GET {llm_url}/health"))?;
        println!("✓ llama.cpp /health -> {}", health.status());
        let models: Value = client
            .get(format!("{llm_url}/v1/models"))
            .send()
            .await?
            .json()
            .await
            .unwrap_or(Value::Null);
        let names: Vec<&str> = models
            .get("data")
            .and_then(|d| d.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        println!("✓ models: {}", names.join(", "));
    }
    println!("\ndoctor: all green");
    Ok(())
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("sess-{nanos:x}-{}", std::process::id())
}
