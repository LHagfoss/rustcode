//! Recording of the JSON-RPC traffic that crosses a harness's stdio
//! (`harness_rpc_messages`, `docs/data-store.md`).
//!
//! These rows are harness-to-RustCode protocol messages. What an external
//! harness asks its own model provider is its own HTTP traffic and never
//! passes through here.
//!
//! One [`RpcRecorder`] belongs to one peer, so one thread and one harness. The
//! RPC loop hands it every frame it parsed or sent and is done: the row is
//! built, merged with the streamed chunk it belongs to, and pushed into a
//! buffer, and a background thread writes the buffer to the store about once a
//! second. A locked or full store therefore cannot stall a turn.

use crate::data_store::redact::prepare_payload;
use crate::data_store::{
    self, DIRECTION_HARNESS_TO_RUSTCODE, DIRECTION_RUSTCODE_TO_HARNESS, KIND_NOTIFICATION,
    KIND_REQUEST, KIND_RESPONSE, MAX_RPC_PAYLOAD_BYTES, RpcMessageRow,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The most rows the buffer holds before it starts dropping and counting
/// them: the store is stuck or far behind, and the RPC loop must not grow.
const MAX_PENDING_ROWS: usize = 4096;
/// How many buffered rows wake the flusher before its interval is up.
const FLUSH_ROWS: usize = 128;
/// How long a row waits for a flush while the traffic is slower than that.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// The most requests whose latency is still being measured. A response to one
/// that fell off waits no longer than the next flush.
const MAX_TRACKED_REQUESTS: usize = 1024;

/// The streamed updates that arrive as one notification each token: their
/// rows are merged per flush window instead of stored one by one.
const STREAMED_UPDATES: [&str; 3] = [
    "agent_message_chunk",
    "agent_thought_chunk",
    "user_message_chunk",
];

/// What one frame said, as much as the store needs.
#[derive(Debug, PartialEq)]
struct Frame {
    kind: &'static str,
    id: Option<String>,
    method: Option<String>,
}

impl Frame {
    /// Read one frame the way the peer routes it: a `method` with an id is a
    /// request, one without is a notification, and a message with neither but
    /// a result or an error answers a request.
    fn of(message: &Value) -> Option<Self> {
        let method = message.get("method").and_then(Value::as_str);
        let id = message
            .get("id")
            .filter(|id| !id.is_null())
            .map(|id| match id {
                Value::String(id) => id.clone(),
                other => other.to_string(),
            });
        let kind = match (method, &id) {
            (Some(_), Some(_)) => KIND_REQUEST,
            (Some(_), None) => KIND_NOTIFICATION,
            (None, _) if message.get("result").is_some() || message.get("error").is_some() => {
                KIND_RESPONSE
            }
            (None, _) => return None,
        };
        Some(Self {
            kind,
            id,
            method: method.map(str::to_owned),
        })
    }
}

/// The update kind a streamed chunk notification carries, when it is one.
fn streamed_update(method: &str, params: &Value) -> Option<String> {
    match method {
        "session/update" => {
            let update = params.get("update")?.get("sessionUpdate")?.as_str()?;
            STREAMED_UPDATES
                .contains(&update)
                .then(|| update.to_owned())
        }
        // `codex app-server` streams in its own notifications (`harness/codex.rs`).
        // Command output is the bulk of the store and RustCode does not even
        // render it: it is folded like text, so only bytes are kept.
        "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
            Some("command_output".to_owned())
        }
        "item/agentMessage/delta" => Some("agent_message_chunk".to_owned()),
        "item/reasoning/summaryTextDelta"
        | "item/reasoning/summaryPartAdded"
        | "item/reasoning/textDelta" => Some("agent_thought_chunk".to_owned()),
        _ => None,
    }
}

/// Whether a frame's body is bulky, repeated noise: streaming output and
/// status pings. The caller keeps the metadata row and drops only the body.
fn is_noise_payload(method: Option<&str>) -> bool {
    method.is_some_and(|method| {
        method.ends_with("/outputDelta") || crate::data_store::NOISE_METHODS.contains(&method)
    })
}

/// A request whose answer has not been seen yet.
struct Tracked {
    method: String,
    at: Instant,
    sequence: u64,
}

#[derive(Default)]
struct Requests {
    entries: HashMap<String, Tracked>,
    next: u64,
}

impl Requests {
    fn key(direction: &str, id: &str) -> String {
        format!("{direction}|{id}")
    }

    /// Remember a request, dropping the oldest entry when the map is full.
    fn track(&mut self, direction: &str, id: &str, method: &str) {
        if self.entries.len() >= MAX_TRACKED_REQUESTS {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, tracked)| tracked.sequence)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.next += 1;
        self.entries.insert(
            Self::key(direction, id),
            Tracked {
                method: method.to_owned(),
                at: Instant::now(),
                sequence: self.next,
            },
        );
    }

    /// The method of the request a response answers, and how long it took.
    fn answer(&mut self, direction: &str, id: &str) -> Option<(String, i64)> {
        let tracked = self.entries.remove(&Self::key(direction, id))?;
        let latency = tracked.at.elapsed().as_millis().min(i64::MAX as u128) as i64;
        Some((tracked.method, latency))
    }
}

#[derive(Default)]
struct Pending {
    rows: Vec<RpcMessageRow>,
    /// Where a merged streamed chunk sits in `rows`, and how many frames it
    /// already stands for.
    streamed: HashMap<(String, String), (usize, u64)>,
    dropped: u64,
}

enum Wake {
    Rows,
    Stop,
}

/// Records one peer's frames for one thread.
pub(crate) struct RpcRecorder {
    thread_id: String,
    harness: Option<String>,
    protocol: Option<String>,
    /// `capture_harness_rpc_payloads`, read once when the peer starts.
    capture_payloads: bool,
    /// A store file of its own (tests); `None` is the data store, and the
    /// flusher owns the copy it writes to.
    #[cfg_attr(not(test), allow(dead_code))]
    path: Option<PathBuf>,
    requests: Mutex<Requests>,
    pending: Arc<Mutex<Pending>>,
    wake: Sender<Wake>,
    flusher: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl RpcRecorder {
    /// A recorder for `thread_id` on `harness`, at the configured store.
    pub(crate) fn new(
        thread_id: impl Into<String>,
        harness: Option<String>,
        protocol: Option<String>,
    ) -> Arc<Self> {
        let capture_payloads = crate::config::load_config().2.capture_harness_rpc_payloads;
        Self::with_target(None, thread_id.into(), harness, protocol, capture_payloads)
    }

    /// A recorder that writes to `path` instead of the data store (tests).
    #[cfg(test)]
    pub(crate) fn to_path(path: PathBuf, thread_id: &str, capture_payloads: bool) -> Arc<Self> {
        Self::with_target(
            Some(path),
            thread_id.to_owned(),
            Some("claude-code".to_owned()),
            Some("acp".to_owned()),
            capture_payloads,
        )
    }

    fn with_target(
        path: Option<PathBuf>,
        thread_id: String,
        harness: Option<String>,
        protocol: Option<String>,
        capture_payloads: bool,
    ) -> Arc<Self> {
        let pending: Arc<Mutex<Pending>> = Arc::default();
        let (wake, woken) = mpsc::channel();
        let buffered = Arc::clone(&pending);
        let target = path.clone();
        let flusher = match std::thread::Builder::new()
            .name("rpc-capture".to_owned())
            .spawn(move || flush_loop(&buffered, target.as_ref(), &woken))
        {
            Ok(flusher) => Some(flusher),
            Err(error) => {
                crate::dbg_log!("harness rpc: no recorder thread ({error}), nothing is stored");
                None
            }
        };
        Arc::new(Self {
            thread_id,
            harness,
            protocol,
            capture_payloads,
            path,
            requests: Mutex::default(),
            pending,
            wake,
            flusher: Mutex::new(flusher),
        })
    }

    /// One frame the harness sent, after it parsed. `bytes` is its length.
    pub(crate) fn record_inbound(&self, message: &Value, bytes: usize) {
        self.record(DIRECTION_HARNESS_TO_RUSTCODE, message, bytes);
    }

    /// One frame RustCode is about to write. `bytes` is its length.
    pub(crate) fn record_outbound(&self, message: &Value, bytes: usize) {
        self.record(DIRECTION_RUSTCODE_TO_HARNESS, message, bytes);
    }

    fn record(&self, direction: &str, message: &Value, bytes: usize) {
        let Some(frame) = Frame::of(message) else {
            return;
        };
        let mut method = frame.method.clone();
        let mut latency_ms = None;
        let id = frame.id.as_deref();
        match frame.kind {
            KIND_REQUEST => {
                let (Some(id), Some(name)) = (id, method.as_deref()) else {
                    return;
                };
                lock(&self.requests).track(direction, id, name);
            }
            KIND_RESPONSE => {
                let Some(id) = id else {
                    return;
                };
                // The request went the other way; the peer's own responses
                // answer what the harness asked RustCode.
                let answered = lock(&self.requests).answer(opposite(direction), id);
                if let Some((name, latency)) = answered {
                    method = Some(name);
                    latency_ms = Some(latency);
                }
            }
            _ => {}
        }
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut row = RpcMessageRow {
            thread_id: self.thread_id.clone(),
            turn_id: None,
            harness: self.harness.clone(),
            protocol: self.protocol.clone(),
            direction: direction.to_owned(),
            message_kind: frame.kind.to_owned(),
            rpc_id: frame.id.clone(),
            method: method.clone(),
            at,
            latency_ms,
            byte_size: Some(bytes as i64),
            payload_json: None,
            payload_truncated: false,
        };
        let streamed = match (frame.kind, method.as_deref()) {
            (KIND_NOTIFICATION, Some(name)) => {
                let params = message.get("params").unwrap_or(&Value::Null);
                streamed_update(name, params).map(|update| (name.to_owned(), update))
            }
            _ => None,
        };
        let Some(streamed) = streamed else {
            // A repeated status ping is noise: its row (method, at, size) is
            // worth keeping, its body is not.
            if self.capture_payloads && !is_noise_payload(method.as_deref()) {
                let (payload, truncated) = prepare_payload(message, MAX_RPC_PAYLOAD_BYTES);
                row.payload_json = Some(payload);
                row.payload_truncated = truncated;
            }
            self.push(row);
            return;
        };
        let mut pending = lock(&self.pending);
        self.push_streamed(&mut pending, row, streamed, bytes);
    }

    fn push(&self, row: RpcMessageRow) {
        let mut pending = lock(&self.pending);
        self.push_locked(&mut pending, row);
    }

    /// Buffer a streamed chunk, or fold it into this window's row for the same
    /// method and chunk kind: one row per window, whatever the token count, so
    /// a long answer costs one row rather than thousands. That row's payload
    /// is metadata it needs to be read at all, not the text.
    fn push_streamed(
        &self,
        pending: &mut Pending,
        mut row: RpcMessageRow,
        streamed: (String, String),
        bytes: usize,
    ) {
        let (method, update) = streamed;
        let key = (method, update.clone());
        if let Some(&(index, count)) = pending.streamed.get(&key) {
            let row = &mut pending.rows[index];
            let total = row.byte_size.unwrap_or(0) + bytes as i64;
            let count = count + 1;
            row.byte_size = Some(total);
            row.payload_json = Some(coalesced_payload(count, &update, total));
            pending.streamed.insert(key, (index, count));
            return;
        }
        row.payload_json = Some(coalesced_payload(1, &update, bytes as i64));
        if let Some(index) = self.push_locked(pending, row) {
            pending.streamed.insert(key, (index, 1));
        }
    }

    /// Buffer one row and answer where it sits, or `None` when the buffer was
    /// full and the row was dropped.
    fn push_locked(&self, pending: &mut Pending, row: RpcMessageRow) -> Option<usize> {
        if pending.rows.len() >= MAX_PENDING_ROWS {
            pending.dropped += 1;
            return None;
        }
        let index = pending.rows.len();
        pending.rows.push(row);
        if pending.rows.len() >= FLUSH_ROWS {
            // The flusher takes the buffer; a closed channel just means the
            // recorder is going away and the buffer goes with it.
            let _ = self.wake.send(Wake::Rows);
        }
        Some(index)
    }

    /// Write what is buffered now instead of waiting for the interval
    /// (tests).
    #[cfg(test)]
    pub(crate) fn flush_now(&self) {
        flush(&self.pending, self.path.as_ref());
    }

    /// The rows buffered now, for a test that does not want a store.
    #[cfg(test)]
    pub(crate) fn buffered(&self) -> Vec<RpcMessageRow> {
        lock(&self.pending).rows.clone()
    }
}

impl Drop for RpcRecorder {
    fn drop(&mut self) {
        // Closing the channel stops the flusher after it wrote what is left.
        let _ = self.wake.send(Wake::Stop);
        let flusher = lock(&self.flusher).take();
        if let Some(flusher) = flusher {
            let _ = flusher.join();
        }
    }
}

fn opposite(direction: &str) -> &str {
    if direction == DIRECTION_RUSTCODE_TO_HARNESS {
        DIRECTION_HARNESS_TO_RUSTCODE
    } else {
        DIRECTION_RUSTCODE_TO_HARNESS
    }
}

fn coalesced_payload(count: u64, update: &str, bytes: i64) -> String {
    json!({"coalesced": count, "update": update, "bytes": bytes}).to_string()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn pending_len(pending: &Mutex<Pending>) -> usize {
    lock(pending).rows.len()
}

/// The flusher: write the buffer when it is full, when a second passed since
/// the first row went in, or when the recorder is dropped.
fn flush_loop(pending: &Mutex<Pending>, path: Option<&PathBuf>, woken: &mpsc::Receiver<Wake>) {
    let mut first: Option<Instant> = None;
    loop {
        let wait = match first {
            Some(first) => FLUSH_INTERVAL.saturating_sub(first.elapsed()),
            None => FLUSH_INTERVAL,
        };
        match woken.recv_timeout(wait) {
            Ok(Wake::Rows) => {
                first.get_or_insert_with(Instant::now);
                if pending_len(pending) >= FLUSH_ROWS {
                    flush(pending, path);
                    first = None;
                }
            }
            Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => {
                flush(pending, path);
                return;
            }
            // The interval is up: whatever is buffered goes now, so a row of a quiet thread waits
            // one interval rather than until `FLUSH_ROWS` more arrive (a thread that went quiet
            // mid-turn looked silent in `harness_rpc_messages` for minutes).
            Err(RecvTimeoutError::Timeout) => {
                first = None;
                if pending_len(pending) > 0 {
                    flush(pending, path);
                }
            }
        }
    }
}

fn flush(pending: &Mutex<Pending>, path: Option<&PathBuf>) {
    let (rows, dropped) = {
        let mut pending = lock(pending);
        pending.streamed.clear();
        (
            std::mem::take(&mut pending.rows),
            std::mem::take(&mut pending.dropped),
        )
    };
    if dropped > 0 {
        crate::dbg_log!("harness rpc: {dropped} message(s) dropped, the buffer was full");
    }
    if rows.is_empty() {
        return;
    }
    match path {
        None => data_store::record_rpc_messages(&rows),
        Some(path) => {
            let references: Vec<&RpcMessageRow> = rows.iter().collect();
            if let Err(error) = data_store::record_rpc_messages_at(path, &references) {
                crate::dbg_log!(
                    "harness rpc: {} message(s) not recorded: {error}",
                    rows.len()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn request(id: u64, method: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"n": 1}})
    }

    fn response(id: u64) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rustcode-rpc-capture-{name}-{}.sqlite",
            std::process::id()
        ))
    }

    fn stored(
        path: &Path,
    ) -> Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<String>,
    )> {
        let connection = rusqlite::Connection::open(path).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT direction, message_kind, rpc_id, method, latency_ms, byte_size, payload_json
                 FROM harness_rpc_messages ORDER BY id",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    #[test]
    fn a_frame_is_a_request_a_notification_or_a_response() {
        assert_eq!(
            Frame::of(&request(7, "session/prompt")),
            Some(Frame {
                kind: KIND_REQUEST,
                id: Some("7".to_owned()),
                method: Some("session/prompt".to_owned()),
            })
        );
        assert_eq!(
            Frame::of(&json!({"jsonrpc": "2.0", "method": "session/update"})),
            Some(Frame {
                kind: KIND_NOTIFICATION,
                id: None,
                method: Some("session/update".to_owned()),
            })
        );
        assert_eq!(
            Frame::of(&response(7)),
            Some(Frame {
                kind: KIND_RESPONSE,
                id: Some("7".to_owned()),
                method: None,
            })
        );
        assert_eq!(
            Frame::of(&json!({"id": "a", "error": {"code": -1, "message": "no"}})),
            Some(Frame {
                kind: KIND_RESPONSE,
                id: Some("a".to_owned()),
                method: None,
            })
        );
        assert_eq!(Frame::of(&json!({"jsonrpc": "2.0"})), None);
    }

    #[test]
    fn a_response_carries_its_requests_method_and_latency() {
        let recorder = RpcRecorder::to_path(scratch("latency"), "thread-1", false);
        let asked = request(3, "session/prompt");
        recorder.record_outbound(&asked, asked.to_string().len());
        std::thread::sleep(Duration::from_millis(15));
        let answered = response(3);
        recorder.record_inbound(&answered, answered.to_string().len());
        let rows = recorder.buffered();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].message_kind, KIND_REQUEST);
        assert_eq!(rows[0].latency_ms, None);
        assert_eq!(rows[1].message_kind, KIND_RESPONSE);
        assert_eq!(rows[1].method.as_deref(), Some("session/prompt"));
        assert!(
            rows[1].latency_ms.unwrap() >= 15,
            "{:?}",
            rows[1].latency_ms
        );
        assert_eq!(rows[1].rpc_id.as_deref(), Some("3"));
        assert_eq!(rows[1].direction, DIRECTION_HARNESS_TO_RUSTCODE);
    }

    #[test]
    fn a_request_the_harness_asked_is_timed_the_other_way() {
        let recorder = RpcRecorder::to_path(scratch("reverse"), "thread-1", false);
        let asked = request(1, "session/request_permission");
        recorder.record_inbound(&asked, asked.to_string().len());
        let answered = response(1);
        recorder.record_outbound(&answered, answered.to_string().len());
        let rows = recorder.buffered();
        assert_eq!(rows[1].direction, DIRECTION_RUSTCODE_TO_HARNESS);
        assert_eq!(
            rows[1].method.as_deref(),
            Some("session/request_permission")
        );
        assert!(rows[1].latency_ms.is_some());
    }

    #[test]
    fn streamed_chunks_become_one_row_per_window() {
        let recorder = RpcRecorder::to_path(scratch("chunks"), "thread-1", false);
        let mut expected = 0;
        for word in ["a", "b", "c"] {
            let update = json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"update": {"sessionUpdate": "agent_message_chunk", "content": word}},
            });
            expected += update.to_string().len();
            recorder.record_inbound(&update, update.to_string().len());
        }
        let thought = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"update": {"sessionUpdate": "agent_thought_chunk", "content": "x"}},
        });
        recorder.record_inbound(&thought, 10);
        let rows = recorder.buffered();
        assert_eq!(rows.len(), 2, "one row per chunk kind: {rows:?}");
        assert_eq!(rows[0].method.as_deref(), Some("session/update"));
        let payload: Value = serde_json::from_str(rows[0].payload_json.as_ref().unwrap()).unwrap();
        assert_eq!(payload["coalesced"], 3);
        assert_eq!(payload["update"], "agent_message_chunk");
        assert_eq!(rows[0].byte_size, Some(expected as i64));
        assert_eq!(payload["bytes"].as_i64().unwrap(), expected as i64);
        assert_eq!(
            serde_json::from_str::<Value>(rows[1].payload_json.as_ref().unwrap()).unwrap()["update"],
            "agent_thought_chunk"
        );
    }

    #[test]
    fn codex_deltas_coalesce_into_the_thought_and_message_kinds() {
        let recorder = RpcRecorder::to_path(scratch("codex"), "thread-1", false);
        for method in ["item/agentMessage/delta", "item/reasoning/textDelta"] {
            for _ in 0..2 {
                let message = json!({"method": method, "params": {"delta": "x"}});
                recorder.record_inbound(&message, 8);
            }
        }
        let rows = recorder.buffered();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].method.as_deref(), Some("item/agentMessage/delta"));
        let payload: Value = serde_json::from_str(rows[0].payload_json.as_ref().unwrap()).unwrap();
        assert_eq!(payload["coalesced"], 2);
        assert_eq!(payload["update"], "agent_message_chunk");
        assert_eq!(rows[1].method.as_deref(), Some("item/reasoning/textDelta"));
    }

    #[test]
    fn a_payload_is_stored_only_when_asked_for_and_then_redacted() {
        let plain = RpcRecorder::to_path(scratch("plain"), "thread-1", false);
        let message = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/prompt",
            "params": {"apiKey": "sk-secret", "text": "hi"},
        });
        plain.record_outbound(&message, 60);
        let rows = plain.buffered();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].payload_json, None);
        assert_eq!(rows[0].byte_size, Some(60));

        let capturing = RpcRecorder::to_path(scratch("capturing"), "thread-1", true);
        capturing.record_outbound(&message, 60);
        let rows = capturing.buffered();
        let payload = rows[0].payload_json.as_ref().unwrap();
        assert!(!payload.contains("sk-secret"), "{payload}");
        let payload: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(payload["params"]["text"], "hi");
        assert!(!rows[0].payload_truncated);
    }

    #[test]
    fn a_full_buffer_drops_rows_without_panicking() {
        let recorder = RpcRecorder::to_path(scratch("overflow"), "thread-1", false);
        // A buffer the flusher never gets to, as when the store is stuck.
        let mut pending = Pending::default();
        for _ in 0..(MAX_PENDING_ROWS + 10) {
            let row = RpcMessageRow {
                thread_id: "thread-1".to_owned(),
                direction: DIRECTION_RUSTCODE_TO_HARNESS.to_owned(),
                message_kind: KIND_REQUEST.to_owned(),
                at: chrono::Utc::now().to_rfc3339(),
                ..RpcMessageRow::default()
            };
            recorder.push_locked(&mut pending, row);
        }
        assert_eq!(pending.rows.len(), MAX_PENDING_ROWS);
        assert_eq!(pending.dropped, 10);
        // A streamed chunk over the limit is dropped whole, never folded into
        // a row that is not there.
        for _ in 0..2 {
            let row = RpcMessageRow {
                thread_id: "thread-1".to_owned(),
                direction: DIRECTION_HARNESS_TO_RUSTCODE.to_owned(),
                message_kind: KIND_NOTIFICATION.to_owned(),
                at: chrono::Utc::now().to_rfc3339(),
                ..RpcMessageRow::default()
            };
            recorder.push_streamed(
                &mut pending,
                row,
                (
                    "session/update".to_owned(),
                    "agent_message_chunk".to_owned(),
                ),
                8,
            );
        }
        assert_eq!(pending.rows.len(), MAX_PENDING_ROWS);
        assert_eq!(pending.dropped, 12);
        // Recording that far past the bound through the RPC path is quiet too.
        for id in 0..(MAX_PENDING_ROWS + 10) {
            let message = json!({"jsonrpc": "2.0", "id": id, "method": "ping"});
            recorder.record_outbound(&message, 4);
        }
    }

    #[test]
    fn a_flush_writes_the_buffer_to_the_store() {
        let path = scratch("flush");
        let _ = std::fs::remove_file(&path);
        {
            let recorder = RpcRecorder::to_path(path.clone(), "thread-1", false);
            let asked = request(9, "session/prompt");
            recorder.record_outbound(&asked, asked.to_string().len());
            let update = json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"update": {"sessionUpdate": "agent_message_chunk", "content": "hi"}},
            });
            recorder.record_inbound(&update, update.to_string().len());
            recorder.flush_now();
        }
        let rows = stored(&path);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].0, DIRECTION_RUSTCODE_TO_HARNESS);
        assert_eq!(rows[0].1, KIND_REQUEST);
        assert_eq!(rows[0].3.as_deref(), Some("session/prompt"));
        assert_eq!(rows[1].0, DIRECTION_HARNESS_TO_RUSTCODE);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_few_rows_are_written_within_the_interval() {
        let path = scratch("interval");
        let _ = std::fs::remove_file(&path);
        let recorder = RpcRecorder::to_path(path.clone(), "thread-1", false);
        let asked = request(1, "session/prompt");
        recorder.record_outbound(&asked, 10);
        // Far fewer than `FLUSH_ROWS`: only the interval writes it, with the recorder still alive.
        // Counted without `stored`, which expects the table the first flush creates.
        let count = || {
            rusqlite::Connection::open(&path)
                .and_then(|db| {
                    db.query_row("SELECT COUNT(*) FROM harness_rpc_messages", [], |row| {
                        row.get::<_, i64>(0)
                    })
                })
                .unwrap_or(0)
        };
        let deadline = Instant::now() + FLUSH_INTERVAL * 5;
        while count() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(count(), 1);
        drop(recorder);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_dropped_recorder_writes_what_it_buffered() {
        let path = scratch("drop");
        let _ = std::fs::remove_file(&path);
        {
            let recorder = RpcRecorder::to_path(path.clone(), "thread-1", false);
            let asked = request(1, "session/prompt");
            recorder.record_outbound(&asked, 10);
        }
        assert_eq!(stored(&path).len(), 1);
        let _ = std::fs::remove_file(&path);
    }
}
