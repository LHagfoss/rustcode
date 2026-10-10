//! The interactive tasks panel: what is running, what finished this session,
//! one task's log, and stopping one task.
//!
//! The frontend owns no task logic. It forwards a [`TasksPanelInput`] and
//! paints the [`TasksPanelView`] carried by the render state, which is rebuilt
//! from the task manager on every projection so elapsed times and states are
//! never a stale copy.

use std::time::{Duration, Instant};

use crate::app::AppState;

/// How much of a task log the panel reads from its end.
const LOG_TAIL_BYTES: usize = 16 * 1024;
/// Finished tasks listed under the running ones, newest first.
const MAX_FINISHED_ROWS: usize = 20;
/// How often an open log of a running task is read again.
const LOG_REFRESH: Duration = Duration::from_millis(500);

/// How a finished task ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskOutcome {
    Done,
    Exit(i32),
    Failed,
    Stopped,
}

impl TaskOutcome {
    /// Word shown behind a finished row.
    pub fn label(&self) -> String {
        match self {
            Self::Done => "done".to_owned(),
            Self::Exit(code) => format!("exit {code}"),
            Self::Failed => "failed".to_owned(),
            Self::Stopped => "stopped".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskRowState {
    Running {
        started_at: Instant,
    },
    Finished {
        outcome: TaskOutcome,
        ran_for: Duration,
    },
}

/// One task in the panel's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskPanelRow {
    pub id: String,
    pub command: String,
    pub state: TaskRowState,
}

impl TaskPanelRow {
    pub fn is_running(&self) -> bool {
        matches!(self.state, TaskRowState::Running { .. })
    }

    /// Single-line command label bounded to `max_chars`.
    pub fn label(&self, max_chars: usize) -> String {
        crate::tools::background_command_label(&self.command, max_chars)
    }
}

/// The tail of one task's captured output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskLogView {
    pub task_id: String,
    pub command: String,
    pub text: String,
    /// Output older than the tail exists and is not shown.
    pub earlier_omitted: bool,
    /// Rows scrolled up from the newest line.
    pub scroll: usize,
}

/// Panel state kept on [`AppState`] while the panel is open.
#[derive(Clone, Debug, Default)]
pub struct TasksPanelState {
    selected: usize,
    /// The selection follows its task when rows move, e.g. when a task ends
    /// and drops from the running group to the finished one.
    selected_id: Option<String>,
    log: Option<TaskLogView>,
    log_scroll_limit: Option<usize>,
    /// The open log belongs to a task that was running at the last read.
    log_live: bool,
    log_read_at: Option<Instant>,
}

impl TasksPanelState {
    /// Publish the last full viewport offset measured by a frontend. Wrapping
    /// can make this larger than the number of raw lines in the captured log.
    pub fn set_log_scroll_limit(&mut self, limit: usize) {
        if let Some(log) = self.log.as_mut() {
            self.log_scroll_limit = Some(limit);
            log.scroll = log.scroll.min(limit);
        }
    }
}

/// What the frontend paints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TasksPanelView {
    /// Running tasks first, then finished ones, newest first.
    pub rows: Vec<TaskPanelRow>,
    pub selected: usize,
    pub log: Option<TaskLogView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TasksPanelInput {
    /// Move the selection, or scroll an open log towards older output.
    Up,
    /// Move the selection, or scroll an open log towards newer output.
    Down,
    /// Show the selected task's log.
    Open,
    /// Leave the log for the list, or close the panel from the list.
    Back,
    /// Stop the selected (or shown) task if it is still running.
    Stop,
    Close,
}

/// Rows for `session_id`: running tasks in start order, then what finished
/// this session, newest first.
pub fn tasks_panel_rows(session_id: &str) -> Vec<TaskPanelRow> {
    use rustcode_tasks::TaskTerminalReason;
    let mut rows = crate::tools::background_task_snapshots(session_id)
        .into_iter()
        .map(|task| TaskPanelRow {
            id: task.id,
            command: task.command,
            state: TaskRowState::Running {
                started_at: task.start_time,
            },
        })
        .collect::<Vec<_>>();
    rows.extend(
        crate::tools::recent_background_task_completions(session_id)
            .into_iter()
            .rev()
            .take(MAX_FINISHED_ROWS)
            .map(|completion| {
                let outcome = match &completion.reason {
                    TaskTerminalReason::Exited { success: true, .. } => TaskOutcome::Done,
                    TaskTerminalReason::Exited {
                        code: Some(code), ..
                    } => TaskOutcome::Exit(*code),
                    TaskTerminalReason::Cancelled => TaskOutcome::Stopped,
                    _ => TaskOutcome::Failed,
                };
                TaskPanelRow {
                    id: completion.id.to_string(),
                    command: completion.command,
                    state: TaskRowState::Finished {
                        outcome,
                        ran_for: completion
                            .ended_at
                            .saturating_duration_since(completion.started_at),
                    },
                }
            }),
    );
    rows
}

fn selected_index(panel: &TasksPanelState, rows: &[TaskPanelRow]) -> usize {
    panel
        .selected_id
        .as_deref()
        .and_then(|id| rows.iter().position(|row| row.id == id))
        .unwrap_or_else(|| panel.selected.min(rows.len().saturating_sub(1)))
}

/// The panel as the frontend should paint it, or `None` while it is closed.
pub(crate) fn tasks_panel_view(state: &AppState) -> Option<TasksPanelView> {
    let panel = state.tasks_panel.as_ref()?;
    let rows = tasks_panel_rows(&state.active_session_id);
    Some(TasksPanelView {
        selected: selected_index(panel, &rows),
        rows,
        log: panel.log.clone(),
    })
}

/// Open the tasks panel: what is running, then what finished this session.
pub fn show_tasks_panel(state: &mut AppState) {
    state.command_panel = None;
    let mut panel = TasksPanelState::default();
    panel.selected_id = tasks_panel_rows(&state.active_session_id)
        .first()
        .map(|row| row.id.clone());
    state.tasks_panel = Some(panel);
    state.request_redraw();
}

/// Apply one key or wheel step to the open panel. Does nothing while closed.
pub fn tasks_panel_input(state: &mut AppState, input: TasksPanelInput) {
    if state.tasks_panel.is_none() {
        return;
    }
    let session_id = state.active_session_id.clone();
    let rows = tasks_panel_rows(&session_id);
    let mut notice = None;
    let mut close = false;
    if let Some(panel) = state.tasks_panel.as_mut() {
        let selected = selected_index(panel, &rows);
        match input {
            TasksPanelInput::Up | TasksPanelInput::Down => {
                let older = input == TasksPanelInput::Up;
                if let Some(log) = panel.log.as_mut() {
                    let limit = panel
                        .log_scroll_limit
                        .unwrap_or_else(|| log.text.lines().count());
                    log.scroll = log.scroll.min(limit);
                    log.scroll = if older {
                        log.scroll.saturating_add(1).min(limit)
                    } else {
                        log.scroll.saturating_sub(1)
                    };
                } else if !rows.is_empty() {
                    let next = if older {
                        selected.saturating_sub(1)
                    } else {
                        (selected + 1).min(rows.len() - 1)
                    };
                    panel.selected = next;
                    panel.selected_id = Some(rows[next].id.clone());
                }
            }
            TasksPanelInput::Open => {
                if panel.log.is_none()
                    && let Some(row) = rows.get(selected)
                {
                    panel.selected = selected;
                    panel.selected_id = Some(row.id.clone());
                    panel.log = Some(read_task_log_view(&session_id, row, 0));
                    panel.log_scroll_limit = None;
                    panel.log_live = row.is_running();
                    panel.log_read_at = Some(Instant::now());
                }
            }
            TasksPanelInput::Back => {
                if panel.log.take().is_none() {
                    close = true;
                }
            }
            TasksPanelInput::Stop => {
                let target = panel
                    .log
                    .as_ref()
                    .map(|log| log.task_id.clone())
                    .or_else(|| rows.get(selected).map(|row| row.id.clone()));
                notice = target.map(|id| {
                    if rows.iter().any(|row| row.id == id && row.is_running()) {
                        stop_task(&session_id, &id)
                    } else {
                        "That task has already finished.".to_owned()
                    }
                });
            }
            TasksPanelInput::Close => close = true,
        }
    }
    if close {
        state.tasks_panel = None;
    }
    if let Some(notice) = notice {
        state.set_transient_notice(notice);
    }
    state.request_redraw();
}

fn stop_task(session_id: &str, task_id: &str) -> String {
    use rustcode_tasks::CancelResult;
    match crate::tools::background_task_manager().cancel_in_session(session_id, task_id) {
        CancelResult::Cancelled => "Stopped 1 task.".to_owned(),
        CancelResult::Requested => "Stop requested; the task is still starting.".to_owned(),
        CancelResult::AlreadyFinished | CancelResult::NotFound => {
            "That task has already finished.".to_owned()
        }
        CancelResult::Failed => "Failed to stop the task.".to_owned(),
    }
}

/// Keep an open log of a running task current. Called from the frontend's
/// event loop; cheap while the panel is closed or shows the list.
pub fn refresh_tasks_panel(state: &mut AppState) {
    let Some(panel) = state.tasks_panel.as_ref() else {
        return;
    };
    let Some(log) = panel.log.as_ref() else {
        return;
    };
    if !panel.log_live
        || panel
            .log_read_at
            .is_some_and(|read_at| read_at.elapsed() < LOG_REFRESH)
    {
        return;
    }
    let session_id = state.active_session_id.clone();
    let rows = tasks_panel_rows(&session_id);
    let Some(row) = rows.iter().find(|row| row.id == log.task_id) else {
        return;
    };
    let fresh = read_task_log_view(&session_id, row, log.scroll);
    let changed = fresh != *log;
    let live = row.is_running();
    if let Some(panel) = state.tasks_panel.as_mut() {
        // One more read after the task ends picks up its last output.
        panel.log_live = live;
        panel.log_read_at = Some(Instant::now());
        if changed {
            panel.log = Some(fresh);
        }
    }
    if changed {
        state.request_redraw();
    }
}

fn read_task_log_view(session_id: &str, row: &TaskPanelRow, scroll: usize) -> TaskLogView {
    let completion = crate::tools::background_task_completion(session_id, &row.id);
    let log_path = crate::tools::background_task_snapshots(session_id)
        .into_iter()
        .find(|task| task.id == row.id)
        .and_then(|task| task.output_log)
        .or_else(|| completion.as_ref().and_then(|c| c.output_log.clone()));
    let (text, earlier_omitted) = match log_path
        .as_deref()
        .map(|path| crate::tools::read_task_log(path, false, LOG_TAIL_BYTES))
    {
        Some(Ok((text, start, _total))) => (log_tail_text(&text, start > 0), start > 0),
        Some(Err(error)) => (error, false),
        None => (
            completion
                .as_ref()
                .and_then(retained_output)
                .unwrap_or_else(|| {
                    if row.is_running() {
                        "No output is captured for this task while it runs.".to_owned()
                    } else {
                        "No output was captured for this task.".to_owned()
                    }
                }),
            false,
        ),
    };
    let text = if text.trim().is_empty() {
        "No output yet.".to_owned()
    } else {
        text
    };
    TaskLogView {
        task_id: row.id.clone(),
        command: row.command.clone(),
        text,
        earlier_omitted,
        scroll,
    }
}

/// Output the task manager kept in memory for a finished task without a log.
fn retained_output(completion: &rustcode_tasks::TaskCompletion) -> Option<String> {
    let Some(output) = completion.output.as_ref() else {
        return completion.error.clone();
    };
    let stdout = rustcode_command::format_bounded_output(&output.stdout);
    let stderr = rustcode_command::format_bounded_output(&output.stderr);
    let mut text = stdout;
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("stderr:\n");
        text.push_str(&stderr);
    }
    Some(log_tail_text(&text, false))
}

/// Make raw log bytes safe to paint: no escape sequences or control
/// characters, a carriage-return progress line reduced to its last state,
/// and no partial first line when the tail starts mid-file.
fn log_tail_text(raw: &str, starts_mid_file: bool) -> String {
    let clean = rustcode_tool_protocol::text::strip_ansi_escapes(raw);
    let body = if starts_mid_file {
        clean.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        clean.as_str()
    };
    body.lines()
        .map(|line| {
            let line = line.trim_end_matches('\r');
            line.rsplit('\r')
                .next()
                .unwrap_or(line)
                .chars()
                .flat_map(|c| match c {
                    '\t' => "    ".chars().collect::<Vec<_>>(),
                    c if c.is_control() => Vec::new(),
                    c => vec![c],
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_log_scroll_stops_at_the_oldest_full_viewport() {
        let mut state = AppState::new();
        state.tasks_panel = Some(TasksPanelState {
            log: Some(TaskLogView {
                task_id: "scroll-bounds".into(),
                command: "test".into(),
                text: (0..10).map(|row| format!("row {row}\n")).collect(),
                earlier_omitted: false,
                scroll: 0,
            }),
            ..Default::default()
        });
        // A five-row body can move only five rows into this ten-row log.
        state.tasks_panel.as_mut().unwrap().set_log_scroll_limit(5);
        for _ in 0..100 {
            tasks_panel_input(&mut state, TasksPanelInput::Up);
        }
        assert_eq!(
            state
                .tasks_panel
                .as_ref()
                .unwrap()
                .log
                .as_ref()
                .unwrap()
                .scroll,
            5
        );
        tasks_panel_input(&mut state, TasksPanelInput::Down);
        assert_eq!(
            state
                .tasks_panel
                .as_ref()
                .unwrap()
                .log
                .as_ref()
                .unwrap()
                .scroll,
            4
        );
        // A narrow body can wrap these same raw lines into many more rows.
        state.tasks_panel.as_mut().unwrap().set_log_scroll_limit(20);
        for _ in 0..100 {
            tasks_panel_input(&mut state, TasksPanelInput::Up);
        }
        assert_eq!(
            state
                .tasks_panel
                .as_ref()
                .unwrap()
                .log
                .as_ref()
                .unwrap()
                .scroll,
            20
        );
        state.tasks_panel.as_mut().unwrap().set_log_scroll_limit(0);
        state.tasks_panel.as_mut().unwrap().set_log_scroll_limit(20);
        assert_eq!(
            state
                .tasks_panel
                .as_ref()
                .unwrap()
                .log
                .as_ref()
                .unwrap()
                .scroll,
            0
        );
    }

    fn spawn(session: &str, id: &str, command: &str, log: Option<std::path::PathBuf>) {
        let mut spec = rustcode_tasks::TaskSpec::new(
            rustcode_tasks::SessionId::new(session),
            rustcode_command::CommandRequest {
                command: command.to_owned(),
                status_command: None,
                sandboxed_shell: false,
                cwd: None,
                env: Vec::new(),
                timeout: Duration::from_secs(30),
                process_group: true,
                inherited_fds: Vec::new(),
            },
        );
        spec.output_log = log;
        crate::tools::background_task_manager()
            .spawn_with_id(id, spec)
            .unwrap();
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("condition was not reached");
    }

    fn state_for(session: &str) -> AppState {
        let mut state = AppState::new();
        state.active_session_id = session.to_owned();
        state
    }

    fn view(state: &AppState) -> TasksPanelView {
        tasks_panel_view(state).expect("panel is open")
    }

    #[test]
    fn log_tail_text_is_safe_to_paint() {
        assert_eq!(
            log_tail_text(
                "partial\n\u{1b}[32mok\u{1b}[0m\n10%\r50%\r100%\r\na\tb\u{7}",
                true
            ),
            "ok\n100%\na    b"
        );
        assert_eq!(log_tail_text("first\nsecond", false), "first\nsecond");
    }

    #[cfg(unix)]
    #[test]
    fn panel_lists_running_then_finished_and_keeps_selection_on_its_task() {
        let session = "tasks-panel-rows-session";
        spawn(session, "tasks-panel-rows-quick", "exit 3", None);
        wait_until(|| {
            crate::tools::background_task_completion(session, "tasks-panel-rows-quick").is_some()
        });
        spawn(session, "tasks-panel-rows-a", "sleep 30", None);
        spawn(session, "tasks-panel-rows-b", "sleep 30", None);
        wait_until(|| {
            crate::tools::background_task_snapshots(session)
                .iter()
                .filter(|task| task.child_pid.is_some())
                .count()
                == 2
        });
        // Another session's task never shows up here.
        spawn(
            "tasks-panel-rows-other",
            "tasks-panel-rows-x",
            "sleep 30",
            None,
        );

        let mut state = state_for(session);
        assert!(tasks_panel_view(&state).is_none());
        show_tasks_panel(&mut state);
        assert!(state.user_overlay_open());
        let opened = view(&state);
        assert_eq!(
            opened
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            [
                "tasks-panel-rows-a",
                "tasks-panel-rows-b",
                "tasks-panel-rows-quick"
            ]
        );
        assert!(opened.rows[0].is_running() && opened.rows[1].is_running());
        assert!(matches!(
            opened.rows[2].state,
            TaskRowState::Finished {
                outcome: TaskOutcome::Exit(3),
                ..
            }
        ));
        assert_eq!(opened.selected, 0);

        // Up at the top and Down at the bottom stay in range.
        tasks_panel_input(&mut state, TasksPanelInput::Up);
        assert_eq!(view(&state).selected, 0);
        tasks_panel_input(&mut state, TasksPanelInput::Down);
        assert_eq!(view(&state).selected, 1);

        // Stopping a finished task is refused; stopping the selected running
        // task stops that one only.
        tasks_panel_input(&mut state, TasksPanelInput::Down);
        tasks_panel_input(&mut state, TasksPanelInput::Down);
        assert_eq!(view(&state).selected, 2);
        tasks_panel_input(&mut state, TasksPanelInput::Stop);
        assert_eq!(crate::tools::background_task_snapshots(session).len(), 2);
        tasks_panel_input(&mut state, TasksPanelInput::Up);
        tasks_panel_input(&mut state, TasksPanelInput::Stop);
        wait_until(|| {
            crate::tools::background_task_completion(session, "tasks-panel-rows-b").is_some()
        });
        let after = view(&state);
        assert_eq!(after.rows[0].id, "tasks-panel-rows-a");
        assert!(after.rows[0].is_running());
        // The stopped task moved to the finished group and the selection
        // moved with it instead of sliding onto a neighbour.
        let stopped = after
            .rows
            .iter()
            .position(|row| row.id == "tasks-panel-rows-b")
            .expect("stopped task is listed as finished");
        assert_eq!(after.selected, stopped);
        assert!(matches!(
            after.rows[stopped].state,
            TaskRowState::Finished {
                outcome: TaskOutcome::Stopped,
                ..
            }
        ));
        assert_eq!(
            crate::tools::background_task_snapshots("tasks-panel-rows-other").len(),
            1
        );

        tasks_panel_input(&mut state, TasksPanelInput::Back);
        assert!(state.tasks_panel.is_none());
        assert!(!state.user_overlay_open());
        crate::tools::stop_background_tasks(session);
        crate::tools::stop_background_tasks("tasks-panel-rows-other");
    }

    #[cfg(unix)]
    #[test]
    fn enter_shows_the_log_tail_and_back_returns_to_the_list() {
        let session = "tasks-panel-log-session";
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("task.log");
        spawn(
            session,
            "tasks-panel-log-task",
            "echo first; echo second; sleep 30",
            Some(log.clone()),
        );
        wait_until(|| std::fs::read_to_string(&log).is_ok_and(|text| text.contains("second")));

        let mut state = state_for(session);
        show_tasks_panel(&mut state);
        tasks_panel_input(&mut state, TasksPanelInput::Open);
        let shown = view(&state).log.expect("log is open");
        assert_eq!(shown.task_id, "tasks-panel-log-task");
        assert!(shown.text.contains("first\nsecond"), "{}", shown.text);
        assert!(!shown.earlier_omitted);

        // Up/Down scroll the log instead of moving the selection.
        tasks_panel_input(&mut state, TasksPanelInput::Up);
        assert_eq!(view(&state).log.unwrap().scroll, 1);
        tasks_panel_input(&mut state, TasksPanelInput::Down);
        tasks_panel_input(&mut state, TasksPanelInput::Down);
        assert_eq!(view(&state).log.unwrap().scroll, 0);

        // New output reaches an open log once the refresh interval passed.
        {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            writeln!(file, "third").unwrap();
        }
        refresh_tasks_panel(&mut state);
        assert!(!view(&state).log.unwrap().text.contains("third"));
        state.tasks_panel.as_mut().unwrap().log_read_at =
            Instant::now().checked_sub(LOG_REFRESH * 2);
        refresh_tasks_panel(&mut state);
        assert!(view(&state).log.unwrap().text.contains("third"));

        // `x` in the log view stops the task being shown.
        tasks_panel_input(&mut state, TasksPanelInput::Stop);
        wait_until(|| {
            crate::tools::background_task_completion(session, "tasks-panel-log-task").is_some()
        });

        tasks_panel_input(&mut state, TasksPanelInput::Back);
        let list = view(&state);
        assert!(list.log.is_none());
        assert_eq!(list.rows.len(), 1);
        tasks_panel_input(&mut state, TasksPanelInput::Close);
        assert!(state.tasks_panel.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_finished_task_without_a_log_shows_its_retained_output() {
        let session = "tasks-panel-retained-session";
        spawn(session, "tasks-panel-retained-task", "echo kept", None);
        wait_until(|| {
            crate::tools::background_task_completion(session, "tasks-panel-retained-task").is_some()
        });
        let mut state = state_for(session);
        show_tasks_panel(&mut state);
        tasks_panel_input(&mut state, TasksPanelInput::Open);
        let shown = view(&state).log.expect("log is open");
        assert!(shown.text.contains("kept"), "{}", shown.text);
    }

    #[test]
    fn an_empty_panel_ignores_every_input_but_close() {
        let mut state = state_for("tasks-panel-empty-session");
        tasks_panel_input(&mut state, TasksPanelInput::Open);
        assert!(state.tasks_panel.is_none());
        show_tasks_panel(&mut state);
        for input in [
            TasksPanelInput::Up,
            TasksPanelInput::Down,
            TasksPanelInput::Open,
            TasksPanelInput::Stop,
        ] {
            tasks_panel_input(&mut state, input);
            let empty = view(&state);
            assert!(empty.rows.is_empty() && empty.log.is_none());
        }
        tasks_panel_input(&mut state, TasksPanelInput::Back);
        assert!(state.tasks_panel.is_none());
    }
}
