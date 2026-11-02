//! A small newline-delimited JSON-RPC peer over a child's stdio. ACP agents
//! and `codex app-server` both speak it; the only difference is that Codex
//! leaves out the `"jsonrpc": "2.0"` field, which this peer accepts either
//! way.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// A message the other side started.
#[derive(Debug)]
pub(crate) enum Incoming {
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// Answered by whoever reads the channel once it handled everything
    /// that arrived before the barrier ([`RpcPeer::barrier`]).
    Barrier(oneshot::Sender<()>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub(crate) fn closed() -> Self {
        Self {
            code: -32099,
            message: "the harness process exited".to_owned(),
            data: None,
        }
    }

    pub(crate) fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("{method} is not supported by RustCode"),
            data: None,
        }
    }

    /// The message with its `data` detail, when there is one.
    pub(crate) fn describe(&self) -> String {
        match &self.data {
            Some(Value::String(detail)) if !detail.is_empty() => {
                format!("{}: {detail}", self.message)
            }
            Some(Value::Null) | None => self.message.clone(),
            Some(detail) => format!("{}: {detail}", self.message),
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

pub(crate) struct RpcPeer {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
    incoming: mpsc::WeakUnboundedSender<Incoming>,
}

impl RpcPeer {
    /// Start reading `reader` and writing `writer`. Messages the other side
    /// starts arrive on the returned channel, in order; it closes when the
    /// other side does.
    pub(crate) fn start<R, W>(
        reader: R,
        writer: W,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<Incoming>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<String>();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let weak_incoming = incoming_tx.downgrade();
        tokio::spawn(async move {
            let mut writer = writer;
            while let Some(line) = outgoing_rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err()
                    || writer.write_all(b"\n").await.is_err()
                    || writer.flush().await.is_err()
                {
                    break;
                }
            }
        });
        {
            let pending = Arc::clone(&pending);
            let closed = Arc::clone(&closed);
            tokio::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    route(message, &pending, &incoming_tx);
                }
                closed.store(true, Ordering::Release);
                let waiting =
                    std::mem::take(&mut *pending.lock().unwrap_or_else(|error| error.into_inner()));
                for (_, sender) in waiting {
                    let _ = sender.send(Err(RpcError::closed()));
                }
            });
        }
        (
            Arc::new(Self {
                outgoing,
                pending,
                next_id: AtomicU64::new(1),
                closed,
                incoming: weak_incoming,
            }),
            incoming_rx,
        )
    }

    /// Wait until the reader of the incoming channel handled every message
    /// that arrived before now, such as the notifications that preceded a
    /// response.
    pub(crate) async fn barrier(&self) {
        let Some(incoming) = self.incoming.upgrade() else {
            return;
        };
        let (sender, receiver) = oneshot::channel();
        if incoming.send(Incoming::Barrier(sender)).is_ok() {
            drop(incoming);
            let _ = receiver.await;
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError::closed());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, sender);
        // The reader may have closed between the check and the insert, after
        // it failed everything pending.
        if self.is_closed() {
            self.pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&id);
            return Err(RpcError::closed());
        }
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.outgoing.send(message.to_string()).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&id);
            return Err(RpcError::closed());
        }
        receiver.await.unwrap_or_else(|_| Err(RpcError::closed()))
    }

    /// [`Self::request`] that gives up after `timeout`.
    pub(crate) async fn request_within(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value, RpcError> {
        tokio::time::timeout(timeout, self.request(method, params))
            .await
            .unwrap_or_else(|_| {
                Err(RpcError {
                    code: -32098,
                    message: format!("{method} timed out after {} s", timeout.as_secs()),
                    data: None,
                })
            })
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        let message = if params.is_null() {
            json!({"jsonrpc": "2.0", "method": method})
        } else {
            json!({"jsonrpc": "2.0", "method": method, "params": params})
        };
        let _ = self.outgoing.send(message.to_string());
    }

    pub(crate) fn respond(&self, id: Value, result: Result<Value, RpcError>) {
        let message = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => {
                let mut body = json!({"code": error.code, "message": error.message});
                if let Some(data) = error.data {
                    body["data"] = data;
                }
                json!({"jsonrpc": "2.0", "id": id, "error": body})
            }
        };
        let _ = self.outgoing.send(message.to_string());
    }
}

fn route(message: Value, pending: &Pending, incoming: &mpsc::UnboundedSender<Incoming>) {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, message.get("id").cloned()) {
        (Some(method), Some(id)) if !id.is_null() => {
            let _ = incoming.send(Incoming::Request { id, method, params });
        }
        (Some(method), _) => {
            let _ = incoming.send(Incoming::Notification { method, params });
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                return;
            };
            let Some(sender) = pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&id)
            else {
                return;
            };
            let result = match message.get("error") {
                Some(error) if !error.is_null() => Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("request failed")
                        .to_owned(),
                    data: error.get("data").cloned(),
                }),
                _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = sender.send(result);
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn requests_responses_and_incoming_messages() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (our_read, our_write) = tokio::io::split(ours);
        let (their_read, their_write) = tokio::io::split(theirs);
        let (peer, _incoming) = RpcPeer::start(our_read, our_write);
        let (other, mut other_incoming) = RpcPeer::start(their_read, their_write);

        let asked = tokio::spawn({
            let peer = Arc::clone(&peer);
            async move { peer.request("ping", json!({"n": 1})).await }
        });
        let Some(Incoming::Request { id, method, params }) = other_incoming.recv().await else {
            panic!("expected a request");
        };
        assert_eq!(method, "ping");
        assert_eq!(params, json!({"n": 1}));
        other.respond(id, Ok(json!("pong")));
        assert_eq!(asked.await.unwrap(), Ok(json!("pong")));

        peer.notify("hello", Value::Null);
        assert!(matches!(
            other_incoming.recv().await,
            Some(Incoming::Notification { method, params: Value::Null }) if method == "hello"
        ));

        let failing = tokio::spawn({
            let peer = Arc::clone(&peer);
            async move { peer.request("boom", Value::Null).await }
        });
        let Some(Incoming::Request { id, .. }) = other_incoming.recv().await else {
            panic!("expected a request");
        };
        other.respond(id, Err(RpcError::method_not_found("boom")));
        assert_eq!(failing.await.unwrap().unwrap_err().code, -32601);
    }

    #[tokio::test]
    async fn pending_requests_fail_when_the_other_side_closes() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (our_read, our_write) = tokio::io::split(ours);
        let (peer, _incoming) = RpcPeer::start(our_read, our_write);
        let asked = tokio::spawn({
            let peer = Arc::clone(&peer);
            async move { peer.request("never", Value::Null).await }
        });
        tokio::task::yield_now().await;
        drop(theirs);
        assert_eq!(asked.await.unwrap(), Err(RpcError::closed()));
        assert!(peer.is_closed());
    }
}
