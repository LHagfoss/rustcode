//! The owner socket: how a terminal that shares its session talks to the
//! gateway (issue #1909, routing half).
//!
//! `owner.sock` sits next to `control.sock` in the gateway's private
//! directory with the same permissions, so only the user who runs the gateway
//! can reach it; the peer's user ID is checked on top. It is a second socket
//! rather than more requests on the control socket because the two have
//! nothing in common beyond their location: a control connection is one
//! request with a five-second deadline, an owner connection lives as long as
//! the session is shared, carries frames both ways and has its own liveness
//! rules. Keeping them apart leaves the control protocol untouched and means
//! a stuck owner cannot get in the way of `rustcode remote revoke`.
//!
//! Frames are newline-delimited JSON with the daemon's 1 MiB bound. They
//! carry the [`crate::remote::owner`] seam: [`OwnerFrame`] is
//! `OwnerMessage` plus replies and a heartbeat, [`GatewayFrame`] is
//! `OwnerCommand` with the reply handle replaced by a request number.

use serde::{Deserialize, Serialize};

use crate::remote::owner::GatewayLinkStatus;
use crate::remote::{
    RemoteEventFrame, RemoteRequest, RemoteResponse, RemoteSessionInfo, RemoteSnapshot,
    ResyncReason, SessionCloseReason, SessionRegistration,
};

pub const OWNER_IPC_VERSION: u32 = 1;

/// Owner → gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OwnerFrame {
    /// First frame of a connection. `snapshot.sequence` is the watermark the
    /// next event follows; it is 0 for a new registration and the owner's
    /// current sequence when it registers again after losing the gateway.
    Register {
        ipc_version: u32,
        registration: SessionRegistration,
        snapshot: Box<RemoteSnapshot>,
    },
    Event {
        frame: RemoteEventFrame,
    },
    Snapshot {
        snapshot: Box<RemoteSnapshot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resync: Option<ResyncReason>,
    },
    SessionInfo {
        info: RemoteSessionInfo,
    },
    /// The owner's decision on the [`GatewayFrame::Request`] numbered `id`.
    Reply {
        id: u64,
        response: RemoteResponse,
    },
    Unregister {
        reason: SessionCloseReason,
    },
    Ping,
}

/// Gateway → owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayFrame {
    /// The registration was accepted.
    Registered {
        gateway_id: String,
        instance_id: String,
        status: GatewayLinkStatus,
    },
    /// A session operation from the authenticated device `device_id`. The
    /// identifier scopes the owner's receipts; it is never a credential.
    Request {
        id: u64,
        device_id: String,
        request: RemoteRequest,
    },
    SnapshotRequested,
    Status {
        status: GatewayLinkStatus,
    },
    /// The registration was refused or ended; the connection closes.
    Closed {
        reason: String,
    },
    Pong,
}

#[cfg(unix)]
pub(super) use server::serve_owner;

#[cfg(unix)]
mod server {
    use super::*;
    use crate::daemon::protocol::{read_async_frame_with_buffer, write_async_frame};
    use crate::remote_gateway::hub::SessionHub;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::BufReader;
    use tokio::net::UnixStream;
    use tokio::net::unix::OwnedWriteHalf;
    use tokio_util::sync::CancellationToken;

    /// How long a new connection may take to send its registration.
    const REGISTER_TIMEOUT: Duration = Duration::from_secs(5);
    /// How long one frame may take to reach an owner's socket.
    const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

    async fn write(writer: &mut OwnedWriteHalf, frame: &GatewayFrame) -> bool {
        matches!(
            tokio::time::timeout(WRITE_TIMEOUT, write_async_frame(writer, frame)).await,
            Ok(Ok(()))
        )
    }

    /// Serve one owner connection from accept to close. The registration
    /// ends with the connection, whatever ended it.
    pub(in crate::remote_gateway) async fn serve_owner(
        hub: Arc<SessionHub>,
        stream: UnixStream,
        shutdown: CancellationToken,
    ) {
        // The directory and socket modes already keep other users out; this
        // holds even if someone loosens them by hand.
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if !stream.peer_cred().is_ok_and(|peer| peer.uid() == me) {
            return;
        }
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        let first = tokio::time::timeout(
            REGISTER_TIMEOUT,
            read_async_frame_with_buffer::<_, OwnerFrame>(&mut reader, &mut buffer),
        )
        .await;
        let Ok(Ok(OwnerFrame::Register {
            ipc_version,
            registration,
            snapshot,
        })) = first
        else {
            return;
        };
        if ipc_version != OWNER_IPC_VERSION {
            let reason = format!(
                "this gateway speaks owner protocol {OWNER_IPC_VERSION}; restart it with `rustcode remote stop` after updating"
            );
            write(&mut writer, &GatewayFrame::Closed { reason }).await;
            return;
        }
        let mut registration = match hub.register_owner(registration, snapshot) {
            Ok(registration) => registration,
            Err(reason) => {
                write(&mut writer, &GatewayFrame::Closed { reason }).await;
                return;
            }
        };
        let owner_id = registration.owner_id;
        if write(&mut writer, &registration.registered).await {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    outbound = registration.frames.recv() => {
                        let Some(frame) = outbound else { break };
                        let last = matches!(frame, GatewayFrame::Closed { .. });
                        if !write(&mut writer, &frame).await || last {
                            break;
                        }
                    }
                    inbound = read_async_frame_with_buffer::<_, OwnerFrame>(&mut reader, &mut buffer) => {
                        match inbound {
                            Ok(frame) => {
                                if !hub.owner_frame(owner_id, frame) {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        }
        hub.owner_gone(owner_id);
    }
}
