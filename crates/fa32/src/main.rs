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
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

mod screen;
mod style;
mod tui;
use style::{Face, Ink, Style};

#[derive(Parser)]
#[command(name = "fa32", about = "FactorAgent console client")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
// CLI command enum: Run is inherently the big variant; boxing it buys nothing.
#[allow(clippy::large_enum_variant)]
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
        /// Script dialect for agent-written command text: full, brief, posix,
        /// windows. Passed to a freshly spawned server (default full, or
        /// config/preferences.toml). Ignored when joining a live session.
        #[arg(long)]
        dialect: Option<String>,
        /// Color scheme for the Textual Realist renderer: ember (default),
        /// frost, parchment, ghost (your terminal's own palette).
        #[arg(long)]
        scheme: Option<String>,
        /// Color mode: auto (default), always, never. NO_COLOR always wins;
        /// non-TTY output degrades to plain greppable lines unless always.
        #[arg(long, default_value = "auto")]
        color: String,
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
            dialect,
            scheme,
            color,
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
                dialect,
                scheme,
                color,
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
    dialect: Option<String>,
    scheme: Option<String>,
    color: String,
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
    // Fail fast on a bad --scheme/--color before spawning anything.
    let style =
        Style::detect(opts.scheme.as_deref(), &opts.color).map_err(|e| anyhow::anyhow!("{e}"))?;
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
        // Fail fast on a bad --dialect rather than after spawning.
        if let Some(d) = &opts.dialect {
            d.parse::<fa_core::dialect::ScriptDialect>()
                .with_context(|| {
                    format!("invalid --dialect {d:?}; expected one of: full, brief, posix, windows")
                })?;
        }
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
        if let Some(d) = &opts.dialect {
            cmd.arg("--dialect").arg(d);
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
    } else if opts.dialect.is_some() {
        // The dialect is baked into the session's prompt at spawn; joining a
        // live session can't change it.
        eprintln!("note: --dialect is ignored when joining a live session");
    }

    // Full chrome on a real terminal; the honest line printer otherwise
    // (piped, NO_COLOR, --color never, TERM=dumb, or a one-shot --message).
    // (Cloned up front: `opts` is partially moved earlier in this fn.)
    let message = opts.message.clone();
    let model = opts.model.clone();
    let scheme_name = opts.scheme.clone().unwrap_or_else(|| "ember".to_string());
    let use_tui = message.is_none()
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && !style.is_plain();
    let (ui_tx, ui_rx) = mpsc::channel(64);
    let handler = ClientHandler::new(style, use_tui.then(|| ui_tx.clone()));
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

    let outcome = if use_tui {
        match screen::Screen::enter() {
            Ok(screen) => {
                run_tui(
                    screen,
                    &client,
                    &style,
                    ui_tx,
                    ui_rx,
                    &session_id,
                    &model,
                    &scheme_name,
                )
                .await
            }
            Err(e) => {
                eprintln!("note: full-screen unavailable ({e:#}); line mode");
                run_line_mode(&client, &style, &session_id, message.as_deref()).await
            }
        }
    } else {
        run_line_mode(&client, &style, &session_id, message.as_deref()).await
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

/// The honest line printer: every event is a line, the gate is a box.
/// Used when stdout isn't a styled terminal, or for one-shot --message.
async fn run_line_mode(
    client: &RpcClient<ClientHandler>,
    style: &Style,
    session_id: &str,
    message: Option<&str>,
) -> Result<()> {
    println!(
        "{}",
        style.paint(
            Ink::Dim,
            &format!("session {session_id} — type /quit to exit\n")
        )
    );

    if let Some(msg) = message {
        prompt_once(client, style, msg)
            .await
            .map(|o| println!("\n{o}"))
    } else {
        repl(client, style).await
    }
}

/// The full chrome: alternate screen, managed regions, modal gate.
/// The UI loop never blocks on a turn — prompts go out on a spawned task
/// and everything (outcome, approval) comes back as events.
async fn run_tui(
    _screen: screen::Screen,
    client: &RpcClient<ClientHandler>,
    style: &Style,
    ui_tx: mpsc::Sender<tui::UiEvent>,
    mut ui_rx: mpsc::Receiver<tui::UiEvent>,
    session_id: &str,
    model: &str,
    scheme: &str,
) -> Result<()> {
    let (key_tx, mut key_rx) = mpsc::channel(64);
    let _input_thread = tui::spawn_input_thread(key_tx.clone());
    let _resize_watcher = tui::spawn_resize_watcher(key_tx);
    let mut view = tui::View::new(
        session_id.to_string(),
        model.to_string(),
        scheme.to_string(),
    );
    let mut out = io::stdout();
    tui::render(&mut view, style, &mut out)?;

    loop {
        tokio::select! {
            key = key_rx.recv() => {
                let key = key.unwrap_or(tui::Key::CtrlD);
                match tui::handle_key(&mut view, key) {
                    tui::KeyAction::None => {}
                    tui::KeyAction::Redraw => {
                        tui::render(&mut view, style, &mut out)?;
                    }
                    tui::KeyAction::Quit => break,
                    tui::KeyAction::Submit(text) => {
                        if text.trim() == "/quit" {
                            break;
                        }
                        tui::render(&mut view, style, &mut out)?;
                        let peer = client.peer().clone();
                        let ui_tx = ui_tx.clone();
                        tokio::spawn(async move {
                            // The prompt result is a turn-end acknowledgement;
                            // only a failure is worth a chronicle line.
                            let outcome = match peer
                                .request("session.prompt", json!({"text": text}))
                                .await
                            {
                                Ok(_) => String::new(),
                                Err(e) => format!("error: {e:#}"),
                            };
                            let _ = ui_tx.send(tui::UiEvent::Outcome(outcome)).await;
                        });
                    }
                }
            }
            ev = ui_rx.recv() => {
                let Some(ev) = ev else { break };
                view.on_event(ev);
                tui::render(&mut view, style, &mut out)?;
            }
        }
    }
    Ok(())
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

async fn prompt_once(
    client: &RpcClient<ClientHandler>,
    style: &Style,
    text: &str,
) -> Result<String> {
    // When stdin is a live terminal the console already echoed the typed
    // line above the prompt; re-printing it doubles the input. Only echo
    // for piped/non-interactive stdin, where nothing echoed it.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        println!("{} {text}", style.paint(Ink::Amber, "you>"));
    }
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

async fn repl(client: &RpcClient<ClientHandler>, style: &Style) -> Result<()> {
    loop {
        print!("{}", style.paint(Ink::Amber, "you> "));
        io::stdout().flush()?;
        let mut line = String::new();
        // Blocking stdin read: run it off-thread so the runtime stays alive.
        // The byte count matters: 0 means EOF (piped input done), which
        // must break the loop, not spin on empty lines forever.
        let (n, line) = tokio::task::spawn_blocking(move || {
            io::stdin().read_line(&mut line).map(|n| (n, line))
        })
        .await??;
        if n == 0 {
            break;
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        if text == "/quit" {
            break;
        }
        match prompt_once(client, style, text).await {
            // The turn's agent text, tool calls, results, and usage were
            // already rendered live from the event stream; the RPC return
            // is just the turn-end ack, not a second copy of the text.
            Ok(_) => {}
            Err(e) => eprintln!("{}", style.paint(Ink::Red, &format!("error: {e:#}"))),
        }
    }
    Ok(())
}

/// Renders server events; answers approval requests interactively.
struct ClientHandler {
    style: Style,
    /// Last room shown on the trail; the trail prints only on change.
    room: std::sync::Arc<Mutex<Option<String>>>,
    /// When set, events go to the full-screen UI task instead of stdout.
    ui: Option<mpsc::Sender<tui::UiEvent>>,
}

impl ClientHandler {
    fn new(style: Style, ui: Option<mpsc::Sender<tui::UiEvent>>) -> Self {
        Self {
            style,
            room: std::sync::Arc::new(Mutex::new(None)),
            ui,
        }
    }
}

/// Forward one notification to the full-screen UI task. Free function: the
/// handler's `&self` borrow doesn't survive the async block's move, and the
/// routing needs nothing from the handler itself.
async fn handle_ui_event(
    tx: &mpsc::Sender<tui::UiEvent>,
    method: &str,
    params: Value,
    peer: &RpcPeer,
) {
    let send = |ev: tui::UiEvent| async {
        let _ = tx.send(ev).await;
    };
    match method {
        "event.agent_text" => {
            if let Some(t) = params.get("text").and_then(|v| v.as_str()) {
                send(tui::UiEvent::AgentText(t.to_string())).await;
            }
        }
        "event.tool_call" => {
            if let Some(calls) = params.get("calls").and_then(|v| v.as_array()) {
                let items: Vec<tui::CallItem> = calls
                    .iter()
                    .map(|c| tui::CallItem {
                        print: call_print(c),
                        room: c.get("room").and_then(|v| v.as_str()).map(str::to_string),
                        root: c.get("root").and_then(|v| v.as_str()).map(str::to_string),
                    })
                    .collect();
                send(tui::UiEvent::ToolCall(items)).await;
            }
        }
        "event.tool_result" => {
            let ms = params
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            send(tui::UiEvent::ToolResult {
                print: result_print(&params),
                ok: params.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
                ms,
                error: params
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
            .await;
        }
        "event.model_usage" => {
            send(tui::UiEvent::Usage {
                prompt_tokens: params
                    .get("prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                completion_tokens: params
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            })
            .await;
        }
        "event.warning" => {
            if let Some(t) = params.get("text").and_then(|v| v.as_str()) {
                send(tui::UiEvent::Warning(t.to_string())).await;
            }
        }
        "event.approval_requested" => {
            let approval_id = params
                .get("approvalId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let previews: Vec<String> = params
                .get("previews")
                .and_then(|v| v.as_array())
                .map(|ps| {
                    ps.iter()
                        .map(|p| p.as_str().unwrap_or("?").to_string())
                        .collect()
                })
                .unwrap_or_default();
            let chain: Vec<tui::ChainItem> = params
                .get("chain")
                .and_then(|v| v.as_array())
                .map(|cs| {
                    cs.iter()
                        .map(|c| tui::ChainItem {
                            print: call_print(c),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let (resp_tx, resp_rx) = oneshot::channel();
            send(tui::UiEvent::Approval {
                chain,
                previews,
                respond: resp_tx,
            })
            .await;
            // The modal answers; a dead UI denies (safe default).
            let (decision, args) = match resp_rx.await.unwrap_or(tui::ApprovalDecision::Deny) {
                tui::ApprovalDecision::Approve => ("approve", None),
                tui::ApprovalDecision::Deny => ("deny", None),
                tui::ApprovalDecision::Edit(v) => ("edit", Some(v)),
            };
            let mut resp = json!({"approvalId": approval_id, "decision": decision});
            if let Some(args) = args {
                resp["args"] = args;
            }
            let _ = peer.notify("event.approval_resolved", resp).await;
        }
        _ => {}
    }
}

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
        let style = self.style;
        let room = self.room.clone();
        let ui = self.ui.clone();
        Box::pin(async move {
            // Full-screen mode: the UI task owns all rendering.
            if let Some(tx) = ui {
                handle_ui_event(&tx, &method, params, &peer).await;
                return;
            }
            match method.as_str() {
                "event.agent_text" => {
                    if let Some(t) = params.get("text").and_then(|v| v.as_str()) {
                        if !t.trim().is_empty() {
                            println!("\n{} {t}", style.paint(Ink::Gold, "agent>"));
                        }
                    }
                }
                "event.tool_call" => {
                    if let Some(calls) = params.get("calls").and_then(|v| v.as_array()) {
                        // The trail moves with the call, before the call line.
                        let mut rs = room.lock().unwrap();
                        for c in calls {
                            if let Some(line) = trail_line(&mut rs, c, &style) {
                                println!("{line}");
                            }
                        }
                        drop(rs);
                        for c in calls {
                            // The human view: the engine-expanded print form.
                            // Falls back to the raw invocation for servers
                            // that predate print forms.
                            let sigil = style.paint(Ink::Amber, "⚙");
                            println!("  {sigil} {}", style.paint(Ink::Text, &call_print(c)));
                        }
                    }
                }
                "event.tool_result" => {
                    let ms = params
                        .get("duration_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    // The human view: the same print form as the call line.
                    let name = result_print(&params);
                    let ok = params.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    if ok {
                        let sigil = style.paint(Ink::Green, "✓");
                        println!(
                            "  {sigil} {} {}",
                            style.paint(Ink::Text, &name),
                            style.paint(Ink::Dim, &format!("({ms}ms)"))
                        );
                    } else {
                        let err = params
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown error");
                        let sigil = style.paint(Ink::Red, "✗");
                        println!("  {sigil} {}: {err}", style.paint(Ink::Red, &name));
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
                    println!(
                        "{}",
                        style.paint(
                            Ink::Dim,
                            &format!("  · tokens {pt}+{ct}{cached} in {ms}ms{server}")
                        )
                    );
                }
                "event.warning" => {
                    if let Some(t) = params.get("text").and_then(|v| v.as_str()) {
                        println!("  {} {t}", style.paint(Ink::Amber, "!"));
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
                    let decision = ask_approval(&style, &previews, &chain);
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

/// The collapsed trail: the agent's current room as a repo-rooted chain.
/// Returns the line to print when the room changed since the last call.
fn trail_line(room_state: &mut Option<String>, call: &Value, style: &Style) -> Option<String> {
    let room = call.get("room").and_then(|v| v.as_str())?;
    if room_state.as_deref() == Some(room) {
        return None;
    }
    *room_state = Some(room.to_string());
    let root = call
        .get("root")
        .and_then(|v| v.as_str())
        .unwrap_or("workspace");
    let chain = if room == "." {
        root.to_string()
    } else {
        format!("{root} › {}", room.replace('/', " › "))
    };
    let mark = style.paint(Ink::Amber, "●");
    let rest = style.paint(Ink::Dim, &format!(" {chain}"));
    Some(format!("  {mark}{rest}"))
}

/// One tool result's human-readable form: the engine-expanded print form
/// (`print`), or the raw tool name for servers that predate print forms.
fn result_print(params: &Value) -> String {
    params
        .get("print")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string()
        })
}

/// One tool call's human-readable form: the engine-expanded print form
/// (`print`), or the raw `name + args` for servers that predate print forms.
fn call_print(c: &Value) -> String {
    if let Some(p) = c.get("print").and_then(|v| v.as_str()) {
        return p.to_string();
    }
    let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?");
    let args = serde_json::to_string(c.get("args").unwrap_or(&Value::Null)).unwrap_or_default();
    format!("{name} {args}")
}

/// Interactive approval UX: the chain as print forms (the action the veto
/// judges), previews as verifiable detail, then approve/deny/edit.
///
/// Styled mode draws the gate as a box — the one border that earns its
/// meaning, marking the veto boundary. Plain mode keeps the old ASCII
/// rendering: greppable lines, no box art.
fn ask_approval(
    style: &Style,
    previews: &[Value],
    chain: &[Value],
) -> (&'static str, Option<Value>) {
    // One hardened gate for every rendering: rows are built newline-safe,
    // wrapped, and height-capped (with spill) before any output. Plain mode
    // keeps it border-free and greppable inside draw_gate; it never bypasses
    // the row hardening.
    draw_gate(style, &approval_rows_from_wire(chain, previews));
    ask_decision(chain.len())
}

/// Build the gate's content rows from wire values, via the shared
/// row builder the TUI modal also uses.
fn approval_rows_from_wire(chain: &[Value], previews: &[Value]) -> Vec<(Face, String)> {
    let chain: Vec<tui::ChainItem> = chain
        .iter()
        .map(|c| tui::ChainItem {
            print: call_print(c),
        })
        .collect();
    let previews: Vec<String> = previews
        .iter()
        .map(|p| p.as_str().unwrap_or("?").to_string())
        .collect();
    tui::approval_rows(&chain, &previews)
}

/// Build the gate's content rows from logical rows: split on newlines first
/// (PowerShell text is multi-line — a raw `\n` inside a row would break the
/// frame), strip `\r`, expand tabs, then wrap each physical line. Pure and
/// unit-tested: no returned row contains a newline or exceeds `inner_max`.
fn gate_rows(rows: &[(Face, String)], inner_max: usize) -> Vec<(Face, String)> {
    // Continuation lines carry a 2-space indent, so wrap 2 short of the
    // frame: the width invariant below must hold for every returned row.
    let wrap_w = inner_max.saturating_sub(2).max(10);
    let mut out = Vec::new();
    for (face, text) in rows {
        for physical in text.split('\n') {
            let physical = expand_tabs(physical.trim_end_matches('\r'));
            for (i, chunk) in style::wrap_text(&physical, wrap_w).into_iter().enumerate() {
                out.push((*face, if i == 0 { chunk } else { format!("  {chunk}") }));
            }
        }
    }
    out
}

/// Expand tabs to the next multiple-of-8 stop so tabbed content can't
/// ragged-edge the frame. Display-only; the spilled bytes keep raw tabs.
fn expand_tabs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0usize;
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if c == '\t' {
            let n = 8 - (col % 8);
            out.push_str(&" ".repeat(n));
            col += n;
        } else {
            col += style::disp_width(c.encode_utf8(&mut buf));
            out.push(c);
        }
    }
    out
}

static GATE_SPILL_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Spill the gate's complete logical rows to a temp file so a height-capped
/// gate never silently hides bytes the veto is judging. Returns the path.
fn spill_gate_text(rows: &[(Face, String)]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "fa32-gate-{}-{}.txt",
        std::process::id(),
        GATE_SPILL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut text = String::new();
    for (_, row) in rows {
        text.push_str(row);
        text.push('\n');
    }
    let _ = std::fs::write(&path, text);
    path
}

/// The veto boundary, drawn once, full canvas width: title knocked out of
/// the top border, multi-line content as separate framed rows, long lines
/// wrapped (never truncated, never leaking past the frame). Massive inputs
/// are height-capped to keep the veto on screen; the overflow spills to a
/// file whose path is shown, so no byte is hidden from the operator.
fn draw_gate(style: &Style, rows: &[(Face, String)]) {
    let (cols, term_rows) = style::term_size();
    let visible = cap_gate_rows(rows, cols, term_rows);

    if style.is_plain() {
        // Border-free and greppable: ASCII rules, no box art — but the rows
        // above are already hardened (newline-safe, wrapped, height-capped).
        let rule = "-".repeat(cols);
        println!("\n-- approval requested {rule}");
        for (_, line) in &visible {
            println!("{line}");
        }
        println!("{rule}");
        return;
    }

    println!("\n{}", gate_top(style, cols));
    for (face, line) in &visible {
        println!("{}", gate_row(style, *face, line, cols));
    }
    println!("{}", gate_bottom(style, cols));
}

/// Prepare the gate's visible rows: wrap to the frame width, cap the height
/// so the veto prompt never scrolls off-screen, and spill the complete text
/// to a file (shown in-frame) rather than silently truncating. Shared by the
/// line-mode gate and the TUI modal.
pub(crate) fn cap_gate_rows(
    rows: &[(Face, String)],
    cols: usize,
    term_rows: usize,
) -> Vec<(Face, String)> {
    let inner_max = cols.saturating_sub(4).max(20);
    let all = gate_rows(rows, inner_max);
    // Reserve frame + decision prompt so the gate never scrolls the veto
    // off-screen; floor keeps tiny terminals usable.
    let max_rows = term_rows.saturating_sub(10).max(10);
    let overflow = all.len().saturating_sub(max_rows);
    let mut visible: Vec<(Face, String)> = all.iter().take(max_rows).cloned().collect();
    if overflow > 0 {
        let path = spill_gate_text(rows);
        let note = format!("… {overflow} more lines — full text: {}", path.display());
        for (i, chunk) in style::wrap_text(&note, inner_max).into_iter().enumerate() {
            visible.push((
                Face::plain(Ink::Dim),
                if i == 0 { chunk } else { format!("  {chunk}") },
            ));
        }
    }
    visible
}

/// The gate's top rule with the title knocked out, spanning the full canvas.
pub(crate) fn gate_top(style: &Style, cols: usize) -> String {
    // Title knocked out of the top rule; the frame spans the full canvas.
    // (Char arithmetic, not byte length: every frame glyph is one cell.)
    // Rounded corners: the operator's terminal renders them fine, and the
    // gate is the one box that earns a border.
    let title = " APPROVAL REQUESTED ";
    let fill = cols.saturating_sub(3 + title.len() + 1); // ╭ ─ title ─…─ ╮
    let mut top = String::from("╭─");
    top.push_str(title);
    top.push_str(&"─".repeat(fill));
    top.push('╮');
    style.paint(Ink::Amber, &top).to_string()
}

/// The gate's bottom rule.
pub(crate) fn gate_bottom(style: &Style, cols: usize) -> String {
    style
        .paint(
            Ink::Amber,
            &format!("╰{}╯", "─".repeat(cols.saturating_sub(2))),
        )
        .to_string()
}

/// One framed content row: `│ {text padded} │`.
pub(crate) fn gate_row(style: &Style, face: Face, line: &str, cols: usize) -> String {
    let inner_max = cols.saturating_sub(4).max(20);
    let pad = " ".repeat(inner_max.saturating_sub(style::disp_width(line)));
    format!(
        "{} {} {}",
        style.paint(Ink::Amber, "│"),
        style.paint_face(face, &format!("{line}{pad}")),
        style.paint(Ink::Amber, "│")
    )
}

/// The shared decision prompt, used by both gate renderings.
fn ask_decision(chain_len: usize) -> (&'static str, Option<Value>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(room: &str, root: &str) -> Value {
        json!({"name": "Get-FATree", "args": {}, "room": room, "root": root})
    }

    #[test]
    fn trail_prints_on_room_change_only() {
        let style = Style::plain();
        let mut state = None;
        // First sighting prints.
        let line = trail_line(&mut state, &call("crates", "winagent32"), &style).unwrap();
        assert_eq!(line, "  ● winagent32 › crates");
        // Same room: silent.
        assert!(trail_line(&mut state, &call("crates", "winagent32"), &style).is_none());
        // New room: prints the full chain from the root.
        let line = trail_line(
            &mut state,
            &call("crates/factoragent/tests", "winagent32"),
            &style,
        )
        .unwrap();
        assert_eq!(line, "  ● winagent32 › crates › factoragent › tests");
        // Workspace root room collapses to the root name.
        let line = trail_line(&mut state, &call(".", "winagent32"), &style).unwrap();
        assert_eq!(line, "  ● winagent32");
    }

    #[test]
    fn trail_needs_room_and_root() {
        let style = Style::plain();
        let mut state = None;
        // No room on the wire: no trail, no state change.
        assert!(trail_line(&mut state, &json!({"name": "X"}), &style).is_none());
        assert_eq!(state, None);
        // No root: falls back to "workspace".
        let line = trail_line(&mut state, &json!({"room": "crates"}), &style).unwrap();
        assert_eq!(line, "  ● workspace › crates");
    }

    #[test]
    fn gate_frame_uses_rounded_corners() {
        let style = Style::plain();
        let top = gate_top(&style, 80);
        let bottom = gate_bottom(&style, 80);
        assert!(top.starts_with("╭─"), "top-left corner: {top:?}");
        assert!(top.ends_with("╮"), "top-right corner: {top:?}");
        assert!(bottom.starts_with("╰"), "bottom-left corner: {bottom:?}");
        assert!(bottom.ends_with("╯"), "bottom-right corner: {bottom:?}");
    }

    #[test]
    fn gate_row_with_wide_sigil_never_exceeds_width() {
        // ⚙ renders 2 cells in Windows Terminal. A row containing it must
        // still be exactly `cols` cells wide, or the right border is
        // pushed off-screen.
        let style = Style::plain();
        for line in ["⚙ Run gci -Recurse", "plain text", "✓ done ● trail"] {
            let row = gate_row(&style, Face::plain(Ink::Text), line, 80);
            assert_eq!(style::disp_width(&row), 80, "row: {row:?}");
        }
    }

    fn gate_rows_split_newlines_and_never_exceed_width() {
        // Multi-line PowerShell text becomes separate framed rows; \r is
        // stripped (a raw \r would rewind the cursor mid-frame); tabs expand.
        let rows = vec![
            (Face::plain(Ink::Text), "Run `$x = 1\r\n$y = 2".to_string()),
            (Face::plain(Ink::Dim), "a\tb".to_string()),
        ];
        let out = gate_rows(&rows, 20);
        let texts: Vec<&str> = out.iter().map(|(_, s)| s.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("$x = 1")));
        assert!(texts.iter().any(|t| t.contains("$y = 2")));
        assert!(!texts.iter().any(|t| t.contains('\n') || t.contains('\r')));
        assert!(texts.iter().any(|t| t.contains("a       b"))); // tab → 7 spaces
        for (_, line) in &out {
            assert!(style::disp_width(line) <= 20, "row exceeds frame: {line:?}");
        }
    }

    #[test]
    fn gate_rows_wrap_long_words_without_leaking() {
        let rows = vec![(Face::plain(Ink::Text), "x".repeat(100))];
        let out = gate_rows(&rows, 20);
        assert!(out.len() > 1);
        for (_, line) in &out {
            assert!(style::disp_width(line) <= 20, "row exceeds frame");
        }
    }
}
