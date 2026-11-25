//! The loopback HTTP proxy every connection of RustCode's browser goes
//! through. Chrome sends page loads, subresources, WebSockets, workers and
//! out-of-process iframes through it alike, so the domain allowlist holds for
//! all of them, not only for the requests the page target's `Fetch` sees.

use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

const MAX_HEAD: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";
const FORBIDDEN: &[u8] =
    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const BAD_GATEWAY: &[u8] =
    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Whether a host (lowercase, IPv6 without brackets) may be reached.
pub(super) type Allow = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Listen on a free loopback port until `stop` is cancelled.
pub(super) async fn bind(allow: Allow, stop: CancellationToken) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = stop.cancelled() => break,
                accepted = listener.accept() => accepted,
            };
            let Ok((client, _)) = accepted else {
                continue;
            };
            let allow = allow.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = stop.cancelled() => {}
                    _ = relay(client, &allow) => {}
                }
            });
        }
    });
    Ok(port)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Request {
    /// `CONNECT host:port`: HTTPS, secure WebSockets and anything tunnelled.
    Connect { host: String, port: u16 },
    /// A plain HTTP request in absolute form, forwarded with `head`.
    Forward {
        host: String,
        port: u16,
        head: Vec<u8>,
    },
}

impl Request {
    fn host(&self) -> &str {
        match self {
            Self::Connect { host, .. } | Self::Forward { host, .. } => host,
        }
    }
    fn port(&self) -> u16 {
        match self {
            Self::Connect { port, .. } | Self::Forward { port, .. } => *port,
        }
    }
}

fn split_authority(authority: &str) -> Option<(String, u16)> {
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once("]:")?,
        None => authority.rsplit_once(':')?,
    };
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    (!host.is_empty()).then(|| (host.to_ascii_lowercase(), port))
}

/// Parse a request head (through the blank line) the browser sent the proxy.
pub(super) fn parse(head: &[u8]) -> Option<Request> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut parts = lines.next()?.split_whitespace();
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return None;
    }
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_authority(target)?;
        return Some(Request::Connect { host, port });
    }
    let url = url::Url::parse(target).ok()?;
    if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let port = url.port_or_known_default()?;
    let mut path = url.path().to_owned();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let mut forwarded = format!("{method} {path} {version}\r\n");
    for line in lines.take_while(|line| !line.is_empty()) {
        let name = line.split(':').next().unwrap_or("").trim();
        if [
            "connection",
            "proxy-connection",
            "proxy-authorization",
            "keep-alive",
        ]
        .iter()
        .any(|hop| name.eq_ignore_ascii_case(hop))
        {
            continue;
        }
        forwarded.push_str(line);
        forwarded.push_str("\r\n");
    }
    forwarded.push_str("Connection: close\r\n\r\n");
    Some(Request::Forward {
        host,
        port,
        head: forwarded.into_bytes(),
    })
}

async fn read_head(stream: &mut TcpStream) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            return Some((buffer, rest));
        }
        if buffer.len() > MAX_HEAD {
            return None;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

async fn relay(mut client: TcpStream, allow: &Allow) {
    let Some((head, rest)) = read_head(&mut client).await else {
        return;
    };
    let Some(request) = parse(&head) else {
        let _ = client.write_all(BAD_REQUEST).await;
        return;
    };
    if !allow(request.host()) {
        let _ = client.write_all(FORBIDDEN).await;
        return;
    }
    let connected = tokio::time::timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((request.host(), request.port())),
    )
    .await;
    let Ok(Ok(mut upstream)) = connected else {
        let _ = client.write_all(BAD_GATEWAY).await;
        return;
    };
    let sent = match &request {
        Request::Connect { .. } => client.write_all(ESTABLISHED).await,
        Request::Forward { head, .. } => upstream.write_all(head).await,
    };
    if sent.is_err() || (!rest.is_empty() && upstream.write_all(&rest).await.is_err()) {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_targets_are_host_and_port() {
        assert_eq!(
            parse(b"CONNECT Example.COM:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"),
            Some(Request::Connect {
                host: "example.com".into(),
                port: 443
            })
        );
        assert_eq!(
            parse(b"CONNECT [::1]:8080 HTTP/1.1\r\n\r\n"),
            Some(Request::Connect {
                host: "::1".into(),
                port: 8080
            })
        );
        for head in [
            &b"CONNECT example.com HTTP/1.1\r\n\r\n"[..],
            b"CONNECT :443 HTTP/1.1\r\n\r\n",
            b"CONNECT example.com:0 HTTP/1.1\r\n\r\n",
            b"GET /relative HTTP/1.1\r\n\r\n",
            b"GET https://example.com/ HTTP/1.1\r\n\r\n",
            b"GET http://user@example.com/ HTTP/1.1\r\n\r\n",
            b"not http at all\r\n\r\n",
        ] {
            assert_eq!(parse(head), None, "{}", String::from_utf8_lossy(head));
        }
    }

    #[test]
    fn plain_http_is_forwarded_in_origin_form_and_closed_after_one_response() {
        let head = b"GET http://Example.com:8080/a/b?c=d HTTP/1.1\r\nHost: example.com:8080\r\nProxy-Connection: keep-alive\r\nConnection: keep-alive\r\nAccept: */*\r\n\r\n";
        let Some(Request::Forward { host, port, head }) = parse(head) else {
            panic!("not forwarded");
        };
        assert_eq!((host.as_str(), port), ("example.com", 8080));
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "GET /a/b?c=d HTTP/1.1\r\nHost: example.com:8080\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
    }

    async fn echo_server() -> (u16, tokio::task::JoinHandle<bool>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = tokio::spawn(async move {
            let Ok(Ok((mut stream, _))) =
                tokio::time::timeout(Duration::from_secs(2), listener.accept()).await
            else {
                return false;
            };
            let mut buffer = [0u8; 5];
            if stream.read_exact(&mut buffer).await.is_ok() {
                let _ = stream.write_all(&buffer).await;
            }
            true
        });
        (port, accepted)
    }

    fn only_ipv4_loopback() -> Allow {
        Arc::new(|host: &str| host == "127.0.0.1")
    }

    #[tokio::test]
    async fn an_allowed_tunnel_relays_bytes_both_ways() {
        let stop = CancellationToken::new();
        let proxy = bind(only_ipv4_loopback(), stop.clone()).await.unwrap();
        let (upstream, accepted) = echo_server().await;
        let mut client = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
        client
            .write_all(format!("CONNECT 127.0.0.1:{upstream} HTTP/1.1\r\n\r\nhello").as_bytes())
            .await
            .unwrap();
        let mut answer = vec![0u8; ESTABLISHED.len() + 5];
        client.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer[..ESTABLISHED.len()], ESTABLISHED);
        assert_eq!(&answer[ESTABLISHED.len()..], b"hello");
        assert!(accepted.await.unwrap());
        stop.cancel();
    }

    #[tokio::test]
    async fn a_host_outside_the_allowlist_is_refused_without_connecting() {
        let stop = CancellationToken::new();
        let proxy = bind(only_ipv4_loopback(), stop.clone()).await.unwrap();
        for request in [
            "CONNECT localhost:{port} HTTP/1.1\r\n\r\n",
            "GET http://localhost:{port}/socket HTTP/1.1\r\nUpgrade: websocket\r\n\r\n",
        ] {
            let (upstream, accepted) = echo_server().await;
            let mut client = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
            client
                .write_all(request.replace("{port}", &upstream.to_string()).as_bytes())
                .await
                .unwrap();
            let mut answer = Vec::new();
            client.read_to_end(&mut answer).await.unwrap();
            assert_eq!(answer, FORBIDDEN, "{request}");
            assert!(!accepted.await.unwrap(), "{request} reached the server");
        }
        stop.cancel();
    }

    #[tokio::test]
    async fn a_malformed_request_is_answered_400() {
        let stop = CancellationToken::new();
        let proxy = bind(only_ipv4_loopback(), stop.clone()).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
        client
            .write_all(b"GET /relative HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).await.unwrap();
        assert_eq!(answer, BAD_REQUEST);
        stop.cancel();
    }
}
