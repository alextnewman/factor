//! Regression test for the RPC approval deadlock.
//!
//! The server's read loop used to `await handler.on_request(...)` inline.
//! During `session.prompt` the handler sends `event.approval_requested` to
//! the client and blocks until the client's `event.approval_resolved`
//! notification arrives — but the read loop was stuck, so the notification
//! was never read: deadlock. The read loop must dispatch every message
//! independently while a long request is in flight.
//!
//! This test mirrors that shape exactly: the server's request handler sends
//! a "ping" notification and waits for the client's "pong" notification
//! before responding.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fa_core::rpc::{RpcClient, RpcHandler, RpcPeer, RpcServer};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

struct Server {
    pong_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl RpcHandler for Server {
    fn on_request<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
        peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            if method != "slow" {
                return Err(format!("unknown method {method}"));
            }
            let (tx, rx) = oneshot::channel();
            *self.pong_tx.lock().await = Some(tx);
            peer.notify("ping", json!({}))
                .await
                .map_err(|e| e.to_string())?;
            rx.await
                .map_err(|_| "pong never arrived (read loop deadlocked)".to_string())?;
            Ok(json!("done"))
        })
    }

    fn on_notification<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
        _peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if method == "pong" {
                if let Some(tx) = self.pong_tx.lock().await.take() {
                    let _ = tx.send(());
                }
            }
        })
    }
}

struct Client;

impl RpcHandler for Client {
    fn on_request<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
        _peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Value, String>> + Send + 'a>> {
        let method = method.to_string();
        Box::pin(async move { Err(format!("client takes no requests ({method})")) })
    }

    fn on_notification<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
        peer: &'a RpcPeer,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if method == "ping" {
                let _ = peer.notify("pong", json!({})).await;
            }
        })
    }
}

#[tokio::test]
async fn rpc_request_survives_notification_roundtrip() {
    let sock = PathBuf::from(format!("/tmp/fa-rpc-test-{}.sock", std::process::id()));
    let stop = Arc::new(AtomicBool::new(false));
    let server = RpcServer::bind(
        &sock,
        Server {
            pong_tx: Mutex::new(None),
        },
    )
    .await
    .unwrap();
    let stop2 = stop.clone();
    let serve_task =
        tokio::spawn(async move { server.serve(move || stop2.load(Ordering::SeqCst)).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = RpcClient::connect(&sock, Client).await.unwrap();
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.peer().request("slow", json!({})),
    )
    .await
    .expect("timed out: approval-style round-trip deadlocked");
    assert_eq!(res.unwrap(), json!("done"));

    stop.store(true, Ordering::SeqCst);
    let _ = serve_task.await;
    std::fs::remove_file(&sock).ok();
}
