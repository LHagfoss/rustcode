//! `rc computer-worker`: the process that sees and drives this computer's
//! desktop for the `computer_*` tools (ADR 0011).
//!
//! The engine starts it on the first desktop call and talks to it over
//! stdin/stdout, one JSON request per line and one JSON answer per line. The
//! worker holds the operating system's permissions (Accessibility and Screen
//! Recording on macOS), shows the agent's own cursor, and runs only actions
//! the engine already approved. Stopping it is a
//! kill; a closed stdin ends it.

pub mod cursor_image;
pub mod cursor_motion;
#[cfg(unix)]
pub mod hosted;
pub mod keys;
#[cfg(target_os = "macos")]
mod macos;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// The first argument that turns `rc` (or `rustcode`) into the worker.
pub const WORKER_ARG: &str = "computer-worker";

/// The most elements one snapshot lists.
pub const MAX_ELEMENTS: usize = 1000;

/// What the engine asks the worker to do. Coordinates are pixels of the
/// latest screenshot of the same window; refs come from the latest snapshot
/// of the same app.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Status,
    RequestPermissions,
    Windows,
    App {
        pid: i32,
    },
    Screenshot {
        pid: i32,
        window_id: u32,
    },
    Snapshot {
        pid: i32,
        #[serde(default)]
        window_id: Option<u32>,
        #[serde(default)]
        max_elements: Option<usize>,
    },
    Click {
        pid: i32,
        #[serde(default, rename = "ref")]
        element: Option<String>,
        #[serde(default)]
        window_id: Option<u32>,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        button: Option<String>,
        #[serde(default)]
        count: Option<u8>,
        #[serde(default)]
        foreground: bool,
    },
    Type {
        pid: i32,
        #[serde(default, rename = "ref")]
        element: Option<String>,
        text: String,
        #[serde(default)]
        foreground: bool,
    },
    Key {
        pid: i32,
        key: String,
        #[serde(default)]
        foreground: bool,
    },
    Scroll {
        pid: i32,
        #[serde(default, rename = "ref")]
        element: Option<String>,
        #[serde(default)]
        window_id: Option<u32>,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
        #[serde(default)]
        foreground: bool,
    },
}

impl Op {
    /// Whether the action brings the app to the front and uses the system's
    /// own input, for apps that ignore input sent to them in the background.
    pub fn foreground(&self) -> bool {
        match self {
            Op::Click { foreground, .. }
            | Op::Type { foreground, .. }
            | Op::Key { foreground, .. }
            | Op::Scroll { foreground, .. } => *foreground,
            _ => false,
        }
    }
}

/// One request line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// One answer line: `result` on success, `error` otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    fn from(id: u64, outcome: Result<Value, String>) -> Self {
        match outcome {
            Ok(result) => Self {
                id,
                ok: true,
                result,
                error: None,
            },
            Err(error) => Self {
                id,
                ok: false,
                result: Value::Null,
                error: Some(error),
            },
        }
    }
}

/// Whether this process was started as the worker.
pub fn requested() -> bool {
    std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == WORKER_ARG)
}

/// Run the worker until stdin closes or the user stops it. Never returns.
#[cfg(target_os = "macos")]
pub fn run() -> ! {
    std::thread::spawn(|| {
        if hosted::listening() {
            hosted::listen(macos::handle)
        }
        serve(macos::handle);
    });
    macos::run_main_thread(hosted::listening())
}

/// Run the worker until stdin closes. Never returns.
#[cfg(not(target_os = "macos"))]
pub fn run() -> ! {
    #[cfg(unix)]
    if hosted::listening() {
        hosted::listen(unsupported)
    }
    serve(unsupported)
}

fn serve(handle: fn(Op) -> Result<Value, String>) -> ! {
    serve_lines(std::io::stdin().lock(), std::io::stdout(), handle);
    std::process::exit(0)
}

/// Answer request lines from `input` on `out` until either side closes.
fn serve_lines(input: impl BufRead, mut out: impl Write, handle: fn(Op) -> Result<Value, String>) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(request) => Reply::from(request.id, handle(request.op)),
            Err(error) => {
                let id = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|value| value["id"].as_u64())
                    .unwrap_or(0);
                Reply::from(id, Err(format!("invalid request: {error}")))
            }
        };
        let Ok(text) = serde_json::to_string(&reply) else {
            continue;
        };
        if writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
}

/// The platform this worker runs on, as `status` reports it.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Why desktop control cannot run on this platform yet, if it cannot.
pub fn unsupported_reason() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        None
    } else {
        Some(
            "Desktop control runs on macOS for now; Windows and Linux follow (docs/adr/0011-os-computer-use.md).",
        )
    }
}

/// Desktop control's permissions here, as Settings shows them; `prompt`
/// asks the operating system to show its permission prompts. The worker the
/// RustCode app hosts answers when it runs, as its grants are the ones the
/// tools use; `hosted` says which answered.
#[cfg(target_os = "macos")]
pub fn permissions(prompt: bool) -> Value {
    let op = if prompt {
        Op::RequestPermissions
    } else {
        Op::Status
    };
    let (mut value, hosted) = match hosted::ask(op) {
        Some(value) => (value, true),
        None => (macos::status(prompt), false),
    };
    value["hosted"] = json!(hosted);
    value
}

/// Desktop control's permissions here: none apply where it is not supported.
#[cfg(not(target_os = "macos"))]
pub fn permissions(_prompt: bool) -> Value {
    unsupported(Op::Status).unwrap_or_default()
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn unsupported(op: Op) -> Result<Value, String> {
    match op {
        Op::Status | Op::RequestPermissions => Ok(json!({
            "platform": platform(),
            "supported": false,
            "reason": unsupported_reason(),
            "accessibility": Value::Null,
            "screen_recording": Value::Null,
        })),
        _ => Err(unsupported_reason()
            .unwrap_or("Desktop control is not available here")
            .to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_with_their_op_and_id() {
        let request: Request =
            serde_json::from_str(r#"{"id":7,"op":"click","pid":42,"ref":"e3","count":2}"#).unwrap();
        assert_eq!(request.id, 7);
        assert_eq!(
            request.op,
            Op::Click {
                pid: 42,
                element: Some("e3".into()),
                window_id: None,
                x: None,
                y: None,
                button: None,
                count: Some(2),
                foreground: false,
            }
        );
        let status: Request = serde_json::from_str(r#"{"id":1,"op":"status"}"#).unwrap();
        assert_eq!(status.op, Op::Status);
    }

    #[test]
    fn a_reply_carries_either_a_result_or_an_error() {
        let ok = serde_json::to_value(Reply::from(3, Ok(json!({"a":1})))).unwrap();
        assert_eq!(ok, json!({"id":3,"ok":true,"result":{"a":1}}));
        let failed = serde_json::to_value(Reply::from(4, Err("no".into()))).unwrap();
        assert_eq!(failed, json!({"id":4,"ok":false,"error":"no"}));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_platforms_report_status_and_refuse_actions() {
        let status = unsupported(Op::Status).unwrap();
        assert_eq!(status["supported"], false);
        assert!(unsupported(Op::Windows).unwrap_err().contains("macOS"));
    }
}
