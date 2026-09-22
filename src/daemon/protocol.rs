use super::model::{JobRecord, JobRunRecord, JobRunState};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonRequest {
    Status,
    Create { job: JobRecord },
    List,
    Get { job_id: String },
    SetPaused { job_id: String, paused: bool },
    Delete { job_id: String },
    History { job_id: String, limit: usize },
    RunNow { job_id: String },
    Shutdown { instance_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub pid: u32,
    pub process_start_time: u64,
    pub instance_id: String,
    pub uptime_seconds: u64,
    pub socket_path: String,
    pub database_path: String,
    pub active_runs: usize,
    pub next_wake_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    Status { status: DaemonStatus },
    Ack,
    Job { job: JobRecord },
    Jobs { jobs: Vec<JobRecord> },
    History { runs: Vec<JobRunRecord> },
    RunAccepted { job_id: String, state: JobRunState },
    Error { code: String, message: String },
}

impl DaemonResponse {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(std::io::Error),
    FrameTooLarge,
    UnexpectedEof,
    InvalidJson(serde_json::Error),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "protocol I/O error: {error}"),
            Self::FrameTooLarge => {
                write!(formatter, "daemon frame exceeds {MAX_FRAME_BYTES} bytes")
            }
            Self::UnexpectedEof => write!(formatter, "daemon frame ended before newline"),
            Self::InvalidJson(error) => write!(formatter, "invalid daemon JSON: {error}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<std::io::Error> for ProtocolError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn decode<T: DeserializeOwned>(mut frame: Vec<u8>) -> Result<T, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES && frame.last() != Some(&b'\n') {
        return Err(ProtocolError::FrameTooLarge);
    }
    if frame.last() != Some(&b'\n') {
        return Err(ProtocolError::UnexpectedEof);
    }
    frame.pop();
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    serde_json::from_slice(&frame).map_err(ProtocolError::InvalidJson)
}

pub fn read_frame<R: Read, T: DeserializeOwned>(reader: R) -> Result<T, ProtocolError> {
    let mut frame = Vec::new();
    BufReader::new(reader)
        .take((MAX_FRAME_BYTES + 2) as u64)
        .read_until(b'\n', &mut frame)?;
    if frame.len() > MAX_FRAME_BYTES + 1 {
        return Err(ProtocolError::FrameTooLarge);
    }
    decode(frame)
}

pub fn write_frame<W: Write, T: Serialize>(mut writer: W, value: &T) -> Result<(), ProtocolError> {
    let frame = serde_json::to_vec(value).map_err(ProtocolError::InvalidJson)?;
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    writer.write_all(&frame)?;
    writer.write_all(b"\n")?;
    Ok(())
}

pub async fn read_async_frame<R: AsyncBufRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> Result<T, ProtocolError> {
    let mut frame = Vec::new();
    reader
        .take((MAX_FRAME_BYTES + 2) as u64)
        .read_until(b'\n', &mut frame)
        .await?;
    if frame.len() > MAX_FRAME_BYTES + 1 {
        return Err(ProtocolError::FrameTooLarge);
    }
    decode(frame)
}

pub async fn write_async_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), ProtocolError> {
    let frame = serde_json::to_vec(value).map_err(ProtocolError::InvalidJson)?;
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    writer.write_all(&frame).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn request_and_response_frames_round_trip() {
        let request = DaemonRequest::History {
            job_id: "daily-report".into(),
            limit: 12,
        };
        let mut request_wire = Vec::new();
        write_frame(&mut request_wire, &request).unwrap();
        assert_eq!(request_wire.last(), Some(&b'\n'));
        assert_eq!(
            read_frame::<_, DaemonRequest>(Cursor::new(request_wire)).unwrap(),
            request
        );

        let response = DaemonResponse::RunAccepted {
            job_id: "daily-report".into(),
            state: JobRunState::Claimed,
        };
        let mut response_wire = Vec::new();
        write_frame(&mut response_wire, &response).unwrap();
        assert_eq!(
            read_frame::<_, DaemonResponse>(Cursor::new(response_wire)).unwrap(),
            response
        );
    }

    #[test]
    fn frame_limit_rejects_payload_before_unbounded_allocation() {
        let input = vec![b'x'; MAX_FRAME_BYTES + 1];
        let error = read_frame::<_, DaemonRequest>(Cursor::new(input)).unwrap_err();
        assert!(matches!(error, ProtocolError::FrameTooLarge));
    }

    #[test]
    fn malformed_request_is_rejected_as_invalid_json() {
        let error = read_frame::<_, DaemonRequest>(Cursor::new(b"{bad json}\n")).unwrap_err();
        assert!(matches!(error, ProtocolError::InvalidJson(_)));
    }

    #[test]
    fn error_response_serializes_stable_code_and_message() {
        let response = DaemonResponse::error("not_found", "job missing");
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["code"], "not_found");
        assert_eq!(json["message"], "job missing");
    }

    #[tokio::test]
    async fn async_frames_enforce_limit_and_require_newline() {
        let mut oversized = std::io::Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 1]);
        assert!(matches!(
            read_async_frame::<_, DaemonRequest>(&mut oversized).await,
            Err(ProtocolError::FrameTooLarge)
        ));
        let mut truncated = std::io::Cursor::new(b"{\"type\":\"status\"}");
        assert!(matches!(
            read_async_frame::<_, DaemonRequest>(&mut truncated).await,
            Err(ProtocolError::UnexpectedEof)
        ));
        let mut frames = std::io::Cursor::new(b"{\"type\":\"status\"}\n{\"type\":\"list\"}\n");
        assert_eq!(
            read_async_frame::<_, DaemonRequest>(&mut frames)
                .await
                .unwrap(),
            DaemonRequest::Status
        );
        assert_eq!(
            read_async_frame::<_, DaemonRequest>(&mut frames)
                .await
                .unwrap(),
            DaemonRequest::List
        );
        let response = DaemonResponse::error("large", "x".repeat(MAX_FRAME_BYTES));
        let mut output = Vec::new();
        assert!(matches!(
            write_async_frame(&mut output, &response).await,
            Err(ProtocolError::FrameTooLarge)
        ));
        assert!(output.is_empty());
    }

    #[test]
    fn exact_frame_limit_is_accepted_and_larger_output_is_atomic() {
        let mut input = vec![b' '; MAX_FRAME_BYTES - 17];
        input.extend_from_slice(b"{\"type\":\"status\"}\n");
        assert_eq!(input.len(), MAX_FRAME_BYTES + 1);
        assert_eq!(
            read_frame::<_, DaemonRequest>(&input[..]).unwrap(),
            DaemonRequest::Status
        );
        let mut output = Vec::new();
        assert!(matches!(
            write_frame(
                &mut output,
                &DaemonResponse::error("large", "x".repeat(MAX_FRAME_BYTES))
            ),
            Err(ProtocolError::FrameTooLarge)
        ));
        assert!(output.is_empty());
    }
}
