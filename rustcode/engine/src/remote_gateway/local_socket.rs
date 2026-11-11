//! The gateway's two private local endpoints: the control socket the CLI and
//! the app ask, and the owner socket session owners register on.
//!
//! On Unix each is a Unix socket, created 0600, and a connection from another
//! user is dropped. Tokio has no Unix sockets on Windows, so there each
//! endpoint is a listener on 127.0.0.1 and its "socket path" is a private file
//! holding the port and a random token ([`loopback`]). A client sends the
//! token before anything else; a connection that does not send it within
//! [`loopback::TOKEN_TIMEOUT`] is dropped unanswered. The token changes every
//! time the gateway starts, and the file is as private as the device store.
use std::path::Path;

#[cfg(unix)]
pub type LocalStream = tokio::net::UnixStream;
#[cfg(unix)]
pub type LocalReadHalf = tokio::net::unix::OwnedReadHalf;
#[cfg(unix)]
pub type LocalWriteHalf = tokio::net::unix::OwnedWriteHalf;

#[cfg(not(unix))]
pub type LocalStream = tokio::net::TcpStream;
#[cfg(not(unix))]
pub type LocalReadHalf = tokio::net::tcp::OwnedReadHalf;
#[cfg(not(unix))]
pub type LocalWriteHalf = tokio::net::tcp::OwnedWriteHalf;

/// File name of the control endpoint in the gateway's socket directory.
#[cfg(unix)]
pub(crate) const CONTROL_ENDPOINT: &str = control_endpoint_name(false);
#[cfg(not(unix))]
pub(crate) const CONTROL_ENDPOINT: &str = control_endpoint_name(true);
/// Pure platform selection, shared by the gateway and scheduler daemon.
const fn control_endpoint_name(loopback: bool) -> &'static str {
    if loopback {
        "control.endpoint"
    } else {
        "control.sock"
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn daemon_platform_transport_selects_the_discovery_file() {
        assert_eq!(control_endpoint_name(false), "control.sock");
        assert_eq!(control_endpoint_name(true), "control.endpoint");
        assert_eq!(CONTROL_ENDPOINT, control_endpoint_name(!cfg!(unix)));
    }
}

/// File name of the owner endpoint.
#[cfg(unix)]
pub(crate) const OWNER_ENDPOINT: &str = "owner.sock";
#[cfg(not(unix))]
pub(crate) const OWNER_ENDPOINT: &str = "owner.endpoint";

/// Connect to the endpoint published at `path`.
pub async fn connect(path: &Path) -> std::io::Result<LocalStream> {
    #[cfg(unix)]
    return tokio::net::UnixStream::connect(path).await;
    #[cfg(not(unix))]
    return loopback::connect(path).await;
}

/// A bound endpoint. Dropping it closes the listener; the gateway removes the
/// path with its registration.
pub(crate) struct LocalListener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(not(unix))]
    inner: loopback::Listener,
}

impl LocalListener {
    /// Bind at `path`, which must not exist (the gateway's claim removed a
    /// dead predecessor's).
    pub(crate) fn bind(path: &Path) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let inner = tokio::net::UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self { inner })
        }
        #[cfg(not(unix))]
        Ok(Self {
            inner: loopback::Listener::bind(path)?,
        })
    }

    /// The next connection. It is not trusted until [`Incoming::verify`],
    /// which the caller runs on the connection's own task so a slow client
    /// never holds up the accept loop.
    pub(crate) async fn accept(&self) -> std::io::Result<Incoming> {
        #[cfg(unix)]
        {
            let (stream, _) = self.inner.accept().await?;
            Ok(Incoming { stream })
        }
        #[cfg(not(unix))]
        {
            let (stream, token) = self.inner.accept().await?;
            Ok(Incoming { stream, token })
        }
    }
}

pub(crate) struct Incoming {
    stream: LocalStream,
    #[cfg(not(unix))]
    token: std::sync::Arc<str>,
}

impl Incoming {
    /// The connection, once it is known to come from this user; `None` means
    /// it was dropped.
    pub(crate) async fn verify(self) -> Option<LocalStream> {
        #[cfg(unix)]
        {
            // The directory and socket modes already keep other users out;
            // this holds even if someone loosens them by hand.
            // SAFETY: geteuid has no preconditions.
            let me = unsafe { libc::geteuid() };
            self.stream
                .peer_cred()
                .is_ok_and(|peer| peer.uid() == me)
                .then_some(self.stream)
        }
        #[cfg(not(unix))]
        loopback::verify(self.stream, &self.token).await
    }
}

/// The Windows transport: loopback TCP authenticated by a token from a private
/// file. Built on every platform so its tests run wherever the suite does.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) mod loopback {
    use crate::daemon::private_files::process_start_time;
    use crate::daemon::private_files::{publish_private_json, read_private_json};
    use anyhow::Context;
    use serde::{Deserialize, Serialize};
    use std::net::Ipv4Addr;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// How long a new connection may take to present the token.
    pub(crate) const TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
    /// Hex digits of a token: 32 random bytes.
    const TOKEN_DIGITS: usize = 64;

    /// What the endpoint file holds.
    #[derive(Serialize, Deserialize)]
    struct Endpoint {
        port: u16,
        token: String,
        pid: u32,
        process_start_time: u64,
    }

    pub(crate) struct Listener {
        listener: TcpListener,
        token: Arc<str>,
        path: PathBuf,
    }

    impl Listener {
        /// Listen on an ephemeral loopback port and publish it, with a fresh
        /// token, at `path`.
        pub(crate) fn bind(path: &Path) -> anyhow::Result<Self> {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            listener.set_nonblocking(true)?;
            let listener = TcpListener::from_std(listener)?;
            let token = hex::encode(rand::random::<[u8; TOKEN_DIGITS / 2]>());
            let pid = std::process::id();
            let process_start_time = process_start_time(pid)?.context("current process missing")?;
            publish_private_json(
                path,
                &Endpoint {
                    port: listener.local_addr()?.port(),
                    token: token.clone(),
                    pid,
                    process_start_time,
                },
            )?;
            Ok(Self {
                listener,
                token: token.into(),
                path: path.to_path_buf(),
            })
        }

        pub(crate) async fn accept(&self) -> std::io::Result<(TcpStream, Arc<str>)> {
            let (stream, _) = self.listener.accept().await?;
            let _ = stream.set_nodelay(true);
            Ok((stream, Arc::clone(&self.token)))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // Remove the discoverable endpoint before the listener field is
            // dropped and its port can be reused by another process.
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Read exactly the token line, so nothing after it is consumed, and
    /// compare it in constant time.
    pub(crate) async fn verify(mut stream: TcpStream, token: &str) -> Option<TcpStream> {
        let mut presented = [0u8; TOKEN_DIGITS + 1];
        tokio::time::timeout(TOKEN_TIMEOUT, stream.read_exact(&mut presented))
            .await
            .ok()?
            .ok()?;
        let expected = token.as_bytes();
        if expected.len() != TOKEN_DIGITS || presented[TOKEN_DIGITS] != b'\n' {
            return None;
        }
        let difference = presented[..TOKEN_DIGITS]
            .iter()
            .zip(expected)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b));
        (difference == 0).then_some(stream)
    }

    /// Dial the endpoint published at `path` and present its token. A missing
    /// file is `NotFound`, as a missing Unix socket is.
    pub(crate) async fn connect(path: &Path) -> std::io::Result<TcpStream> {
        let endpoint: Endpoint = read_private_json(path, 4 * 1024).map_err(|error| {
            match error.downcast::<std::io::Error>() {
                Ok(error) => error,
                Err(error) => std::io::Error::new(std::io::ErrorKind::InvalidData, error),
            }
        })?;
        let process_start_time = process_start_time(endpoint.pid).map_err(std::io::Error::other)?;
        if process_start_time != Some(endpoint.process_start_time) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "the gateway endpoint belongs to a process that is no longer running",
            ));
        }
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port)).await?;
        let _ = stream.set_nodelay(true);
        stream
            .write_all(format!("{}\n", endpoint.token).as_bytes())
            .await?;
        Ok(stream)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::io::{AsyncBufReadExt, BufReader};

        #[tokio::test]
        async fn a_client_with_the_token_is_verified_and_its_frames_arrive_whole() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("control.endpoint");
            let listener = Listener::bind(&path).unwrap();
            let client = tokio::spawn({
                let path = path.clone();
                async move {
                    let mut stream = connect(&path).await.unwrap();
                    stream.write_all(b"{\"type\":\"status\"}\n").await.unwrap();
                    let mut answer = String::new();
                    BufReader::new(stream).read_line(&mut answer).await.unwrap();
                    answer
                }
            });
            let (stream, token) = listener.accept().await.unwrap();
            let stream = verify(stream, &token).await.expect("token accepted");
            let (reader, mut writer) = stream.into_split();
            let mut frame = String::new();
            BufReader::new(reader).read_line(&mut frame).await.unwrap();
            assert_eq!(frame, "{\"type\":\"status\"}\n");
            writer.write_all(b"ok\n").await.unwrap();
            assert_eq!(client.await.unwrap(), "ok\n");
        }

        #[tokio::test]
        async fn a_wrong_or_missing_token_is_dropped() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("owner.endpoint");
            let listener = Listener::bind(&path).unwrap();
            let port = listener.listener.local_addr().unwrap().port();

            let mut wrong = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            wrong
                .write_all(format!("{}\n", "0".repeat(TOKEN_DIGITS)).as_bytes())
                .await
                .unwrap();
            let (stream, token) = listener.accept().await.unwrap();
            assert!(verify(stream, &token).await.is_none());

            // A browser's request to the port is not a token line.
            let mut http = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            http.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .await
                .unwrap();
            drop(http);
            let (stream, token) = listener.accept().await.unwrap();
            assert!(verify(stream, &token).await.is_none());
        }

        #[tokio::test(start_paused = true)]
        async fn a_silent_client_is_dropped_after_the_timeout() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("owner.endpoint");
            let listener = Listener::bind(&path).unwrap();
            let port = listener.listener.local_addr().unwrap().port();
            let _silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            let (stream, token) = listener.accept().await.unwrap();
            assert!(verify(stream, &token).await.is_none());
        }

        #[tokio::test]
        async fn a_missing_endpoint_is_not_found_and_tokens_differ_per_bind() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("control.endpoint");
            let error = connect(&path).await.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            let first = Listener::bind(&path).unwrap().token.clone();
            let second = Listener::bind(&directory.path().join("other"))
                .unwrap()
                .token
                .clone();
            assert_eq!(first.len(), TOKEN_DIGITS);
            assert_ne!(first, second);
        }

        #[tokio::test]
        async fn a_stale_endpoint_is_rejected_before_dialing_a_reused_port() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("control.endpoint");
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            publish_private_json(
                &path,
                &Endpoint {
                    port,
                    token: "0".repeat(TOKEN_DIGITS),
                    pid: u32::MAX,
                    process_start_time: 1,
                },
            )
            .unwrap();

            let error = connect(&path).await.unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
            assert!(
                tokio::time::timeout(Duration::from_millis(10), listener.accept())
                    .await
                    .is_err()
            );
        }

        #[tokio::test]
        async fn dropping_a_listener_removes_its_endpoint() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("control.endpoint");
            let listener = Listener::bind(&path).unwrap();
            drop(listener);

            assert!(!path.exists());
            assert_eq!(
                connect(&path).await.unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
        }
    }
}
