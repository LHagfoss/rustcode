//! A browser-facing loopback listener. Each accepted socket gets its own
//! authenticated ACP channel so preview traffic cannot delay chat requests.

use super::{config_dir, Machines};
use base64::{engine::general_purpose::STANDARD, Engine};
use rustcode_client::{AcpChannel, MachineRegistry};
use serde_json::{json, Value};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::{timeout, Instant},
};

const METHOD: &str = "_rustcode/preview/connection";
const CHUNK: usize = 32 * 1024;

#[tauri::command]
pub async fn machines_preview(
    machines: tauri::State<'_, Arc<Machines>>,
    host: String,
    session_id: String,
    port: u16,
) -> Result<String, String> {
    if port == 0 {
        return Err("Choose a valid preview port.".into());
    }
    let config = config_dir()?;
    let machine = MachineRegistry::load(&config)
        .map_err(super::chain)?
        .get(&host)
        .cloned()
        .ok_or_else(|| "This machine is no longer paired.".to_owned())?;
    let tokens = machines.inner().tokens(&config).await?;
    let connector = Arc::new(Machines::connector(&config)?);
    let machine = Arc::new(machine);
    let session_id = Arc::new(session_id);

    let first = connector
        .connect_raced(&machine, &tokens)
        .await
        .map_err(super::chain)?;
    let mut first = first.session.acp().await.map_err(super::chain)?;
    let id = open(&mut first, &session_id, port)
        .await
        .map_err(super::chain)?;
    let _ = first
        .request(3, METHOD, json!({"action": "close", "id": id}))
        .await;

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| error.to_string())?;
    let address = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .to_string();
    let expected_host = address.clone();
    tauri::async_runtime::spawn(async move {
        let until = Instant::now() + Duration::from_secs(60 * 60);
        let mut tasks = JoinSet::new();
        let mut first = Some(first);
        while let Ok(Ok((socket, _))) = tokio::time::timeout_at(until, listener.accept()).await {
            while tasks.try_join_next().is_some() {}
            if tasks.len() >= 32 {
                continue;
            }
            let connector = Arc::clone(&connector);
            let tokens = Arc::clone(&tokens);
            let machine = Arc::clone(&machine);
            let session_id = Arc::clone(&session_id);
            let expected_host = expected_host.clone();
            let first = first.take();
            tasks.spawn(async move {
                let _ = serve(socket, first, &expected_host, &session_id, port, async {
                    connector
                        .connect_raced(&machine, &tokens)
                        .await?
                        .session
                        .acp()
                        .await
                })
                .await;
            });
        }
    });
    Ok(format!("http://{address}/"))
}

async fn open(channel: &mut AcpChannel, session_id: &str, port: u16) -> anyhow::Result<String> {
    channel
        .request(1, "initialize", json!({"protocolVersion": 1}))
        .await?;
    let response = channel
        .request(
            2,
            METHOD,
            json!({
                "action": "open", "sessionId": session_id, "port": port,
            }),
        )
        .await?;
    response["result"]["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("The machine did not return a preview connection."))
}

async fn serve(
    mut socket: TcpStream,
    first: Option<AcpChannel>,
    expected_host: &str,
    session_id: &str,
    port: u16,
    connect: impl Future<Output = anyhow::Result<AcpChannel>>,
) -> anyhow::Result<()> {
    let mut request = Vec::with_capacity(CHUNK);
    let mut buffer = vec![0; CHUNK];
    let header_end = timeout(Duration::from_secs(5), async {
        loop {
            let n = socket.read(&mut buffer).await?;
            anyhow::ensure!(n > 0, "Browser disconnected");
            request.extend_from_slice(&buffer[..n]);
            if let Some(end) = request.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
                return Ok::<_, anyhow::Error>(end);
            }
            anyhow::ensure!(
                request.len() < CHUNK,
                "Preview request headers are too large"
            );
        }
    })
    .await??;
    let header = std::str::from_utf8(&request[..header_end])?;
    if !allowed_request(header, expected_host) {
        socket
            .write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(());
    }
    let initialized = first.is_some();
    let mut channel = match first {
        Some(channel) => channel,
        None => connect.await?,
    };
    let id = if initialized {
        stream_id(
            &channel
                .request(
                    4,
                    METHOD,
                    json!({
                        "action": "open", "sessionId": session_id, "port": port,
                    }),
                )
                .await?,
        )?
    } else {
        open(&mut channel, session_id, port).await?
    };
    let result = relay(&mut channel, &id, &mut socket, &request).await;
    let _ = channel
        .request(u64::MAX / 2, METHOD, json!({"action": "close", "id": id}))
        .await;
    result
}

fn stream_id(response: &Value) -> anyhow::Result<String> {
    response["result"]["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("The machine did not open the preview stream."))
}

fn allowed_request(header: &str, host: &str) -> bool {
    let mut lines = header.split("\r\n");
    if !lines.next().is_some_and(|line| {
        line.split_whitespace()
            .nth(1)
            .is_some_and(|target| target.starts_with('/') && !target.starts_with("//"))
    }) {
        return false;
    }
    let mut hosts = 0;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        if name.is_empty() || name.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return false;
        }
        let value = value.trim();
        if name.eq_ignore_ascii_case("host") {
            if value != host {
                return false;
            }
            hosts += 1;
        }
        if name.eq_ignore_ascii_case("origin") && value.strip_prefix("http://") != Some(host) {
            return false;
        }
        if name.eq_ignore_ascii_case("sec-fetch-site") && !matches!(value, "none" | "same-origin") {
            return false;
        }
    }
    hosts == 1
}

async fn relay(
    channel: &mut AcpChannel,
    id: &str,
    socket: &mut TcpStream,
    initial: &[u8],
) -> anyhow::Result<()> {
    let mut buffer = vec![0; CHUNK];
    let mut next_id = 5_u64;
    let mut initial = initial.chunks(CHUNK);
    let mut browser_eof = false;
    loop {
        let (data, eof) = if let Some(chunk) = initial.next() {
            (chunk, false)
        } else if browser_eof {
            (&[][..], false)
        } else {
            match timeout(Duration::from_millis(5), socket.read(&mut buffer)).await {
                Ok(Ok(0)) => {
                    browser_eof = true;
                    (&[][..], true)
                }
                Ok(Ok(n)) => (&buffer[..n], false),
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => (&[][..], false),
            }
        };
        let response = channel
            .request(
                next_id,
                METHOD,
                json!({
                    "action": "exchange", "id": id, "data": STANDARD.encode(data), "eof": eof,
                }),
            )
            .await?;
        next_id += 1;
        let response = &response["result"];
        let body = response["data"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Preview response omitted data"))?;
        socket.write_all(&STANDARD.decode(body)?).await?;
        if response["eof"] == true {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::allowed_request;

    #[test]
    fn loopback_listener_rejects_foreign_hosts_and_origins() {
        let host = "127.0.0.1:41000";
        assert!(allowed_request(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:41000\r\nSec-Fetch-Site: none",
            host,
        ));
        assert!(allowed_request(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:41000\r\nOrigin: http://127.0.0.1:41000",
            host,
        ));
        for bad in [
            "Host: example.test",
            "Host: 127.0.0.1:41000\r\nHost: example.test",
            "Host: 127.0.0.1:41000\r\nHost : example.test",
            "Host: 127.0.0.1:41000\r\nOrigin: https://example.test",
            "Host: 127.0.0.1:41000\r\nSec-Fetch-Site: cross-site",
        ] {
            assert!(!allowed_request(&format!("GET / HTTP/1.1\r\n{bad}"), host));
        }
    }
}
