//! Plug-in point for everything that happens after authentication.
//!
//! The gateway foundation understands only handshake frames. Once a device is
//! authenticated, each text frame it sends is handed, unparsed, to a
//! [`FrameRouter`]. The session registry and routing half of issue #1909
//! implements this trait; until then [`NoSessionsRouter`] answers every frame
//! with a typed `not_implemented` error.

use super::handshake::{ErrorCode, HandshakeResponse};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The authenticated device behind one connection. `device_id` comes from
/// the device store; `device_name` is a label the device chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceContext {
    /// Unique per connection within one gateway instance.
    pub connection_id: u64,
    pub device_id: String,
    pub device_name: String,
}

/// Receives the opaque frames of authenticated devices.
///
/// Calls arrive on the connection's own task, in order per connection.
/// Implementations must not block: queue work elsewhere and reply through
/// the [`FrameSink`], which may be cloned and kept for later pushes.
pub trait FrameRouter: Send + Sync + 'static {
    /// A device finished its handshake. Nothing has been sent to it beyond
    /// the handshake response.
    fn connected(&self, _device: &DeviceContext, _sink: &FrameSink) {}

    /// One text frame from the device, at most `MAX_FRAME_BYTES` long.
    fn frame(&self, device: &DeviceContext, frame: &str, sink: &FrameSink);

    /// The connection ended, for any reason including revocation.
    fn disconnected(&self, _device: &DeviceContext) {}
}

/// Why a queued frame was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkError {
    /// The device's queue was full; the connection is being closed.
    Overflow,
    /// The connection is already gone.
    Closed,
}

/// Bounded, non-blocking outbound queue of one connection.
///
/// A device that does not read fast enough fills its own queue and is
/// disconnected with `slow_consumer`; the sender never waits on the network,
/// so one stuck device cannot stall a router or another device.
#[derive(Clone)]
pub struct FrameSink {
    queue: mpsc::Sender<String>,
    control: Arc<ConnectionControl>,
}

impl FrameSink {
    pub(super) fn new(capacity: usize, control: Arc<ConnectionControl>) -> (Self, FrameQueue) {
        let (queue, receiver) = mpsc::channel(capacity);
        (Self { queue, control }, receiver)
    }

    /// A sink with no gateway behind it, for exercising a [`FrameRouter`].
    pub fn detached(capacity: usize) -> (Self, FrameQueue) {
        Self::new(
            capacity,
            Arc::new(ConnectionControl::new(CancellationToken::new())),
        )
    }

    /// Queue one text frame for the device.
    pub fn send(&self, frame: String) -> Result<(), SinkError> {
        match self.queue.try_send(frame) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.control.close(ErrorCode::SlowConsumer);
                Err(SinkError::Overflow)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(SinkError::Closed),
        }
    }

    /// Whether the connection has been told to close.
    pub fn is_closed(&self) -> bool {
        self.control.cancel.is_cancelled() || self.queue.is_closed()
    }
}

pub type FrameQueue = mpsc::Receiver<String>;

/// Close switch of one connection, shared by its task, its sink and the
/// gateway's live registry.
pub(super) struct ConnectionControl {
    pub(super) cancel: CancellationToken,
    reason: Mutex<Option<ErrorCode>>,
}

impl ConnectionControl {
    pub(super) fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            reason: Mutex::new(None),
        }
    }

    /// Ask the connection to close now. The first reason wins.
    pub(super) fn close(&self, reason: ErrorCode) {
        self.reason
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert(reason);
        self.cancel.cancel();
    }

    /// The recorded reason; a cancellation without one is a gateway shutdown.
    pub(super) fn reason(&self) -> ErrorCode {
        self.reason
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unwrap_or(ErrorCode::ShuttingDown)
    }
}

/// Stub router of the gateway foundation: no sessions exist to route to.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSessionsRouter;

impl FrameRouter for NoSessionsRouter {
    fn frame(&self, _device: &DeviceContext, _frame: &str, sink: &FrameSink) {
        let _ = sink.send(
            HandshakeResponse::error(
                ErrorCode::NotImplemented,
                "this gateway shares no sessions yet",
            )
            .to_text(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceContext {
        DeviceContext {
            connection_id: 1,
            device_id: "dev".into(),
            device_name: "phone".into(),
        }
    }

    #[test]
    fn stub_router_answers_with_a_typed_not_implemented_error() {
        let (sink, mut queue) = FrameSink::detached(4);
        NoSessionsRouter.frame(&device(), r#"{"type":"list_sessions"}"#, &sink);
        assert_eq!(
            queue.try_recv().unwrap(),
            r#"{"type":"error","code":"not_implemented","message":"this gateway shares no sessions yet"}"#
        );
        assert!(queue.try_recv().is_err());
    }

    #[test]
    fn overflowing_sink_closes_its_connection_instead_of_waiting() {
        let (sink, _queue) = FrameSink::detached(2);
        assert_eq!(sink.send("a".into()), Ok(()));
        assert_eq!(sink.send("b".into()), Ok(()));
        assert!(!sink.is_closed());
        assert_eq!(sink.send("c".into()), Err(SinkError::Overflow));
        assert!(sink.is_closed());
        assert_eq!(sink.control.reason(), ErrorCode::SlowConsumer);
    }

    #[test]
    fn first_close_reason_wins_and_shutdown_is_the_default() {
        let control = ConnectionControl::new(CancellationToken::new());
        assert_eq!(control.reason(), ErrorCode::ShuttingDown);
        control.close(ErrorCode::Revoked);
        control.close(ErrorCode::SlowConsumer);
        assert_eq!(control.reason(), ErrorCode::Revoked);
        assert!(control.cancel.is_cancelled());
    }

    #[test]
    fn sink_reports_a_dropped_connection() {
        let (sink, queue) = FrameSink::detached(1);
        drop(queue);
        assert_eq!(sink.send("a".into()), Err(SinkError::Closed));
        assert!(sink.is_closed());
    }
}
