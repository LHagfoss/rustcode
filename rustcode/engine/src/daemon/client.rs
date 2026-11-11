//! One request per connection. Transport failures never implicitly start a daemon.
use super::protocol::{DaemonRequest, DaemonResponse, read_async_frame, write_async_frame};
use crate::remote_gateway::local_socket;
use anyhow::Result;
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio::io::BufReader;

pub type ResponseFuture<'a> = Pin<Box<dyn Future<Output = Result<DaemonResponse>> + Send + 'a>>;

/// Keep platform transport separate from CLI/harness request construction.
pub trait DaemonTransport: Send + Sync {
    fn request(&self, request: DaemonRequest) -> ResponseFuture<'_>;
}

pub struct LocalTransport {
    pub socket_path: PathBuf,
    pub timeout: Duration,
}

impl DaemonTransport for LocalTransport {
    fn request(&self, request: DaemonRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            tokio::time::timeout(self.timeout, async {
                let mut stream = local_socket::connect(&self.socket_path).await?;
                write_async_frame(&mut stream, &request).await?;
                Ok(read_async_frame(&mut BufReader::new(stream)).await?)
            })
            .await?
        })
    }
}

#[derive(Clone)]
pub struct DaemonClient {
    transport: Arc<dyn DaemonTransport>,
}

impl DaemonClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self::with_transport(LocalTransport {
            socket_path: socket_path.into(),
            timeout: Duration::from_secs(5),
        })
    }

    pub fn with_transport(transport: impl DaemonTransport + 'static) -> Self {
        Self {
            transport: Arc::new(transport),
        }
    }

    pub async fn request(&self, request: DaemonRequest) -> Result<DaemonResponse> {
        self.transport.request(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn silent_peer_cannot_hang_client() {
        let dir = crate::daemon::test_directory();
        let path = dir.path().join("silent.sock");
        let _listener = local_socket::LocalListener::bind(&path).unwrap();
        let client = DaemonClient::with_transport(LocalTransport {
            socket_path: path,
            timeout: Duration::from_millis(30),
        });
        let error = client.request(DaemonRequest::Status).await.unwrap_err();
        assert!(
            error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some()
        );
    }
}
