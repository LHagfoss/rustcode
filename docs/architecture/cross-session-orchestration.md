# Cross-session agent orchestration

Status: proposed design (#2216). This document adds no runtime. The macOS app
is the first target; TUI parity is a later, optional follow-up.

## Problem

Today an agent can coordinate work only inside its own session: it spawns
subagents whose histories live in the parent's `agents.json` and disappear
from view outside that chat. Users already run several chats side by side in
the macOS app, often in different projects, and move information between them
by hand. The goal is to let an agent, under the user's supervision:

- find existing sessions and read what is relevant in them;
- create or resume a session in an explicitly selected project;
- start work in that session, follow its status and results, and send it
  follow-ups;
- bring the results back to the task that asked for them.

It must do this without silently sharing whole histories and without granting
ambient access to other projects.

## What exists today

| Capability | Where | What it gives us | Gap for cross-session use |
| --- | --- | --- | --- |
| Subagents (`spawn_agent`, `send_message`, `followup_task`, `wait_agent`, `inspect_agent`, `list_agents`, `cancel_agent`) | `rustcode/engine/src/app/subagent_controller.rs`, `subagent_context.rs`, `subagent_persistence.rs`, tools in `tools/misc.rs`; [multi-agent.md](../multi-agent.md) | A settled tool vocabulary and limits: explicit context inheritance (`minimal`/`evidence`/`recent`/`fork`), 32-message mailboxes, 8 KiB messages and completions, bounded inspect tails, interrupt semantics, cancellation down the tree, resume marks running work `interrupted` | Children are session-local (numeric ids, stored in the parent's `agents.json`), share the parent's workspace and policy, and are not chats the user can open in the sidebar |
| `delegate_task` | `tools/delegate.rs` | Hand a self-contained task to an external CLI | Fire-and-forget; no status, no follow-up, no result routing |
| ACP sessions | `rustcode/engine/src/acp.rs`, `acp/prompt.rs`; [acp.md](../acp.md) | One engine process hosts many sessions, each with its own `AppState`, cwd/workspace root, sandbox, per-session turn gate (`SessionTurnState`), cancellation and background task routes. `session/new`, `session/load`, `session/list` (per project or all), `session/close` | Sessions cannot see each other; there is no engine-side API for one session to address another |
| Session store and search | `rustcode/core/rustcode-session`, `config/session.rs`, `rustcode sessions search --json`, `SessionSearch.swift` | Persistent sessions filed by date with `workspace.json`, content search with snippets, exact-project and all-project scopes | Only the user can browse; no bounded, permissioned read for an agent |
| Remote control plane | `remote/protocol.rs` (`RemoteOperation`), `remote/owner.rs`, `remote_gateway/`; [remote.md](../remote.md) | A tested contract for driving a session from outside its owner: list, create, attach, `GetHistory`, `SubmitPrompt`, `Steer`, `Queue`, `CancelTurn`, `AnswerQuestion`, `ResolveApproval`, `ResumeSession`, request receipts (`Received`/`Applied`/`Rejected`/`Unknown`) | Built for a paired device and a human, not for an agent inside another session; terminal-owned |
| Scheduler daemon | `daemon/`; [daemon.md](../daemon.md) | `JobAction::Prompt { prompt, workspace, session_id }`: durable, out-of-turn prompts into a session | Scheduled, not interactive; no result routing back to a caller |
| macOS app | `macos/Sources/RustCode/RustCodeApp.swift` | Several open sessions per engine (`openSessions`), per-session queues and turn activity, permission sheets driven by `session/request_permission`, sidebar search across projects | No notion of one chat driving another |

Two lessons from #2215 shape this design:

1. Concurrent turns of different sessions in one engine are already normal.
   Their logs used to be misattributed (fixed by scoping log attribution to
   the running turn, #2223). Any orchestration must keep identity explicit per turn,
   never through process-wide "active session" state.
2. Users lose context when a follow-up lands in a different chat than they
   think. Every cross-session action must be visible in both chats.

## Decision: where it lives

Use both models, with one tool vocabulary.

- **Subagents stay** for short-lived helpers inside one task. They are cheaper,
  share the parent's workspace and need no new UI.
- **Session orchestration is a separate engine component**, the
  `SessionOrchestrator`, owned by the ACP server (the process the macOS app
  runs). It addresses real, user-visible sessions. Its tools mirror the
  subagent verbs so the model learns one pattern:

  | Subagent tool | Session tool (proposed) |
  | --- | --- |
  | `list_agents` | `list_sessions` (metadata, search) |
  | `inspect_agent` | `read_session` (bounded excerpt) |
  | `spawn_agent` | `start_session` (new or resumed, in a selected project) |
  | `send_message` / `followup_task` | `message_session` (queue or start a turn) |
  | `wait_agent` | `wait_session` |
  | `cancel_agent` | `cancel_session_turn` |

- **The remote gateway is not the transport for M1.** The orchestrator runs
  inside the ACP engine and calls the same session internals the ACP handlers
  call. Its operations deliberately match `RemoteOperation` (`SubmitPrompt`,
  `Queue`, `CancelTurn`, `GetHistory`, receipts) so that reaching
  terminal-owned sessions later is an adapter over the gateway, not a new
  model.

## Identity

- **Session reference**: `{ session_id, project }`, where `project` is the
  canonical workspace root recorded in the session's `workspace.json`. Tools
  always return both, and the engine re-validates the pair on every call so a
  stale or forged id cannot cross projects.
- **Link**: one orchestrating session driving one target session. `link_id`
  is `"<orchestrator>/<n>"`, stable for the orchestrator's lifetime, so the
  model never has to juggle raw session ids. A target may have at most one
  active link at a time.
- **Turn attribution**: every turn the orchestrator starts in a target carries
  `origin = { link_id, orchestrator_session_id, orchestrator_turn_id }` in the
  target's turn metadata and history, and every operational event carries both
  session ids explicitly (`orchestration.message`, `orchestration.turn.finish`,
  …). Nothing relies on the process-wide active session.

## Context sharing

Decisions:

- **No history is shared by default, in either direction.** A target starts
  from its own history (resumed) or empty (new), plus the message.
- **Outbound context is explicit**, like subagent `evidence`: the orchestrator
  sends a message (at most 8 KiB) and optional selected evidence (at most
  8 KiB). There is no `fork` or `recent` mode across sessions; forking a
  conversation remains `/fork` by the user.
- **Inbound results are bounded**: `wait_session` returns the target turn's
  structured completion (status, final answer, changed paths, truncation
  flag) capped at 8 KiB, the same budget as a subagent completion. It never
  returns the transcript.
- **Reading other sessions is a separate, narrower grant**. `list_sessions`
  returns metadata and search snippets (title, project, updated time, match
  snippet as `rustcode sessions search` prints it). `read_session` returns a
  bounded excerpt: at most 8 entries of 768 characters (the `inspect_agent`
  tail limits), or the entries around a search match. Tool outputs, images,
  reasoning and system reminders are never returned. Excerpts pass a
  secret-redaction step before they are returned; none exists for transcript
  content today, so it is new work for M2.
- Messages arrive in the target as a user-role message with a visible header
  (`From chat "<title>" (agent)`), so the target model and the user both see
  where the instruction came from. They are not system messages and cannot
  change the target's policy.

## Authorization and project boundaries

Decisions:

- **Off unless the client supports it.** The tools are advertised only when
  the ACP client declares `_meta.rustcode.orchestration` in `initialize`
  (as presence does today) and `delegation_enabled` is not false. Project
  config cannot enable them.
- **Grants are per orchestrating session and per target project**, asked once
  through `session/request_permission` with the options *Allow for this chat*,
  *Allow once*, *Deny*. Reading (`list_sessions` beyond the orchestrator's own
  project, `read_session`) and driving (`start_session`, `message_session`)
  are separate grants: being allowed to read a project does not allow
  starting work in it.
- **The orchestrator's own project is not special for driving.** Even in the
  same project, the first `message_session` to a session the user did not
  create through this orchestrator asks once.
- **Projects are never chosen silently.** `start_session` takes a project
  path that must be either the orchestrator's project or one the user picks
  in the permission prompt (the macOS sheet shows the path and offers the
  folder picker). A path the user has never opened is rejected.
- **Targets keep their own policy.** A target runs with its own workspace
  root, sandbox mode, settings and approval rules. The orchestrator's grants,
  auto-approve setting and sandbox never extend to the target; tool approvals
  inside the target appear in the target chat (and as a badge in the
  orchestrator's linked-chat chip), never as a pass-through.
- **No chains.** A target session driven by a link does not get the
  orchestration tools for that turn. Only sessions started by the user can
  orchestrate, so there is no unbounded recursion or loop between chats.

## Persistence and resumability

- The orchestrator records links in its own session directory as
  `session_links.json`, written atomically like `agents.json`: link id, target
  reference, grants, mailbox, last request receipt, last completion.
- The target records only a back-reference (`linked_by`) in its metadata and
  the attributed messages in its history, so it remains an ordinary chat that
  can be opened, continued or deleted on its own.
- On `session/load` of an orchestrator, links whose target turn was queued or
  running are marked `interrupted`. Nothing restarts automatically, matching
  subagent resume. The model can `wait_session` (returns `interrupted`) or
  `message_session` again.
- Grants persist with the orchestrator for *Allow for this chat*; *Allow
  once* grants are not persisted.

## Concurrency and message routing

- A target's turns stay serialized by its existing `SessionTurnState`.
  `message_session` to a busy target queues the message in the link mailbox
  (32 messages, oldest-first) and returns receipt `queued`; it is delivered as
  the target's next turn. It never steers or interrupts a turn the user
  started.
- `interrupt: true` is allowed only for a turn the same link started, as with
  `followup_task`.
- The orchestrator may have at most 4 active links and the engine at most
  8 orchestrated turns running at once; further turns queue, and waiting
  yields the orchestrator's slot as `wait_agent` does.
- `wait_session` accepts one `link_id` or several (first to finish wins),
  default timeout 60 s, maximum 1 h, and wakes on completion, a question or an
  approval waiting in the target.

## Cancellation

- Stopping the orchestrator's turn cancels pending waits and any target turn
  that this turn started through a link. It does not cancel turns the user
  started in the target, and it does not cancel queued messages from earlier
  turns (they are listed and can be cancelled from the chip).
- `cancel_session_turn` cancels only a link-started turn.
- Stopping a target turn by hand is always allowed and returns `cancelled` to
  the waiting orchestrator.
- Closing or deleting the orchestrator detaches its links: queued messages
  are dropped, running target turns continue as ordinary chats.

## Failure cases

| Case | Behaviour |
| --- | --- |
| Grant denied | Tool returns a permission error; nothing is created or sent |
| Target project missing or not a directory | `start_session` fails before creating anything |
| Target session deleted or moved to the Trash | Link becomes `broken`; tools return `target_unavailable` |
| Target busy with a user turn | Message queued, receipt `queued` |
| Target asks a question or needs approval | Shown in the target chat; `wait_session` returns `needs_user` with the target link so the orchestrator can tell the user |
| Target turn fails or hits limits | Completion with `status = failed` and the error summary |
| Engine exits or app quits | On reload, in-flight links are `interrupted`; nothing restarts |
| Mailbox full | `message_session` fails with `mailbox_full`; nothing is dropped silently |
| Two orchestrators target one session | Second link is refused with `target_linked` |

## macOS UX

- **Grant sheet**: the existing permission sheet, showing the orchestrator
  chat, the target project (with folder picker when starting in a new
  project) and what is asked (read, or start work).
- **Linked-chat chips** above the orchestrator's composer: one per link with
  title, project, state (queued, working, needs you, done, failed,
  interrupted) and a stop control. Clicking opens the target chat.
- **Target banner**: a target chat shows "Driven by <orchestrator chat>" with
  *Open*, *Pause messages* and *Detach*. Attributed messages render with the
  orchestrator's title.
- **Sidebar**: driven chats get a link badge; "needs you" uses the same badge
  as pending approvals.
- The app sends `_meta.rustcode.orchestration` only when the user turns the
  feature on in Settings (off by default in M1).

## Protocol and engine changes

Engine:

1. `SessionOrchestrator` owned by `AcpServer`: lookup of open sessions,
   loading of saved ones (refactoring the `session/new` and `session/load`
   handlers into callable functions), link registry, mailboxes, grants.
2. A narrow host handle for tools, injected per turn with a `task_local`
   (as `DIRECT_MCP_REGISTRY` and the turn's log session are), so tools reach
   the orchestrator without global state.
3. Tools `list_sessions`, `read_session`, `start_session`,
   `message_session`, `wait_session`, `cancel_session_turn`, each a
   `pub const …: Tool` with a new `ToolCapability::SessionOrchestration`,
   added to `TOOLS` and withheld unless the client and config enable it.
4. Turn origin metadata on target turns and attributed user messages in the
   target history.
5. `session_links.json` persistence and resume marking.
6. Operational events `orchestration.grant`, `orchestration.link`,
   `orchestration.message`, `orchestration.turn.finish`, each carrying both
   session ids.

ACP (extensions, versioned like presence):

- `initialize` client `_meta.rustcode.orchestration: 1`.
- Grants reuse `session/request_permission` with
  `_meta.rustcode.orchestration` describing the request.
- `_rustcode/orchestration/links` notification per session: the links and
  their states, for chips and banners.
- `session/new` and `session/load` responses and `session/list` entries gain
  `_meta.rustcode.linkedBy` for targets.

Later, for terminal-owned sessions: an adapter from the orchestrator to the
remote gateway's `RemoteOperation`s.

## First milestone (M1): ask another chat in the same project

Scope, kept small on purpose:

- Only sessions in the orchestrator's own project, and only sessions loaded
  in the same engine (open, or loadable from the store).
- Tools: `list_sessions` (own project, metadata only), `message_session`
  (existing session), `wait_session`. No `read_session`, no
  `start_session`, no cross-project anything.
- One grant per target (*Allow for this chat* / *Allow once* / *Deny*).
- Links persisted, resume marks `interrupted`.
- macOS: Settings toggle, grant sheet, linked-chat chip, target banner.

User-visible flow: in chat A the user asks "ask the chat that set up the
OAuth server which redirect URI it used". The agent lists the project's chats,
picks chat B, the app asks once whether chat A may send messages to chat B,
chat B shows the attributed question and answers, chat A's chip turns to
*done* and the agent answers using chat B's completion.

### Test plan

Engine (`cargo test --workspace`, in-process ACP tests next to the existing
ones in `rustcode/engine/src/acp.rs`, with a scripted model):

- **Session selection**: `list_sessions` returns only the orchestrator's
  project; a session id from another project is rejected even when it exists;
  a forged `{id, project}` pair is rejected.
- **Agent messaging**: a message to an idle target starts one turn whose
  history entry carries the attribution header and origin metadata; a message
  to a busy target is queued and runs after the user's turn without steering
  it; the 33rd queued message fails with `mailbox_full`.
- **Result delivery**: `wait_session` returns the target's completion capped
  at 8 KiB with the truncation flag; timeouts leave the link usable; a target
  question returns `needs_user`.
- **Permission boundaries**: no tools without the client capability or with
  `delegation_enabled = false`; deny blocks sending; *Allow once* is not
  persisted; a driven target does not get orchestration tools; target tool
  approvals are never auto-approved by the orchestrator's settings.
- **Interrupted and resumed work**: reloading the orchestrator mid-turn marks
  the link `interrupted` and does not restart it; stopping the orchestrator
  cancels only the link-started turn; deleting the target makes the link
  `broken`.
- **Attribution**: two concurrent turns (orchestrator and target) write their
  operational events to their own session logs (extends the #2215 regression).

macOS (`swift test` in `macos/`, RustCodeKit):

- Decoding `_rustcode/orchestration/links` and the grant request metadata.
- Chip state mapping, including `needs_user` and `interrupted`.
- The capability is sent in `initialize` only when the Settings toggle is on.

### Later milestones

- M2: `read_session` and cross-project `list_sessions` behind the read grant.
- M3: `start_session` (new or resumed) in a user-selected project.
- M4: terminal-owned sessions through the remote gateway; TUI chips.
