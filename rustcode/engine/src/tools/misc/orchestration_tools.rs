use super::{Tool, ToolCapability, ToolSafety, Value};

fn orchestration_unavailable(_args: &Value) -> Result<String, String> {
    Err(crate::orchestration::UNAVAILABLE_MESSAGE.to_owned())
}

/// Threads that message other threads (#2637). Offered only to root
/// sessions whose client hosts threads; the client asks the user before a
/// message goes, and runs it as an attributed turn of the target thread.
pub const LIST_SESSIONS: Tool = Tool {
    name: crate::orchestration::LIST_SESSIONS,
    description: "List the user's threads, newest first: id, title, project, last update, the machine it runs on and whether one is working. Pass `machine` for one paired computer's threads. Reads no transcripts. This project's threads by default, every project's with all_projects. It answers what the user has been working on and which threads exist, so ask it instead of looking for RustCode's data elsewhere.",
    arguments: r#"{"query": "optional words in the title", "all_projects": false}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "all_projects": {"type": "boolean", "description": "Every project's threads, with each thread's project path"},
                "machine": {"type": "string"}
            },
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const MESSAGE_SESSION: Tool = Tool {
    name: crate::orchestration::MESSAGE_SESSION,
    description: "Message a thread in any project. The user is asked once. Idle starts a turn; busy queues. steer: true delivers mid-turn if supported, else queues; delivered says steered, queued or started. By default sends one-way. return_to: me returns a link_id for wait_session and wakes you on reply. Send the whole task and brief it fully, as start_session's description says.",
    arguments: r#"{"session_id": "id from list_sessions", "message": "what to ask or do", "attachments": ["/abs/path/to/file"], "client_request_id": "retry key"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "message": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_MESSAGE_BYTES},
                "steer": {"type": "boolean"},
                "return_to": {"type": "string", "enum": ["me", "none"], "default": "none", "description": "me waits for a reply; none sends one-way"},
                "machine": {"type": "string"},
                "attachments": {
                    "type": "array",
                    "maxItems": crate::orchestration::MAX_ATTACHMENTS,
                    "items": {"type": "string", "minLength": 1},
                    "description": "Absolute paths of files to attach, each under this thread's project or worktree and at most 5 MB"
                },
                "client_request_id": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_CLIENT_REQUEST_ID_CHARS, "description": "Retry key: a repeat returns the first result"}
            },
            "required": ["session_id", "message"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const WAIT_SESSION: Tool = Tool {
    name: crate::orchestration::WAIT_SESSION,
    description: "Wait up to 25 minutes on delegate link IDs. Returns the first result, or with all: true every result or a request for the user. Use read_session if answer_truncated is true. RustCode wakes the parent when a delegate finishes or needs approval. Wait only when you need the result now. Timeouts report child activity.",
    arguments: r#"{"link_ids": ["from start_session or message_session"], "all": true, "timeout_ms": 1200000}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "link_id": {"type": "string"},
                "link_ids": {"type": "array", "minItems": 1, "items": {"type": "string"}},
                "all": {"type": "boolean", "default": false},
                "machine": {"type": "string"},
                "timeout_ms": {"type": "integer", "minimum": 1, "maximum": crate::orchestration::MAX_WAIT_MS, "default": crate::orchestration::DEFAULT_WAIT_MS}
            },
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 2 (`docs/orchestrator.md`): offered only to
/// clients that advertise `_meta.rustcode.orchestration >= 2`.
pub const READ_SESSION: Tool = Tool {
    name: crate::orchestration::READ_SESSION,
    description: "Read the last turns of one thread by id as text: title, status, project, and each turn's request, answer and changed files. Any thread of any project. Given a thread id, read it before answering about it. Long answers are paged: the answer carries text, offset and next_offset; pass next_offset as offset to continue. Pages join with nothing lost or repeated, and next_offset is absent on the last page.",
    arguments: r#"{"session_id": "id from list_sessions", "last_turns": 3, "offset": 0}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "last_turns": {"type": "integer", "minimum": 1, "maximum": crate::orchestration::MAX_LAST_TURNS, "default": crate::orchestration::DEFAULT_LAST_TURNS},
                "machine": {"type": "string"},
                "offset": {"type": "integer", "minimum": 0, "description": "Bytes into the rendered answer to start from; pass the previous page's next_offset"}
            },
            "required": ["session_id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const START_SESSION: Tool = Tool {
    name: crate::orchestration::START_SESSION,
    description: "Start a thread. For a request to start a new thread or an agent in a new thread, set standalone to true when supported. This creates a top-level thread and returns session_id. Omit standalone only for explicit delegation. A delegate gets its own worktree by default and returns link_id for wait_session. Give the full task, repo rules, acceptance criteria and timeouts. Explicit agent fields, including a named account, override the preset from list_harnesses. The user approves.",
    arguments: r#"{"prompt": "the task", "title": "short title", "harness": "codex", "client_request_id": "retry key"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_MESSAGE_BYTES},
                "title": {"type": "string", "maxLength": crate::orchestration::MAX_TITLE_CHARS},
                "harness": {"type": "string"},
                "model": {"type": "string"},
                "effort": {"type": "string"},
                "account": {"type": "string", "description": "Named login from list_harnesses; omitted is the caller's on the same harness, else the default"},
                "worktree": {"type": "boolean"},
                "preset": {"type": "string"},
                "access": {"type": "string", "enum": ["ask", "edits", "full"]},
                "standalone": {"type": "boolean", "description": "Set true for a new top-level thread. Set false or omit only for an explicitly requested delegate"},
                "project": {"type": "string", "description": "absolute path of another project"},
                "machine": {"type": "string"},
                "client_request_id": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_CLIENT_REQUEST_ID_CHARS, "description": "Retry key"}
            },
            "required": ["prompt"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const FAN_OUT: Tool = Tool {
    name: crate::orchestration::FAN_OUT,
    description: "Start the same full task on 2–4 harnesses in separate worktrees. Returns link IDs for wait_session, or session IDs for standalone threads. Specify agent settings, named accounts or presets from list_harnesses. User approval applies.",
    arguments: r#"{"prompt": "the task", "harnesses": [{"harness": "codex"}, {"harness": "claude-code"}], "client_request_id": "retry key"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_MESSAGE_BYTES},
                "title": {"type": "string", "maxLength": crate::orchestration::MAX_TITLE_CHARS},
                "harnesses": {
                    "type": "array",
                    "minItems": crate::orchestration::MIN_FAN_OUT,
                    "maxItems": crate::orchestration::MAX_FAN_OUT,
                    "items": {
                        "type": "object",
                        "properties": {
                            "harness": {"type": "string", "minLength": 1},
                            "model": {"type": "string"},
                            "effort": {"type": "string"},
                            "account": {"type": "string"},
                            "preset": {"type": "string"}
                        },
                        "additionalProperties": false
                    }
                },
                "preset": {"type": "string"},
                "access": {"type": "string", "enum": ["ask", "edits", "full"]},
                "standalone": {"type": "boolean", "description": "The user's own top-level threads: nothing comes back"},
                "project": {"type": "string", "description": "absolute path of the project on that machine"},
                "machine": {"type": "string"},
                "client_request_id": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_CLIENT_REQUEST_ID_CHARS, "description": "Retry key"}
            },
            "required": ["prompt", "harnesses"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 3 (`docs/orchestrator.md`): offered only to
/// clients that advertise `_meta.rustcode.orchestration >= 3`.
pub const LIST_HARNESSES: Tool = Tool {
    name: crate::orchestration::LIST_HARNESSES,
    description: "List harnesses, models, effort, named accounts and delegate presets. Filter with `harness` or `model`. Defaults and recent models come first. Without a model filter, omitted_models counts models beyond the first 12. Check account usage (as_of, windows with used_percent, window_minutes and resets_at) before choosing a login.",
    arguments: r#"{"harness": "codex", "model": "opus"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "harness": {"type": "string", "description": "Only this harness"},
                "model": {"type": "string", "description": "Part of a model id or name, any case"},
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const SEARCH_SESSIONS: Tool = Tool {
    name: crate::orchestration::SEARCH_SESSIONS,
    description: "Search the user's threads by title and by what was said in them: each match's session_id, title, project, status and the words around it. This project's threads by default, every project's with all_projects. Ask it when the user asks about past work rather than a named thread.",
    arguments: r#"{"query": "login form", "all_projects": false}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1},
                "all_projects": {"type": "boolean", "description": "Search every project's threads, with each match's project path"},
                "machine": {"type": "string"}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const LIST_WORKTREES: Tool = Tool {
    name: crate::orchestration::LIST_WORKTREES,
    description: "List the threads that work in their own git worktree: session_id, title, project, branch and path. This project's by default, every project's with all_projects.",
    arguments: r#"{"all_projects": false}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "all_projects": {"type": "boolean", "description": "Every project's worktree threads, with each one's project path"},
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const COMPACT_SESSION: Tool = Tool {
    name: crate::orchestration::COMPACT_SESSION,
    description: "Request native context compaction in the app. Omit session_id to compact yourself. Other threads require permission. Runs after the active turn. Set resume: true to continue the task afterward.",
    arguments: r#"{"resume": false}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string"},
                "resume": {"type": "boolean", "default": false}
            },
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const UPDATE_SESSION: Tool = Tool {
    name: crate::orchestration::UPDATE_SESSION,
    description: "Rename, pin, mark unread (true) or read (false), snooze or change the status of a thread. Omit session_id for this thread; changing another asks the user first. Name this thread with your first tool call if you have not yet, with a short title of 6 words or fewer, even when it already has one; a title the user chose stays, and title_kept says so. Status: active (back in the list), review (needs a look), settled (done; what \"settle\" asks; the app closes it). snoozed_until: how long it leaves the list: 1h, evening, tomorrow, next_week or an ISO 8601 time; null brings it back.",
    arguments: r#"{"title": "Fix login", "pinned": true, "status": "settled"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "title": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_TITLE_CHARS},
                "pinned": {"type": "boolean"},
                "unread": {"type": "boolean"},
                "status": {"type": "string", "enum": crate::orchestration::SESSION_STATUSES},
                "snoozed_until": {"type": ["string", "null"], "description": "1h, evening, tomorrow, next_week, an ISO 8601 time, or null"},
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const INTERRUPT_SESSION: Tool = Tool {
    name: crate::orchestration::INTERRUPT_SESSION,
    description: "Stop the running turn of another thread of this project. The user is asked first. The thread stays and can be messaged again.",
    arguments: r#"{"session_id": "id from list_sessions"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "machine": {"type": "string"}
            },
            "required": ["session_id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const CONFIGURE_SESSION: Tool = Tool {
    name: crate::orchestration::CONFIGURE_SESSION,
    description: "Read a thread's harness, model, effort and access, or set its model and effort. Omit session_id for this thread. With neither model nor effort it only reads; changing another thread asks the user first. Models come from list_harnesses.",
    arguments: r#"{"session_id": "id from list_sessions", "effort": "high"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "model": {"type": "string", "minLength": 1},
                "effort": {"type": "string", "minLength": 1},
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const TRANSFER_SESSION: Tool = Tool {
    name: crate::orchestration::TRANSFER_SESSION,
    description: "Move an idle thread to another harness, account or model (ids from list_harnesses); it takes the conversation so far. Not itself. The user approves it.",
    arguments: r#"{"session_id": "id", "harness": "codex"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "harness": {"type": "string", "minLength": 1},
                "account": {"type": "string"},
                "model": {"type": "string"}
            },
            "required": ["session_id", "harness"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const LIST_PENDING_REQUESTS: Tool = Tool {
    name: crate::orchestration::LIST_PENDING_REQUESTS,
    description: "List the approvals and questions this project's open threads are waiting on from the user: session_id, request_id, thread title, whether it is an approval or a question, and what it asks. Ask it when the user asks what is waiting on them, then answer_request one of them if the user says what to answer. Pass `machine` to ask another paired computer.",
    arguments: r#"{}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const ANSWER_REQUEST: Tool = Tool {
    name: crate::orchestration::ANSWER_REQUEST,
    description: "Answer one approval or question a thread waits on, from list_pending_requests' session_id and request_id. An approval takes choice allow_once, allow_always or deny, exactly one of the scopes the request offers. A question takes answer, either free text or the id of one of its options, or choice deny for the Decline its card offers. Only a thread in this thread's own delegation tree can be answered, never this thread's own request, and the user approves first.",
    arguments: r#"{"session_id": "from list_pending_requests", "request_id": "from list_pending_requests", "choice": "allow_once"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "request_id": {"type": "string", "minLength": 1},
                "choice": {"type": "string", "enum": crate::orchestration::APPROVAL_ANSWERS},
                "answer": {"type": "string", "minLength": 1},
                "machine": {"type": "string"}
            },
            "required": ["session_id", "request_id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const ORGANIZE_THREADS: Tool = Tool {
    name: crate::orchestration::ORGANIZE_THREADS,
    description: "Set the title, pin, unread or archived (settled) state of several threads at once. The batch form of update_session: the same changes reach every listed thread, the user approves once, and a thread outside this project is skipped with the reason. changed counts the threads a change actually reached; one that already had the value is not counted.",
    arguments: r#"{"session_ids": ["id from list_sessions"], "pinned": true, "archived": false}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_ids": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": crate::orchestration::MAX_ORGANIZED_THREADS,
                    "items": {"type": "string", "minLength": 1}
                },
                "title": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_TITLE_CHARS},
                "pinned": {"type": "boolean"},
                "unread": {"type": "boolean", "description": "true marks them unread, false marks them read"},
                "archived": {"type": "boolean", "description": "true settles the threads, false brings them back"}
            },
            "required": ["session_ids"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const LIST_PROJECTS: Tool = Tool {
    name: crate::orchestration::LIST_PROJECTS,
    description: "List the projects the user has threads in, newest first, at most 50: path, name, how many threads each has and whether any is working. Ask it when the user asks what they have been working on or where a thread lives. Use a path as start_session's project and all_projects on list_sessions and search_sessions. Pass `machine` for another paired computer's projects, each with its git remote.",
    arguments: r#"{}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 12 (`docs/orchestrator.md`): the machines the user has
/// paired, which `start_session` and the other machine-aware tools take.
pub const LIST_MACHINES: Tool = Tool {
    name: crate::orchestration::LIST_MACHINES,
    description: "List paired computers and this computer, with their connection state, routes, threads, harnesses and projects. Use the ID or name in machine arguments. Remote thread IDs already include the machine. Remote changes ask for approval. With probe, retry each route. Offline machines are refused.",
    arguments: r#"{}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "probe": {"type": "boolean"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const LIST_PULL_REQUESTS: Tool = Tool {
    name: crate::orchestration::LIST_PULL_REQUESTS,
    description: "List this project's pull requests on its git host, at most 30: number, title, state, branch and url. Open ones by default; pass state closed, merged or all to see others. Fails when the folder is not a GitHub repository or its CLI is missing.",
    arguments: r#"{"state": "open"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "state": {"type": "string", "enum": crate::orchestration::PR_STATES},
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 9 (`docs/orchestrator.md`): a delegate's pull
/// request and its checks, merging it when green, and cleaning up after it.
pub const THREAD_STATUS: Tool = Tool {
    name: crate::orchestration::THREAD_STATUS,
    description: "Report delegates' status, project, worktree, branch, pull request and checks. Running delegates include idle time and the last recorded provider error. Omit session_id for every delegate this thread started or messaged.",
    arguments: r#"{"session_id": "id from list_sessions, or omit for all delegates"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1, "description": "One delegate; omit to report every delegate of this thread"},
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const MERGE_WHEN_GREEN: Tool = Tool {
    name: crate::orchestration::MERGE_WHEN_GREEN,
    description: "Squash-merge a PR and delete its branch after checks pass, with user approval. Pass number for a PR in this thread's repository (its current-branch PR, or one on another branch such as a release PR): returns after approval and wakes this thread with the outcome; end the turn, do not poll. Or pass session_id for a delegate: waits up to timeout_ms (default/max 25 minutes). Advisory portability jobs and cancelled/superseded runs do not block; two quiet rounds catch late jobs. Failing checks stop the merge and report their names. Returns merged, failing, closed, no_checks or pending; no checks stops immediately.",
    arguments: r#"{"session_id": "the delegate whose PR should merge", "timeout_ms": 1500000}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "number": {"type": "integer", "minimum": 1},
                "timeout_ms": {"type": "integer", "minimum": 1, "maximum": crate::orchestration::MAX_MERGE_WAIT_MS, "description": "How long to wait for checks; at most and by default 25 minutes per call"}
            },
            "oneOf": [{"required": ["session_id"]}, {"required": ["number"]}],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const CLEANUP_WORKTREE: Tool = Tool {
    name: crate::orchestration::CLEANUP_WORKTREE,
    description: "Remove a merged delegate's worktree and its branch, then settle the thread unless settle is false. It refuses a worktree on the base or default branch, dirtied, holding ignored files that are not build output, or with commits `git cherry <base> <branch>` shows are not on the base and not part of the merged pull request's head; the refusal says which commits or files those are. A branch the delegate renamed after the caller read its pull request is accepted when that branch's pull request merged and nothing would be lost, and the branch it was recorded on is removed too; otherwise the refusal names the merged-pull-request check that failed. Only worktrees under RustCode's worktrees folder are touched, never the main checkout, and a git that cannot answer refuses instead of guessing. The user approves it.",
    arguments: r#"{"session_id": "the merged delegate", "settle": true}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "settle": {"type": "boolean", "description": "Set the thread to Settled after cleanup; true by default"}
            },
            "required": ["session_id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 11 (`docs/orchestrator.md`): the user's projects and
/// the scheduler daemon's work (`docs/daemon.md`).
pub const READ_PROJECT: Tool = Tool {
    name: crate::orchestration::READ_PROJECT,
    description: "Read one of the user's projects: path, name, whether it is a git repository, its remote and branch, the branch it calls default, its threads, and its display name and default harness/model. Reads any project while \"Let agents read threads in other projects\" is on, else only this thread's.",
    arguments: r#"{"path": "/abs/project"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "description": "absolute path of the project, as list_projects prints it"},
                "machine": {"type": "string"}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const CREATE_PROJECT: Tool = Tool {
    name: crate::orchestration::CREATE_PROJECT,
    description: "Register a project folder: an existing one, or a new empty folder that gets `git init` (init: false leaves it plain). Nothing is cloned; see clone_project. The user approves it.",
    arguments: r#"{"path": "/abs/new-project", "init": true}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "description": "absolute path of the folder"},
                "init": {"type": "boolean", "description": "`git init` a folder that is not a repository; true by default"}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const CLONE_PROJECT: Tool = Tool {
    name: crate::orchestration::CLONE_PROJECT,
    description: "Clone a git repository into parent/name (the parent defaults to its usual folder) and register it as a project. `git clone` runs with a ten-minute wait. The user approves it.",
    arguments: r#"{"url": "https://github.com/owner/repo.git", "parent": "/abs/folder", "name": "repo"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "minLength": 1},
                "parent": {"type": "string", "minLength": 1, "description": "absolute folder the clone goes in; the default is the folder new threads start in"},
                "name": {"type": "string", "minLength": 1, "description": "the new folder's name; the url's last part by default"},
                "machine": {"type": "string"}
            },
            "required": ["url"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const UPDATE_PROJECT: Tool = Tool {
    name: crate::orchestration::UPDATE_PROJECT,
    description: "Set project name or new-thread harness/model. Omitted fields stay. destination moves its folder on one filesystem, keeping threads/history; close affected threads first. User approval required.",
    arguments: r#"{"path": "/abs/project", "name": "RustCode", "harness": "claude-code", "model": "opus"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1},
                "destination": {"type": "string", "minLength": 1, "description": "absolute new path; existing parent"},
                "name": {"type": "string", "minLength": 1, "maxLength": 200, "description": "sidebar name"},
                "harness": {"type": "string", "minLength": 1, "description": "id from list_harnesses"},
                "model": {"type": "string", "minLength": 1}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const REMOVE_PROJECT: Tool = Tool {
    name: crate::orchestration::REMOVE_PROJECT,
    description: "Unregister a project: it leaves the project list with its display name and defaults. Its folder, files and threads stay. The user approves it.",
    arguments: r#"{"path": "/abs/project"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const CREATE_LOOP: Tool = Tool {
    name: crate::orchestration::CREATE_LOOP,
    description: "Create a job (once or recurring) on any harness, shown in Jobs. Each run is a fresh read-only thread by default; runs while RustCode is open. Agent IDs: list_harnesses. The user approves it.",
    arguments: r#"{"name":"Morning check","prompt":"Check the build","schedule":{"kind":"cron","expression":"0 9 * * 1-5"}}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        let mut schema = (SCHEDULE_TASK.schema)();
        let properties = schema["properties"].as_object_mut().unwrap();
        properties.remove("model_profile");
        properties.extend(serde_json::Map::from_iter([
            ("harness".into(), serde_json::json!({"type":"string","description":"Harness ID from list_harnesses; omitted uses the default harness"})),
            ("account".into(), serde_json::json!({"type":"string","description":"Named account ID from list_harnesses; requires harness"})),
            ("model".into(), serde_json::json!({"type":"string","description":"Model ID from list_harnesses"})),
            ("effort".into(), serde_json::json!({"type":"string","description":"Reasoning effort supported by the model"})),
            ("thread_id".into(), serde_json::json!({"type":"string","description":"Existing thread in the project; omit harness, account, model and effort when supplied"})),
            ("read_only".into(), serde_json::json!({"type":"boolean","default":true,"description":"Decline approvals during unattended runs"})),
            ("client_request_id".into(), serde_json::json!({"type":"string","description":"Reuse the same ID when retrying this exact request"})),
        ]));
        properties["name"]["description"] = serde_json::json!("Name shown in Jobs");
        schema["required"] = serde_json::json!(["name", "prompt", "schedule"]);
        schema
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const UPDATE_LOOP: Tool = Tool {
    name: crate::orchestration::UPDATE_LOOP,
    description: "Update, pause or resume a job by id, shown in Jobs. Omitted fields stay unchanged. The user approves it.",
    arguments: r#"{"id":"loop-1750000000000-1a2b","paused":true}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "minLength": 1, "description": "Job id from list_jobs"},
                "paused": {"type": "boolean", "description": "true pauses the job, false resumes it"},
                "name": {"type": "string", "minLength": 1, "maxLength": 200, "description": "Name shown in Jobs"},
                "prompt": {"type": "string", "minLength": 1, "maxLength": 32768, "description": "Prompt each run is asked to do"},
                "schedule": {
                    "oneOf": [
                        {"type": "object", "properties": {"kind": {"const": "cron"}, "expression": {"type": "string", "minLength": 1}, "timezone": {"type": "string", "minLength": 1}}, "required": ["kind", "expression"], "additionalProperties": false},
                        {"type": "object", "properties": {"kind": {"const": "at"}, "at": {"type": "string", "minLength": 1}}, "required": ["kind", "at"], "additionalProperties": false},
                        {"type": "object", "properties": {"kind": {"const": "manual"}}, "required": ["kind"], "additionalProperties": false}
                    ]
                },
                "harness": {"type": "string", "minLength": 1, "description": "Harness ID from list_harnesses"},
                "account": {"type": "string", "minLength": 1, "description": "Named account ID from list_harnesses"},
                "model": {"type": "string", "minLength": 1, "description": "Model ID from list_harnesses"},
                "effort": {"type": "string", "minLength": 1, "description": "Reasoning effort supported by the model"},
                "thread_id": {"type": "string", "minLength": 1, "description": "Existing thread in the project; omit harness, account, model and effort when supplied"},
                "read_only": {"type": "boolean", "description": "Decline approvals during unattended runs"}
            },
            "required": ["id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const RUN_MACHINE_COMMAND: Tool = Tool {
    name: crate::orchestration::RUN_MACHINE_COMMAND,
    description: "Run a shell command directly on any machine in the fleet (like SSH, over RustCode's paired connection or hub link) and return its output. Pass `machine` (id or name from list_machines) for another machine, or omit it for this machine. Respects the target machine's command policy and the caller's access level.",
    arguments: r#"{"command": "uptime && df -h", "machine": "lagosvps", "cwd": "/home/lucas/code/rustcode", "timeout_secs": 60}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "minLength": 1, "maxLength": 32768, "description": "Shell command to run on the target machine"},
                "machine": {"type": "string", "minLength": 1, "description": "Target machine id or name from list_machines; omit for this machine"},
                "cwd": {"type": "string", "minLength": 1, "description": "Absolute working directory on the target machine; defaults to the machine's matching project or home folder"},
                "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 300, "description": "Command timeout in seconds (default 60, max 300)"}
            },
            "required": ["command"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const SCHEDULE_TASK: Tool = Tool {
    name: crate::orchestration::SCHEDULE_TASK,
    description: "Schedule a built-in-harness task: a prompt run once at an ISO 8601 time or on a cron. `schedule` is {kind: cron, expression, timezone?} or {kind: at, at}; timezone defaults to the user's. For another harness or Jobs, use create_job. Needs the scheduler running; report its next run. The user approves it.",
    arguments: r#"{"prompt": "Check the build", "schedule": {"kind": "cron", "expression": "0 9 * * 1-5", "timezone": "Europe/Oslo"}, "project": "/abs/project", "name": "Morning check"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "minLength": 1, "maxLength": 32768, "description": "what each run is asked to do"},
                "schedule": {
                    "oneOf": [
                        {"type": "object", "properties": {"kind": {"const": "cron"}, "expression": {"type": "string", "minLength": 1}, "timezone": {"type": "string", "minLength": 1}}, "required": ["kind", "expression"], "additionalProperties": false},
                        {"type": "object", "properties": {"kind": {"const": "at"}, "at": {"type": "string", "minLength": 1}}, "required": ["kind", "at"], "additionalProperties": false}
                    ]
                },
                "project": {"type": "string", "minLength": 1, "description": "absolute folder the prompt runs in; this thread's project by default"},
                "name": {"type": "string", "minLength": 1, "maxLength": 200, "description": "what the task is called; the prompt's first line by default"},
                "model_profile": {"type": "string", "minLength": 1, "description": "a model profile from the config, as manage_scheduled_jobs takes"}
            },
            "required": ["prompt", "schedule"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const LIST_SCHEDULED_TASKS: Tool = Tool {
    name: crate::orchestration::LIST_SCHEDULED_TASKS,
    description: "List jobs (once, recurring and webhook): id, name, paused, schedule, workspace and next run, at most 100. `daemon_running: false` means the scheduler is stopped; `hint` says how to start it.",
    arguments: r#"{}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const DELETE_SCHEDULED_TASK: Tool = Tool {
    name: crate::orchestration::DELETE_SCHEDULED_TASK,
    description: "Delete a job by id. It stops at once with its run history. The user approves it.",
    arguments: r#"{"id": "loop-1750000000000-1a2b"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "minLength": 1}
            },
            "required": ["id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const RUN_SCHEDULED_TASK_NOW: Tool = Tool {
    name: crate::orchestration::RUN_SCHEDULED_TASK_NOW,
    description: "Run a job now by id, keeping its schedule. Needs the scheduler. The user approves it.",
    arguments: r#"{"id": "loop-1750000000000-1a2b"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "minLength": 1}
            },
            "required": ["id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 13 (`docs/orchestrator.md`): where a worktree stands,
/// and handing its work back to the user.
pub const WORKTREE_STATUS: Tool = Tool {
    name: crate::orchestration::WORKTREE_STATUS,
    description: "Report where worktrees stand: the owning thread, its branch and path, commits ahead of and behind the ref it tracks, uncommitted files, the last commit and the pull request its branch heads. A read: no approval.",
    arguments: r#"{"session_id": "a thread's id, or omit for every worktree"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1, "description": "One thread, its own included; omit for every worktree this thread can reach"},
                "all_projects": {"type": "boolean", "description": "Every project's worktree threads, with each one's project path"},
                "machine": {"type": "string"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const HANDOFF_WORKTREE: Tool = Tool {
    name: crate::orchestration::HANDOFF_WORKTREE,
    description: "Move a thread's work to where the user can continue it, keeping every commit. `to: \"checkout\"` (the default) removes a delegate's worktree, switches the user's active checkout to its branch and moves the thread there; `to: \"worktree\"` gives a thread that runs in the checkout a worktree of its own on a new branch from the checkout's HEAD, leaving the checkout untouched. It refuses rather than forces: a worktree with uncommitted or ignored files, a checkout with uncommitted changes, a checkout on another branch holding commits no remote has, and a running thread. Nothing is stashed, rebased or reset, and no branch is deleted. The user approves it.",
    arguments: r#"{"session_id": "the delegate whose work the user takes over", "to": "checkout"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "to": {"type": "string", "enum": crate::orchestration::HANDOFF_TARGETS, "description": "checkout (default) moves a worktree's branch into the user's checkout; worktree moves a checkout thread into its own worktree"}
            },
            "required": ["session_id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Link a thread to a pull request (#3369) and read it back: the engine keeps
/// checking through the watch machinery (`pull_watch.rs`), so the thread hears
/// about a change once instead of polling `gh` itself. Built in since #3376
/// freed the room for it; the `rustcode` MCP server answers it too.
pub const MANAGE_PULL_LINKS: Tool = Tool {
    name: crate::pull_watch::LINK_TOOL_NAME,
    description: "Link this thread to a pull request and keep it checked without polling: `link` answers its state now, then each change reaches this thread once through `status` (checks, reviews, comments, conflicts, merged, closed); `unlink` stops. It stops itself when the PR merges or closes, after six failed GitHub reads, or after two weeks; 5 per thread, 20 at once, and a PR several threads link is read once.",
    arguments: r#"{"action": "link|status|unlink", "repo": "owner/name", "number": 7}"#,
    handler: manage_pull_links,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": crate::pull_watch::LINK_ACTIONS},
                "repo": {"type": "string", "pattern": "^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$", "description": "owner/name; defaults to this thread's folder"},
                "number": {"type": "integer", "minimum": 1}
            },
            "required": ["action"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::Network, ToolCapability::SessionState],
    safety: ToolSafety::ControlPlane,
};

fn manage_pull_links(args: &Value) -> Result<String, String> {
    let session_id = crate::tools::get_active_session_id()
        .ok_or("no session is active to link a pull request for")?;
    // A link reads GitHub through `gh --repo`, so any folder will do; this
    // thread's own is the one whose sign-in and remotes are already set up.
    let cwd = crate::checkpoints::thread_cwd(&session_id)
        .filter(|cwd| cwd.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .ok_or("there is no folder to read the pull request in")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to create async runtime: {error}"))?;
    runtime.block_on(crate::pull_watch::answer(args, &session_id, &cwd))
}

/// Tokens and the cost a harness reported, from the local data store
/// (`docs/data-store.md`). The app answers it, so the project scope follows
/// "Let agents read threads in other projects".
pub const THREAD_USAGE: Tool = Tool {
    name: crate::orchestration::THREAD_USAGE,
    description: "Tokens and cost from what RustCode recorded of every turn, as rows with totals: input, output, cached and cache-write tokens, cost_usd (only the USD a harness reported, never a price RustCode computed), turns, per-column counts of turns that reported no input, output, cached or cache-write tokens, and how many reported no cost. Ask it when the user asks what they have spent or used. group_by: harness (default), model, day, project or thread; since takes a day or month, YYYY-MM-DD or YYYY-MM; project limits it to one project's threads.",
    arguments: r#"{"since": "2026-11-01", "group_by": "harness", "project": "/abs/project"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "since": {"type": "string", "description": "YYYY-MM-DD or YYYY-MM; every turn from that day or month on"},
                "group_by": {"type": "string", "enum": crate::data_store::GroupBy::ALL},
                "project": {"type": "string", "description": "absolute path of one project, as list_projects prints it"}
            },
            "required": [],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// One read-only SQL statement over the local data store, answered by the app.
pub const QUERY_DATA: Tool = Tool {
    name: crate::orchestration::QUERY_DATA,
    description: concat!(
        "Run one read-only SQL statement (SQLite) over what RustCode recorded, for questions thread_usage cannot answer: which tools fail, an exit code, a day's turns. At most 1000 rows. ",
        "Tables: threads(id, title, project, harness, model, created_at); turns(thread_id, started_at, finished_at, harness, model, input_tokens, output_tokens, cached_tokens, cache_write_tokens, cost_usd, tool_calls, status, duration_ms, context_tokens, context_window, stop_reason, error_kind), Codex input includes cached; approvals(thread_id, tool_name, decision); tool_calls(thread_id, turn_id, tool_name, harness_tool, success, exit_code, at, duration_ms, output_bytes, truncated, mcp_server, command_name, error_kind), harness_tool is the harness's own name; compactions(thread_id, harness, model, at, duration_ms, trigger, mode, tokens_before, tokens_after, count, success, source), trigger is auto_context, manual, cache_expiry or harness_auto; thread_links; thread_outcomes (linked PR per thread); daily_usage, daily_compactions, tool_failure_rates, turn_latency_daily, pr_outcomes_by_model views; provider_requests(thread_id, started_at, status_code, error_kind, request_payload_json); harness_rpc_messages(thread_id, direction, method, at, latency_ms, payload_json); operational_events(thread_id, sequence, event_type, at, fields_json). Raw payloads are NULL unless captured. Unreported tokens/cost are NULL, never guessed. Needs \"Let agents read threads in other projects\" on."
    ),
    arguments: r#"{"sql": "SELECT tool_name, COUNT(*) FROM tool_calls WHERE success = 0 GROUP BY 1 ORDER BY 2 DESC", "limit": 50}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "sql": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_SQL_BYTES},
                "limit": {"type": "integer", "minimum": 1, "maximum": 1000, "default": 1000}
            },
            "required": ["sql"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 6 (`docs/orchestrator.md`): one thread's recorded
/// activity, read back from the local data store.
pub const READ_ACTIVITY: Tool = Tool {
    name: crate::orchestration::READ_ACTIVITY,
    description: "Page a thread's events, provider attempts and harness messages, metadata only. order newest starts with the latest; default oldest. Pass next as after with the same order.",
    arguments: r#"{"session_id": "id from list_sessions", "after": "next from the last page", "limit": 50}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "after": {"type": "string", "description": "Opaque `next` token from an earlier page; omit for the first page"},
                "order": {"type": "string", "enum": ["oldest", "newest"], "default": "oldest"},
                "limit": {"type": "integer", "minimum": 1, "maximum": crate::data_store::MAX_ACTIVITY_ITEMS, "default": crate::data_store::DEFAULT_ACTIVITY_ITEMS},
                "machine": {"type": "string"}
            },
            "required": ["session_id"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Orchestration version 7 (`docs/orchestrator.md`): the messages this
/// thread queued on another one, and what it may still do to them.
pub const MANAGE_QUEUE: Tool = Tool {
    name: crate::orchestration::MANAGE_QUEUE,
    description: "List, edit, cancel or steer this thread's queued messages in any project; the user is asked once. Lists queue_id, first 200 characters and time in send order. Only this thread's own queued messages can change, never one that started. steer delivers mid-turn if supported. For new input use message_session with steer: true.",
    arguments: r#"{"session_id": "the busy thread", "action": "list|edit|cancel|steer", "queue_id": "from list", "text": "the new text, for edit"}"#,
    handler: orchestration_unavailable,
    requires_confirmation: false,
    schema: || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "minLength": 1},
                "action": {"type": "string", "enum": crate::orchestration::QUEUE_ACTIONS},
                "queue_id": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_QUEUE_ID_CHARS, "description": "A queued message's id, from list"},
                "text": {"type": "string", "minLength": 1, "maxLength": crate::orchestration::MAX_MESSAGE_BYTES, "description": "What the message says instead, for edit"},
                "machine": {"type": "string"}
            },
            "required": ["session_id", "action"],
            "additionalProperties": false
        })
    },
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};
