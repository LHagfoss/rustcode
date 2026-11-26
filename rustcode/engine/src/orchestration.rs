//! Threads that message other threads (#2637), milestone 1 of
//! `docs/architecture/cross-session-orchestration.md`: an agent lists the
//! threads of its own project, sends one a message and waits for its answer.
//! Version 2 (`docs/orchestrator.md`) adds `read_session`, `start_session`
//! and `fan_out`, and offers the same tools to external harnesses through
//! `rustcode mcp-orchestrator` ([`mcp_server`], [`hub`]).
//!
//! The client that hosts the threads carries them out. Each call becomes a
//! `_rustcode/orchestration` request to the client that advertised
//! `clientCapabilities._meta.rustcode.orchestration`. That client asks the
//! user for a grant, delivers the message as an attributed turn of the
//! target thread through the same turn queue a typed message uses, and
//! answers with a bounded result. The engine never drives another session
//! itself, so the target keeps its own turn gate, sandbox and approvals.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::preview::PreviewClient;
use rustcode_core::ToolErrorKind;
use rustcode_tools::ToolExecutionOutput;

pub mod ask_user;
mod hub_receipts;
pub mod local;
mod local_interrupt;
mod local_merge;
pub(crate) mod local_threads;
pub(crate) mod local_wake;
mod mcp_names;
pub mod mcp_server;
pub(crate) mod memory_tools;
pub mod outside;
pub mod outside_http;
pub mod outside_hub;
pub(crate) mod outside_manage;
pub mod outside_passkey;
pub mod outside_signin;
pub mod outside_web;
pub mod presets;
pub mod secret;
pub mod tool_socket;
mod validation;

pub use validation::validate;
pub(crate) use validation::{absolute_on_any_machine, check_request_id};

pub const METHOD: &str = "_rustcode/orchestration";
/// Told to the client when a call was dropped before its answer arrived: the
/// harness stopped waiting for it (its own timeout, a cancel, the MCP server
/// closing), so the client ends the call at once instead of holding on to it.
pub const CANCELLED_METHOD: &str = "_rustcode/orchestration/cancelled";
pub const LIST_SESSIONS: &str = "list_sessions";
pub const MESSAGE_SESSION: &str = "message_session";
pub const WAIT_SESSION: &str = "wait_session";
pub const READ_SESSION: &str = "read_session";
pub const START_SESSION: &str = "start_session";
pub const FAN_OUT: &str = "fan_out";
pub const LIST_HARNESSES: &str = "list_harnesses";
pub const SEARCH_SESSIONS: &str = "search_sessions";
pub const LIST_WORKTREES: &str = "list_worktrees";
pub const COMPACT_SESSION: &str = "compact_session";
pub const UPDATE_SESSION: &str = "update_session";
pub const INTERRUPT_SESSION: &str = "interrupt_session";
pub const CONFIGURE_SESSION: &str = "configure_session";
pub const LIST_PROJECTS: &str = "list_projects";
pub const LIST_PULL_REQUESTS: &str = "list_pull_requests";
pub const TRANSFER_SESSION: &str = "transfer_session";
pub const LIST_PENDING_REQUESTS: &str = "list_pending_requests";
pub const ANSWER_REQUEST: &str = "answer_request";
pub const ORGANIZE_THREADS: &str = "organize_threads";
pub const THREAD_USAGE: &str = "thread_usage";
pub const QUERY_DATA: &str = "query_data";
pub const READ_ACTIVITY: &str = "read_activity";
pub const MANAGE_QUEUE: &str = "manage_queue";
/// Version 8: a delegate's pull request, merging it, and cleaning up after it.
pub const THREAD_STATUS: &str = "thread_status";
pub const MERGE_WHEN_GREEN: &str = "merge_when_green";
pub const CLEANUP_WORKTREE: &str = "cleanup_worktree";
/// Version 10: the user's projects, and the scheduler daemon's work.
pub const READ_PROJECT: &str = "read_project";
pub const CREATE_PROJECT: &str = "create_project";
pub const CLONE_PROJECT: &str = "clone_project";
pub const UPDATE_PROJECT: &str = "update_project";
pub const REMOVE_PROJECT: &str = "remove_project";
pub const CREATE_LOOP: &str = "create_loop";
pub const UPDATE_LOOP: &str = "update_loop";
pub const SCHEDULE_TASK: &str = "schedule_task";
pub const LIST_SCHEDULED_TASKS: &str = "list_scheduled_tasks";
pub const DELETE_SCHEDULED_TASK: &str = "delete_scheduled_task";
pub const RUN_SCHEDULED_TASK_NOW: &str = "run_scheduled_task_now";
/// Version 12: the other computers this app is paired with, and the machine a
/// call acts on.
pub const LIST_MACHINES: &str = "list_machines";
/// Version 22: run a shell command on any machine in the fleet.
pub const RUN_MACHINE_COMMAND: &str = "run_machine_command";
/// Version 13: where a thread's work stands, and moving it to where the user
/// can continue it (`docs/orchestrator.md` *Worktree handoff*).
pub const WORKTREE_STATUS: &str = "worktree_status";
pub const HANDOFF_WORKTREE: &str = "handoff_worktree";
/// The M1 tools, what a client advertising version 1 gets.
pub const TOOL_NAMES: [&str; 3] = [LIST_SESSIONS, MESSAGE_SESSION, WAIT_SESSION];
/// Added by version 2.
pub const V2_TOOL_NAMES: [&str; 3] = [READ_SESSION, START_SESSION, FAN_OUT];
/// Added by version 3: discover what can run, find threads, and manage them.
pub const V3_TOOL_NAMES: [&str; 10] = [
    LIST_HARNESSES,
    SEARCH_SESSIONS,
    LIST_WORKTREES,
    UPDATE_SESSION,
    INTERRUPT_SESSION,
    CONFIGURE_SESSION,
    LIST_PROJECTS,
    LIST_PULL_REQUESTS,
    TRANSFER_SESSION,
    LIST_PENDING_REQUESTS,
];
/// Added by version 4: the data store — tokens and reported cost, and SQL.
pub const V4_TOOL_NAMES: [&str; 2] = [THREAD_USAGE, QUERY_DATA];
/// The tools the engine answers itself even while an app hosts the others:
/// reads of this machine's data store, which the app could only pass through.
pub const ENGINE_TOOL_NAMES: [&str; 2] = [THREAD_USAGE, QUERY_DATA];

/// Whether the engine answers `name` itself, whoever hosts the thread's other
/// orchestration calls.
pub fn engine_answers(name: &str) -> bool {
    ENGINE_TOOL_NAMES.contains(&name)
}

/// Added by version 6: one thread's recorded activity, paged.
pub const V6_TOOL_NAMES: [&str; 1] = [READ_ACTIVITY];
/// Added by version 7: the messages this thread queued on another one.
pub const V7_TOOL_NAMES: [&str; 1] = [MANAGE_QUEUE];
/// Added by version 9: a delegate's pull request, merging and cleanup.
pub const V9_TOOL_NAMES: [&str; 3] = [THREAD_STATUS, MERGE_WHEN_GREEN, CLEANUP_WORKTREE];
/// Added by version 10: answering what a thread waits on, and organizing several at once.
pub const V10_TOOL_NAMES: [&str; 2] = [ANSWER_REQUEST, ORGANIZE_THREADS];
/// Added by version 11: the user's projects and the daemon's scheduled work.
pub const V11_TOOL_NAMES: [&str; 9] = [
    READ_PROJECT,
    CREATE_PROJECT,
    CLONE_PROJECT,
    UPDATE_PROJECT,
    REMOVE_PROJECT,
    SCHEDULE_TASK,
    LIST_SCHEDULED_TASKS,
    DELETE_SCHEDULED_TASK,
    RUN_SCHEDULED_TASK_NOW,
];
/// Added by version 12: the machines the app is paired with.
pub const V12_TOOL_NAMES: [&str; 1] = [LIST_MACHINES];
/// Added by version 13: where a thread's work stands, and handing it over.
pub const V13_TOOL_NAMES: [&str; 2] = [WORKTREE_STATUS, HANDOFF_WORKTREE];
/// The newest version this engine speaks.
pub const VERSION: u8 = MACHINE_EXEC_VERSION;
/// Version 22 adds running shell commands on any machine in the fleet (`run_machine_command`).
pub const MACHINE_EXEC_VERSION: u8 = 22;
/// Version 21 adds updating, pausing and resuming Loops (`update_loop`).
pub const UPDATE_LOOP_VERSION: u8 = 21;
pub const COMPACT_VERSION: u8 = 20;
/// Version 19 adds creating Loops.
pub const LOOPS_VERSION: u8 = 19;
/// Version 14 adds mid-turn messaging.
pub const STEER_VERSION: u8 = 14;
/// Version 15 adds `standalone` to `start_session` and `fan_out`: the thread
/// it starts belongs to the user at the top level, with no parent link, so
/// nothing is delivered back and `wait_session` has nothing to wait on. Like
/// the other argument versions, a client below it never sees the field and has
/// a call that carries one refused, so an older app cannot quietly start a
/// delegate the caller then waits for. No tool is added.
pub const STANDALONE_VERSION: u8 = 15;
/// Version 17 adds one-way messages and `return_to` to `message_session`.
pub const ONE_WAY_VERSION: u8 = 17;
/// Version 18 adds `account` and `model` to `transfer_session` (#3821): an
/// older app would move the thread on the harness's remembered login and model
/// instead, so it never sees the fields and has a call with them refused.
pub const TRANSFER_CHOICE_VERSION: u8 = 18;
/// Version 12 adds `machine` to the tools that read or act on a thread: the id
/// or name `list_machines` returned, omitted for this computer. A client that
/// advertised less never sees the field and has it refused, so an older app
/// cannot quietly act on this computer's thread instead of the named one.
pub const MACHINE_VERSION: u8 = 12;
/// The tools `machine` applies to.
pub const MACHINE_TOOL_NAMES: [&str; 11] = [
    LIST_SESSIONS,
    READ_SESSION,
    MESSAGE_SESSION,
    WAIT_SESSION,
    START_SESSION,
    FAN_OUT,
    LIST_HARNESSES,
    LIST_PROJECTS,
    INTERRUPT_SESSION,
    THREAD_STATUS,
    LIST_PENDING_REQUESTS,
];
/// Version 16 adds `machine` to the rest of the thread tools and the project
/// reads, so a remote delegate's questions, queue, activity and worktrees are
/// reached from here, and a project is read or cloned there. Gated like
/// version 12: an older app never sees the field and has one refused.
pub const REMOTE_TOOLS_VERSION: u8 = 16;
/// The tools version 16 gives `machine`.
pub const V16_MACHINE_TOOL_NAMES: [&str; 11] = [
    ANSWER_REQUEST,
    MANAGE_QUEUE,
    SEARCH_SESSIONS,
    READ_ACTIVITY,
    CONFIGURE_SESSION,
    UPDATE_SESSION,
    WORKTREE_STATUS,
    LIST_WORKTREES,
    LIST_PULL_REQUESTS,
    READ_PROJECT,
    CLONE_PROJECT,
];

/// The version that gave `name` its `machine` argument, or `None` for a tool
/// that never takes one.
pub fn machine_version(name: &str) -> Option<u8> {
    if MACHINE_TOOL_NAMES.contains(&name) {
        Some(MACHINE_VERSION)
    } else if V16_MACHINE_TOOL_NAMES.contains(&name) {
        Some(REMOTE_TOOLS_VERSION)
    } else if name == RUN_MACHINE_COMMAND {
        Some(MACHINE_EXEC_VERSION)
    } else {
        None
    }
}

/// Whether a client of `version` may name a machine on `name`.
pub fn takes_machine(name: &str, version: u8) -> bool {
    machine_version(name).is_some_and(|since| version >= since)
}
/// What `schedule_task` may ask the scheduler for.
pub const SCHEDULE_KINDS: [&str; 2] = ["cron", "at"];
/// Version 5 adds named account targeting to the existing `start_session` and
/// `fan_out` args. No tool is added, but a client that advertised less never
/// sees the `account` field and has it refused, so an older app cannot quietly
/// ignore it and start the child on another login.
pub const ACCOUNT_VERSION: u8 = 5;
/// Version 10 adds `attachments` to `message_session`. Like `account`, a client
/// below it never sees the argument and has one refused.
pub const ATTACHMENT_VERSION: u8 = 10;
/// What `update_session` may set a thread's status to.
pub const SESSION_STATUSES: [&str; 3] = ["active", "review", "settled"];
/// What `list_pull_requests` may ask for.
pub const PR_STATES: [&str; 4] = ["open", "closed", "merged", "all"];
/// What `manage_queue` may do to a message this thread queued on another one.
pub const QUEUE_ACTIONS: [&str; 4] = ["list", "edit", "cancel", "steer"];
/// Where `handoff_worktree` may move a thread's work.
pub const HANDOFF_TARGETS: [&str; 2] = ["checkout", "worktree"];
/// What `answer_request` may answer an approval with.
pub const APPROVAL_ANSWERS: [&str; 3] = ["allow_once", "allow_always", "deny"];
/// How many files one `message_session` may attach. The app also refuses one
/// over 5 MB; only it can measure a file.
pub const MAX_ATTACHMENTS: usize = 10;
/// How many threads one `organize_threads` call changes.
pub const MAX_ORGANIZED_THREADS: usize = 50;
/// The longest `queue_id` `manage_queue` accepts: one a list returned.
pub const MAX_QUEUE_ID_CHARS: usize = 128;

/// Turns `read_session` returns by default, and at most.
pub const DEFAULT_LAST_TURNS: u64 = 3;
pub const MAX_LAST_TURNS: u64 = 10;
/// How many harnesses one `fan_out` starts.
pub const MIN_FAN_OUT: usize = 2;
pub const MAX_FAN_OUT: usize = 4;
/// The retry key a caller may give a call that creates a thread or delivers a
/// message: the app stores that call's receipt under it, so a retry whose
/// answer was dropped returns the first result instead of acting twice.
pub const CLIENT_REQUEST_ID: &str = "client_request_id";
/// The tools a retry key applies to.
pub const KEYED_TOOL_NAMES: [&str; 4] = [MESSAGE_SESSION, START_SESSION, FAN_OUT, CREATE_LOOP];
/// The longest retry key, and the characters it may hold.
pub const MAX_CLIENT_REQUEST_ID_CHARS: usize = 128;
/// The longest SQL `query_data` takes: a question, not a dump.
pub const MAX_SQL_BYTES: usize = 8 * 1024;
/// The longest `after` cursor `read_activity` accepts: an `at`, a source and a
/// row id, so anything longer is not one of its tokens.
pub const MAX_ACTIVITY_CURSOR_BYTES: usize = 128;
/// The longest title a new thread takes.
pub const MAX_TITLE_CHARS: usize = 200;
/// How long `merge_when_green` waits for checks by default, and at most: 25
/// minutes, like `wait_session`. An MCP client aborts a call that sends nothing
/// for about half an hour (Claude Code), so a call never blocks longer than
/// this; a longer wait is another call (`pending` says the checks still run).
pub const DEFAULT_MERGE_WAIT_MS: u64 = MAX_WAIT_MS;
pub const MAX_MERGE_WAIT_MS: u64 = MAX_WAIT_MS;

/// The most text a message or a prompt carries: 32 KiB.
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;
/// The most an engine-rendered answer carries before it is a safety cut: one
/// `read_session` page with worst-case JSON escaping stays under it.
pub const MAX_OUTPUT_BYTES: usize = 128 * 1024;
/// How long `wait_session` waits by default, and at most: 25 minutes. A longer
/// `timeout_ms` is cut to this on the server, not refused. The default is long
/// because a wait returns as soon as a link finishes, and every wait that ends
/// with nothing done is another request that resends the caller's whole
/// context: a one-minute default turned an hour of waiting into sixty of them.
pub const DEFAULT_WAIT_MS: u64 = 20 * 60 * 1000;
pub const MAX_WAIT_MS: u64 = 25 * 60 * 1000;
/// How long the client may take on a list or a message: a message can wait
/// for the user to answer the grant.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Starting threads also creates worktrees and starts harnesses after the
/// user answers.
const START_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub const UNAVAILABLE_MESSAGE: &str = "error: messaging other threads needs the RustCode macOS app with \"Let threads message each other\" turned on in Settings. Nothing was sent.";

pub const PROMPT_HINT: &str = "\n\nOther threads: `list_sessions` lists this project's threads; `message_session` sends one a message (the user approves it once, and the thread answers in its own turn); `wait_session` returns that answer. Use them only when the user asks to involve another thread.";

/// [`PROMPT_HINT`] for version 2 clients.
pub const PROMPT_HINT_V2: &str = "\n\nOther threads: `list_sessions` and `read_session` show this project's threads; `message_session` messages one, `start_session` starts one, `fan_out` runs one task on 2-4 harnesses; `wait_session` returns their answers. The user approves first. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task, and use your own subagents for quick lookups. Use them only when the user asks to involve other threads.";

/// [`PROMPT_HINT_V2`] for version 3 clients.
pub const PROMPT_HINT_V3: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it. `list_harnesses` shows what can run, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user. `start_session`, `fan_out` and `message_session` give other threads work, `wait_session` returns their answers; A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task, and use your own subagents for quick lookups. `update_session`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those change other threads, so do them only when the user asks; the user approves. The app wakes this thread by itself when a thread it started finishes or asks a question, so never poll for one with sleeps: wait on it only when you need the answer in this turn.";

/// [`PROMPT_HINT_V3`] for version 4 clients, which can read the data store.
pub const PROMPT_HINT_V4: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user. `start_session`, `fan_out` and `message_session` give other threads work, `wait_session` returns their answers; A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task, and use your own subagents for quick lookups. `update_session`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those change other threads, so do them only when the user asks; the user approves.";

/// [`PROMPT_HINT_V4`] for version 5 clients, which can name a login.
pub const PROMPT_HINT_V5: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, `wait_session` returns their answers; A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task, and use your own subagents for quick lookups. `update_session`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those change other threads, so do them only when the user asks; the user approves.";

/// [`PROMPT_HINT_V5`] for version 6 clients, which can page a thread's activity.
pub const PROMPT_HINT_V6: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, `wait_session` returns their answers; A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task, and use your own subagents for quick lookups. `update_session`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those change other threads, so do them only when the user asks; the user approves.";

/// [`PROMPT_HINT_V6`] for version 7 clients, which can correct what they queued.
pub const PROMPT_HINT_V7: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task, and use your own subagents for quick lookups. `update_session`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those change other threads, so do them only when the user asks; the user approves.";

/// [`PROMPT_HINT_V7`] for version 8 clients: delegation goes through RustCode's
/// tools, as a Claude Code thread has no native subagent tool (#3176).
pub const PROMPT_HINT_V8: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`, while a harness with its own subagent tool may use it for quick lookups. `update_session`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those change other threads, so do them only when the user asks; the user approves.";

/// [`PROMPT_HINT_V8`] for version 9 clients, which can finish a delegate's work.
pub const PROMPT_HINT_V9: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`. `update_session`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, merge and cleanup change other threads, so do them only when the user asks; the user approves. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on all links in one `wait_session` with `link_ids`, or end the turn: the app wakes this thread when a child finishes. Call `list_harnesses` once.";

/// [`PROMPT_HINT_V9`] for version 10 clients, which can answer what a thread
/// waits on and organize several at once.
pub const PROMPT_HINT_V10: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user, and `answer_request` answers one of those on a thread in your own tree. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, and `message_session` attaches files by path; `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`. `update_session`, `organize_threads`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, answering, merge and cleanup change other threads, so do them only when the user asks; the user approves. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on all links in one `wait_session` with `link_ids`, or end the turn: the app wakes this thread when a child finishes. Call `list_harnesses` once.";
/// [`PROMPT_HINT_V10`] for version 11 clients, which can manage projects and
/// the scheduler daemon's work.
pub const PROMPT_HINT_V11: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user, and `answer_request` answers one of those on a thread in your own tree. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, and `message_session` attaches files by path; `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`. `update_session`, `organize_threads`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, answering, merge and cleanup change other threads, so do them only when the user asks; the user approves. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on all links in one `wait_session` with `link_ids`, or end the turn: the app wakes this thread when a child finishes. Call `list_harnesses` once. Projects: `read_project` reads a folder's git remote, branch and default branch; `create_project` registers a folder (an empty one gets `git init`), `clone_project` clones a URL into a parent folder, `update_project` sets a project's display name or the harness and model new threads in it start on, and `remove_project` unregisters one without deleting any files. Scheduled work: `schedule_task` adds a prompt that runs on a cron expression or once at an ISO time, `list_scheduled_tasks`, `delete_scheduled_task` and `run_scheduled_task_now` manage what the scheduler daemon holds; the daemon is `rustcode daemon start`, and without it nothing runs. Do both only when the user asks; the user approves.";

/// [`PROMPT_HINT_V11`] for version 12 clients, which can reach the other
/// computers this app is paired with.
pub const PROMPT_HINT_V12: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user, and `answer_request` answers one of those on a thread in your own tree. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, and `message_session` attaches files by path; `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it. The thread tools take another paired computer's id or name, as `list_machines` lists them, in `machine`: omitted, they act on this computer, and a remote thread's id carries its machine. Name one only when the user did. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`. `update_session`, `organize_threads`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, answering, merge and cleanup change other threads, so do them only when the user asks; the user approves, and a card for another machine names it. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on all links in one `wait_session` with `link_ids`, or end the turn: the app wakes this thread when a child finishes. Call `list_harnesses` once. Projects: `read_project` reads a folder's git remote, branch and default branch; `create_project` registers a folder (an empty one gets `git init`), `clone_project` clones a URL into a parent folder, `update_project` sets a project's display name or the harness and model new threads in it start on, and `remove_project` unregisters one without deleting any files. Scheduled work: `schedule_task` adds a prompt that runs on a cron expression or once at an ISO time, `list_scheduled_tasks`, `delete_scheduled_task` and `run_scheduled_task_now` manage what the scheduler daemon holds; the daemon is `rustcode daemon start`, and without it nothing runs. Do both only when the user asks; the user approves.";
pub const PROMPT_HINT_V13: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user, and `answer_request` answers one of those on a thread in your own tree. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, and `message_session` attaches files by path; `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it; `worktree_status` reports a worktree's branch, its distance from the branch it tracks, changed files, last commit and pull request; `handoff_worktree` moves a delegate's work into the user's checkout, or a thread's checkout work into a worktree of its own, and keeps every commit. The thread tools take another paired computer's id or name, as `list_machines` lists them, in `machine`: omitted, they act on this computer, and a remote thread's id carries its machine. Name one only when the user did. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`. `update_session`, `organize_threads`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, answering, merge, cleanup and handoff change other threads, so do them only when the user asks; the user approves, and a card for another machine names it. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on all links in one `wait_session` with `link_ids`, or end the turn: the app wakes this thread when a child finishes. Call `list_harnesses` once. Projects: `read_project` reads a folder's git remote, branch and default branch; `create_project` registers a folder (an empty one gets `git init`), `clone_project` clones a URL into a parent folder, `update_project` sets a project's display name or the harness and model new threads in it start on, and `remove_project` unregisters one without deleting any files. Scheduled work: `schedule_task` adds a prompt that runs on a cron expression or once at an ISO time, `list_scheduled_tasks`, `delete_scheduled_task` and `run_scheduled_task_now` manage what the scheduler daemon holds; the daemon is `rustcode daemon start`, and without it nothing runs. Do both only when the user asks; the user approves.";

pub const PROMPT_HINT_V14: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user, and `answer_request` answers one of those on a thread in your own tree. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, and `message_session` reaches any project with user approval, attaches files by path and takes `steer: true` for mid-turn input (otherwise queued); `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it; `worktree_status` reports a worktree's branch, its distance from the branch it tracks, changed files, last commit and pull request; `handoff_worktree` moves a delegate's work into the user's checkout, or a thread's checkout work into a worktree of its own, and keeps every commit. The thread tools take another paired computer's id or name, as `list_machines` lists them, in `machine`: omitted, they act on this computer, and a remote thread's id carries its machine. Name one only when the user did. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`. `update_session`, `organize_threads`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, answering, merge, cleanup and handoff change other threads, so do them only when the user asks; the user approves, and a card for another machine names it. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on all links in one `wait_session` with `link_ids`, or end the turn: the app wakes this thread when a child finishes. Call `list_harnesses` once. Projects: `read_project` reads a folder's git remote, branch and default branch; `create_project` registers a folder (an empty one gets `git init`), `clone_project` clones a URL into a parent folder, `update_project` sets a project's display name or the harness and model new threads in it start on, and `remove_project` unregisters one without deleting any files. Scheduled work: `schedule_task` adds a prompt that runs on a cron expression or once at an ISO time, `list_scheduled_tasks`, `delete_scheduled_task` and `run_scheduled_task_now` manage what the scheduler daemon holds; the daemon is `rustcode daemon start`, and without it nothing runs. Do both only when the user asks; the user approves.";

/// [`PROMPT_HINT_V14`] for version 15 clients, which can start a thread
/// of the user's own instead of a delegate (issue 3443).
pub const PROMPT_HINT_V15: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user, and `answer_request` answers one of those on a thread in your own tree. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, and `message_session` reaches any project with user approval, attaches files by path and takes `steer: true` for mid-turn input (otherwise queued); `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it; `worktree_status` reports a worktree's branch, its distance from the branch it tracks, changed files, last commit and pull request; `handoff_worktree` moves a delegate's work into the user's checkout, or a thread's checkout work into a worktree of its own, and keeps every commit. The thread tools take another paired computer's id or name, as `list_machines` lists them, in `machine`: omitted, they act on this computer, and a remote thread's id carries its machine. Name one only when the user did. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. A thread `start_session` or `fan_out` starts is a delegate, shown nested under this one: give it a whole task; a Claude Code thread has no subagent tool, so delegate through `start_session` or `fan_out`. For a thread the user will follow up themselves, pass `standalone: true`: it is a thread of their own at the top level of its project, nothing is delivered back and `wait_session` does not apply, so name its session id to them. `update_session`, `organize_threads`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, answering, merge, cleanup and handoff change other threads, so do them only when the user asks; the user approves, and a card for another machine names it. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on all links in one `wait_session` with `link_ids`, or end the turn: the app wakes this thread when a child finishes. Call `list_harnesses` once. Projects: `read_project` reads a folder's git remote, branch and default branch; `create_project` registers a folder (an empty one gets `git init`), `clone_project` clones a URL into a parent folder, `update_project` sets a project's display name or the harness and model new threads in it start on, and `remove_project` unregisters one without deleting any files. Scheduled work: `schedule_task` adds a prompt that runs on a cron expression or once at an ISO time, `list_scheduled_tasks`, `delete_scheduled_task` and `run_scheduled_task_now` manage what the scheduler daemon holds; the daemon is `rustcode daemon start`, and without it nothing runs. Do both only when the user asks; the user approves.";
/// [`PROMPT_HINT_V15`] for version 16 clients, which reach the rest of the
/// thread tools, and a project by its git remote, on another machine.
pub const PROMPT_HINT_V16: &str = "\n\nRustCode's threads: `list_sessions`, `search_sessions`, `read_session`, `list_worktrees` and `list_projects` answer what the user asks about their threads, projects and past work, so ask them instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it, and `read_activity` pages what RustCode recorded about it (events, retries, provider attempts) as metadata, never prompts. `thread_usage` totals tokens and the cost each harness reported (never a price RustCode computed) by harness, model, day, project or thread; `query_data` runs one read-only SQL statement over what the harnesses recorded (turns, tool_calls, threads, daily_usage). `list_harnesses` shows what can run and its named accounts, `list_pull_requests` this project's pull requests, `list_pending_requests` which threads wait on the user, and `answer_request` answers one of those on a thread in your own tree. `start_session` and `fan_out` take a harness's named `account`; `start_session`, `fan_out` and `message_session` give other threads work, and `message_session` reaches any project with user approval, attaches files by path and takes `steer: true` for mid-turn input (otherwise queued); `wait_session` returns their answers; `manage_queue` lists, edits, cancels or steers what this thread queued on a busy one. `thread_status` reports a delegate's worktree, branch, pull request and checks; `merge_when_green` merges it once green; `cleanup_worktree` removes its worktree and branch and settles it; `worktree_status` reports a worktree's branch, its distance from the branch it tracks, changed files, last commit and pull request; `handoff_worktree` moves a delegate's work into the user's checkout, or a thread's checkout work into a worktree of its own, and keeps every commit. The thread tools, `list_projects`, `read_project` and `clone_project` take another paired computer's id or name, as `list_machines` lists them, in `machine`: omitted, they act on this computer, and a remote thread's id carries its machine. Name one only when the user did, and never fall back to this computer when it is offline. There a thread starts in the project with this one's git remote; with none, pass `project` from the list the refusal gives, or `clone_project` there. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path. Use `start_session` with `standalone: true` when the user says “start a new thread” or “start an agent in a new thread”. It appears at the top level of the project. Give the user its session id. Use `start_session` without standalone or `fan_out` for delegates only when the user explicitly asks for a delegate or asks for the work to report back here. A native subagent is a helper inside the current thread. Use one only when the user explicitly asks for a subagent and the harness supports it. If “start an agent” alone leaves the intent unclear, ask which they mean before starting anything. `update_session`, `organize_threads`, `configure_session`, `transfer_session` and `interrupt_session` manage them. Those, answering, merge, cleanup and handoff change other threads, so do them only when the user asks; the user approves, and a card for another machine names it. If you have not named this thread yet, make `update_session` your first tool call, with a short title (6 words or fewer) for the task; do it even if the thread already has a title, since a title the user chose stays and the answer says `title_kept`. When the user says to settle (\"settle\", \"settle this\", \"settle it\"), call `update_session` with `status: \"settled\"`: on this thread, or on the thread they name. Settling only marks it done: it leaves the active list and the app closes it if it is on screen; it does not merge, push or clean anything up. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates, and check their pull requests with `thread_status`. Wait on a whole wave in one `wait_session` with `link_ids` and `all: true`, which answers once every link has settled, or end the turn: the app wakes this thread when children finish, several in one wake. Call `list_harnesses` once. Projects: `read_project` reads a folder's git remote, branch and default branch; `create_project` registers a folder (an empty one gets `git init`), `clone_project` clones a URL into a parent folder, `update_project` sets a project's display name or the harness and model new threads in it start on, and `remove_project` unregisters one without deleting any files. Use `request_secret` for credentials instead of asking for them in chat; never read or print the destination file. `manage_settings` lists and reads RustCode's own settings (auto-compact, delegate limits, default access and the rest) and sets or resets one when the user asks, and `manage_capabilities` lists and toggles skills and MCP servers, restarts an MCP server, picks the default harness and switches the theme; the user approves each change, so never edit `settings.json` or `config.toml` by hand. Scheduled work: `schedule_task` adds a prompt that runs on a cron expression or once at an ISO time, `list_scheduled_tasks`, `delete_scheduled_task` and `run_scheduled_task_now` manage what the scheduler daemon holds; the daemon is `rustcode daemon start`, and without it nothing runs. Do both only when the user asks; the user approves.";

/// The prompt hint a client of `version` gets.
pub fn prompt_hint(version: u8) -> &'static str {
    match version {
        0 => "",
        1 => PROMPT_HINT,
        2 => PROMPT_HINT_V2,
        3 => PROMPT_HINT_V3,
        4 => PROMPT_HINT_V4,
        5 => PROMPT_HINT_V5,
        6 => PROMPT_HINT_V6,
        7 => PROMPT_HINT_V7,
        8 => PROMPT_HINT_V8,
        9 => PROMPT_HINT_V9,
        10 => PROMPT_HINT_V10,
        11 => PROMPT_HINT_V11,
        12 => PROMPT_HINT_V12,
        13 => PROMPT_HINT_V13,
        14 => PROMPT_HINT_V14,
        15 => PROMPT_HINT_V15,
        16..=18 => PROMPT_HINT_V16,
        _ => {
            static HINT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            HINT.get_or_init(|| {
                format!("{PROMPT_HINT_V16} To manage jobs (prompts run once, on a schedule, or from a webhook, shown in the app's Jobs page), use `create_loop`, `list_scheduled_tasks`, `update_loop`, `run_scheduled_task_now` and `delete_scheduled_task` when the user asks. The user approves changes in the app.")
            })
        }
    }
}

/// The longest `instructions` an MCP client keeps. Claude Code cuts the text
/// off a little past 4,000 characters, and [`PROMPT_HINT_V16`] is over 5,000:
/// the rules at its end (credentials, settings files, scheduling) never
/// reached the model.
#[cfg(test)]
pub const MCP_INSTRUCTIONS_LIMIT: usize = 4000;

/// What the MCP server tells a version 16 client: the rules that span tools,
/// with the ones that protect the user first. What a single tool does is in
/// that tool's description, which a client does not cut.
pub const MCP_INSTRUCTIONS_V16: &str = "RustCode's threads, projects, settings and usage. Rules first. Name this thread first: if you have not called `update_session` with a title in this thread yet, make it your first tool call, with a short title (6 words or fewer) for the task. Do it even if the thread already has a title; a title the user chose stays and the answer says `title_kept`. Use `request_secret` for credentials instead of asking for them in chat; never read or print the destination file. Change RustCode's settings, skills and MCP servers with `manage_settings` and `manage_capabilities`; never edit `settings.json` or `config.toml` by hand. Tools that change another thread (`update_session`, `organize_threads`, `configure_session`, `transfer_session`, `interrupt_session`, `answer_request`, `merge_when_green`, `cleanup_worktree`, `handoff_worktree`), a project or a loop run only when the user asks; the user approves, and a card for another machine names it. `machine` takes another paired computer's id or name from `list_machines`: omitted, a tool acts on this computer. Name one only when the user did, and never fall back to this computer when it is offline. Delegates use `$TMPDIR/rustcode/<session>`, never a shared `/tmp` path.\n\nQuestions about the user's threads, projects, past work and usage: ask `list_sessions`, `search_sessions`, `read_session`, `list_worktrees`, `list_projects`, `thread_usage` and `query_data` instead of looking for that data elsewhere, and pass `all_projects` to reach every project. Given a thread id, `read_session` it before answering about it.\n\nUse `start_session` with `standalone: true` when the user says “start a new thread” or “start an agent in a new thread”. It appears at the top level of the project. Give the user its session id. Use `start_session` without standalone or `fan_out` for delegates only when the user explicitly asks for a delegate or asks for the work to report back here. A native subagent is a helper inside the current thread. Use one only when the user explicitly asks for a subagent and the harness supports it. If “start an agent” alone leaves the intent unclear, ask which they mean before starting anything. Use `message_session` to send work to an existing thread when the user asks. Stay lean: every call resends this thread's context, so leave reading code, diffs, logs and CI to delegates and check their pull requests with `thread_status`. Wait on a whole wave in one `wait_session` with `link_ids` and `all: true`, or end the turn: the app wakes this thread when children finish. Call `list_harnesses` once.\n\nThis thread: when the user says to settle (\"settle\", \"settle this\", \"settle it\"), call `update_session` with `status: \"settled\"` on this thread or the one they name. Settling only marks it done; it does not merge, push or clean anything up.";

/// The setting that lists [`EXTRA_TOOL_NAMES`] to a thread; off by default.
pub const ALL_TOOLS_SETTING: &str = "orchestration-all-tools";

/// The tools a thread's MCP server leaves out of its list unless the user
/// turns [`ALL_TOOLS_SETTING`] on. Every listed tool is sent with each request
/// of a harness that does not defer them, and these are the ones few threads
/// call: managing projects, loop mutations (`update_loop`, `run_loop`,
/// `delete_loop`, legacy `schedule_task`), workflow runs and moving a worktree.
/// Leaving them out takes about a third off the list. One that is called anyway
/// is still answered, so a harness holding an older list keeps working.
pub const EXTRA_TOOL_NAMES: [&str; 14] = [
    READ_PROJECT,
    CREATE_PROJECT,
    CLONE_PROJECT,
    UPDATE_PROJECT,
    REMOVE_PROJECT,
    SCHEDULE_TASK,
    UPDATE_LOOP,
    DELETE_SCHEDULED_TASK,
    RUN_SCHEDULED_TASK_NOW,
    WORKTREE_STATUS,
    HANDOFF_WORKTREE,
    crate::workflow::RUN_TOOL_NAME,
    crate::workflow::STATUS_TOOL_NAME,
    crate::workflow::STOP_TOOL_NAME,
];

/// What the MCP server adds to its instructions when it lists the core tools
/// alone, so an agent asked for one of the others knows where it went.
pub const CORE_TOOLS_NOTE: &str = "Not listed here: the tools that manage projects, edit/run/delete jobs (`update_job`, `run_job`, `delete_job`), workflow runs and worktree handoff. When the user asks for one, turn `orchestration-all-tools` on with `manage_settings`; threads started after that list them.";

/// Whether the user has every tool listed to threads ([`ALL_TOOLS_SETTING`]).
pub fn lists_all_tools() -> bool {
    crate::acp::settings::value(ALL_TOOLS_SETTING)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// The `instructions` the MCP server gives a client of `version`. Up to
/// version 15 the prompt hint fits what a client keeps (4,000 characters); from 16
/// it does not, so those clients get the cross-tool rules alone.
pub fn mcp_instructions(version: u8) -> &'static str {
    match version {
        0..=15 => prompt_hint(version).trim(),
        16..=18 => MCP_INSTRUCTIONS_V16,
        _ => {
            static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            TEXT.get_or_init(|| {
                format!("{MCP_INSTRUCTIONS_V16}\n\nJobs: `list_scheduled_tasks` lists jobs, and `create_loop` adds a job to the app's Jobs page when the user asks; the user approves the schedule and prompt in the app.")
            })
        }
    }
}

/// Every orchestration tool of any version.
pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
        || V2_TOOL_NAMES.contains(&name)
        || V3_TOOL_NAMES.contains(&name)
        || V4_TOOL_NAMES.contains(&name)
        || V6_TOOL_NAMES.contains(&name)
        || V7_TOOL_NAMES.contains(&name)
        || V9_TOOL_NAMES.contains(&name)
        || V10_TOOL_NAMES.contains(&name)
        || V11_TOOL_NAMES.contains(&name)
        || V12_TOOL_NAMES.contains(&name)
        || V13_TOOL_NAMES.contains(&name)
        || matches!(
            name,
            CREATE_LOOP | UPDATE_LOOP | COMPACT_SESSION | RUN_MACHINE_COMMAND
        )
}

/// Whether `name` needs a version 2 client.
pub fn is_v2_tool(name: &str) -> bool {
    V2_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 3 client.
pub fn is_v3_tool(name: &str) -> bool {
    V3_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 4 client (the local data store).
pub fn is_v4_tool(name: &str) -> bool {
    V4_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 6 client (the activity read).
pub fn is_v6_tool(name: &str) -> bool {
    V6_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 7 client (its own queued messages).
pub fn is_v7_tool(name: &str) -> bool {
    V7_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 9 client (delegate pull requests and cleanup).
pub fn is_v9_tool(name: &str) -> bool {
    V9_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 10 client (answering requests, batching a manage).
pub fn is_v10_tool(name: &str) -> bool {
    V10_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 11 client (projects and scheduled tasks).
pub fn is_v11_tool(name: &str) -> bool {
    V11_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 12 client (the other paired machines).
pub fn is_v12_tool(name: &str) -> bool {
    V12_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 13 client (worktree status and handoff).
pub fn is_v13_tool(name: &str) -> bool {
    V13_TOOL_NAMES.contains(&name)
}

/// The tools a client of `version` gets, in order.
pub fn tool_names(version: u8) -> Vec<&'static str> {
    match version {
        0 => Vec::new(),
        1 => TOOL_NAMES.to_vec(),
        2 => [
            LIST_SESSIONS,
            READ_SESSION,
            MESSAGE_SESSION,
            WAIT_SESSION,
            START_SESSION,
            FAN_OUT,
        ]
        .to_vec(),
        3 => {
            let mut names = tool_names(2);
            names.extend(V3_TOOL_NAMES);
            names
        }
        4 => {
            let mut names = tool_names(3);
            names.extend(V4_TOOL_NAMES);
            names
        }
        // Version 5 adds account targeting to existing tools, not another tool.
        5 => tool_names(4),
        6 => {
            let mut names = tool_names(5);
            names.extend(V6_TOOL_NAMES);
            names
        }
        7 => {
            let mut names = tool_names(6);
            names.extend(V7_TOOL_NAMES);
            names
        }
        // Version 8 adds native-subagent guidance to the hint, not another tool.
        8 => tool_names(7),
        9 => {
            let mut names = tool_names(8);
            names.extend(V9_TOOL_NAMES);
            names
        }
        10 => {
            let mut names = tool_names(9);
            names.extend(V10_TOOL_NAMES);
            names
        }
        11 => {
            let mut names = tool_names(10);
            names.extend(V11_TOOL_NAMES);
            names
        }
        12 => {
            let mut names = tool_names(11);
            names.extend(V12_TOOL_NAMES);
            names
        }
        13..=18 => {
            let mut names = tool_names(12);
            names.extend(V13_TOOL_NAMES);
            names
        }
        19 => {
            let mut names = tool_names(18);
            names.push(CREATE_LOOP);
            names
        }
        20 => {
            let mut names = tool_names(19);
            names.push(COMPACT_SESSION);
            names
        }
        21 => {
            let mut names = tool_names(20);
            names.push(UPDATE_LOOP);
            names
        }
        _ => {
            let mut names = tool_names(21);
            names.push(RUN_MACHINE_COMMAND);
            names
        }
    }
}

/// The orchestration version initialize `clientCapabilities._meta`
/// advertises, capped at the one this engine speaks; 0 for none.
pub fn client_meta_version(meta: Option<&serde_json::Map<String, Value>>) -> u8 {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("orchestration"))
        .and_then(Value::as_u64)
        .map_or(0, |version| version.min(u64::from(VERSION)) as u8)
}

/// Whether a client that hosts no orchestration asks the engine to serve the
/// tools itself: `_meta.rustcode.engineOrchestration` in `initialize`. The
/// desktop app sends it on its connection to a paired machine while its own
/// setting is on; it refuses tool calls from there, so without this a thread
/// on that machine had no tools at all.
pub fn client_meta_engine_hosted(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("engineOrchestration"))
        .and_then(Value::as_u64)
        .is_some_and(|version| version >= 1)
}

/// The schema a client of `version` gets. Named-account targeting arrived in
/// version 5, so a client that advertised less is never told about `account`;
/// [`execute_versioned`] refuses one it sends anyway.
pub fn schema_for(name: &str, version: u8, mut schema: Value) -> Value {
    if version < STEER_VERSION && name == MESSAGE_SESSION {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("steer");
        }
    }
    if version < ONE_WAY_VERSION && name == MESSAGE_SESSION {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("return_to");
        }
    }
    if version < TRANSFER_CHOICE_VERSION && name == TRANSFER_SESSION {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("account");
            properties.remove("model");
        }
    }
    // An older client never sees an argument it would ignore.
    if version < ATTACHMENT_VERSION && name == MESSAGE_SESSION {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("attachments");
        }
    }
    // A standalone thread arrived in version 14: an older app would file it
    // under the caller as a delegate instead.
    if version < STANDALONE_VERSION && (name == START_SESSION || name == FAN_OUT) {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("standalone");
        }
    }
    // Machine targeting arrived in version 12 (16 for the rest of the tools),
    // and leaves the schema the same way.
    if machine_version(name).is_some() && !takes_machine(name, version) {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("machine");
        }
    }
    if version >= ACCOUNT_VERSION {
        return schema;
    }
    match name {
        START_SESSION => {
            if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
                properties.remove("account");
            }
        }
        FAN_OUT => {
            if let Some(properties) = schema
                .get_mut("properties")
                .and_then(|properties| properties.get_mut("harnesses"))
                .and_then(|harnesses| harnesses.get_mut("items"))
                .and_then(|items| items.get_mut("properties"))
                .and_then(Value::as_object_mut)
            {
                properties.remove("account");
            }
        }
        _ => {}
    }
    schema
}

/// Whether a message asks for its reply to return to the caller.
pub(crate) fn waits_for_reply(args: &Value) -> bool {
    args.get("return_to").and_then(Value::as_str) == Some("me")
}

/// Whether a call asks for a one-way message by name, which only a
/// [`ONE_WAY_VERSION`] client can deliver without a link. An older client never
/// sees `return_to`, so a call without it is the message it always sent (with
/// a link the client makes), not a refusal.
pub(crate) fn names_wait(name: &str, args: &Value) -> bool {
    name == MESSAGE_SESSION
        && args
            .get("return_to")
            .and_then(Value::as_str)
            .is_some_and(|to| to != "me")
}

/// Whether a call asks for a standalone thread, which only a version 14 client
/// can do: an older app would file it under the caller as a delegate.
pub(crate) fn requests_standalone(name: &str, args: &Value) -> bool {
    (name == START_SESSION || name == FAN_OUT)
        && args
            .get("standalone")
            .is_some_and(|standalone| *standalone == Value::Bool(true))
}

/// Whether a call tries to name a machine, which only a client of
/// [`machine_version`] can do: an older app would act on this computer instead.
pub(crate) fn names_a_machine(name: &str, args: &Value) -> bool {
    machine_version(name).is_some()
        && args
            .get("machine")
            .is_some_and(|machine| !machine.is_null())
}

/// Whether a call tries to attach files, which only a version 10 client can do.
pub(crate) fn carries_attachments(name: &str, args: &Value) -> bool {
    name == MESSAGE_SESSION
        && args
            .get("attachments")
            .is_some_and(|attachments| !attachments.is_null())
}

/// Whether a call tries to name a login, which only a version 5 client can do.
/// Whether a `transfer_session` call names an account or a model, which only
/// a [`TRANSFER_CHOICE_VERSION`] client applies.
pub(crate) fn names_a_transfer_choice(name: &str, args: &Value) -> bool {
    name == TRANSFER_SESSION
        && ["account", "model"]
            .iter()
            .any(|key| args.get(*key).is_some_and(|value| !value.is_null()))
}

pub(crate) fn names_an_account(name: &str, args: &Value) -> bool {
    match name {
        START_SESSION => args
            .get("account")
            .is_some_and(|account| !account.is_null()),
        FAN_OUT => args
            .get("harnesses")
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .get("account")
                        .is_some_and(|account| !account.is_null())
                })
            }),
        _ => false,
    }
}

/// The retry key a call carries, when the tool takes one: `None` for a call
/// with no key, which behaves exactly as before.
pub fn request_key<'a>(name: &str, args: &'a Value) -> Option<&'a str> {
    if !KEYED_TOOL_NAMES.contains(&name) {
        return None;
    }
    args.get(CLIENT_REQUEST_ID).and_then(Value::as_str)
}

/// What a retry key is checked against: sha256 of the action and the call's
/// arguments, so a retry that spelled the same arguments in another order
/// still matches. The prompt is part of that hash and is never stored itself.
pub fn args_hash(name: &str, args: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut canonical = String::new();
    canonical_json(args, &mut canonical);
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

/// `value` as JSON with object keys sorted: `serde_json` keeps the order it
/// read (`preserve_order`), and a hash of that would change with the wording
/// alone. Anything else is written as it parses.
fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                canonical_json(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The wait a `wait_session` or `merge_when_green` call asked for, cut to the
/// cap; `None` for any other tool.
pub fn requested_wait(name: &str, args: &Value) -> Option<Duration> {
    let (default, max) = match name {
        WAIT_SESSION => (DEFAULT_WAIT_MS, MAX_WAIT_MS),
        MERGE_WHEN_GREEN => (DEFAULT_MERGE_WAIT_MS, MAX_MERGE_WAIT_MS),
        _ => return None,
    };
    let wait = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(default)
        .clamp(1, max);
    Some(Duration::from_millis(wait))
}

/// How long the client has to answer after a `wait_session` wait ends.
pub const WAIT_ANSWER_GRACE: Duration = Duration::from_secs(30);
/// How long the MCP server itself lets a `wait_session` run past its wait
/// before it answers `timeout` on its own, whatever the app is doing.
pub const WAIT_SERVER_GRACE: Duration = Duration::from_secs(5);
/// How long the client has to answer after a `merge_when_green` wait ends: it
/// merges once its own wait ends, re-reads the pull request, and answers; the
/// loop's tail can outlast the wait by a poll and a second quiet round.
pub const MERGE_ANSWER_GRACE: Duration = Duration::from_secs(180);

/// How long to wait for the client's answer to `name`.
pub fn timeout(name: &str, args: &Value) -> Duration {
    if name == START_SESSION || name == FAN_OUT {
        START_TIMEOUT
    } else if let Some(wait) = requested_wait(name, args) {
        wait + if name == WAIT_SESSION {
            WAIT_ANSWER_GRACE
        } else {
            MERGE_ANSWER_GRACE
        }
    } else {
        REQUEST_TIMEOUT
    }
}

/// The params that end a dropped call on the client: the session it was made
/// for, and the call id its request carried.
pub fn cancelled_params(session_id: &str, call_id: &str) -> Value {
    json!({ "sessionId": session_id, "callId": call_id })
}

/// An id for one forwarded call, unique in this process: the client keys what a
/// call holds by it, and drops that when the call goes.
fn next_call_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(1);
    format!("call-{}", CALLS.fetch_add(1, Ordering::Relaxed))
}

/// A built-in turn's `run_workflow`, `workflow_status` or `stop_workflow`
/// (#3943): the engine runs it, as the hub does for a harness's MCP server,
/// and the run's workers start through the same host as this thread's other
/// orchestration calls.
pub async fn execute_workflow(
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    let (client, version, session_id) = {
        let state = state.lock().await;
        // With no app the engine answers for itself at its own version, as
        // the tool menu does (`network.rs`).
        let version = if state.orchestration_client.is_some() {
            state.orchestration_version
        } else {
            VERSION
        };
        (
            state.orchestration_client.clone(),
            version,
            state.active_session_id.clone(),
        )
    };
    if let Err(reason) = crate::workflow::validate(name, args) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: invalid {name} call: {reason}. Nothing was started."),
            ToolErrorKind::InvalidArguments,
            false,
        );
    }
    match crate::workflow::answer(&session_id, client, version, cancel, name, args, true).await {
        Ok(content) => ToolExecutionOutput::success(content),
        Err(reason) => ToolExecutionOutput::failure(format!("error: {name} failed: {reason}")),
    }
}

/// Run one call through the session's host.
pub async fn execute(
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    let (client, version, local, session_id) = {
        let state = state.lock().await;
        (
            state.orchestration_client.clone(),
            state.orchestration_version,
            state.local_orchestration,
            state.active_session_id.clone(),
        )
    };
    let expanded = match with_preset(name, args) {
        Ok(expanded) => expanded,
        Err(refusal) => return refusal,
    };
    let args = expanded.as_ref().unwrap_or(args);
    // No client hosts the tools: the engine answers what it can itself and
    // names the app for the rest (#3422), instead of saying the tool does not
    // exist.
    let output = if (client.is_some() && engine_answers(name)) || (client.is_none() && local) {
        local::answer(&session_id, cancel, name, args).await
    } else {
        execute_versioned(client, version, cancel, name, args).await
    };
    with_preset_listing(name, output)
}

/// The call with its delegate `preset` expanded (#3391), before anything reads
/// it: the app and the no-app path see ordinary arguments. `None` when the call
/// names no preset.
pub(crate) fn with_preset(name: &str, args: &Value) -> Result<Option<Value>, ToolExecutionOutput> {
    let refuse = |reason: String| {
        ToolExecutionOutput::failure_with_kind(
            format!("error: invalid {name} call: {reason}. Nothing was sent."),
            ToolErrorKind::InvalidArguments,
            false,
        )
    };
    let expanded = presets::expand(name, args).map_err(refuse)?;
    // What the preset gave is checked like arguments the call passed: a
    // fan_out entry whose preset names no harness still needs one.
    if let Some(expanded) = &expanded {
        validate(name, expanded).map_err(refuse)?;
    }
    Ok(expanded)
}

/// `list_harnesses` ends with the saved delegate presets, so an agent learns
/// the names it can pass as `preset`.
pub(crate) fn with_preset_listing(
    name: &str,
    mut output: ToolExecutionOutput,
) -> ToolExecutionOutput {
    if name == LIST_HARNESSES
        && output.success
        && let Some(listing) = presets::listing(&presets::saved())
    {
        output.content = format!("{}\n\n{listing}", output.content.trim_end());
    }
    output
}

/// Run one call for a client of `version`: a version 1 client never gets
/// a version 2 call, even one the model made up.
pub async fn execute_versioned(
    client: Option<Arc<dyn PreviewClient>>,
    version: u8,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    if client.is_some() && !tool_names(version).contains(&name) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: {name} needs a newer RustCode app. Nothing was sent."),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    // An older client ignores arguments it does not know, so a login it would
    // ignore is refused here instead of starting the thread on another one.
    if client.is_some() && version < ACCOUNT_VERSION && names_an_account(name, args) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: {name} with account needs a newer RustCode app. Nothing was sent."),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    if client.is_some()
        && version < STEER_VERSION
        && name == MESSAGE_SESSION
        && args.get("steer").is_some()
    {
        return ToolExecutionOutput::failure_with_kind(
            "error: message_session with steer needs a newer RustCode app. Nothing was sent."
                .to_owned(),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    if client.is_some() && version < ONE_WAY_VERSION && names_wait(name, args) {
        return ToolExecutionOutput::failure_with_kind(
            "error: one-way message_session needs a newer RustCode app. Nothing was sent."
                .to_owned(),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    if client.is_some() && version < TRANSFER_CHOICE_VERSION && names_a_transfer_choice(name, args)
    {
        return ToolExecutionOutput::failure_with_kind(
            "error: transfer_session with account or model needs a newer RustCode app. Nothing was moved."
                .to_owned(),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    // The same for files an older client would drop on the floor.
    if client.is_some() && version < ATTACHMENT_VERSION && carries_attachments(name, args) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: {name} with attachments needs a newer RustCode app. Nothing was sent."),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    // And for a machine: acting on this computer when the user asked for another
    // one is worse than saying the app is too old.
    if client.is_some() && !takes_machine(name, version) && names_a_machine(name, args) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: {name} with machine needs a newer RustCode app. Nothing was sent."),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    execute_with(client, cancel, name, args).await
}

pub async fn execute_with(
    client: Option<Arc<dyn PreviewClient>>,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    if let Err(reason) = validate(name, args) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: invalid {name} call: {reason}. Nothing was sent."),
            ToolErrorKind::InvalidArguments,
            false,
        );
    }
    let Some(client) = client else {
        return ToolExecutionOutput::failure_with_kind(
            UNAVAILABLE_MESSAGE.to_owned(),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    };
    // The client is told this call by an id of its own, so a dropped call can be
    // ended on its side: an answer it is still preparing would be read by nobody.
    let call_id = next_call_id();
    let mut forwarded = json!({ "tool": name, "args": args, "callId": call_id });
    // A call that can create a thread or deliver a message carries its retry key
    // and the hash the client checks it against, so a retry after a dropped
    // answer is answered from the receipt instead of acting twice.
    if let Some(key) = request_key(name, args) {
        forwarded["clientRequestId"] = json!(key);
        forwarded["argsHash"] = json!(args_hash(name, args));
    }
    let request = client.request(forwarded);
    let answer = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            client.call_cancelled(&call_id);
            return ToolExecutionOutput::failure_with_kind(
                format!(
                    "error: {name} was cancelled before the app answered; if it was a message, it may have been delivered{}",
                    retry_hint(name, args)
                ),
                ToolErrorKind::Cancelled,
                false,
            );
        }
        answer = tokio::time::timeout(timeout(name, args), request) => answer,
    };
    match answer {
        Err(_) => ToolExecutionOutput::failure_with_kind(
            format!(
                "error: the app did not answer {name} in time; if it was a message, it may have been delivered{}",
                retry_hint(name, args)
            ),
            ToolErrorKind::UnavailableDependency,
            true,
        ),
        Ok(Err(error)) => ToolExecutionOutput::failure_with_kind(
            format!("error: {name} failed: {error}"),
            ToolErrorKind::UnavailableDependency,
            false,
        ),
        Ok(Ok(value)) => output(name, value),
    }
}

/// The sentence a dropped call ends with: a call that carried a retry key is
/// safe to retry, because the app replays what it already did.
pub(crate) fn retry_hint(name: &str, args: &Value) -> &'static str {
    if KEYED_TOOL_NAMES.contains(&name) && request_key(name, args).is_some() {
        "; retry it with the same client_request_id and the app replays that result instead of acting twice"
    } else {
        ""
    }
}

/// The client's answer as the tool's result: its `error` as a failure,
/// the rest as JSON for the model.
/// What a failure says when the app sent a code and no message (issue 3711).
fn default_reason(code: &str) -> &'static str {
    match code {
        "denied" => "the user did not allow it, or dismissed the approval",
        "unavailable" => "the app could not do it right now",
        "not_found" => "the thread or project was not found",
        "target_unavailable" => "the target thread is not available",
        "not_allowed" => "this thread is not allowed to do that",
        "conflict" => "it conflicts with the state of the thread",
        "timeout" => "it timed out waiting for the app or the user",
        _ => "the app reported no reason; the thread's debug log may say more",
    }
}

fn output(name: &str, value: Value) -> ToolExecutionOutput {
    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        // A failure never reaches the model without a reason: an app that sent none gets the one its
        // code implies, so the caller (and the row the user reads) can tell what to do next.
        let message = match error.get("message").and_then(Value::as_str).map(str::trim) {
            Some(message) if !message.is_empty() => message,
            _ => default_reason(code),
        };
        let kind = match code {
            "denied" => ToolErrorKind::PermissionDenied,
            "unavailable" => ToolErrorKind::UnavailableDependency,
            "not_found" | "target_unavailable" | "not_allowed" | "conflict" => {
                ToolErrorKind::Validation
            }
            _ => ToolErrorKind::Unknown,
        };
        // The slots a busy refusal leaves are part of its answer, not only of
        // its wording: the model reads them to decide what to do next.
        let left = error
            .get("links_left")
            .and_then(Value::as_u64)
            .map(|left| format!(" (links_left: {left})"))
            .unwrap_or_default();
        return ToolExecutionOutput::failure_with_kind(
            format!("error: {name} {code}: {message}{left}"),
            kind,
            false,
        );
    }
    // Compact, not pretty: the model reads either as well, and every later
    // request of the caller resends this text, so indentation is paid for again
    // on each one.
    let mut content = serde_json::to_string(&value).unwrap_or_default();
    // The client caps answers; this holds if one does not.
    if content.len() > MAX_OUTPUT_BYTES {
        let mut end = MAX_OUTPUT_BYTES;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        content.truncate(end);
        content.push_str("\n[truncated]");
    }
    ToolExecutionOutput::success(content)
}

#[cfg(test)]
mod tests;
