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
use crate::tools::{ToolErrorKind, ToolExecutionOutput};

pub mod hub;
pub mod mcp_server;

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
pub const SCHEDULE_TASK: &str = "schedule_task";
pub const LIST_SCHEDULED_TASKS: &str = "list_scheduled_tasks";
pub const DELETE_SCHEDULED_TASK: &str = "delete_scheduled_task";
pub const RUN_SCHEDULED_TASK_NOW: &str = "run_scheduled_task_now";
/// Version 12: the other computers this app is paired with, and the machine a
/// call acts on.
pub const LIST_MACHINES: &str = "list_machines";
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
pub const VERSION: u8 = 13;
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
pub const KEYED_TOOL_NAMES: [&str; 3] = [MESSAGE_SESSION, START_SESSION, FAN_OUT];
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
        _ => PROMPT_HINT_V13,
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
        _ => {
            let mut names = tool_names(12);
            names.extend(V13_TOOL_NAMES);
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

/// The schema a client of `version` gets. Named-account targeting arrived in
/// version 5, so a client that advertised less is never told about `account`;
/// [`execute_versioned`] refuses one it sends anyway.
pub fn schema_for(name: &str, version: u8, mut schema: Value) -> Value {
    // An older client never sees an argument it would ignore.
    if version < ATTACHMENT_VERSION && name == MESSAGE_SESSION {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.remove("attachments");
        }
    }
    // Machine targeting arrived in version 12, and leaves the schema the same way.
    if version < MACHINE_VERSION && MACHINE_TOOL_NAMES.contains(&name) {
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

/// Whether a call tries to name a machine, which only a version 12 client can
/// do: an older app would act on this computer instead.
pub(crate) fn names_a_machine(name: &str, args: &Value) -> bool {
    MACHINE_TOOL_NAMES.contains(&name)
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

/// A retry key: 1 to [`MAX_CLIENT_REQUEST_ID_CHARS`] characters of A-Z, a-z,
/// 0-9, `.`, `_`, `:` and `-`. The app's receipt store checks the same thing,
/// through this one rule.
pub(crate) fn check_request_id(id: &str) -> Result<(), String> {
    let allowed = !id.is_empty()
        && id.chars().count() <= MAX_CLIENT_REQUEST_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
    if allowed {
        return Ok(());
    }
    Err(format!(
        "{CLIENT_REQUEST_ID} must be 1 to {MAX_CLIENT_REQUEST_ID_CHARS} characters of A-Z, a-z, 0-9, '.', '_', ':' or '-'"
    ))
}

/// An optional retry key, absent or [`check_request_id`]'s shape.
fn optional_request_id(args: &Value) -> Result<(), String> {
    match args.get(CLIENT_REQUEST_ID) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(id)) => check_request_id(id),
        Some(_) => Err(format!(
            "{CLIENT_REQUEST_ID} must be 1 to {MAX_CLIENT_REQUEST_ID_CHARS} characters of A-Z, a-z, 0-9, '.', '_', ':' or '-'"
        )),
    }
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// A required non-empty text of at most [`MAX_MESSAGE_BYTES`].
fn bounded_text(args: &Value, key: &str) -> Result<(), String> {
    let value = text(args, key).unwrap_or("");
    if value.trim().is_empty() {
        return Err(format!("{key} is required"));
    }
    if value.len() > MAX_MESSAGE_BYTES {
        return Err(format!("{key} is over {MAX_MESSAGE_BYTES} bytes"));
    }
    Ok(())
}

/// An optional field that, when present, must be a non-empty string.
fn required_name(args: &Value, key: &str) -> Result<(), String> {
    match args.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(()),
        _ => Err(format!("{key} is required")),
    }
}

fn optional_name(args: &Value, key: &str) -> Result<(), String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) if !value.trim().is_empty() && value.len() <= 200 => Ok(()),
        Some(_) => Err(format!("{key} must be a short non-empty string")),
    }
}

/// An optional `attachments` list: absolute file paths, at most
/// [`MAX_ATTACHMENTS`] of them. Each must be a file of at most
/// [`MAX_ATTACHMENT_BYTES`], which only the app can measure; the engine checks
/// the shape so a harness cannot send a path it made up as an attachment.
fn optional_attachments(args: &Value) -> Result<(), String> {
    match args.get("attachments") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(paths)) => {
            if paths.len() > MAX_ATTACHMENTS {
                return Err(format!("attachments takes at most {MAX_ATTACHMENTS} files"));
            }
            for path in paths {
                let Some(path) = path.as_str().filter(|path| !path.trim().is_empty()) else {
                    return Err("each attachments entry must be a path".to_owned());
                };
                if !std::path::Path::new(path).is_absolute() {
                    return Err(format!("attachments entry {path} is not an absolute path"));
                }
            }
            Ok(())
        }
        Some(_) => Err("attachments must be a list of absolute paths".to_owned()),
    }
}

/// An optional flag: when present it must be a boolean, never a string.
fn optional_flag(args: &Value, key: &str) -> Result<(), String> {
    match args.get(key) {
        None | Some(Value::Null | Value::Bool(_)) => Ok(()),
        Some(_) => Err(format!("{key} must be true or false")),
    }
}

fn optional_title(args: &Value) -> Result<(), String> {
    match args.get("title") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(title)) if title.chars().count() <= MAX_TITLE_CHARS => Ok(()),
        Some(_) => Err(format!(
            "title must be a string of at most {MAX_TITLE_CHARS} characters"
        )),
    }
}

/// `path`: the absolute folder of an existing project, which every project
/// tool names.
fn absolute_project(args: &Value) -> Result<(), String> {
    match args.get("path") {
        Some(Value::String(path)) if std::path::Path::new(path).is_absolute() => Ok(()),
        Some(Value::String(_)) => Err("path must be an absolute folder".to_owned()),
        _ => Err("path is required".to_owned()),
    }
}

/// Check the arguments before they leave the engine.
pub fn validate(name: &str, args: &Value) -> Result<(), String> {
    // The machine a call acts on is the same argument on every tool that takes it.
    if MACHINE_TOOL_NAMES.contains(&name) {
        optional_name(args, "machine")?;
    }
    match name {
        LIST_SESSIONS | LIST_WORKTREES => optional_flag(args, "all_projects"),
        THREAD_USAGE => {
            if let Some(since) = args.get("since").and_then(Value::as_str) {
                crate::data_store::parse_since(since)?;
            }
            match args.get("group_by") {
                None | Some(Value::Null) => {}
                Some(Value::String(group))
                    if crate::data_store::GroupBy::parse(group).is_some() => {}
                Some(_) => {
                    return Err(format!(
                        "group_by must be one of {:?}",
                        crate::data_store::GroupBy::ALL
                    ));
                }
            }
            optional_name(args, "project")
        }
        QUERY_DATA => {
            let sql = text(args, "sql").unwrap_or("");
            if sql.trim().is_empty() {
                return Err("sql is required".to_owned());
            }
            if sql.len() > MAX_SQL_BYTES {
                return Err(format!("sql is over {MAX_SQL_BYTES} bytes"));
            }
            match args.get("limit") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::Number(limit)) if limit.as_u64().is_some() => Ok(()),
                Some(_) => Err("limit must be a whole number of rows".to_owned()),
            }
        }
        LIST_HARNESSES => {
            optional_name(args, "harness")?;
            optional_name(args, "model")
        }
        LIST_PROJECTS | LIST_PENDING_REQUESTS | LIST_MACHINES => Ok(()),
        TRANSFER_SESSION => {
            required_name(args, "session_id")?;
            required_name(args, "harness")
        }
        LIST_PULL_REQUESTS => match args.get("state") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(state)) if PR_STATES.contains(&state.as_str()) => Ok(()),
            Some(_) => Err(format!("state must be one of {PR_STATES:?}")),
        },
        SEARCH_SESSIONS => {
            optional_flag(args, "all_projects")?;
            if text(args, "query").unwrap_or("").trim().is_empty() {
                return Err("query is required".to_owned());
            }
            Ok(())
        }
        INTERRUPT_SESSION => {
            if text(args, "session_id").unwrap_or("").trim().is_empty() {
                return Err("session_id is required".to_owned());
            }
            Ok(())
        }
        CONFIGURE_SESSION => {
            optional_name(args, "session_id")?;
            optional_name(args, "model")?;
            optional_name(args, "effort")
        }
        UPDATE_SESSION => {
            optional_name(args, "session_id")?;
            optional_title(args)?;
            for key in ["pinned", "unread"] {
                if !matches!(args.get(key), None | Some(Value::Null | Value::Bool(_))) {
                    return Err(format!("{key} must be true or false"));
                }
            }
            match args.get("status") {
                None | Some(Value::Null) => {}
                Some(Value::String(status)) if SESSION_STATUSES.contains(&status.as_str()) => {}
                Some(_) => return Err(format!("status must be one of {SESSION_STATUSES:?}")),
            }
            match args.get("snoozed_until") {
                None | Some(Value::Null | Value::String(_)) => {}
                Some(_) => return Err("snoozed_until must be 1h, evening, tomorrow, next_week, an ISO 8601 time or null".to_owned()),
            }
            // `snoozed_until: null` is a change: it brings the thread back.
            if args.get("snoozed_until").is_some() {
                return Ok(());
            }
            let changes = ["title", "pinned", "unread", "status"];
            if !changes
                .iter()
                .any(|key| !matches!(args.get(*key), None | Some(Value::Null)))
            {
                return Err(
                    "give at least one of title, pinned, unread, status or snoozed_until"
                        .to_owned(),
                );
            }
            Ok(())
        }
        MESSAGE_SESSION => {
            let target = args.get("session_id").and_then(Value::as_str).unwrap_or("");
            let message = args.get("message").and_then(Value::as_str).unwrap_or("");
            if target.trim().is_empty() {
                return Err("session_id is required".to_owned());
            }
            if message.trim().is_empty() {
                return Err("message is required".to_owned());
            }
            if message.len() > MAX_MESSAGE_BYTES {
                return Err(format!("message is over {MAX_MESSAGE_BYTES} bytes"));
            }
            optional_attachments(args)?;
            optional_request_id(args)
        }
        READ_SESSION => {
            if text(args, "session_id").unwrap_or("").trim().is_empty() {
                return Err("session_id is required".to_owned());
            }
            match args.get("last_turns") {
                None | Some(Value::Null) => {}
                Some(turns)
                    if turns
                        .as_u64()
                        .is_some_and(|turns| (1..=MAX_LAST_TURNS).contains(&turns)) => {}
                Some(_) => return Err(format!("last_turns must be 1 to {MAX_LAST_TURNS}")),
            }
            match args.get("offset") {
                None | Some(Value::Null) => Ok(()),
                Some(offset) if offset.as_u64().is_some() => Ok(()),
                Some(_) => Err("offset must be a whole number of bytes".to_owned()),
            }
        }
        READ_ACTIVITY => {
            required_name(args, "session_id")?;
            match args.get("order") {
                None | Some(Value::Null) => {}
                Some(Value::String(order)) if order == "oldest" || order == "newest" => {}
                Some(_) => return Err("order must be oldest or newest".to_owned()),
            }
            match args.get("after") {
                None | Some(Value::Null) => {}
                Some(Value::String(after)) if after.len() <= MAX_ACTIVITY_CURSOR_BYTES => {}
                Some(_) => {
                    return Err(
                        "after must be the `next` token an earlier read_activity returned"
                            .to_owned(),
                    );
                }
            }
            match args.get("limit") {
                None | Some(Value::Null) => Ok(()),
                Some(limit) if limit.as_u64().is_some() => Ok(()),
                Some(_) => Err("limit must be a whole number of items".to_owned()),
            }
        }
        MANAGE_QUEUE => {
            required_name(args, "session_id")?;
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            if !QUEUE_ACTIONS.contains(&action) {
                return Err(format!("action must be one of {QUEUE_ACTIONS:?}"));
            }
            // A list names no message; the other three act on one of this thread's own.
            if action == "list" {
                return Ok(());
            }
            let queue_id = text(args, "queue_id").unwrap_or("");
            if queue_id.trim().is_empty() {
                return Err(format!("queue_id is required to {action}"));
            }
            if queue_id.len() > MAX_QUEUE_ID_CHARS {
                return Err("queue_id is not one a list returned".to_owned());
            }
            if action == "edit" {
                bounded_text(args, "text")?;
            }
            Ok(())
        }
        THREAD_STATUS => optional_name(args, "session_id"),
        WORKTREE_STATUS => {
            optional_name(args, "session_id")?;
            optional_flag(args, "all_projects")
        }
        HANDOFF_WORKTREE => {
            required_name(args, "session_id")?;
            // `to` defaults to `checkout`: the direction an orchestrator asks for
            // when the user says "I'll take it from here".
            match args.get("to") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(to)) if HANDOFF_TARGETS.contains(&to.as_str()) => Ok(()),
                Some(_) => Err(format!("to must be one of {HANDOFF_TARGETS:?}")),
            }
        }
        ANSWER_REQUEST => {
            required_name(args, "session_id")?;
            if !matches!(args.get("request_id"), Some(Value::String(id)) if !id.trim().is_empty()) {
                return Err(
                    "request_id is required, as list_pending_requests printed it (a string)"
                        .to_owned(),
                );
            }
            let choice = match args.get("choice") {
                None | Some(Value::Null) => None,
                Some(Value::String(choice)) if APPROVAL_ANSWERS.contains(&choice.as_str()) => {
                    Some(choice)
                }
                Some(_) => return Err(format!("choice must be one of {APPROVAL_ANSWERS:?}")),
            };
            let answer = match args.get("answer") {
                None | Some(Value::Null) => None,
                Some(Value::String(answer)) if !answer.trim().is_empty() => Some(answer),
                Some(_) => return Err("answer must be a sentence or an option id".to_owned()),
            };
            match (choice, answer) {
                (Some(_), Some(_)) => {
                    Err("give choice for an approval or answer for a question, not both".to_owned())
                }
                (None, None) => {
                    Err("choice (an approval) or answer (a question) is required".to_owned())
                }
                (Some(_), None) | (None, Some(_)) => Ok(()),
            }
        }
        ORGANIZE_THREADS => {
            let Some(ids) = args.get("session_ids").and_then(Value::as_array) else {
                return Err("session_ids is required".to_owned());
            };
            if ids.is_empty() {
                return Err("session_ids needs at least one thread".to_owned());
            }
            if ids.len() > MAX_ORGANIZED_THREADS {
                return Err(format!(
                    "session_ids takes at most {MAX_ORGANIZED_THREADS} threads"
                ));
            }
            if ids
                .iter()
                .any(|id| id.as_str().is_none_or(|id| id.trim().is_empty()))
            {
                return Err("each session_ids entry must be a thread id".to_owned());
            }
            optional_title(args)?;
            for key in ["pinned", "unread", "archived"] {
                if !matches!(args.get(key), None | Some(Value::Null | Value::Bool(_))) {
                    return Err(format!("{key} must be true or false"));
                }
            }
            if !["title", "pinned", "unread", "archived"]
                .iter()
                .any(|key| !matches!(args.get(*key), None | Some(Value::Null)))
            {
                return Err("give at least one of title, pinned, unread or archived".to_owned());
            }
            Ok(())
        }
        MERGE_WHEN_GREEN => {
            required_name(args, "session_id")?;
            match args.get("timeout_ms") {
                None | Some(Value::Null) => Ok(()),
                // A wait over the cap is cut to it (see `timeout`), not refused.
                Some(timeout) if timeout.as_u64().is_some_and(|ms| ms >= 1) => Ok(()),
                Some(_) => Err("timeout_ms must be a positive number of milliseconds".to_owned()),
            }
        }
        CLEANUP_WORKTREE => {
            required_name(args, "session_id")?;
            optional_flag(args, "settle")
        }
        READ_PROJECT | REMOVE_PROJECT => absolute_project(args),
        CREATE_PROJECT => {
            absolute_project(args)?;
            optional_flag(args, "init")
        }
        CLONE_PROJECT => {
            required_name(args, "url")?;
            match args.get("parent") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(path)) if std::path::Path::new(path).is_absolute() => Ok(()),
                Some(_) => Err("parent must be an absolute folder".to_owned()),
            }?;
            optional_name(args, "name")
        }
        UPDATE_PROJECT => {
            absolute_project(args)?;
            for key in ["name", "harness", "model"] {
                optional_name(args, key)?;
            }
            if !["name", "harness", "model"]
                .iter()
                .any(|key| !matches!(args.get(*key), None | Some(Value::Null)))
            {
                return Err("give at least one of name, harness or model".to_owned());
            }
            Ok(())
        }
        SCHEDULE_TASK => {
            bounded_text(args, "prompt")?;
            optional_name(args, "name")?;
            optional_name(args, "model_profile")?;
            match args.get("project") {
                None | Some(Value::Null) => {}
                Some(Value::String(path)) if std::path::Path::new(path).is_absolute() => {}
                Some(_) => return Err("project must be an absolute path".to_owned()),
            }
            let Some(schedule) = args.get("schedule").filter(|value| value.is_object()) else {
                return Err("schedule is required".to_owned());
            };
            let Some(kind) = schedule
                .get("kind")
                .and_then(Value::as_str)
                .filter(|kind| SCHEDULE_KINDS.contains(kind))
            else {
                return Err(format!("schedule.kind must be one of {SCHEDULE_KINDS:?}"));
            };
            match schedule.get("expression") {
                None | Some(Value::Null) => {}
                Some(Value::String(expression)) if expression.split_whitespace().count() == 5 => {}
                Some(_) => {
                    return Err(
                        "schedule.expression must be a five-field cron expression".to_owned()
                    );
                }
            }
            match schedule.get("at") {
                None | Some(Value::Null) => {}
                Some(Value::String(at)) if chrono::DateTime::parse_from_rfc3339(at).is_ok() => {}
                Some(_) => {
                    return Err("schedule.at must be an ISO 8601 time with its offset".to_owned());
                }
            }
            // The kind's own field is the one `schedule_spec` reads; without it the call
            // would only fail later, as a target that stopped answering.
            match kind {
                "cron" if schedule.get("expression").and_then(Value::as_str).is_none() => {
                    Err("schedule.expression is required for a cron schedule".to_owned())
                }
                "at" if schedule.get("at").and_then(Value::as_str).is_none() => {
                    Err("schedule.at is required for a one-off schedule".to_owned())
                }
                _ => Ok(()),
            }
        }
        LIST_SCHEDULED_TASKS => Ok(()),
        DELETE_SCHEDULED_TASK | RUN_SCHEDULED_TASK_NOW => required_name(args, "id"),
        START_SESSION => {
            bounded_text(args, "prompt")?;
            optional_title(args)?;
            for key in ["harness", "model", "effort", "account"] {
                optional_name(args, key)?;
            }
            if !matches!(
                args.get("worktree"),
                None | Some(Value::Null | Value::Bool(_))
            ) {
                return Err("worktree must be true or false".to_owned());
            }
            match args.get("project") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(path)) if std::path::Path::new(path).is_absolute() => Ok(()),
                Some(_) => Err("project must be an absolute path".to_owned()),
            }?;
            optional_request_id(args)
        }
        FAN_OUT => {
            bounded_text(args, "prompt")?;
            optional_title(args)?;
            let Some(harnesses) = args.get("harnesses").and_then(Value::as_array) else {
                return Err("harnesses is required".to_owned());
            };
            if !(MIN_FAN_OUT..=MAX_FAN_OUT).contains(&harnesses.len()) {
                return Err(format!(
                    "harnesses takes {MIN_FAN_OUT} to {MAX_FAN_OUT} entries"
                ));
            }
            let mut seen = Vec::with_capacity(harnesses.len());
            for entry in harnesses {
                if !entry.is_object() || text(entry, "harness").is_none() {
                    return Err("each harnesses entry needs a harness".to_owned());
                }
                for key in ["harness", "model", "effort", "account"] {
                    optional_name(entry, key)?;
                }
                // The same harness on two models, or on two named accounts, is a fair comparison.
                let key = (
                    text(entry, "harness"),
                    text(entry, "model"),
                    text(entry, "effort"),
                    text(entry, "account"),
                );
                if seen.contains(&key) {
                    return Err("harnesses has the same entry twice".to_owned());
                }
                seen.push(key);
            }
            // The project on that machine, as `start_session` takes it.
            match args.get("project") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(path)) if std::path::Path::new(path).is_absolute() => Ok(()),
                Some(_) => Err("project must be an absolute path".to_owned()),
            }?;
            optional_request_id(args)
        }
        WAIT_SESSION => {
            let one = args.get("link_id").and_then(Value::as_str).is_some();
            let many = args
                .get("link_ids")
                .and_then(Value::as_array)
                .is_some_and(|ids| !ids.is_empty());
            if one || many {
                Ok(())
            } else {
                Err("link_id or link_ids is required".to_owned())
            }
        }
        other => Err(format!("{other} is not an orchestration tool")),
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

/// Run one call through the session's host.
pub async fn execute(
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    let (client, version) = {
        let state = state.lock().await;
        (
            state.orchestration_client.clone(),
            state.orchestration_version,
        )
    };
    execute_versioned(client, version, cancel, name, args).await
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
    if client.is_some() && version < MACHINE_VERSION && names_a_machine(name, args) {
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
fn output(name: &str, value: Value) -> ToolExecutionOutput {
    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        let kind = match code {
            "denied" => ToolErrorKind::PermissionDenied,
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
mod tests {
    use super::*;

    struct Answering(Value);

    impl PreviewClient for Answering {
        fn request(&self, params: Value) -> crate::preview::PreviewFuture {
            let answer = self.0.clone();
            Box::pin(async move {
                assert!(params.get("tool").is_some());
                Ok(answer)
            })
        }
    }

    fn client(answer: Value) -> Option<Arc<dyn PreviewClient>> {
        Some(Arc::new(Answering(answer)) as Arc<dyn PreviewClient>)
    }

    #[test]
    fn the_client_must_advertise_the_host() {
        let meta = |value: Value| value.as_object().cloned();
        let version = |value: Value| client_meta_version(meta(value).as_ref());
        assert_eq!(version(json!({"rustcode": {"orchestration": 1}})), 1);
        assert_eq!(version(json!({"rustcode": {"orchestration": 2}})), 2);
        // A newer client gets what this engine speaks.
        assert_eq!(version(json!({"rustcode": {"orchestration": 9}})), 9);
        assert_eq!(version(json!({"rustcode": {"orchestration": 10}})), 10);
        assert_eq!(version(json!({"rustcode": {"orchestration": 11}})), 11);
        // A newer client gets what this engine speaks.
        assert_eq!(version(json!({"rustcode": {"orchestration": 14}})), VERSION);
        assert_eq!(version(json!({"rustcode": {"preview": 1}})), 0);
        assert_eq!(client_meta_version(None), 0);
    }

    #[test]
    fn version_one_keeps_exactly_the_m1_tools() {
        assert!(tool_names(0).is_empty());
        assert_eq!(tool_names(1), TOOL_NAMES.to_vec());
        let v2 = tool_names(2);
        assert_eq!(v2.len(), 6);
        for name in TOOL_NAMES.iter().chain(V2_TOOL_NAMES.iter()) {
            assert!(v2.contains(name), "{name}");
            assert!(is_tool(name));
        }
        assert!(V2_TOOL_NAMES.iter().all(|name| is_v2_tool(name)));
        assert!(!TOOL_NAMES.iter().any(|name| is_v2_tool(name)));
    }

    #[test]
    fn version_three_adds_the_management_tools_only_for_its_clients() {
        assert!(
            V3_TOOL_NAMES
                .iter()
                .all(|name| !tool_names(2).contains(name))
        );
        let v3 = tool_names(3);
        assert_eq!(v3.len(), 16);
        for name in V3_TOOL_NAMES {
            assert!(v3.contains(&name), "{name}");
            assert!(is_tool(name) && is_v3_tool(name));
            assert!(PROMPT_HINT_V3.contains(name), "{name}");
        }
    }

    #[test]
    fn version_four_adds_the_data_store_tools_only_for_its_clients() {
        assert_eq!(VERSION, 13);
        let v3 = tool_names(3);
        assert!(v3.iter().all(|name| !is_v4_tool(name)), "{v3:?}");
        let v4 = tool_names(4);
        assert_eq!(v4.len(), v3.len() + 2, "{v4:?}");
        assert!(v4.contains(&THREAD_USAGE) && v4.contains(&QUERY_DATA));
        // Version 5 adds account targeting to existing tools, not another tool.
        assert_eq!(tool_names(5), v4);
        // Version 6 adds the activity read, version 7 the queue control.
        assert_eq!(tool_names(6), [v4.clone(), V6_TOOL_NAMES.to_vec()].concat());
        assert_eq!(
            tool_names(7),
            [tool_names(6), V7_TOOL_NAMES.to_vec()].concat()
        );
        // Version 8 adds native-subagent guidance to the hint, not another tool.
        assert_eq!(tool_names(8), tool_names(7));
        assert_eq!(
            tool_names(9),
            [tool_names(8), V9_TOOL_NAMES.to_vec()].concat()
        );
        // Version 10 adds answering requests and organizing several threads.
        assert_eq!(
            tool_names(10),
            [tool_names(9), V10_TOOL_NAMES.to_vec()].concat()
        );
        // Version 11 adds the projects and the scheduler's work.
        assert_eq!(
            tool_names(11),
            [tool_names(10), V11_TOOL_NAMES.to_vec()].concat()
        );
        // Version 12 adds the other paired machines.
        assert_eq!(
            tool_names(12),
            [tool_names(11), V12_TOOL_NAMES.to_vec()].concat()
        );
        // Version 13 adds worktree status and handoff.
        assert_eq!(
            tool_names(13),
            [tool_names(12), V13_TOOL_NAMES.to_vec()].concat()
        );
        assert_eq!(
            tool_names(14),
            tool_names(13),
            "a newer client gets what this engine speaks"
        );
        // The hint says how to reach a machine and that omitting it stays here.
        for said in ["list_machines", "`machine`", "this computer"] {
            assert!(PROMPT_HINT_V12.contains(said), "{said}");
        }
        for name in V10_TOOL_NAMES {
            assert!(is_tool(name) && is_v10_tool(name), "{name}");
            assert!(
                !is_v9_tool(name) && !tool_names(9).contains(&name),
                "{name} is v10 only"
            );
            assert!(PROMPT_HINT_V10.contains(name), "{name}");
            assert!(!PROMPT_HINT_V9.contains(name), "{name} is v10 only");
        }
        for name in V11_TOOL_NAMES {
            assert!(is_tool(name) && is_v11_tool(name), "{name}");
            assert!(
                !is_v10_tool(name) && !tool_names(10).contains(&name),
                "{name} is v11 only"
            );
            assert!(PROMPT_HINT_V11.contains(name), "{name}");
            assert!(!PROMPT_HINT_V10.contains(name), "{name} is v11 only");
        }
        for name in V12_TOOL_NAMES {
            assert!(is_tool(name) && is_v12_tool(name), "{name}");
            assert!(
                !is_v11_tool(name) && !tool_names(11).contains(&name),
                "{name} is v12 only"
            );
            assert!(PROMPT_HINT_V12.contains(name), "{name}");
            assert!(!PROMPT_HINT_V11.contains(name), "{name} is v12 only");
        }
        for name in V13_TOOL_NAMES {
            assert!(is_tool(name) && is_v13_tool(name), "{name}");
            assert!(
                !is_v12_tool(name) && !tool_names(12).contains(&name),
                "{name} is v13 only"
            );
            assert!(PROMPT_HINT_V13.contains(name), "{name}");
            assert!(!PROMPT_HINT_V12.contains(name), "{name} is v13 only");
        }
        // The v13 hint keeps what v11 and v12 taught.
        for said in [
            "read_project",
            "schedule_task",
            "list_machines",
            "handoff_worktree",
        ] {
            assert!(PROMPT_HINT_V13.contains(said), "{said}");
        }
        // Every tool `machine` applies to is named in the v12 hint.
        for name in MACHINE_TOOL_NAMES {
            assert!(PROMPT_HINT_V12.contains(name), "{name} in the v12 hint");
        }
        for name in [THREAD_USAGE, QUERY_DATA] {
            assert!(is_tool(name) && is_v4_tool(name));
            assert!(PROMPT_HINT_V4.contains(name), "{name}");
            assert!(!PROMPT_HINT_V3.contains(name), "{name} is v4 only");
        }
        for name in V6_TOOL_NAMES {
            assert!(is_tool(name) && is_v6_tool(name));
            assert!(
                !is_v4_tool(name) && !v4.contains(&name),
                "{name} is v6 only"
            );
            assert!(PROMPT_HINT_V6.contains(name), "{name}");
            assert!(!PROMPT_HINT_V5.contains(name), "{name} is v6 only");
        }
        for name in V7_TOOL_NAMES {
            assert!(is_tool(name) && is_v7_tool(name));
            assert!(
                !is_v6_tool(name) && !tool_names(6).contains(&name),
                "{name} is v7 only"
            );
            assert!(PROMPT_HINT_V7.contains(name), "{name}");
            assert!(!PROMPT_HINT_V6.contains(name), "{name} is v7 only");
        }
        for name in V9_TOOL_NAMES {
            assert!(is_tool(name) && is_v9_tool(name));
            assert!(
                !is_v7_tool(name) && !tool_names(8).contains(&name),
                "{name} is v9 only"
            );
            assert!(PROMPT_HINT_V9.contains(name), "{name}");
            assert!(!PROMPT_HINT_V8.contains(name), "{name} is v9 only");
            assert!(!PROMPT_HINT_V7.contains(name), "{name} is v9 only");
        }
        // An orchestrator is told how to keep its own context small .
        for said in ["Stay lean", "link_ids", "end the turn", "thread_status"] {
            assert!(PROMPT_HINT_V9.contains(said), "{said}");
        }
        // The hint a client is told follows the version it advertised.
        assert_eq!(prompt_hint(1), PROMPT_HINT);
        assert_eq!(prompt_hint(2), PROMPT_HINT_V2);
        assert_eq!(prompt_hint(3), PROMPT_HINT_V3);
        assert_eq!(prompt_hint(4), PROMPT_HINT_V4);
        assert_eq!(prompt_hint(5), PROMPT_HINT_V5);
        assert_eq!(prompt_hint(6), PROMPT_HINT_V6);
        assert_eq!(prompt_hint(7), PROMPT_HINT_V7);
        assert_eq!(prompt_hint(8), PROMPT_HINT_V8);
        assert_eq!(prompt_hint(9), PROMPT_HINT_V9);
        assert_eq!(prompt_hint(10), PROMPT_HINT_V10);
        assert_eq!(prompt_hint(11), PROMPT_HINT_V11);
        assert_eq!(prompt_hint(12), PROMPT_HINT_V12);
        assert_eq!(prompt_hint(13), PROMPT_HINT_V13);
        assert_eq!(prompt_hint(14), PROMPT_HINT_V13);
        assert_eq!(prompt_hint(0), "");
        // Accounts arrived in 5: a v4 hint promises nothing it cannot do.
        assert!(!PROMPT_HINT_V4.contains("account"));
        assert!(PROMPT_HINT_V5.contains("account"));
        assert_eq!(
            client_meta_version(Some(
                &json!({"rustcode": {"orchestration": 11}})
                    .as_object()
                    .cloned()
                    .unwrap()
            )),
            11
        );
        assert_eq!(
            client_meta_version(Some(
                &json!({"rustcode": {"orchestration": 12}})
                    .as_object()
                    .cloned()
                    .unwrap()
            )),
            12
        );
        assert_eq!(
            client_meta_version(Some(
                &json!({"rustcode": {"orchestration": 3}})
                    .as_object()
                    .cloned()
                    .unwrap()
            )),
            3
        );
        // It rides in the root prompt and in the MCP server's instructions, so
        // it is bounded: v4 adds the two data tools, v5 the account line and
        // the line that says a thread an agent starts is its delegate, v6 the
        // activity read, v7 the queue control, v8 that native subagents go
        // through RustCode's tools, v9 a delegate's pull request.

        let tokens = crate::network::compaction::estimate_tokens(PROMPT_HINT_V11);
        println!(
            "prompt hints: v3 {} tokens, v4 {} tokens, v5 {} tokens, v6 {} tokens, v9 {tokens} tokens",
            crate::network::compaction::estimate_tokens(PROMPT_HINT_V3),
            crate::network::compaction::estimate_tokens(PROMPT_HINT_V4),
            crate::network::compaction::estimate_tokens(PROMPT_HINT_V5),
            crate::network::compaction::estimate_tokens(PROMPT_HINT_V6)
        );
        // v9 adds the three delegate pull-request tools, all named with their use,
        // and how an orchestrator keeps its own context small: those few lines
        // save far more than they cost on each request.
        assert!(tokens < 1000, "{tokens}");
        // v12 is the v11 hint plus the machine sentences, still bounded.
        let v12 = crate::network::compaction::estimate_tokens(PROMPT_HINT_V12);
        assert!(v12 < 1150, "{v12}");
        // v13 adds the worktree pair, and the seconds each call saves.
        let v13 = crate::network::compaction::estimate_tokens(PROMPT_HINT_V13);
        assert!(v13 < 1200, "{v13}");
    }

    #[test]
    fn worktree_tool_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        // Where every worktree stands: the caller's own, one thread's, or all.
        assert!(ok(WORKTREE_STATUS, json!({})));
        assert!(ok(WORKTREE_STATUS, json!({"session_id": "d1"})));
        assert!(ok(
            WORKTREE_STATUS,
            json!({"session_id": "d1", "all_projects": true})
        ));
        assert!(!ok(WORKTREE_STATUS, json!({"session_id": " "})));
        assert!(!ok(WORKTREE_STATUS, json!({"all_projects": "yes"})));
        // Handoff: `to` is `checkout` unless it says `worktree`.
        assert!(ok(HANDOFF_WORKTREE, json!({"session_id": "d1"})));
        assert!(ok(
            HANDOFF_WORKTREE,
            json!({"session_id": "d1", "to": "checkout"})
        ));
        assert!(ok(
            HANDOFF_WORKTREE,
            json!({"session_id": "d1", "to": "worktree"})
        ));
        assert!(!ok(HANDOFF_WORKTREE, json!({})));
        assert!(!ok(HANDOFF_WORKTREE, json!({"session_id": " "})));
        assert!(!ok(
            HANDOFF_WORKTREE,
            json!({"session_id": "d1", "to": "nowhere"})
        ));
        assert!(!ok(HANDOFF_WORKTREE, json!({"session_id": "d1", "to": 1})));
    }

    #[test]
    fn delegate_pull_request_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        assert!(ok(THREAD_STATUS, json!({})));
        assert!(ok(THREAD_STATUS, json!({"session_id": "d1"})));
        assert!(!ok(THREAD_STATUS, json!({"session_id": " "})));
        assert!(ok(MERGE_WHEN_GREEN, json!({"session_id": "d1"})));
        assert!(ok(
            MERGE_WHEN_GREEN,
            json!({"session_id": "d1", "timeout_ms": 60000})
        ));
        assert!(!ok(MERGE_WHEN_GREEN, json!({})));
        assert!(!ok(
            MERGE_WHEN_GREEN,
            json!({"session_id": "d1", "timeout_ms": 0})
        ));
        // Over the cap is cut to it by `timeout`, not refused.
        assert!(ok(
            MERGE_WHEN_GREEN,
            json!({"session_id": "d1", "timeout_ms": MAX_MERGE_WAIT_MS + 1})
        ));
        assert!(!ok(
            MERGE_WHEN_GREEN,
            json!({"session_id": "d1", "timeout_ms": "soon"})
        ));
        assert!(ok(CLEANUP_WORKTREE, json!({"session_id": "d1"})));
        assert!(ok(
            CLEANUP_WORKTREE,
            json!({"session_id": "d1", "settle": false})
        ));
        assert!(!ok(CLEANUP_WORKTREE, json!({})));
        assert!(!ok(
            CLEANUP_WORKTREE,
            json!({"session_id": "d1", "settle": "yes"})
        ));
        // The merge waits 25 minutes and the engine outlasts the loop's tail,
        // still inside the external harness's MCP timeout.
        let deadline = timeout(MERGE_WHEN_GREEN, &json!({"session_id": "d1"}));
        assert!(deadline >= Duration::from_millis(MAX_MERGE_WAIT_MS) + Duration::from_secs(170));
        assert!(deadline < Duration::from_secs(hub::TOOL_TIMEOUT_SECS));
        // A longer ask is cut to the cap.
        assert_eq!(
            timeout(
                MERGE_WHEN_GREEN,
                &json!({"session_id": "d1", "timeout_ms": 3_300_000})
            ),
            deadline
        );
    }

    #[test]
    fn project_and_scheduled_task_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        assert!(ok(READ_PROJECT, json!({"path": "/abs/project"})));
        assert!(!ok(READ_PROJECT, json!({"path": "project"})));
        assert!(!ok(READ_PROJECT, json!({})));
        assert!(ok(
            CREATE_PROJECT,
            json!({"path": "/abs/new", "init": false})
        ));
        assert!(!ok(
            CREATE_PROJECT,
            json!({"path": "/abs/new", "init": "yes"})
        ));
        assert!(ok(CLONE_PROJECT, json!({"url": "git@github.com:o/r.git"})));
        assert!(ok(
            CLONE_PROJECT,
            json!({"url": "git@github.com:o/r.git", "parent": "/abs/code", "name": "r"})
        ));
        assert!(!ok(CLONE_PROJECT, json!({"url": " "})));
        assert!(!ok(
            CLONE_PROJECT,
            json!({"url": "git@github.com:o/r.git", "parent": "code"})
        ));
        assert!(ok(
            UPDATE_PROJECT,
            json!({"path": "/abs/p", "name": "RustCode"})
        ));
        assert!(ok(
            UPDATE_PROJECT,
            json!({"path": "/abs/p", "model": "opus"})
        ));
        assert!(!ok(UPDATE_PROJECT, json!({"path": "/abs/p"})));
        assert!(ok(REMOVE_PROJECT, json!({"path": "/abs/p"})));

        let task = json!({
            "prompt": "Check the build",
            "schedule": {"kind": "cron", "expression": "0 9 * * 1-5", "timezone": "Europe/Oslo"},
            "project": "/abs/project",
        });
        assert!(ok(SCHEDULE_TASK, task.clone()));
        assert!(ok(
            SCHEDULE_TASK,
            json!({"prompt": "Once", "schedule": {"kind": "at", "at": "2026-03-04T09:30:00+01:00"}})
        ));
        assert!(!ok(
            SCHEDULE_TASK,
            json!({"schedule": task["schedule"].clone()})
        ));
        assert!(!ok(
            SCHEDULE_TASK,
            json!({"prompt": "Check", "schedule": {"kind": "daily", "hour": 9}})
        ));
        assert!(!ok(
            SCHEDULE_TASK,
            json!({"prompt": "Check", "schedule": {"kind": "cron", "expression": "0 9 * *"}})
        ));
        // The kind's own field is required here, not later in `create`: a cron with no
        // expression and an `at` with no time are invalid arguments, not a dead daemon.
        assert!(!ok(
            SCHEDULE_TASK,
            json!({"prompt": "Check", "schedule": {"kind": "cron", "timezone": "UTC"}})
        ));
        assert!(!ok(
            SCHEDULE_TASK,
            json!({"prompt": "Check", "schedule": {"kind": "at"}})
        ));
        assert!(!ok(
            SCHEDULE_TASK,
            json!({"prompt": "Check", "schedule": {"kind": "at", "at": "tomorrow"}})
        ));
        assert!(!ok(
            SCHEDULE_TASK,
            json!({"prompt": "Check", "schedule": {"kind": "at", "at": "2026-03-04T09:30:00+01:00"}, "project": "relative"})
        ));
        assert!(ok(LIST_SCHEDULED_TASKS, json!({})));
        assert!(ok(DELETE_SCHEDULED_TASK, json!({"id": "task-1"})));
        assert!(!ok(DELETE_SCHEDULED_TASK, json!({})));
        assert!(ok(RUN_SCHEDULED_TASK_NOW, json!({"id": "task-1"})));
        assert!(!ok(RUN_SCHEDULED_TASK_NOW, json!({"id": " "})));
    }

    #[test]
    fn answering_and_batching_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        let ask = json!({"session_id": "d1", "request_id": "7"});
        assert!(ok(
            ANSWER_REQUEST,
            json!({"session_id": "d1", "request_id": "7", "choice": "allow_once"})
        ));
        assert!(ok(
            ANSWER_REQUEST,
            json!({"session_id": "d1", "request_id": "7", "answer": "yes, run it"})
        ));
        assert!(!ok(ANSWER_REQUEST, ask));
        assert!(!ok(
            ANSWER_REQUEST,
            json!({"session_id": "d1", "request_id": "7", "choice": "allow_hour"})
        ));
        // One of the two, never both.
        assert!(!ok(
            ANSWER_REQUEST,
            json!({"session_id": "d1", "request_id": "7", "choice": "deny", "answer": "no"})
        ));
        assert!(!ok(
            ANSWER_REQUEST,
            json!({"request_id": "7", "choice": "deny"})
        ));
        assert!(!ok(
            ANSWER_REQUEST,
            json!({"session_id": "d1", "choice": "deny"})
        ));
        assert!(!ok(
            ANSWER_REQUEST,
            json!({"session_id": "d1", "request_id": "7", "answer": "  "})
        ));
        // The id is the string the list printed, not the number the engine sent.
        assert!(!ok(
            ANSWER_REQUEST,
            json!({"session_id": "d1", "request_id": 7, "choice": "deny"})
        ));

        assert!(ok(
            ORGANIZE_THREADS,
            json!({"session_ids": ["a"], "pinned": true})
        ));
        assert!(ok(
            ORGANIZE_THREADS,
            json!({"session_ids": ["a", "b"], "title": "Done", "archived": true})
        ));
        assert!(!ok(ORGANIZE_THREADS, json!({"session_ids": []})));
        assert!(!ok(ORGANIZE_THREADS, json!({"session_ids": ["a"]})));
        assert!(!ok(ORGANIZE_THREADS, json!({"pinned": true})));
        assert!(!ok(
            ORGANIZE_THREADS,
            json!({"session_ids": ["a"], "pinned": "yes"})
        ));
        assert!(!ok(
            ORGANIZE_THREADS,
            json!({"session_ids": [1], "pinned": true})
        ));

        // Attachments are absolute paths, at most ten of them.
        let message = |extra: Value| {
            let mut args = json!({"session_id": "d1", "message": "look"});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().cloned().unwrap());
            args
        };
        assert!(ok(MESSAGE_SESSION, message(json!({}))));
        assert!(ok(
            MESSAGE_SESSION,
            message(json!({"attachments": ["/repo/a.png", "/repo/b.log"]}))
        ));
        assert!(!ok(
            MESSAGE_SESSION,
            message(json!({"attachments": ["docs/notes.md"]}))
        ));
        assert!(!ok(
            MESSAGE_SESSION,
            message(json!({"attachments": "a.png"}))
        ));
        let many: Vec<String> = (0..=MAX_ATTACHMENTS)
            .map(|i| format!("/repo/{i}.txt"))
            .collect();
        assert!(!ok(MESSAGE_SESSION, message(json!({"attachments": many}))));
    }

    #[test]
    fn version_four_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        assert!(ok(THREAD_USAGE, json!({})));
        assert!(ok(
            THREAD_USAGE,
            json!({"since": "2026-11-01", "group_by": "day"})
        ));
        assert!(ok(
            THREAD_USAGE,
            json!({"since": "2026-11", "group_by": "model"})
        ));
        assert!(ok(THREAD_USAGE, json!({"project": "/repo/a"})));
        assert!(!ok(THREAD_USAGE, json!({"since": "yesterday"})));
        assert!(!ok(THREAD_USAGE, json!({"group_by": "tool"})));
        assert!(!ok(THREAD_USAGE, json!({"project": ""})));
        assert!(ok(QUERY_DATA, json!({"sql": "SELECT 1"})));
        assert!(ok(QUERY_DATA, json!({"sql": "SELECT 1", "limit": 5})));
        assert!(!ok(QUERY_DATA, json!({})));
        assert!(!ok(QUERY_DATA, json!({"sql": "  "})));
        assert!(!ok(QUERY_DATA, json!({"sql": "SELECT 1", "limit": "5"})));
        assert!(!ok(
            QUERY_DATA,
            json!({"sql": "x".repeat(MAX_SQL_BYTES + 1)})
        ));
        // The descriptions are how an agent learns the store without another call.
        let tool = |name: &str| {
            crate::tools::TOOLS
                .iter()
                .find(|tool| tool.name == name)
                .unwrap()
        };
        assert!(tool(THREAD_USAGE).description.contains("group_by"));
        assert!(tool(THREAD_USAGE).description.contains("never a price"));
        for table in [
            "threads(",
            "turns(",
            "tool_calls(",
            "compactions(",
            "daily_usage",
        ] {
            assert!(tool(QUERY_DATA).description.contains(table), "{table}");
        }
        // Columns an agent will write SQL against, by their real names: the
        // description is the only place it learns them.
        for column in [
            "input_tokens",
            "output_tokens",
            "context_tokens",
            "stop_reason",
            "tokens_before",
            "tokens_after",
        ] {
            assert!(tool(QUERY_DATA).description.contains(column), "{column}");
        }
        // No invented column: `turns.tokens` does not exist.
        assert!(
            !tool(QUERY_DATA)
                .description
                .contains("model, tokens, cost_usd")
        );
        assert!(tool(QUERY_DATA).description.contains("read-only"));
        assert!((tool(QUERY_DATA).schema)()["properties"]["limit"]["maximum"] == 1000);
    }

    /// One tool of the table, by name.
    fn tool(name: &str) -> &'static crate::tools::Tool {
        crate::tools::TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    }

    #[test]
    fn version_seven_offers_the_queue_control_to_its_clients_only() {
        let names = tool_names(7);
        assert!(names.contains(&MANAGE_QUEUE));
        assert!(!tool_names(6).contains(&MANAGE_QUEUE));
        assert!(is_tool(MANAGE_QUEUE) && is_v7_tool(MANAGE_QUEUE));
        // It says what it may touch, and that a change asks the user.
        let description = tool(MANAGE_QUEUE).description;
        for said in [
            "queue_id",
            "200 characters",
            "Only this thread's own queued messages",
            "started",
            "steer",
            "asked once",
        ] {
            assert!(description.contains(said), "{said}: {description}");
        }
        let schema = (tool(MANAGE_QUEUE).schema)();
        assert_eq!(schema["properties"]["action"]["enum"], json!(QUEUE_ACTIONS));
        assert_eq!(
            schema["properties"]["text"]["maxLength"],
            json!(MAX_MESSAGE_BYTES),
            "an edit is as long as the message it replaces"
        );
        assert_eq!(schema["required"], json!(["session_id", "action"]));
    }

    #[test]
    fn queue_control_arguments_are_checked() {
        let ok = |args: Value| validate(MANAGE_QUEUE, &args).is_ok();
        assert!(ok(json!({"session_id": "b", "action": "list"})));
        // A list names no message.
        assert!(ok(
            json!({"session_id": "b", "action": "list", "queue_id": "ignored"})
        ));
        // A cancel or a steer names one message and nothing else; an edit says what instead.
        for action in ["cancel", "steer"] {
            assert!(
                ok(json!({"session_id": "b", "action": action, "queue_id": "q1"})),
                "{action}"
            );
            assert!(
                !ok(json!({"session_id": "b", "action": action})),
                "{action}"
            );
            assert!(
                !ok(json!({"session_id": "b", "action": action, "queue_id": " "})),
                "{action}"
            );
        }
        assert!(ok(
            json!({"session_id": "b", "action": "edit", "queue_id": "q1", "text": "hi"})
        ));
        assert!(!ok(
            json!({"session_id": "b", "action": "edit", "queue_id": "q1"})
        ));
        assert!(!ok(
            json!({"session_id": "b", "action": "edit", "queue_id": "q1", "text": " "})
        ));
        let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
        assert!(!ok(
            json!({"session_id": "b", "action": "edit", "queue_id": "q1", "text": long})
        ));
        assert!(!ok(
            json!({"session_id": "b", "action": "reorder", "queue_id": "q1"})
        ));
        assert!(!ok(json!({"session_id": "b"})));
        // A list ignores a queue_id; one that acts on a message takes only a real id.
        let enormous = "x".repeat(MAX_QUEUE_ID_CHARS + 1);
        assert!(ok(
            json!({"session_id": "b", "action": "list", "queue_id": enormous.clone()})
        ));
        assert!(!ok(
            json!({"session_id": "b", "action": "cancel", "queue_id": enormous})
        ));
        assert!(!ok(json!({"action": "list"})));
        assert_eq!(timeout(MANAGE_QUEUE, &json!({})), REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn a_client_below_seven_never_gets_the_queue_control() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let args = json!({"session_id": "b", "action": "list"});
        let refused = execute_versioned(
            client(json!({"queued": []})),
            6,
            &cancel,
            MANAGE_QUEUE,
            &args,
        )
        .await;
        assert!(!refused.success);
        assert!(refused.content.contains("newer RustCode app"));
        let answered = execute_versioned(
            client(json!({"queued": []})),
            7,
            &cancel,
            MANAGE_QUEUE,
            &args,
        )
        .await;
        assert!(answered.success, "{}", answered.content);
    }

    #[test]
    fn the_version_three_hint_and_read_tools_say_what_they_answer() {
        // Reads answer questions about the user's work, across projects when asked.
        assert!(PROMPT_HINT_V3.contains("all_projects"));
        assert!(PROMPT_HINT_V3.contains("before answering about it"));
        assert!(PROMPT_HINT_V3.contains("do them only when the user asks"));
        let tool = |name: &str| {
            crate::tools::TOOLS
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name}"))
        };
        for name in [
            LIST_SESSIONS,
            SEARCH_SESSIONS,
            READ_SESSION,
            LIST_WORKTREES,
            LIST_PROJECTS,
            LIST_PENDING_REQUESTS,
            LIST_HARNESSES,
            LIST_PULL_REQUESTS,
        ] {
            assert!(
                !tool(name).description.contains("involve another thread"),
                "{name} reads: no delegation-only rule"
            );
        }
        assert!(
            tool(LIST_SESSIONS)
                .description
                .contains("what the user has been working on")
        );
        assert!(
            (tool(LIST_SESSIONS).schema)()["properties"]["all_projects"].is_object(),
            "list_sessions takes all_projects"
        );
        assert!((tool(SEARCH_SESSIONS).schema)()["properties"]["all_projects"].is_object());
        assert!((tool(LIST_WORKTREES).schema)()["properties"]["all_projects"].is_object());
        assert!(tool(READ_SESSION).description.contains("next_offset"));
        assert!((tool(READ_SESSION).schema)()["properties"]["offset"].is_object());
        assert!(tool(WAIT_SESSION).description.contains("answer_truncated"));
        // The longest prompt and message are 32 KiB in the schema and in the check.
        assert_eq!(
            (tool(MESSAGE_SESSION).schema)()["properties"]["message"]["maxLength"],
            32768
        );
        assert_eq!(
            (tool(START_SESSION).schema)()["properties"]["prompt"]["maxLength"],
            32768
        );
        // Tools that give another thread work keep the approval rule.
        assert!(
            tool(MESSAGE_SESSION)
                .description
                .contains("The user is asked once")
        );
        assert!(
            tool(START_SESSION)
                .description
                .contains("The user approves")
        );
    }

    #[test]
    fn version_three_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        assert!(ok(LIST_HARNESSES, json!({})));
        // Narrowing a long model list is optional, as an id and a substring.
        assert!(ok(LIST_HARNESSES, json!({"harness": "codex"})));
        assert!(ok(
            LIST_HARNESSES,
            json!({"harness": "codex", "model": "opus"})
        ));
        assert!(!ok(LIST_HARNESSES, json!({"harness": ""})));
        assert!(!ok(LIST_HARNESSES, json!({"model": 7})));
        assert!(!ok(LIST_HARNESSES, json!({"harness": ["codex"]})));
        assert!(ok(SEARCH_SESSIONS, json!({"query": "login"})));
        assert!(!ok(SEARCH_SESSIONS, json!({"query": " "})));
        // Reaching every project is a flag, never a string, on the reads.
        assert!(ok(
            SEARCH_SESSIONS,
            json!({"query": "login", "all_projects": true})
        ));
        assert!(!ok(
            SEARCH_SESSIONS,
            json!({"query": "login", "all_projects": "yes"})
        ));
        assert!(ok(LIST_SESSIONS, json!({"all_projects": true})));
        assert!(!ok(LIST_SESSIONS, json!({"all_projects": 1})));
        assert!(ok(LIST_WORKTREES, json!({"all_projects": false})));
        assert!(!ok(LIST_WORKTREES, json!({"all_projects": "false"})));
        assert!(ok(INTERRUPT_SESSION, json!({"session_id": "b"})));
        assert!(!ok(INTERRUPT_SESSION, json!({})));
        assert!(ok(CONFIGURE_SESSION, json!({})));
        assert!(ok(
            CONFIGURE_SESSION,
            json!({"model": "m", "effort": "high"})
        ));
        assert!(ok(LIST_PENDING_REQUESTS, json!({})));
        assert!(ok(
            TRANSFER_SESSION,
            json!({"session_id": "a", "harness": "codex"})
        ));
        assert!(!ok(TRANSFER_SESSION, json!({"session_id": "a"})));
        assert!(!ok(TRANSFER_SESSION, json!({"harness": "codex"})));
        assert!(ok(LIST_PROJECTS, json!({})));
        assert!(ok(LIST_PULL_REQUESTS, json!({})));
        assert!(ok(LIST_PULL_REQUESTS, json!({"state": "merged"})));
        assert!(!ok(LIST_PULL_REQUESTS, json!({"state": "draft"})));
        assert!(!ok(LIST_PULL_REQUESTS, json!({"state": 1})));
        assert!(ok(UPDATE_SESSION, json!({"pinned": true})));
        assert!(ok(
            UPDATE_SESSION,
            json!({"session_id": "b", "status": "settled"})
        ));
        assert!(!ok(UPDATE_SESSION, json!({"session_id": "b"})));
        assert!(!ok(UPDATE_SESSION, json!({"status": "gone"})));
        assert!(!ok(UPDATE_SESSION, json!({"pinned": "yes"})));
        assert!(ok(
            UPDATE_SESSION,
            json!({"snoozed_until": "2026-11-05T09:00:00Z"})
        ));
        assert!(ok(UPDATE_SESSION, json!({"snoozed_until": null})));
        assert!(!ok(UPDATE_SESSION, json!({"snoozed_until": 5})));
    }

    #[test]
    fn version_two_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        assert!(ok(READ_SESSION, json!({"session_id": "b"})));
        assert!(ok(READ_SESSION, json!({"session_id": "b", "offset": 0})));
        assert!(ok(
            READ_SESSION,
            json!({"session_id": "b", "last_turns": 10, "offset": 16384})
        ));
        assert!(!ok(READ_SESSION, json!({"session_id": "b", "offset": -1})));
        assert!(!ok(READ_SESSION, json!({"session_id": "b", "offset": "0"})));
        assert!(ok(
            READ_SESSION,
            json!({"session_id": "b", "last_turns": 10})
        ));
        assert!(!ok(
            READ_SESSION,
            json!({"session_id": "b", "last_turns": 0})
        ));
        assert!(!ok(
            READ_SESSION,
            json!({"session_id": "b", "last_turns": 11})
        ));
        assert!(!ok(READ_SESSION, json!({})));

        assert!(ok(START_SESSION, json!({"prompt": "fix it"})));
        assert!(ok(
            START_SESSION,
            json!({"prompt": "fix it", "title": "Fix", "harness": "codex", "model": "m",
                   "effort": "high", "worktree": false, "project": "/abs/repo"})
        ));
        assert!(!ok(START_SESSION, json!({"prompt": " "})));
        let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
        assert!(!ok(START_SESSION, json!({"prompt": long})));
        assert!(!ok(
            START_SESSION,
            json!({"prompt": "a", "project": "relative/path"})
        ));
        assert!(!ok(
            START_SESSION,
            json!({"prompt": "a", "worktree": "yes"})
        ));
        assert!(!ok(START_SESSION, json!({"prompt": "a", "harness": ""})));
        // A named login is optional, and when given a non-empty string.
        assert!(ok(
            START_SESSION,
            json!({"prompt": "a", "harness": "claude-code", "account": "work"})
        ));
        assert!(!ok(START_SESSION, json!({"prompt": "a", "account": ""})));
        assert!(!ok(START_SESSION, json!({"prompt": "a", "account": 5})));
        let title = "t".repeat(MAX_TITLE_CHARS + 1);
        assert!(!ok(START_SESSION, json!({"prompt": "a", "title": title})));

        let two = json!([{"harness": "codex"}, {"harness": "claude-code", "model": "m"}]);
        assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": two})));
        assert!(!ok(
            FAN_OUT,
            json!({"prompt": "a", "harnesses": [{"harness": "codex"}]})
        ));
        let five = json!([{"harness": "a"}, {"harness": "b"}, {"harness": "c"},
                          {"harness": "d"}, {"harness": "e"}]);
        assert!(!ok(FAN_OUT, json!({"prompt": "a", "harnesses": five})));
        let twice = json!([{"harness": "codex"}, {"harness": "codex"}]);
        assert!(!ok(FAN_OUT, json!({"prompt": "a", "harnesses": twice})));
        // The same harness on two models is a fair comparison.
        let models =
            json!([{"harness": "codex", "model": "a"}, {"harness": "codex", "model": "b"}]);
        assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": models})));
        // So are two named accounts of one harness and model.
        let accounts = json!([
            {"harness": "claude-code", "model": "opus", "account": "work"},
            {"harness": "claude-code", "model": "opus", "account": "home"}
        ]);
        assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": accounts})));
        assert!(!ok(
            FAN_OUT,
            json!({"prompt": "a", "harnesses": [{"harness": "a", "account": ""},
                                                  {"harness": "b"}]})
        ));
        assert!(!ok(
            FAN_OUT,
            json!({"prompt": "a", "harnesses": [{"model": "x"}, {"harness": "b"}]})
        ));
        assert!(!ok(FAN_OUT, json!({"harnesses": two})));
        // A fan-out names the project on that machine as a start does.
        assert!(ok(
            FAN_OUT,
            json!({"prompt": "a", "harnesses": two, "project": "/home/lars/code/rustcode"})
        ));
        assert!(!ok(
            FAN_OUT,
            json!({"prompt": "a", "harnesses": two, "project": "rustcode"})
        ));

        assert_eq!(timeout(START_SESSION, &json!({})), START_TIMEOUT);
        assert_eq!(timeout(FAN_OUT, &json!({})), START_TIMEOUT);
        assert_eq!(timeout(READ_SESSION, &json!({})), REQUEST_TIMEOUT);
    }

    #[test]
    fn a_named_login_is_optional_and_advertised_where_it_is_used() {
        let tool = |name: &str| {
            crate::tools::TOOLS
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name}"))
        };
        // The schema is what tells an agent it may name an account, from v5 on.
        let start = (tool(START_SESSION).schema)();
        assert!(start["properties"]["account"].is_object());
        let fan = (tool(FAN_OUT).schema)();
        assert!(fan["properties"]["harnesses"]["items"]["properties"]["account"].is_object());
        // A v4 client is not told about a field its app cannot answer.
        let start_v4 = schema_for(START_SESSION, 4, start);
        assert!(start_v4["properties"]["account"].is_null());
        let fan_v4 = schema_for(FAN_OUT, 4, fan);
        assert!(fan_v4["properties"]["harnesses"]["items"]["properties"]["account"].is_null());
        // Other fields stay as they were.
        assert!(start_v4["properties"]["prompt"].is_object());
        assert!(fan_v4["properties"]["harnesses"]["items"]["properties"]["model"].is_object());
        // The MCP server builds its tools from the same versioned schema.
        let mcp_v4 = crate::orchestration::mcp_server::tools(4, false);
        let mcp_start = mcp_v4
            .iter()
            .find(|tool| tool["name"] == START_SESSION)
            .expect("start_session for a v4 client");
        assert!(mcp_start["inputSchema"]["properties"]["account"].is_null());
        let mcp_v5 = crate::orchestration::mcp_server::tools(5, false);
        let mcp_start = mcp_v5
            .iter()
            .find(|tool| tool["name"] == START_SESSION)
            .expect("start_session for a v5 client");
        assert!(mcp_start["inputSchema"]["properties"]["account"].is_object());
        assert!(tool(START_SESSION).description.contains("named account"));
        assert!(tool(FAN_OUT).description.contains("named account"));
        // Discovery: list_harnesses names the accounts start_session takes.
        assert!(tool(LIST_HARNESSES).description.contains("named accounts"));
    }

    /// A machine is a version 11 argument: a v10 app never sees the field, and
    /// a call that names one is refused rather than acted on here.
    #[test]
    fn a_machine_is_optional_and_advertised_where_it_is_used() {
        let tool = |name: &str| {
            crate::tools::TOOLS
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name}"))
        };
        for name in MACHINE_TOOL_NAMES {
            let schema = (tool(name).schema)();
            assert_eq!(
                schema["properties"]["machine"]["type"], "string",
                "{name} takes a machine"
            );
            // A v11 client is not told about a field its app cannot answer.
            let old = schema_for(name, 11, schema.clone());
            assert!(old["properties"]["machine"].is_null(), "{name} at v11");
            // At v12 it is there, and the fields it had stay.
            let new = schema_for(name, 12, schema);
            assert!(new["properties"]["machine"].is_object(), "{name} at v12");
            assert!(PROMPT_HINT_V12.contains(name), "{name} in the hint");
        }
        // The tools that do not act on a machine are unchanged.
        for name in [MERGE_WHEN_GREEN, CLEANUP_WORKTREE, SEARCH_SESSIONS] {
            let schema = (tool(name).schema)();
            assert!(
                schema["properties"]["machine"].is_null(),
                "{name} takes no machine"
            );
        }
        // list_machines is what names them, and carries no machine of its own.
        let listed = (tool(LIST_MACHINES).schema)();
        assert!(listed["properties"]["machine"].is_null());
        // The MCP server builds its tools from the same versioned schema.
        let mcp_v11 = crate::orchestration::mcp_server::tools(11, false);
        let start = mcp_v11
            .iter()
            .find(|tool| tool["name"] == START_SESSION)
            .expect("start_session for a v11 client");
        assert!(start["inputSchema"]["properties"]["machine"].is_null());
        assert!(
            !mcp_v11.iter().any(|tool| tool["name"] == LIST_MACHINES),
            "a v11 client never sees list_machines"
        );
        let mcp_v12 = crate::orchestration::mcp_server::tools(12, false);
        let start = mcp_v12
            .iter()
            .find(|tool| tool["name"] == START_SESSION)
            .expect("start_session for a v12 client");
        assert!(start["inputSchema"]["properties"]["machine"].is_object());
        let machines = mcp_v12
            .iter()
            .find(|tool| tool["name"] == LIST_MACHINES)
            .expect("list_machines for a v12 client");
        assert!(
            machines["description"]
                .as_str()
                .unwrap_or("")
                .contains("machine")
        );
        assert!(validate(START_SESSION, &json!({"prompt": "go", "machine": "Studio"})).is_ok());
        assert!(validate(START_SESSION, &json!({"prompt": "go", "machine": "  "})).is_err());
    }

    /// A machine a v11 app would ignore is refused the same way an account a
    /// v4 app would ignore is, whichever side the call is made from.
    #[tokio::test]
    async fn an_older_client_cannot_act_on_another_machine() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let start = json!({"prompt": "go", "machine": "Studio"});
        let refused = execute_versioned(
            client(json!({"session_id": "c"})),
            11,
            &cancel,
            START_SESSION,
            &start,
        )
        .await;
        assert!(!refused.success);
        assert!(refused.content.contains("newer RustCode app"));
        assert!(refused.content.contains("Nothing was sent"));
        let sent = execute_versioned(
            client(json!({"session_id": "c"})),
            12,
            &cancel,
            START_SESSION,
            &start,
        )
        .await;
        assert!(sent.success, "{}", sent.content);
        // The same refusal on a list, not only on a start.
        let refused = execute_versioned(
            client(json!({"sessions": []})),
            11,
            &cancel,
            LIST_SESSIONS,
            &json!({"machine": "Studio"}),
        )
        .await;
        assert!(!refused.success);
        assert!(refused.content.contains("newer RustCode app"));
        // A null is no machine, so it is not a version 12 argument.
        let null = json!({"prompt": "go", "machine": null});
        let sent = execute_versioned(
            client(json!({"session_id": "c"})),
            11,
            &cancel,
            START_SESSION,
            &null,
        )
        .await;
        assert!(sent.success, "{}", sent.content);
    }

    /// An orchestrator that wants room left on an account reads
    /// `list_harnesses` (answered by the app, from its plan cache): the tool
    /// has to say the field is there and what its numbers mean, or a model
    /// never looks at it and delegates onto a window that is nearly used up.
    #[test]
    fn list_harnesses_advertises_each_logins_usage_windows() {
        let tool = crate::tools::TOOLS
            .iter()
            .find(|tool| tool.name == LIST_HARNESSES)
            .expect("list_harnesses");
        for field in [
            "usage",
            "used_percent",
            "window_minutes",
            "resets_at",
            "as_of",
        ] {
            assert!(tool.description.contains(field), "{field}");
        }
        // An external harness gets the same wording through the MCP server.
        let listed = crate::orchestration::mcp_server::tools(VERSION, false)
            .into_iter()
            .find(|tool| tool["name"] == LIST_HARNESSES)
            .expect("list_harnesses for an MCP harness");
        let description = listed["description"].as_str().unwrap_or_default();
        assert!(description.contains("used_percent"), "{description}");
    }

    /// A harness may list hundreds of models (OpenCode, Pi), so the tool says
    /// how to narrow the list; the schema carries both optional filters.
    #[test]
    fn list_harnesses_advertises_its_harness_and_model_filters() {
        let tool = crate::tools::TOOLS
            .iter()
            .find(|tool| tool.name == LIST_HARNESSES)
            .expect("list_harnesses");
        assert!(
            tool.description.contains("`harness`"),
            "{}",
            tool.description
        );
        assert!(tool.description.contains("`model`"), "{}", tool.description);
        assert!(
            tool.description.contains("omitted_models"),
            "{}",
            tool.description
        );
        let schema = (tool.schema)();
        assert_eq!(schema["properties"]["harness"]["type"], "string");
        assert_eq!(schema["properties"]["model"]["type"], "string");
        // Nothing is required: a call with no filter is the plain list.
        assert!(schema["required"].as_array().is_some_and(Vec::is_empty));
    }

    #[tokio::test]
    async fn an_older_client_cannot_smuggle_an_account_past_its_version() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let start = json!({"prompt": "go", "account": "work"});
        // Version 4 would otherwise ignore the argument and pick another login.
        let refused = execute_versioned(
            client(json!({"session_id": "c"})),
            4,
            &cancel,
            START_SESSION,
            &start,
        )
        .await;
        assert!(!refused.success);
        assert!(refused.content.contains("newer RustCode app"));
        assert!(refused.content.contains("Nothing was sent"));
        let fan = json!({"prompt": "go", "harnesses": [
            {"harness": "a"},
            {"harness": "b", "account": "work"}
        ]});
        let refused =
            execute_versioned(client(json!({"links": []})), 4, &cancel, FAN_OUT, &fan).await;
        assert!(!refused.success);
        assert!(refused.content.contains("newer RustCode app"));
        // The same calls go through for a v5 client.
        for (name, args) in [(START_SESSION, start), (FAN_OUT, fan)] {
            let sent =
                execute_versioned(client(json!({"session_id": "c"})), 5, &cancel, name, &args)
                    .await;
            assert!(sent.success, "{name}: {}", sent.content);
        }
        // A null is no login, so it is not a version 5 argument.
        let null = json!({"prompt": "go", "account": null});
        let sent = execute_versioned(
            client(json!({"session_id": "c"})),
            4,
            &cancel,
            START_SESSION,
            &null,
        )
        .await;
        assert!(sent.success, "{}", sent.content);
    }

    #[tokio::test]
    async fn a_version_one_client_never_gets_a_version_two_call() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let args = json!({"prompt": "go"});
        let refused = execute_versioned(
            client(json!({"session_id": "c"})),
            1,
            &cancel,
            START_SESSION,
            &args,
        )
        .await;
        assert!(!refused.success);
        assert!(refused.content.contains("Nothing was sent"));
        let started = execute_versioned(
            client(json!({"session_id": "c"})),
            2,
            &cancel,
            START_SESSION,
            &args,
        )
        .await;
        assert!(started.success, "{}", started.content);
    }

    #[test]
    fn the_version_two_hint_stays_short() {
        let tokens = crate::network::compaction::estimate_tokens(PROMPT_HINT_V2);
        assert!(tokens < 115, "{tokens}");
        for name in tool_names(2) {
            assert!(PROMPT_HINT_V2.contains(name), "{name}");
        }
    }

    #[test]
    fn arguments_are_checked_before_anything_is_sent() {
        assert!(
            validate(
                MESSAGE_SESSION,
                &json!({"session_id": "b", "message": "hi"})
            )
            .is_ok()
        );
        assert!(validate(MESSAGE_SESSION, &json!({"session_id": "b"})).is_err());
        let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
        assert!(
            validate(
                MESSAGE_SESSION,
                &json!({"session_id": "b", "message": long})
            )
            .is_err()
        );
        // 32 KiB fits; one byte more does not.
        assert!(
            validate(
                MESSAGE_SESSION,
                &json!({"session_id": "b", "message": "x".repeat(MAX_MESSAGE_BYTES)})
            )
            .is_ok()
        );
        assert!(validate(WAIT_SESSION, &json!({})).is_err());
        assert!(validate(WAIT_SESSION, &json!({"link_ids": ["a/1"]})).is_ok());
        assert_eq!(
            timeout(WAIT_SESSION, &json!({"timeout_ms": 9_999_999})),
            Duration::from_millis(MAX_WAIT_MS) + Duration::from_secs(30)
        );
        assert_eq!(MAX_WAIT_MS, 25 * 60 * 1000);
        assert_eq!(
            requested_wait(WAIT_SESSION, &json!({"timeout_ms": 1_950_000})),
            Some(Duration::from_millis(MAX_WAIT_MS))
        );
        assert_eq!(
            requested_wait(WAIT_SESSION, &json!({"timeout_ms": 5_000})),
            Some(Duration::from_secs(5))
        );
        assert_eq!(requested_wait(LIST_SESSIONS, &json!({})), None);
    }

    #[test]
    fn a_retry_key_is_checked_and_hashes_the_same_arguments_the_same() {
        let keyed = |args: Value| validate(MESSAGE_SESSION, &args).is_ok();
        assert!(keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": "call-1"})
        ));
        assert!(keyed(json!({"session_id": "b", "message": "hi"})));
        assert!(keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": null})
        ));
        // 1 to 128 characters of A-Z, a-z, 0-9, '.', '_', ':' and '-'.
        assert!(keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": "a.b_c:d-9"})
        ));
        assert!(keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": "x".repeat(128)})
        ));
        assert!(!keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": "x".repeat(129)})
        ));
        assert!(!keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": ""})
        ));
        assert!(!keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": "has space"})
        ));
        assert!(!keyed(
            json!({"session_id": "b", "message": "hi", "client_request_id": 7})
        ));
        // The same tools take it, and start_session and fan_out too.
        assert!(
            validate(
                START_SESSION,
                &json!({"prompt": "go", "client_request_id": "k"})
            )
            .is_ok()
        );
        assert!(
            validate(
                FAN_OUT,
                &json!({"prompt": "go", "harnesses": [{"harness": "a"}, {"harness": "b"}],
                        "client_request_id": "k"})
            )
            .is_ok()
        );
        assert!(
            validate(
                FAN_OUT,
                &json!({"prompt": "go", "harnesses": [{"harness": "a"}, {"harness": "b"}],
                        "client_request_id": "not allowed"})
            )
            .is_err()
        );

        // Only the keyed tools forward a key.
        let key = json!({"session_id": "b", "message": "hi", "client_request_id": "k"});
        assert_eq!(request_key(MESSAGE_SESSION, &key), Some("k"));
        assert_eq!(request_key(LIST_SESSIONS, &key), None);
        assert_eq!(
            request_key(MESSAGE_SESSION, &json!({"message": "hi"})),
            None
        );

        // The hash ignores how the arguments were ordered and says nothing about
        // the text: the same prompt and session hash alike, and a changed
        // prompt, target or action does not.
        let one = json!({"session_id": "b", "message": "hi", "client_request_id": "k"});
        let other = json!({"client_request_id": "k", "message": "hi", "session_id": "b"});
        assert_eq!(
            args_hash(MESSAGE_SESSION, &one),
            args_hash(MESSAGE_SESSION, &other)
        );
        assert_ne!(
            args_hash(MESSAGE_SESSION, &one),
            args_hash(
                MESSAGE_SESSION,
                &json!({"session_id": "b", "message": "hi there", "client_request_id": "k"})
            )
        );
        assert_ne!(
            args_hash(MESSAGE_SESSION, &one),
            args_hash(
                START_SESSION,
                &json!({"session_id": "b", "message": "hi", "client_request_id": "k"})
            )
        );
        let hash = args_hash(MESSAGE_SESSION, &one);
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!hash.contains("hi"));
    }

    #[tokio::test]
    async fn a_retry_key_and_its_args_hash_are_forwarded_to_the_client() {
        struct Recording(Arc<std::sync::Mutex<Vec<Value>>>);
        impl PreviewClient for Recording {
            fn request(&self, params: Value) -> crate::preview::PreviewFuture {
                self.0.lock().unwrap().push(params);
                Box::pin(async { Ok(json!({"link_id": "a/1"})) })
            }
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let args = json!({"session_id": "b", "message": "hi", "client_request_id": "call-1"});
        let output = execute_with(
            Some(Arc::new(Recording(Arc::clone(&seen))) as Arc<dyn PreviewClient>),
            &cancel,
            MESSAGE_SESSION,
            &args,
        )
        .await;
        assert!(output.success, "{}", output.content);
        let sent = seen.lock().unwrap()[0].clone();
        assert_eq!(sent["clientRequestId"], "call-1");
        assert_eq!(sent["argsHash"], json!(args_hash(MESSAGE_SESSION, &args)));

        // Without a key nothing is added, so the client sees the call it always saw.
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        execute_with(
            Some(Arc::new(Recording(Arc::clone(&seen))) as Arc<dyn PreviewClient>),
            &cancel,
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "hi"}),
        )
        .await;
        let sent = seen.lock().unwrap()[0].clone();
        assert!(sent.get("clientRequestId").is_none());
        assert!(sent.get("argsHash").is_none());
    }

    #[tokio::test]
    async fn calls_go_to_the_host_and_its_errors_come_back_as_failures() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let none = execute_with(None, &cancel, LIST_SESSIONS, &json!({})).await;
        assert!(!none.success);
        assert!(none.content.contains("macOS app"));

        let listed = execute_with(
            client(json!({"sessions": [{"session_id": "b"}]})),
            &cancel,
            LIST_SESSIONS,
            &json!({}),
        )
        .await;
        assert!(listed.success);
        assert!(listed.content.contains("\"b\""));

        let denied = execute_with(
            client(json!({"error": {"code": "denied", "message": "the user said no"}})),
            &cancel,
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "hi"}),
        )
        .await;
        assert!(!denied.success);
        assert_eq!(denied.error_kind, Some(ToolErrorKind::PermissionDenied));

        let invalid = execute_with(client(json!({})), &cancel, MESSAGE_SESSION, &json!({})).await;
        assert_eq!(invalid.error_kind, Some(ToolErrorKind::InvalidArguments));

        // A busy refusal keeps the wording and the slots it leaves.
        let busy = execute_with(
            client(json!({"error": {"code": "busy", "message": "at most 8 threads at once; wait_session on one first", "links_left": 0}})),
            &cancel,
            START_SESSION,
            &json!({"prompt": "go"}),
        )
        .await;
        assert!(!busy.success);
        assert!(busy.content.contains("wait_session on one first"));
        assert!(busy.content.contains("links_left: 0"));
    }

    #[tokio::test]
    async fn a_cancelled_call_says_a_message_may_have_gone() {
        struct Never;
        impl PreviewClient for Never {
            fn request(&self, _params: Value) -> crate::preview::PreviewFuture {
                Box::pin(std::future::pending())
            }
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let output = execute_with(
            Some(Arc::new(Never) as Arc<dyn PreviewClient>),
            &cancel,
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "hi"}),
        )
        .await;
        assert_eq!(output.error_kind, Some(ToolErrorKind::Cancelled));
        assert!(output.content.contains("may have been delivered"));
    }

    /// A client whose call stays pending until it is cancelled: the harness
    /// dropped it, and the app is still preparing an answer nobody will read.
    struct Held(Arc<std::sync::Mutex<Vec<String>>>);

    impl PreviewClient for Held {
        fn request(&self, params: Value) -> crate::preview::PreviewFuture {
            let call = params["callId"].as_str().unwrap_or_default().to_owned();
            self.0.lock().unwrap().push(format!("asked:{call}"));
            Box::pin(std::future::pending())
        }

        fn call_cancelled(&self, call_id: &str) {
            self.0.lock().unwrap().push(format!("cancelled:{call_id}"));
        }
    }

    #[tokio::test]
    async fn a_dropped_call_is_reported_to_the_client_by_its_own_id() {
        let said = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cancel = tokio_util::sync::CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            token.cancel();
        });
        let output = execute_with(
            Some(Arc::new(Held(Arc::clone(&said))) as Arc<dyn PreviewClient>),
            &cancel,
            WAIT_SESSION,
            &json!({"link_ids": ["a/1"], "timeout_ms": 600_000}),
        )
        .await;
        assert_eq!(output.error_kind, Some(ToolErrorKind::Cancelled));
        let said = said.lock().unwrap().clone();
        assert_eq!(said.len(), 2, "{said:?}");
        let asked = said[0]
            .strip_prefix("asked:")
            .expect("the call id")
            .to_owned();
        assert!(!asked.is_empty(), "{said:?}");
        // The very call the request carried is the one the client is told to end.
        assert_eq!(said[1], format!("cancelled:{asked}"));
    }

    #[test]
    fn a_dropped_call_is_named_by_its_session_and_id() {
        assert!(CANCELLED_METHOD.starts_with("_rustcode/orchestration"));
        assert_eq!(
            cancelled_params("s-1", "call-7"),
            json!({"sessionId": "s-1", "callId": "call-7"})
        );
        // Every call of this engine gets an id of its own.
        assert_ne!(next_call_id(), next_call_id());
    }

    #[test]
    fn answers_are_compact_json() {
        let out = output(
            LIST_SESSIONS,
            json!({"sessions": [{"id": "a", "title": "x"}]}),
        );
        assert!(out.success);
        assert_eq!(out.content, r#"{"sessions":[{"id":"a","title":"x"}]}"#);
    }

    #[test]
    fn a_wait_defaults_to_a_long_one() {
        assert_eq!(
            requested_wait(WAIT_SESSION, &json!({"link_id": "a/1"})),
            Some(Duration::from_millis(20 * 60 * 1000))
        );
    }
}
