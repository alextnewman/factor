//! Minimal JSON-RPC 2.0 over a Unix socket (Linux prototype).
//!
//! Windows uses a named pipe with identical framing. Three shapes:
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
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

use crate::{FaError, Result};

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
    writer: Arc<tokio::sync::Mutex<BufWriter<OwnedWriteHalf>>>,
    next_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
}

impl RpcPeer {
    fn new(writer: OwnedWriteHalf) -> Self {
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

async fn read_loop<H: RpcHandler + 'static>(reader: OwnedReadHalf, peer: RpcPeer, handler: Arc<H>) {
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
    listener: UnixListener,
    handler: Arc<H>,
}

impl<H: RpcHandler + 'static> RpcServer<H> {
    pub async fn bind(path: &Path, handler: H) -> Result<Self> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self {
            listener,
            handler: Arc::new(handler),
        })
    }

    /// Accept connections until `should_stop()` is true. Each connection
    /// gets its own read loop; the handler is shared.
    pub async fn serve<F>(self, mut should_stop: F) -> Result<()>
    where
        F: FnMut() -> bool,
    {
        loop {
            if should_stop() {
                break;
            }
            tokio::select! {
                res = self.listener.accept() => {
                    let Ok((stream, _)) = res else { continue };
                    let (rh, wh) = stream.into_split();
                    let peer = RpcPeer::new(wh);
                    let handler = self.handler.clone();
                    tokio::spawn(async move {
                        read_loop(rh, peer, handler).await;
                    });
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
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
    pub async fn connect(path: &Path, handler: H) -> Result<Self> {
        let stream = UnixStream::connect(path).await?;
        let (rh, wh) = stream.into_split();
        let peer = RpcPeer::new(wh);
        let handler = Arc::new(handler);
        let peer2 = peer.clone();
        let handler2 = handler.clone();
        tokio::spawn(async move {
            read_loop(rh, peer2, handler2).await;
        });
        Ok(Self {
            peer,
            _handler: handler,
        })
    }

    pub fn peer(&self) -> &RpcPeer {
        &self.peer
    }
}
