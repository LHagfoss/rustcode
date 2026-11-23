//! Temporary public links to explicitly selected loopback HTTP servers.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const LIFETIME_MINUTES: i64 = 60;

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum Request {
    List,
    Start {
        session_id: String,
        port: u16,
        approved: bool,
    },
    Stop {
        id: String,
    },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Link {
    id: String,
    session_id: String,
    port: u16,
    url: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

struct Share {
    link: Link,
    cancel: CancellationToken,
}

static SHARES: LazyLock<Mutex<HashMap<String, Share>>> = LazyLock::new(Default::default);

pub(super) async fn handle(request: Request) -> Result<Value, String> {
    match request {
        Request::List => {
            let shares = SHARES.lock().unwrap();
            let mut links: Vec<_> = shares
                .values()
                .filter(|share| !share.cancel.is_cancelled())
                .map(|share| &share.link)
                .filter(|link| !link.url.is_empty())
                .collect();
            links.sort_by_key(|link| link.expires_at);
            Ok(json!({"links": links}))
        }
        Request::Stop { id } => {
            if let Some(share) = SHARES.lock().unwrap().get(&id) {
                share.cancel.cancel();
            }
            Ok(json!({}))
        }
        Request::Start {
            session_id,
            port,
            approved,
        } => {
            if !approved {
                return Err("Confirm public access for this preview first.".to_owned());
            }
            start(session_id, port).await
        }
    }
}

pub(super) fn validate(session_id: &str, port: u16) -> Result<(), String> {
    if port == 0
        || !rustcode_session::valid_session_id(session_id)
        || !crate::sessions::get_active_session_dir(session_id).is_some_and(|dir| dir.is_dir())
        || crate::thread_status::is_settled(session_id)
    {
        return Err("Choose an active thread and a valid local port.".to_owned());
    }
    Ok(())
}

async fn start(session_id: String, port: u16) -> Result<Value, String> {
    validate(&session_id, port)?;

    let address = format!("127.0.0.1:{port}");
    tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&address),
    )
    .await
    .map_err(|_| format!("Timed out connecting to {address}."))?
    .map_err(|error| format!("Could not connect to {address}: {error}"))?;

    let id = uuid::Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(LIFETIME_MINUTES);
    {
        let mut shares = SHARES.lock().unwrap();
        if shares.len() >= 8 {
            return Err("Stop an existing preview link before starting another.".to_owned());
        }
        shares.insert(
            id.clone(),
            Share {
                link: Link {
                    id: id.clone(),
                    session_id: session_id.clone(),
                    port,
                    url: String::new(),
                    expires_at,
                },
                cancel: cancel.clone(),
            },
        );
    }
    let (ready, result) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let outcome = run(&id, &session_id, &address, cancel, ready).await;
        SHARES.lock().unwrap().remove(&id);
        if let Err(error) = outcome {
            tracing::debug!(%error, "preview tunnel stopped");
        }
    });
    result
        .await
        .map_err(|_| "The preview tunnel stopped before it returned a URL.".to_owned())?
}

async fn run(
    id: &str,
    session_id: &str,
    address: &str,
    cancel: CancellationToken,
    ready: tokio::sync::oneshot::Sender<Result<Value, String>>,
) -> Result<(), String> {
    // An existing account tunnel can carry its own ingress rules. Give this
    // process an isolated configuration so only the approved port is routed.
    let config = match tempfile::NamedTempFile::new().and_then(|mut file| {
        std::io::Write::write_all(&mut file, b"{}\n")?;
        Ok(file)
    }) {
        Ok(config) => config,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "Could not prepare the preview tunnel: {error}"
            )));
            return Ok(());
        }
    };
    let mut command = Command::new("cloudflared");
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if name.starts_with("TUNNEL_") || name.starts_with("CLOUDFLARED_") {
            command.env_remove(key);
        }
    }
    let child = command
        .args(["tunnel", "--config"])
        .arg(config.path())
        .args([
            "--no-autoupdate",
            "--url",
            &format!("http://{address}"),
            "--http-host-header",
            address,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            let _ = ready.send(Err(format!("Could not start cloudflared: {error}")));
            return Ok(());
        }
    };
    let mut lines = BufReader::new(child.stderr.take().expect("piped stderr")).lines();
    let mut ready = Some(ready);
    let startup = tokio::time::sleep(Duration::from_secs(25));
    let expiry = tokio::time::sleep(Duration::from_secs((LIFETIME_MINUTES * 60) as u64));
    tokio::pin!(startup, expiry);
    let mut status = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = &mut expiry => break,
            _ = &mut startup, if ready.is_some() => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err("Cloudflare did not return a preview URL in time.".to_owned()));
                }
                break;
            }
            _ = status.tick() => {
                if crate::thread_status::is_settled(session_id) {
                    break;
                }
            }
            line = lines.next_line() => {
                let Some(line) = line.map_err(|error| error.to_string())? else { break };
                if ready.is_some() && let Some(url) = tunnel_url(&line) {
                    let result = {
                        let mut shares = SHARES.lock().unwrap();
                        let share = shares.get_mut(id).expect("share is removed after its task exits");
                        share.link.url = url.to_owned();
                        json!(share.link)
                    };
                    if ready.take().expect("sender checked above").send(Ok(result)).is_err() {
                        break;
                    }
                }
            }
        }
    }
    let _ = child.kill().await;
    Ok(())
}

fn tunnel_url(line: &str) -> Option<&str> {
    line.split_whitespace().find(|word| {
        word.strip_prefix("https://")
            .and_then(|host| host.strip_suffix(".trycloudflare.com"))
            .is_some_and(|label| {
                !label.is_empty()
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
    })
}

pub(crate) const TOOL_NAME: &str = "share_preview";

pub(crate) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "port": { "type": "integer", "minimum": 1, "maximum": 65535,
                "description": "Loopback HTTP port on this thread's machine. Prefer a static build server." }
        },
        "required": ["port"],
        "additionalProperties": false
    })
}

pub(crate) async fn answer(
    session_id: &str,
    args: &Value,
    asker: Option<crate::orchestration::ask_user::Asker>,
    cancel: &CancellationToken,
) -> Result<Value, String> {
    let port = args["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| "Choose a valid preview port.".to_owned())?;
    validate(session_id, port)?;
    let asker =
        asker.ok_or_else(|| "Open this thread in RustCode to approve sharing.".to_owned())?;
    let title = format!(
        "Share port {port} publicly for one hour? Anyone with the link can open it. Dev servers can expose source files."
    );
    if !crate::manage::approved(&asker, session_id, &title, args, cancel).await {
        return Err("Public preview sharing was declined.".to_owned());
    }
    if cancel.is_cancelled() {
        return Err("Public preview sharing was cancelled.".to_owned());
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Public preview sharing was cancelled.".to_owned()),
        result = start(session_id.to_owned(), port) => result,
    }
}

pub(crate) async fn execute(
    state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    args: &Value,
    cancel: &CancellationToken,
) -> rustcode_tools::ToolExecutionOutput {
    let (session_id, asker) = {
        let state = state.lock().await;
        (state.active_session_id.clone(), state.secret_asker.clone())
    };
    match answer(&session_id, args, asker, cancel).await {
        Ok(value) => rustcode_tools::ToolExecutionOutput::success(value.to_string()),
        Err(error) => rustcode_tools::ToolExecutionOutput::failure(error),
    }
}

pub(super) async fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
) -> Result<(), agent_client_protocol::Error> {
    let outcome = if method == "_rustcode/preview/share" {
        handle(super::parse_params(params)?).await
    } else {
        super::preview_connection::handle(super::parse_params(params)?).await
    };
    match outcome {
        Ok(answer) => responder.respond(answer),
        Err(error) => {
            responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tunnel_url;

    #[test]
    fn accepts_only_an_https_quick_tunnel_hostname() {
        assert_eq!(
            tunnel_url("INF | https://my-test-2.trycloudflare.com |"),
            Some("https://my-test-2.trycloudflare.com"),
        );
        for other in [
            "http://preview.trycloudflare.com",
            "https://preview.trycloudflare.com.example.com",
            "https://example.com@preview.trycloudflare.com",
            "https://preview.trycloudflare.com/some/path",
        ] {
            assert_eq!(tunnel_url(other), None);
        }
    }
}
