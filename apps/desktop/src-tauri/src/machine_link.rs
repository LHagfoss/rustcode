//! The windows that share one paired machine's connection.
//!
//! Every window runs its own ACP client, numbering its requests from 1, while
//! a machine has one supervisor and one socket ([`crate::machines`]). This
//! keeps each window's channel, rewrites request ids through the same
//! [`RpcMux`] the local engine uses, and sends each answer back to the window
//! that asked. A window that attaches while the machine is already open is
//! told so at once, and its `initialize` is answered from the first one's.

use crate::engine::EngineEvent;
use crate::engine_mux::{InboundDecision, OutboundDecision, RpcMux};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::ipc::Channel;

/// Where one window's events go.
pub trait Sink: Send + Sync {
    /// Whether the window took the event.
    fn deliver(&self, event: EngineEvent) -> bool;
}

impl Sink for Channel<EngineEvent> {
    fn deliver(&self, event: EngineEvent) -> bool {
        self.send(event).is_ok()
    }
}

/// No window is attached to take an event.
#[derive(Debug, PartialEq, Eq)]
pub struct NoWindow;

struct Windows<S> {
    attached: Vec<(String, S)>,
    mux: RpcMux,
    /// The `Open` of the connection that is up now, for a window that
    /// attaches after it.
    open: Option<EngineEvent>,
}

/// One machine's windows and the routing between them and its socket.
pub struct Link<S = Channel<EngineEvent>> {
    windows: Mutex<Windows<S>>,
}

impl<S> Default for Link<S> {
    fn default() -> Self {
        Self {
            windows: Mutex::new(Windows {
                attached: Vec::new(),
                mux: RpcMux::default(),
                open: None,
            }),
        }
    }
}

impl<S: Sink> Link<S> {
    /// Attach a window, or give a window that attached before its new
    /// channel. Answers whether it was attached already. A window that joins
    /// an open connection gets its `Open` now.
    pub fn attach(&self, window: &str, sink: S) -> bool {
        let mut windows = self.windows.lock().unwrap();
        if let Some(open) = windows.open.clone() {
            sink.deliver(open);
        }
        windows.mux.detach_window(window);
        if let Some(slot) = windows
            .attached
            .iter_mut()
            .find(|(label, _)| label == window)
        {
            slot.1 = sink;
            return true;
        }
        windows.attached.push((window.to_owned(), sink));
        false
    }

    /// Detach a window. Answers whether one was attached and none is left.
    pub fn detach(&self, window: &str) -> bool {
        let mut windows = self.windows.lock().unwrap();
        let before = windows.attached.len();
        windows.attached.retain(|(label, _)| label != window);
        windows.mux.detach_window(window);
        before != windows.attached.len() && windows.attached.is_empty()
    }

    /// The connection ended: a window that attaches next waits for the new
    /// one's `Open`.
    pub fn end(&self) {
        let mut windows = self.windows.lock().unwrap();
        windows.mux.reset();
        windows.open = None;
    }

    pub fn is_empty(&self) -> bool {
        self.windows.lock().unwrap().attached.is_empty()
    }

    /// Whether `window` is attached.
    pub fn has(&self, window: &str) -> bool {
        self.windows
            .lock()
            .unwrap()
            .attached
            .iter()
            .any(|(label, _)| label == window)
    }

    /// One event from the supervisor. Lines go through the multiplexer; an
    /// `Open` starts a new connection, whose numbering starts over, and a
    /// `Closed` ends it. Fails when no window is attached.
    pub fn send(&self, event: EngineEvent) -> Result<(), NoWindow> {
        let mut windows = self.windows.lock().unwrap();
        match &event {
            EngineEvent::Open { .. } => {
                windows.mux.reset();
                windows.open = Some(event.clone());
            }
            EngineEvent::Closed { .. } => {
                windows.mux.reset();
                windows.open = None;
            }
            _ => {}
        }
        if windows.attached.is_empty() {
            return Err(NoWindow);
        }
        let lines = match event {
            EngineEvent::Line { line } => vec![line],
            EngineEvent::Lines { lines } => lines,
            other => {
                for (_, sink) in &windows.attached {
                    sink.deliver(other.clone());
                }
                return Ok(());
            }
        };
        let primary = primary(&windows.attached);
        let mut per_window: HashMap<String, Vec<String>> = HashMap::new();
        for line in lines {
            match windows.mux.inbound(&line, primary.as_deref()) {
                InboundDecision::Broadcast(frame) => {
                    for (label, _) in &windows.attached {
                        per_window
                            .entry(label.clone())
                            .or_default()
                            .push(frame.clone());
                    }
                }
                InboundDecision::Deliver(deliveries) => {
                    for (label, frame) in deliveries {
                        per_window.entry(label).or_default().push(frame);
                    }
                }
            }
        }
        for (label, sink) in &windows.attached {
            if let Some(batch) = per_window.remove(label) {
                sink.deliver(batch_event(batch));
            }
        }
        Ok(())
    }

    /// One line a window wrote. Answers the line to write to the machine, or
    /// none when it was answered here or is not needed.
    pub fn outbound(&self, window: &str, line: &str) -> Option<String> {
        let mut windows = self.windows.lock().unwrap();
        match windows.mux.outbound(window, line) {
            OutboundDecision::Forward(out)
            | OutboundDecision::ForwardAndNotifyPermissionResolved { line: out, .. } => Some(out),
            OutboundDecision::ReplyLocal { window, line } => {
                if let Some((_, sink)) = windows.attached.iter().find(|(label, _)| *label == window)
                {
                    sink.deliver(EngineEvent::Line { line });
                }
                None
            }
            OutboundDecision::QueueInit | OutboundDecision::Drop => None,
        }
    }
}

/// The window that answers the machine's own requests: `main` when it is
/// attached, else the first.
fn primary<S>(attached: &[(String, S)]) -> Option<String> {
    if attached.iter().any(|(label, _)| label == "main") {
        return Some("main".to_owned());
    }
    attached.first().map(|(label, _)| label.clone())
}

fn batch_event(mut lines: Vec<String>) -> EngineEvent {
    if lines.len() == 1 {
        EngineEvent::Line {
            line: lines.pop().unwrap_or_default(),
        }
    } else {
        EngineEvent::Lines { lines }
    }
}

#[cfg(test)]
#[path = "machine_link_tests.rs"]
mod tests;
