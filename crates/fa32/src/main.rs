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

mod style;
use style::{Ink, Style};

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

    let handler = ClientHandler::new(style);
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
    println!(
        "{}",
        style.paint(
            Ink::Dim,
            &format!("session {session_id} — type /quit to exit\n")
        )
    );

    let outcome = if let Some(msg) = opts.message {
        prompt_once(&client, &style, &msg)
            .await
            .map(|o| println!("\n{o}"))
    } else {
        repl(&client, &style).await
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
}

impl ClientHandler {
    fn new(style: Style) -> Self {
        Self {
            style,
            room: std::sync::Arc::new(Mutex::new(None)),
        }
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
        Box::pin(async move {
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
                    let name = params
                        .get("print")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            params
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?")
                                .to_string()
                        });
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
    if style.is_plain() {
        return ask_approval_plain(previews, chain);
    }
    let mut rows: Vec<(Ink, String)> = Vec::new();
    for c in chain {
        rows.push((Ink::Text, format!("⚙ {}", call_print(c))));
    }
    if !previews.is_empty() {
        rows.push((Ink::Faint, "─ detail ─".to_string()));
        for p in previews {
            rows.push((Ink::Dim, format!("  {}", p.as_str().unwrap_or("?"))));
        }
    }
    if rows.is_empty() {
        rows.push((Ink::Dim, "(nothing to show)".to_string()));
    }
    draw_gate(style, &rows);
    ask_decision(chain.len())
}

/// The veto boundary, drawn once: title knocked out of the top border,
/// complete forms wrapping (never truncating) inside, width-aware.
fn draw_gate(style: &Style, rows: &[(Ink, String)]) {
    let width = style::term_width().min(100);
    let inner_max = width.saturating_sub(4).max(20);
    let mut lines: Vec<(Ink, String)> = Vec::new();
    for (ink, text) in rows {
        for (i, chunk) in style::wrap_text(text, inner_max).into_iter().enumerate() {
            lines.push((*ink, if i == 0 { chunk } else { format!("  {chunk}") }));
        }
    }
    let content_w = lines
        .iter()
        .map(|(_, l)| style::disp_width(l))
        .fold(0, usize::max)
        .min(inner_max);
    let border = |s: &str| style.paint(Ink::Amber, s);
    // The frame is at least wide enough for its own title.
    let total = (content_w + 4).max(style::disp_width(" approval requested ") + 6);
    let mut top = String::from("╭─");
    top.push_str(" approval requested ");
    top.push_str(&"─".repeat(total.saturating_sub(style::disp_width(&top) + 1)));
    top.push('╮');
    println!("\n{}", border(&top));
    for (ink, line) in &lines {
        let pad = " ".repeat(content_w.saturating_sub(style::disp_width(line)));
        println!(
            "{} {} {}",
            border("│"),
            style.paint(*ink, &format!("{line}{pad}")),
            border("│")
        );
    }
    println!(
        "{}",
        border(&format!("╰{}╯", "─".repeat(total.saturating_sub(2))))
    );
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

/// Plain-mode approval UX: the original ASCII rendering, no box art.
fn ask_approval_plain(previews: &[Value], chain: &[Value]) -> (&'static str, Option<Value>) {
    println!("\n── approval requested ──────────────────────");
    for c in chain {
        println!("  • {}", call_print(c));
    }
    if !previews.is_empty() {
        println!("  ── detail ──");
        for p in previews {
            println!("  • {}", p.as_str().unwrap_or("?"));
        }
    }
    if chain.is_empty() && previews.is_empty() {
        println!("  • (nothing to show)");
    }
    println!("────────────────────────────────────────────");
    ask_decision(chain.len())
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
}
