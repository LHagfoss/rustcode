//! One request per connection. Transport failures never implicitly start a daemon.
use super::protocol::{DaemonRequest, DaemonResponse, read_async_frame, write_async_frame};
use anyhow::Result;
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio::{io::BufReader, net::UnixStream};

pub type ResponseFuture<'a> = Pin<Box<dyn Future<Output = Result<DaemonResponse>> + Send + 'a>>;

/// Keep platform transport separate from CLI/harness request construction.
pub trait DaemonTransport: Send + Sync {
    fn request(&self, request: DaemonRequest) -> ResponseFuture<'_>;
}

pub struct UnixTransport {
    pub socket_path: PathBuf,
    pub timeout: Duration,
}

impl DaemonTransport for UnixTransport {
    fn request(&self, request: DaemonRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            tokio::time::timeout(self.timeout, async {
                let mut stream = UnixStream::connect(&self.socket_path).await?;
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
        Self::with_transport(UnixTransport {
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
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = dir.path().join("silent.sock");
        let _listener = tokio::net::UnixListener::bind(&path).unwrap();
        let client = DaemonClient::with_transport(UnixTransport {
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
