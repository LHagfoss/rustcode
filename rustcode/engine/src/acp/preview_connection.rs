//! A paired client relays a browser connection in bounded chunks over ACP.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout};

const CHUNK: usize = 32 * 1024;
const IDLE: Duration = Duration::from_secs(15);

struct Stream {
    session_id: String,
    socket: tokio::sync::Mutex<TcpStream>,
    used: Mutex<Instant>,
}

static STREAMS: LazyLock<Mutex<HashMap<String, Arc<Stream>>>> = LazyLock::new(Default::default);

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum Request {
    Open { session_id: String, port: u16 },
    Exchange { id: String, data: String, eof: bool },
    Close { id: String },
}

pub(super) async fn handle(request: Request) -> Result<Value, String> {
    match request {
        Request::Open { session_id, port } => {
            super::preview_share::validate(&session_id, port)?;
            let socket = timeout(
                Duration::from_secs(3),
                TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
            )
            .await
            .map_err(|_| "The preview server did not answer.".to_owned())?
            .map_err(|error| format!("Could not connect to preview port {port}: {error}"))?;
            let mut streams = STREAMS.lock().unwrap();
            if streams.len() >= 64 {
                return Err("Too many active preview connections.".to_owned());
            }
            let id = uuid::Uuid::new_v4().to_string();
            let stream = Arc::new(Stream {
                session_id,
                socket: tokio::sync::Mutex::new(socket),
                used: Mutex::new(Instant::now()),
            });
            streams.insert(id.clone(), Arc::clone(&stream));
            let key = id.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(IDLE).await;
                    let mut streams = STREAMS.lock().unwrap();
                    if !streams.contains_key(&key) {
                        break;
                    }
                    if stream.used.lock().unwrap().elapsed() >= IDLE {
                        streams.remove(&key);
                        break;
                    }
                }
            });
            Ok(json!({"id": id}))
        }
        Request::Close { id } => {
            STREAMS.lock().unwrap().remove(&id);
            Ok(json!({}))
        }
        Request::Exchange { id, data, eof } => {
            if data.len() > CHUNK.div_ceil(3) * 4 {
                return Err("Preview data chunk is too large.".to_owned());
            }
            let data = STANDARD.decode(data).map_err(|error| error.to_string())?;
            if data.len() > CHUNK {
                return Err("Preview data chunk is too large.".to_owned());
            }
            let stream = STREAMS
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or_else(|| "The preview connection ended.".to_owned())?;
            if crate::thread_status::is_settled(&stream.session_id) {
                STREAMS.lock().unwrap().remove(&id);
                return Err("The preview thread is settled.".to_owned());
            }
            *stream.used.lock().unwrap() = Instant::now();
            let mut socket = stream
                .socket
                .try_lock()
                .map_err(|_| "Preview transfer already in progress.".to_owned())?;
            timeout(Duration::from_secs(3), async {
                socket.write_all(&data).await?;
                if eof {
                    socket.shutdown().await?;
                }
                Ok::<_, std::io::Error>(())
            })
            .await
            .map_err(|_| "Preview write timed out.".to_owned())?
            .map_err(|error| error.to_string())?;
            let mut buffer = vec![0; CHUNK];
            match timeout(Duration::from_millis(200), socket.read(&mut buffer)).await {
                Ok(Ok(n)) => Ok(json!({"data": STANDARD.encode(&buffer[..n]), "eof": n == 0})),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Ok(json!({"data": "", "eof": false})),
            }
        }
    }
}
