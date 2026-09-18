//! Minimal JSON-RPC 2.0 over a local session transport.
//!
//! Unix: a socket file (`session.sock` under the session dir).
//! Windows: a named pipe `\\.\pipe\WinAgent32\<session-id>`.
//!
//! Identical newline-delimited framing on both. Three shapes:
//! - request:    {"jsonrpc":"2.0","id":N,"method":"...","params":{...}}
//! - response:   {"jsonrpc":"2.0","id":N,"result":...} (or "error")
//! - notify:     {"jsonrpc":"2.0","method":"...","params":{...}} (no id)
//!
//! The prototype's method set:
//!   C->S session.prompt {text}            -> {text, turnsUsed}
//!   S->C event.agent_text {text}
//!   S->C event.tool_call {name, args}
//!   S->C event.tool_result {name, ok, error?, duration_ms}
//!   S->C event.model_usage {prompt_tokens, completion_tokens, latency_ms}
//!   S->C event.warning {text}
//!   S->C event.approval_requested {approvalId, chain, previews}
//!   C->S event.approval_resolved {approvalId, decision, args?}
//!   C->S session.shutdown {}              -> {}

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::oneshot;

use crate::{FaError, Result};

/// Owned, type-erased stream halves. Unix and Windows expose different
/// concrete stream types; the framing above them is identical.
type BoxRead = Pin<Box<dyn AsyncRead + Send>>;
type BoxWrite = Pin<Box<dyn AsyncWrite + Send>>;

/// Session endpoint addressing, per platform.
#[cfg(unix)]
pub mod endpoint {
    use super::{BoxRead, BoxWrite};
    use crate::Result;
    use std::path::{Path, PathBuf};
    use tokio::net::{UnixListener, UnixStream};

    pub struct Listener(UnixListener);

    /// Socket file for a session.
    pub fn path_for(state_dir: &Path, session_id: &str) -> PathBuf {
        state_dir.join("sessions").join(session_id).join("session.sock")
    }

    pub async fn bind(path: &Path) -> Result<Listener> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(Listener(UnixListener::bind(path)?))
    }

    pub async fn accept(listener: &Listener) -> Result<(BoxRead, BoxWrite)> {
        let (stream, _) = listener.0.accept().await?;
        let (rh, wh) = stream.into_split();
        Ok((Box::pin(rh), Box::pin(wh)))
    }

    pub async fn connect(path: &Path) -> Result<(BoxRead, BoxWrite)> {
        let stream = UnixStream::connect(path).await?;
        let (rh, wh) = stream.into_split();
        Ok((Box::pin(rh), Box::pin(wh)))
    }

    /// True if a live server answers at the endpoint right now.
    pub async fn listening(path: &Path) -> bool {
        path.exists() && UnixStream::connect(path).await.is_ok()
    }
}

/// Session endpoint addressing, per platform.
#[cfg(windows)]
pub mod endpoint {
    use super::{BoxRead, BoxWrite};
    use crate::Result;
    use std::time::{Duration, Instant};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    /// `ERROR_PIPE_BUSY` (231): `CreateFile` on a pipe whose instances are
    /// all connected, or none of which is in `ConnectNamedPipe`. The
    /// MSDN-sanctioned client answer is `WaitNamedPipe` + retry; tokio
    /// exposes no `WaitNamedPipe`, so `connect` below does the same with
    /// bounded backoff.
    const ERROR_PIPE_BUSY: i32 = 231;
    /// How long `connect` keeps retrying a busy pipe before giving up.
    const CONNECT_RETRY_TIMEOUT: Duration = Duration::from_secs(10);

    pub struct Listener {
        name: String,
        /// The instance currently waiting for the next client. `accept`
        /// keeps this slot filled — the replacement is created *before*
        /// the accepted instance is handed out — so a client never finds
        /// "no listener".
        pending: NamedPipeServer,
    }

    /// `\\.\pipe\WinAgent32\<session-id>` — the locked pipe naming.
    /// The id is sanitized: a pipe name must not contain path separators.
    pub fn pipe_name(session_id: &str) -> String {
        let safe: String = session_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        format!(r"\\.\pipe\WinAgent32\{safe}")
    }

    pub async fn bind(name: &str) -> Result<Listener> {
        // The first instance takes FILE_FLAG_FIRST_PIPE_INSTANCE: creation
        // fails fast with ERROR_ACCESS_DENIED if the name is already owned
        // (live server or squatter) instead of silently sharing it.
        let pending = ServerOptions::new()
            .first_pipe_instance(true)
            .create(name)?;
        Ok(Listener {
            name: name.to_string(),
            pending,
        })
    }

    pub async fn accept(listener: &mut Listener) -> Result<(BoxRead, BoxWrite)> {
        // Wait for the next client on the pending instance...
        listener.pending.connect().await?;
        // ...then install the replacement BEFORE handing the connected
        // instance out (MSDN multithreaded-pipe-server ordering). Without
        // this, a client arriving in the handoff window gets
        // ERROR_PIPE_BUSY.
        let connected = std::mem::replace(
            &mut listener.pending,
            ServerOptions::new().create(&listener.name)?,
        );
        let (rh, wh) = tokio::io::split(connected);
        Ok((Box::pin(rh), Box::pin(wh)))
    }

    pub async fn connect(name: &str) -> Result<(BoxRead, BoxWrite)> {
        let start = Instant::now();
        loop {
            match ClientOptions::new().open(name) {
                Ok(client) => {
                    let (rh, wh) = tokio::io::split(client);
                    return Ok((Box::pin(rh), Box::pin(wh)));
                }
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    // All instances connected right now — typically our own
                    // `listening()` probe racing the real connect, or a
                    // client landing in an accept handoff. Wait for a
                    // listener the way WaitNamedPipe would.
                    if start.elapsed() >= CONNECT_RETRY_TIMEOUT {
                        return Err(e.into());
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// True if a server instance is accepting right now. Opening a pipe
    /// with no listener fails immediately — no waiting involved. Note the
    /// probe itself consumes one accept; `accept` replaces the instance
    /// before returning, so probes are harmless.
    pub async fn listening(name: &str) -> bool {
        ClientOptions::new().open(name).is_ok()
    }
}

/// Server-side handler: requests return a result; notifications are observed.
pub trait RpcHandler: Send + Sync {
    fn on_request<'a>(
        &'a self,
        method: &'a str,
        params: Value,
        peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Value, String>> + Send + 'a>>;
    fn on_notification<'a>(
        &'a self,
        method: &'a str,
        params: Value,
        peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// One side of a connection. Cloneable; the writer is shared.
#[derive(Clone)]
pub struct RpcPeer {
    writer: Arc<tokio::sync::Mutex<BufWriter<BoxWrite>>>,
    next_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
}

impl RpcPeer {
    fn new(writer: BoxWrite) -> Self {
        Self {
            writer: Arc::new(tokio::sync::Mutex::new(BufWriter::new(writer))),
            next_id: Arc::new(AtomicU64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn send_value(&self, v: &Value) -> Result<()> {
        let mut line = serde_json::to_string(v)?;
        line.push('\n');
        let mut w = self.writer.lock().await;
        w.write_all(line.as_bytes()).await?;
        w.flush().await?;
        Ok(())
    }

    /// Send a notification (no id).
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send_value(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    /// Send a request and wait for its response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let send = self
            .send_value(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        if let Err(e) = send {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let v = rx.await.map_err(|_| FaError::Rpc("peer hung up".into()))?;
        if let Some(err) = v.get("__rpc_error") {
            return Err(FaError::Rpc(format!("peer error: {err}")));
        }
        Ok(v)
    }

    fn complete(&self, id: u64, value: Value) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(value);
        }
    }
}

async fn read_loop<H: RpcHandler + 'static>(reader: BoxRead, peer: RpcPeer, handler: Arc<H>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = v
            .get("method")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string());
        match (v.get("id"), method) {
            (Some(id), Some(m)) => {
                // Dispatch independently: the read loop MUST keep reading
                // while a long request is in flight. session.prompt waits
                // on the client's event.approval_resolved notification;
                // awaiting on_request inline here deadlocks that round-trip.
                // Response writes stay serialized via the peer's writer
                // mutex; responses are matched by id, so order is free.
                let idv = id.clone();
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                let peer2 = peer.clone();
                let handler2 = handler.clone();
                tokio::spawn(async move {
                    let result = handler2.on_request(&m, params, &peer2).await;
                    let resp = match result {
                        Ok(r) => json!({"jsonrpc": "2.0", "id": idv, "result": r}),
                        Err(e) => json!({"jsonrpc": "2.0", "id": idv,
                            "error": {"code": -32000, "message": e}}),
                    };
                    let _ = peer2.send_value(&resp).await;
                });
            }
            (Some(id), None) => {
                // Response to our own outgoing request: complete inline
                // (fast, no await) so peer.request() unblocks promptly.
                if let Some(n) = id.as_u64() {
                    if let Some(err) = v.get("error") {
                        peer.complete(n, json!({"__rpc_error": err}));
                    } else {
                        peer.complete(n, v.get("result").cloned().unwrap_or(Value::Null));
                    }
                }
            }
            (None, Some(m)) => {
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                let peer2 = peer.clone();
                let handler2 = handler.clone();
                tokio::spawn(async move {
                    handler2.on_notification(&m, params, &peer2).await;
                });
            }
            _ => {}
        }
    }
}

pub struct RpcServer<H: RpcHandler> {
    listener: endpoint::Listener,
    handler: Arc<H>,
}

impl<H: RpcHandler + 'static> RpcServer<H> {
    /// Bind the session endpoint: socket path on Unix, pipe name on Windows.
    #[cfg(unix)]
    pub async fn bind(path: &std::path::Path, handler: H) -> Result<Self> {
        Ok(Self {
            listener: endpoint::bind(path).await?,
            handler: Arc::new(handler),
        })
    }

    /// Bind the session endpoint: socket path on Unix, pipe name on Windows.
    #[cfg(windows)]
    pub async fn bind(pipe_name: &str, handler: H) -> Result<Self> {
        Ok(Self {
            listener: endpoint::bind(pipe_name).await?,
            handler: Arc::new(handler),
        })
    }

    /// Accept connections until `should_stop()` is true. Each connection
    /// gets its own read loop; the handler is shared.
    ///
    /// The stop condition is polled every 200 ms *without* cancelling the
    /// in-flight accept: on Windows the accept future owns the listening
    /// pipe instance, and dropping it deletes the listener — clients then
    /// see ERROR_PIPE_BUSY. A `select!` that raced the two tripped exactly
    /// that on session start.
    pub async fn serve<F>(mut self, mut should_stop: F) -> Result<()>
    where
        F: FnMut() -> bool,
    {
        loop {
            if should_stop() {
                break;
            }
            // One accept future per iteration, pinned across the stop-poll
            // ticks: the timer only re-checks `should_stop()`, it never
            // drops the accept.
            let accept_fut = endpoint::accept(&mut self.listener);
            tokio::pin!(accept_fut);
            let accepted = loop {
                tokio::select! {
                    res = &mut accept_fut => break Some(res),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                        if should_stop() {
                            break None;
                        }
                    }
                }
            };
            let Some(res) = accepted else { break };
            {
                let Ok((rh, wh)) = res else { continue };
                let peer = RpcPeer::new(wh);
                let handler = self.handler.clone();
                tokio::spawn(async move {
                    read_loop(rh, peer, handler).await;
                });
            }
        }
        Ok(())
    }
}

/// Client side: connect, then use `peer()` for requests/notifications.
/// Incoming notifications go to the handler's `on_notification`.
pub struct RpcClient<H: RpcHandler> {
    peer: RpcPeer,
    _handler: Arc<H>,
}

impl<H: RpcHandler + 'static> RpcClient<H> {
    #[cfg(unix)]
    pub async fn connect(path: &std::path::Path, handler: H) -> Result<Self> {
        let (rh, wh) = endpoint::connect(path).await?;
        Ok(Self::from_halves(rh, wh, handler))
    }

    #[cfg(windows)]
    pub async fn connect(pipe_name: &str, handler: H) -> Result<Self> {
        let (rh, wh) = endpoint::connect(pipe_name).await?;
        Ok(Self::from_halves(rh, wh, handler))
    }

    fn from_halves(rh: BoxRead, wh: BoxWrite, handler: H) -> Self {
        let peer = RpcPeer::new(wh);
        let handler = Arc::new(handler);
        let peer2 = peer.clone();
        let handler2 = handler.clone();
        tokio::spawn(async move {
            read_loop(rh, peer2, handler2).await;
        });
        Self {
            peer,
            _handler: handler,
        }
    }

    pub fn peer(&self) -> &RpcPeer {
        &self.peer
    }
}
