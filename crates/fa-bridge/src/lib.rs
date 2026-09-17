//! Async client for the FactorAgent pwsh host bridge.
//!
//! The bridge (`bridge.ps1`) speaks newline-delimited JSON-RPC 2.0 over
//! stdio on every platform. This client spawns the host, issues calls,
//! and owns shutdown/kill semantics.
//!
//! Process-group note: the host is spawned in its own process group so that
//! session shutdown reaps the whole tree (host + terminals it spawned).
//! Unix spells that `setpgid`; Windows spells it CREATE_NEW_PROCESS_GROUP,
//! and the prototype reaps the tree with `taskkill /T` (v1 hardening:
//! a Job Object owned by the bridge).

use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bridge closed the pipe unexpectedly")]
    Closed,
    #[error("tool error in {method}: {message}")]
    Tool { method: String, message: String },
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, BridgeError>;

/// Session facts the host needs: passed as environment, read by Get-FASession
/// and the terminal lifecycle cmdlets. No secrets ever travel this channel.
pub struct SessionEnv {
    /// JSON: {"id": ..., "mode": ..., "backend": ...}
    pub session_json: String,
    /// "Stop" | "Continue" — maps onto $ErrorActionPreference per call.
    pub error_action: String,
    /// FA_MAX_TERMINALS.
    pub max_terminals: u32,
    /// Host working directory.
    pub cwd: String,
}

pub struct HostBridge {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    /// Unix: process group id (== child pid after setpgid).
    /// Windows: child pid (== group id under CREATE_NEW_PROCESS_GROUP).
    pid: u32,
}

impl HostBridge {
    /// Spawn the host. Returns the bridge and the cold-start duration
    /// (spawn -> first successful response, module import included).
    pub async fn spawn(bridge_ps1: &str, env: &SessionEnv) -> Result<(Self, Duration)> {
        let t0 = Instant::now();
        let mut cmd = Command::new("pwsh");
        cmd.args(["-NoProfile", "-NonInteractive", "-File", bridge_ps1])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("FA_SESSION_JSON", &env.session_json)
            .env("FA_ERROR_ACTION", &env.error_action)
            .env("FA_MAX_TERMINALS", env.max_terminals.to_string())
            .current_dir(&env.cwd);
        // Own process group: kill_tree() reaps host + terminals together.
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id().ok_or(BridgeError::Closed)?;
        let stdin = BufWriter::new(child.stdin.take().ok_or(BridgeError::Closed)?);
        let stdout = BufReader::new(child.stdout.take().ok_or(BridgeError::Closed)?);
        let mut bridge = Self {
            child,
            stdin,
            stdout,
            next_id: AtomicU64::new(1),
            pid,
        };
        // Handshake doubles as the cold-start measurement and proves the
        // module imported cleanly.
        bridge
            .call("Get-FASession", &serde_json::Map::new())
            .await?;
        Ok((bridge, t0.elapsed()))
    }

    /// One JSON-RPC call. `args` are passed as the params object; the special
    /// key `_WhatIf: true` runs the cmdlet under `-WhatIf` (preview only).
    pub async fn call(
        &mut self,
        method: &str,
        args: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": args});
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;

        let mut resp_line = String::new();
        let n = self.stdout.read_line(&mut resp_line).await?;
        if n == 0 {
            return Err(BridgeError::Closed);
        }
        let v: serde_json::Value = serde_json::from_str(&resp_line)
            .map_err(|e| BridgeError::Protocol(format!("unparseable response line: {e}")))?;
        if v.get("id") != Some(&serde_json::json!(id)) {
            return Err(BridgeError::Protocol(format!("id mismatch in {v}")));
        }
        if let Some(err) = v.get("error") {
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown tool error")
                .to_string();
            return Err(BridgeError::Tool {
                method: method.to_string(),
                message,
            });
        }
        Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }

    /// Clean shutdown: close stdin (the bridge exits on EOF), then reap.
    pub async fn shutdown(mut self) -> Result<()> {
        drop(self.stdin);
        let _ = self.child.wait().await;
        Ok(())
    }

    /// Kill the whole process tree (host + terminals) and reap.
    /// Best-effort: a vanished tree is not an error.
    pub async fn kill_tree(mut self) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(self.pid as i32, libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            // Prototype tree-kill via taskkill; v1 moves this to a Job Object.
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.wait().await;
    }
}
