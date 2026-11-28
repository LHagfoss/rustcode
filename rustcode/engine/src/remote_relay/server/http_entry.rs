//! The relay's HTTP entry, for a client that has only WebSocket and `fetch`
//! (the phone app).
//!
//! `GET /v1/challenge` issues a single-use challenge. `GET /v1/dial` with
//! `Upgrade: websocket` names the machine, the device key, that challenge and
//! the device's signature of [`signed_message`] for it in its query. Once the
//! signature checks out and the machine opens a pipe, the relay writes the
//! request head into the pipe unchanged and splices: the machine's gateway
//! answers the upgrade itself, so the relay stays blind. A refusal before the
//! splice is a plain HTTP status with `{"code", "message"}` JSON.

use super::{Halves, Relay, WRITE_TIMEOUT, splice};
use crate::remote_gateway::device_key::{normalize_public_key, verify};
use crate::remote_relay::protocol::{ErrorCode, Reply, Role, signed_message};
use crate::remote_relay::state::random_token;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::OwnedSemaphorePermit;

/// The longest request head the entry reads.
pub(in super::super) const MAX_HEAD_BYTES: usize = 8 * 1024;

/// How long an issued challenge may be signed and spent.
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(30);

/// Challenges issued by `GET /v1/challenge` and not yet spent, at most `cap`.
pub(super) struct Challenges {
    issued: Mutex<HashMap<String, Instant>>,
    cap: usize,
}

impl Challenges {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            issued: Mutex::new(HashMap::new()),
            cap,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        self.issued
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A fresh challenge, or `None` while `cap` live ones are outstanding.
    fn issue(&self) -> Option<String> {
        let now = Instant::now();
        let mut issued = self.lock();
        issued.retain(|_, at| now.duration_since(*at) < CHALLENGE_LIFETIME);
        if issued.len() >= self.cap {
            return None;
        }
        let challenge = random_token(32);
        issued.insert(challenge.clone(), now);
        Some(challenge)
    }

    /// Spend `challenge`: true once, while it is live.
    fn spend(&self, challenge: &str) -> bool {
        self.lock()
            .remove(challenge)
            .is_some_and(|at| at.elapsed() < CHALLENGE_LIFETIME)
    }
}

/// Why no request head was read.
enum HeadError {
    /// The client went away before sending anything.
    Closed,
    /// Too long, cut off or unreadable.
    Malformed,
}

/// The parts of a request the entry acts on.
struct HttpRequest {
    path: String,
    query: HashMap<String, String>,
    upgrade: bool,
}

impl HttpRequest {
    fn parse(head: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(head).ok()?;
        let mut lines = text.lines();
        let mut request_line = lines.next()?.split(' ');
        let method = request_line.next()?;
        let target = request_line.next()?;
        let version = request_line.next()?;
        if method != "GET" || !version.starts_with("HTTP/1.") {
            return None;
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let query = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        let upgrade = lines
            .filter_map(|line| line.split_once(':'))
            .any(|(name, value)| {
                name.trim().eq_ignore_ascii_case("upgrade")
                    && value.trim().eq_ignore_ascii_case("websocket")
            });
        Some(Self {
            path: path.to_owned(),
            query,
            upgrade,
        })
    }

    fn param(&self, name: &str) -> Result<&str, Reply> {
        self.query
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Reply::error(
                    ErrorCode::BadRequest,
                    format!("the dial has no `{name}` parameter"),
                )
            })
    }
}

impl Relay {
    /// Serve one HTTP request on a connection that opened with neither TLS
    /// nor a relay frame.
    pub(super) async fn http_entry(
        &self,
        client: Halves,
        deadline: tokio::time::Instant,
        permit: OwnedSemaphorePermit,
    ) {
        let (mut reader, mut writer) = client;
        let head = match tokio::time::timeout_at(deadline, read_head(&mut reader)).await {
            Ok(Ok(head)) => head,
            Ok(Err(HeadError::Closed)) => return,
            Ok(Err(HeadError::Malformed)) => {
                let refusal = Reply::error(
                    ErrorCode::BadRequest,
                    "the request head is too long, cut off or not HTTP",
                );
                respond_refusal(&mut writer, &refusal).await;
                return;
            }
            Err(_) => {
                let refusal = Reply::error(ErrorCode::Timeout, "no request arrived in time");
                respond_refusal(&mut writer, &refusal).await;
                return;
            }
        };
        let Some(request) = HttpRequest::parse(&head) else {
            let refusal = Reply::error(ErrorCode::BadRequest, "expected an HTTP/1.1 GET request");
            respond_refusal(&mut writer, &refusal).await;
            return;
        };
        match request.path.as_str() {
            "/v1/challenge" => self.answer_challenge(&mut writer).await,
            "/v1/dial" => {
                let (machine_id, public_key) = match self.check_dial(&request) {
                    Ok(checked) => checked,
                    Err(refusal) => {
                        respond_refusal(&mut writer, &refusal).await;
                        return;
                    }
                };
                let (member, slot) = match self.open_pipe(&machine_id, &public_key).await {
                    Ok(opened) => opened,
                    Err(refusal) => {
                        respond_refusal(&mut writer, &refusal).await;
                        return;
                    }
                };
                drop(permit);
                let (member_reader, mut member_writer) = member;
                let forwarded = tokio::time::timeout(WRITE_TIMEOUT, async {
                    member_writer.write_all(&head).await?;
                    member_writer.flush().await
                })
                .await;
                if !matches!(forwarded, Ok(Ok(()))) {
                    return;
                }
                splice(
                    (reader, writer),
                    (member_reader, member_writer),
                    self.limits.idle_timeout,
                    self.shutdown.clone(),
                )
                .await;
                drop(slot);
            }
            _ => {
                let refusal = Reply::error(
                    ErrorCode::BadRequest,
                    "this relay answers GET /v1/challenge and GET /v1/dial",
                );
                respond_refusal(&mut writer, &refusal).await;
            }
        }
    }

    async fn answer_challenge<W: AsyncWrite + Unpin>(&self, writer: &mut W) {
        match self.challenges.issue() {
            Some(challenge) => {
                let body = json!({ "challenge": challenge, "relay_id": self.relay_id() });
                respond(writer, 200, &body).await;
            }
            None => {
                let refusal = Reply::error(
                    ErrorCode::Busy,
                    "too many challenges are outstanding; try again shortly",
                );
                respond_refusal(writer, &refusal).await;
            }
        }
    }

    /// The machine a dial request names and the device key it proved it
    /// holds, normalized, after spending its challenge.
    fn check_dial(&self, request: &HttpRequest) -> Result<(String, String), Reply> {
        if !request.upgrade {
            return Err(Reply::error(
                ErrorCode::BadRequest,
                "a dial is a WebSocket upgrade",
            ));
        }
        let machine_id = request.param("machine")?;
        let key = request.param("key")?;
        let challenge = request.param("challenge")?;
        let signature = request.param("signature")?;
        let public_key = normalize_public_key(key)
            .ok_or_else(|| Reply::error(ErrorCode::BadRequest, "the public key is not one"))?;
        if !self.challenges.spend(challenge) {
            return Err(Reply::error(
                ErrorCode::Unauthorized,
                "the challenge is unknown, spent or expired; fetch another",
            ));
        }
        let message = signed_message(Role::Client, &self.relay_id(), challenge, machine_id);
        if !verify(&public_key, &message, signature) {
            return Err(Reply::error(
                ErrorCode::Unauthorized,
                "the signature does not match the key",
            ));
        }
        Ok((machine_id.to_owned(), public_key))
    }
}

/// The request head through its blank line, reading no byte past it.
async fn read_head<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, HeadError> {
    let mut head = Vec::new();
    loop {
        let room = MAX_HEAD_BYTES.saturating_sub(head.len()) as u64;
        let read = (&mut *reader)
            .take(room)
            .read_until(b'\n', &mut head)
            .await
            .map_err(|_| HeadError::Malformed)?;
        if read == 0 {
            return Err(if head.is_empty() {
                HeadError::Closed
            } else {
                HeadError::Malformed
            });
        }
        if head.ends_with(b"\n\r\n") || head.ends_with(b"\n\n") {
            return Ok(head);
        }
    }
}

/// The HTTP status a refusal answers with.
fn status_of(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::UnsupportedVersion | ErrorCode::BadRequest => 400,
        ErrorCode::Unauthorized => 401,
        ErrorCode::KeyMismatch | ErrorCode::NotAllowed => 403,
        ErrorCode::MachineOffline => 404,
        ErrorCode::Busy => 503,
        ErrorCode::Timeout => 504,
    }
}

async fn respond_refusal<W: AsyncWrite + Unpin>(writer: &mut W, refusal: &Reply) {
    let (code, message) = match refusal {
        Reply::Error { code, message } => (*code, message.as_str()),
        _ => (
            ErrorCode::BadRequest,
            "the relay could not serve this request",
        ),
    };
    respond(
        writer,
        status_of(code),
        &json!({ "code": code, "message": message }),
    )
    .await;
}

/// Answer with `status` and `body` as JSON, then close.
async fn respond<W: AsyncWrite + Unpin>(writer: &mut W, status: u16, body: &Value) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::time::timeout(WRITE_TIMEOUT, async {
        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?;
        writer.shutdown().await
    })
    .await;
}
