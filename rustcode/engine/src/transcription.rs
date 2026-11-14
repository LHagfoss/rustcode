//! Voice dictation: recorded audio in, text out, through an OpenAI-compatible
//! `/audio/transcriptions` endpoint. The provider and its key are a text
//! generator entry (`text_generation.rs`), so there is one key store. The
//! audio lives only in this request: it is never written to a thread's history,
//! a log or a file.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

/// Decoded audio above this is refused before anything is sent.
pub(crate) const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;
/// A recording longer than this is refused; the app stops recording at it.
pub(crate) const MAX_DURATION_MS: u64 = 5 * 60 * 1000;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Deserialize, Serialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/voice/transcribe", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranscribeRequest {
    /// The text generator entry whose endpoint and key transcribe.
    pub generator_id: String,
    pub model: String,
    #[serde(default)]
    pub language: Option<String>,
    /// The recording, base64.
    pub audio: String,
    pub mime_type: String,
    pub duration_ms: u64,
}

// The audio never reaches a log line, even through `{:?}`.
impl std::fmt::Debug for TranscribeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscribeRequest")
            .field("generator_id", &self.generator_id)
            .field("model", &self.model)
            .field("mime_type", &self.mime_type)
            .field("duration_ms", &self.duration_ms)
            .finish_non_exhaustive()
    }
}

/// The file extension the endpoint expects for a container the webviews record:
/// WebKit on macOS and WebKitGTK give `audio/mp4` or `audio/ogg`, WebView2 `audio/webm`.
fn extension(mime_type: &str) -> Option<&'static str> {
    let base = mime_type.split(';').next().unwrap_or_default().trim();
    Some(match base.to_ascii_lowercase().as_str() {
        "audio/webm" | "video/webm" => "webm",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "video/mp4" => "mp4",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" => "mp3",
        _ => return None,
    })
}

/// Checks a request and decodes its audio, before any key is read.
fn validate(r: &TranscribeRequest) -> Result<(Vec<u8>, &'static str)> {
    use base64::Engine;
    anyhow::ensure!(
        r.duration_ms <= MAX_DURATION_MS,
        "Recording is too long (the limit is {} minutes)",
        MAX_DURATION_MS / 60_000
    );
    anyhow::ensure!(
        r.audio.len() <= MAX_AUDIO_BYTES.div_ceil(3) * 4,
        "Recording is too large to transcribe"
    );
    let ext = extension(&r.mime_type).context("Unsupported audio format")?;
    let audio = base64::engine::general_purpose::STANDARD
        .decode(r.audio.as_bytes())
        .context("Recording could not be read")?;
    anyhow::ensure!(!audio.is_empty(), "Recording is empty");
    anyhow::ensure!(
        audio.len() <= MAX_AUDIO_BYTES,
        "Recording is too large to transcribe"
    );
    anyhow::ensure!(!r.model.trim().is_empty(), "Choose a transcription model");
    Ok((audio, ext))
}

/// A `multipart/form-data` body for `/audio/transcriptions`, and its boundary.
fn multipart(
    audio: &[u8],
    ext: &str,
    mime_type: &str,
    model: &str,
    language: Option<&str>,
) -> (String, Vec<u8>) {
    let boundary = format!("rustcode-{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::with_capacity(audio.len() + 512);
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };
    field("model", model.trim());
    field("response_format", "json");
    if let Some(language) = language.map(str::trim).filter(|l| !l.is_empty()) {
        field("language", language);
    }
    let base = mime_type.split(';').next().unwrap_or_default().trim();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.{ext}\"\r\nContent-Type: {base}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

pub(crate) async fn transcribe(r: TranscribeRequest) -> Result<Value> {
    let (audio, ext) = validate(&r)?;
    let config = crate::text_generation::load()?;
    let generator = config
        .generators
        .iter()
        .find(|g| g.id == r.generator_id)
        .context("No transcription provider: choose one in Settings › Agents › Voice")?;
    anyhow::ensure!(
        matches!(generator.provider.as_str(), "openai" | "custom"),
        "This provider has no speech-to-text endpoint"
    );
    let url = generator.profile()?.url;
    let key = crate::provider_auth::NativeCredentialStore
        .get("text-generation", &generator.id, "api-key")
        .await
        .context("Transcription credential unavailable")?;
    tokio::time::timeout(
        TIMEOUT,
        send(
            &url,
            &key,
            &audio,
            ext,
            &r.mime_type,
            &r.model,
            r.language.as_deref(),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Transcription timed out"))?
}

async fn send(
    base_url: &str,
    key: &str,
    audio: &[u8],
    ext: &str,
    mime_type: &str,
    model: &str,
    language: Option<&str>,
) -> Result<Value> {
    let (boundary, body) = multipart(audio, ext, mime_type, model, language);
    let mut request = reqwest::Client::new()
        .post(format!(
            "{}/audio/transcriptions",
            base_url.trim_end_matches('/')
        ))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body);
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Could not reach the transcription provider"))?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = answer["error"]["message"]
            .as_str()
            .map(|m| m.chars().take(200).collect::<String>());
        match detail {
            Some(detail) => bail!("Transcription failed (HTTP {}): {detail}", status.as_u16()),
            None => bail!("Transcription failed (HTTP {})", status.as_u16()),
        }
    }
    let text = answer["text"]
        .as_str()
        .context("Transcription provider returned no text")?
        .trim();
    Ok(json!({ "text": text }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn request(audio: &[u8], mime: &str, duration_ms: u64) -> TranscribeRequest {
        TranscribeRequest {
            generator_id: "g".into(),
            model: "gpt-4o-mini-transcribe".into(),
            language: Some("en".into()),
            audio: base64::engine::general_purpose::STANDARD.encode(audio),
            mime_type: mime.into(),
            duration_ms,
        }
    }

    #[test]
    fn accepts_the_webview_containers() {
        for mime in [
            "audio/webm;codecs=opus",
            "audio/mp4",
            "audio/ogg; codecs=opus",
            "audio/wav",
        ] {
            assert!(validate(&request(b"abc", mime, 1000)).is_ok(), "{mime}");
        }
        assert!(validate(&request(b"abc", "audio/flac", 1000)).is_err());
    }

    #[test]
    fn refuses_long_large_or_empty_recordings() {
        let long = validate(&request(b"abc", "audio/webm", MAX_DURATION_MS + 1)).unwrap_err();
        assert!(long.to_string().contains("too long"));
        let large =
            validate(&request(&vec![0; MAX_AUDIO_BYTES + 1], "audio/webm", 1000)).unwrap_err();
        assert!(large.to_string().contains("too large"));
        assert!(validate(&request(b"", "audio/webm", 1000)).is_err());
    }

    #[test]
    fn debug_omits_the_audio() {
        let text = format!("{:?}", request(b"secret-audio", "audio/webm", 1000));
        assert!(!text.contains(&base64::engine::general_purpose::STANDARD.encode(b"secret-audio")));
    }

    #[tokio::test]
    async fn fake_endpoint_receives_a_multipart_transcription_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = socket.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|v| v.parse().ok())
                        })
                        .unwrap();
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let answer = br#"{"text":" hello world "}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        answer.len()
                    )
                    .as_bytes(),
                )
                .unwrap();
            socket.write_all(answer).unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        });
        let answer = send(
            &format!("http://{address}/v1/"),
            "fake-secret",
            b"AUDIO",
            "webm",
            "audio/webm;codecs=opus",
            "whisper-1",
            Some("nb"),
        )
        .await
        .unwrap();
        assert_eq!(answer["text"], "hello world");
        let raw = receiver.await.unwrap();
        let lower = raw.to_lowercase();
        assert!(raw.starts_with("POST /v1/audio/transcriptions "));
        assert!(lower.contains("authorization: bearer fake-secret"));
        assert!(lower.contains("content-type: multipart/form-data; boundary=rustcode-"));
        assert!(raw.contains("name=\"model\"\r\n\r\nwhisper-1\r\n"));
        assert!(raw.contains("name=\"language\"\r\n\r\nnb\r\n"));
        assert!(
            raw.contains(
                "filename=\"dictation.webm\"\r\nContent-Type: audio/webm\r\n\r\nAUDIO\r\n"
            )
        );
    }
}
