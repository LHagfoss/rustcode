//! Starting and messaging threads with no app attached (#3422): the mutating
//! half of [`super::local`], for `start_session`, `message_session` and
//! `fan_out`.
//!
//! The engine cannot hand a thread to a client that is not there, so it opens
//! one itself: a loopback ACP connection to the endpoint this very process
//! published (`server/engine_endpoint.rs`, `server/mod.rs`), and the same
//! `session/new` and `session/prompt` the app sends. The new thread is an
//! ordinary engine thread on any harness, saved in the store and listed when
//! a client opens the app; its updates go to this connection, which answers
//! its permission requests itself — only ever when the caller runs at Full
//! access, because nobody else can approve a card.
//!
//! Links are written to the app's own link book, so `wait_session` settles a
//! delegate however it was started and the app shows the same links when it
//! opens. The turn that settles a link runs here, in the engine, so a link
//! reaches `done` with no app to update it.
//!
//! What this path does not do is stated in `docs/orchestrator.md` › *No app
//! attached*: another machine is out of scope, attachments need the app, and
//! a thread's per-thread access is app-only, so the caller's access is read
//! from what the engine can see (its `/yolo` and Settings › General › Default
//! access).

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ClientCapabilities, ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;

use super::local::{clean_path, project_of, reach_of};

/// How long opening the loopback connection and its `initialize` may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `session/new` may take: an external harness starts its process,
/// which `npx` may spend downloading the first time.
const CREATE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long `session/load` may take: an external harness is started again.
const LOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// The most active links one thread has, and the machine as a whole, when the
/// app has not written its own numbers: the app's defaults.
const DEFAULT_PER_THREAD: u64 = 8;
const DEFAULT_TOTAL: u64 = 16;
/// How deep delegation goes, as the app bounds it (`docs/orchestrator.md`).
const MAX_DEPTH: usize = 2;
/// The events one link keeps, as the app bounds them.
const MAX_LINK_EVENTS: usize = 12;
/// The preview a `message_session` answer carries.
const PREVIEW_CHARS: usize = 200;

/// What a delegate is asked to end with, word for word as the app asks it.
const DELEGATE_REPORT: &str = "When you finish, end with a short report for the orchestrator, at most about 12 lines: the outcome, the branch and pull request, the checks you ran and their result, and anything left open. No code, diffs or logs in it; it reads your thread if it needs more.";

fn error(code: &str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": code, "message": message.into() } })
}

/// A refusal that names the app, so an agent learns what to do next.
fn needs_app(what: &str) -> Value {
    error(
        "unavailable",
        format!(
            "{what} needs the RustCode app open to approve it; this thread runs on a machine whose app is not attached. Open the RustCode app and retry. Nothing was sent."
        ),
    )
}

fn failed(message: impl Into<String>) -> Value {
    error("failed", message)
}

fn unavailable(message: impl Into<String>) -> Value {
    error("unavailable", message)
}

fn not_allowed(message: impl Into<String>) -> Value {
    error("not_allowed", message)
}

// ----------------------------------------------------------------- policy

/// What this engine can tell about a thread's access without the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Ask,
    Edits,
    Full,
}

impl Access {
    fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Edits => "Auto-edit",
            Self::Full => "Full access",
        }
    }
}

/// The access a thread runs at, from the record the engine keeps: its own
/// `/yolo`, else Settings › General › Default access, which the app seeds into
/// `settings.json`. A per-thread mode the app narrowed the thread to is
/// app-only state the engine cannot read; `docs/orchestrator.md` says so.
fn access_of(session_id: &str) -> Access {
    let full = crate::config::load_session_yolo(session_id)
        .unwrap_or_else(|| crate::config::load_config().2.yolo);
    if full {
        return Access::Full;
    }
    match crate::acp::settings::value("default-permission-mode")
        .and_then(|value| value.as_str().map(str::to_owned))
    {
        Some(mode) if mode == "full" => Access::Full,
        Some(mode) if mode == "edits" || mode == "auto_edit" => Access::Edits,
        _ => Access::Ask,
    }
}

/// Whether a read of another thread would have shown the app's approval card:
/// only Ask does; Auto-edit and Full read without one.
pub(super) fn read_needs_approval(session_id: &str) -> bool {
    access_of(session_id) == Access::Ask
}

/// Delegating runs work somewhere the caller cannot see, so it needs Full
/// access. With no app attached there is no card to ask for anything less.
fn delegation(caller: &str, what: &str) -> Result<(), Value> {
    let access = access_of(caller);
    if access == Access::Full {
        return Ok(());
    }
    Err(error(
        "unavailable",
        format!(
            "{what} needs Full access, and this thread runs at {}; no app is attached to ask, so it needs the RustCode app open to approve it. Open the app, or set this thread's access to Full, and retry. Nothing was sent.",
            access.label()
        ),
    ))
}

/// Whether the caller is already as deep as delegation goes: a top-level
/// thread starts children, they start grandchildren, and a grandchild starts
/// none (the app's `MAX_DEPTH`).
fn chain_refusal(caller: &str) -> bool {
    let mut current = caller.to_owned();
    let mut depth = 0;
    while depth < MAX_DEPTH {
        let Some(parent) =
            crate::config::load_session_parent(&current).and_then(|parent| parent.id)
        else {
            return false;
        };
        depth += 1;
        current = parent;
    }
    depth >= MAX_DEPTH
}

// ------------------------------------------------------------ the calls

/// One thread this call starts.
struct Ask<'a> {
    prompt: &'a str,
    title: Option<&'a str>,
    project: Option<&'a str>,
    harness: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    account: Option<&'a str>,
    worktree: Option<bool>,
    standalone: bool,
    fan_out: Option<&'a str>,
}

fn ask_of(args: &Value) -> Ask<'_> {
    Ask {
        prompt: args.get("prompt").and_then(Value::as_str).unwrap_or(""),
        title: args.get("title").and_then(Value::as_str),
        project: args.get("project").and_then(Value::as_str),
        harness: args.get("harness").and_then(Value::as_str),
        model: args.get("model").and_then(Value::as_str),
        effort: args.get("effort").and_then(Value::as_str),
        account: args.get("account").and_then(Value::as_str),
        worktree: args.get("worktree").and_then(Value::as_bool),
        standalone: args.get("standalone").and_then(Value::as_bool) == Some(true),
        fan_out: None,
    }
}

/// `start_session`: one thread, its first turn running on its own from here.
pub(crate) async fn start(caller: &str, args: &Value) -> Value {
    if let Err(remote) = super::local::local_machine(args) {
        return remote;
    }
    if let Err(refusal) = delegation(caller, "start_session") {
        return refusal;
    }
    match start_one(caller, &ask_of(args)).await {
        Ok(answer) => answer,
        Err(refusal) => refusal,
    }
}

/// `fan_out`: the same task on two to four harnesses, one link each.
pub(crate) async fn fan_out(caller: &str, args: &Value) -> Value {
    if let Err(remote) = super::local::local_machine(args) {
        return remote;
    }
    if let Err(refusal) = delegation(caller, "fan_out") {
        return refusal;
    }
    if chain_refusal(caller) {
        return not_allowed(format!(
            "delegation goes {MAX_DEPTH} levels deep; this thread is at the bottom of one and cannot fan out"
        ));
    }
    let Some(entries) = args.get("harnesses").and_then(Value::as_array) else {
        return error("invalid", "harnesses is required");
    };
    let base = ask_of(args);
    if let Some(refusal) = slot_refusal(caller, entries.len() as u64) {
        return refusal;
    }
    let fan_id = fan_id();
    let title = title_of(&base);
    let mut out: Vec<Value> = Vec::new();
    let mut link_ids: Vec<String> = Vec::new();
    for entry in entries {
        let ask = Ask {
            prompt: base.prompt,
            title: base.title,
            project: base.project,
            harness: entry.get("harness").and_then(Value::as_str),
            model: entry.get("model").and_then(Value::as_str),
            effort: entry.get("effort").and_then(Value::as_str),
            account: entry.get("account").and_then(Value::as_str),
            worktree: base.worktree,
            standalone: base.standalone,
            fan_out: Some(&fan_id),
        };
        match start_one(caller, &ask).await {
            Ok(answer) => {
                if let Some(link) = answer.get("link_id").and_then(Value::as_str) {
                    link_ids.push(link.to_owned());
                }
                out.push(answer);
            }
            Err(refusal) => out.push(json!({
                "harness": ask.harness,
                "error": refusal.get("error").cloned().unwrap_or(refusal),
            })),
        }
    }
    if !base.standalone && !link_ids.is_empty() {
        record_fan_out(caller, &fan_id, &title, base.prompt, &link_ids);
    }
    if base.standalone {
        // A fan-out of the user's own threads: nothing links them or comes back.
        return json!({ "links": out, "standalone": true });
    }
    json!({
        "links": out,
        "fan_out_id": fan_id,
        "links_left": links_left(caller),
    })
}

fn title_of(ask: &Ask<'_>) -> String {
    ask.title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| title_from(ask.prompt, 60))
}

/// Start one thread and hand its first turn to [`deliver`].
async fn start_one(caller: &str, ask: &Ask<'_>) -> Result<Value, Value> {
    if chain_refusal(caller) {
        return Err(not_allowed(format!(
            "delegation goes {MAX_DEPTH} levels deep; this thread is at the bottom of one and cannot start another"
        )));
    }
    if !ask.standalone
        && let Some(refusal) = slot_refusal(caller, 1)
    {
        return Err(refusal);
    }
    let caller_meta = crate::config::session_meta_by_id(caller)
        .ok_or_else(|| failed("this thread's own record is missing"))?;
    let caller_title = crate::config::load_session_title(caller)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| caller_meta.title.clone());
    let project = project_for(caller, &caller_meta, ask.project)?;
    let harness = harness_for(ask.harness)?;
    let model = model_for(&harness, ask.model)?;
    let account = account_for(&harness, ask.account)?;
    let title = title_of(ask);

    // Everything the engine reads from `session/new` sits under `_meta.rustcode`
    // (`harness::requested`, `thread_parent::requested`, `worktrees::requested`).
    let mut meta = Map::new();
    meta.insert("harness".to_owned(), json!(harness.id));
    meta.insert("title".to_owned(), json!(title));
    // The engine tells the clients that list threads, since this one cannot.
    meta.insert("announce".to_owned(), json!(true));
    if let Some(model) = model.as_ref() {
        meta.insert("model".to_owned(), json!(model));
    }
    if let Some(effort) = ask
        .effort
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
    {
        meta.insert("effort".to_owned(), json!(effort));
    }
    if let Some(account) = account.as_ref() {
        meta.insert("account".to_owned(), json!(account));
    }
    if !ask.standalone {
        meta.insert("parent".to_owned(), json!(caller));
    }
    // The app starts a delegate in a worktree unless the caller says otherwise,
    // and in its folder when the project is not a git repository.
    let in_git = crate::checkpoints::available(std::path::Path::new(&project));
    match ask.worktree {
        Some(true) if !in_git => {
            return Err(not_allowed(format!(
                "{project} is not a git repository, so it has no worktrees to start one in; pass worktree false to run in the folder"
            )));
        }
        Some(false) => {}
        _ if in_git => {
            meta.insert(
                "worktree".to_owned(),
                json!({ "repo": project, "title": title }),
            );
        }
        _ => {}
    }

    let engine = Engine::open().await?;
    let mut request = NewSessionRequest::new(PathBuf::from(&project));
    request.meta = Some(Map::from_iter([(
        "rustcode".to_owned(),
        Value::Object(meta),
    )]));
    // The engine's own `session/new` handler makes the worktree and records it.
    let response = engine
        .request(request, CREATE_TIMEOUT)
        .await
        .map_err(|message| failed(format!("the thread could not start: {message}")))?;
    let session_id = response.session_id.to_string();

    let mut answer = json!({
        "session_id": session_id,
        "receipt": "started",
        "title": title,
        "harness": harness.id,
        "harness_name": harness.name,
        "access": "full",
    });
    if let Some(model) = model.as_ref() {
        answer["model"] = json!(model);
    }
    if let Some(account) = account.as_ref() {
        answer["account"] = json!(account);
    }
    if ask.standalone {
        // A thread of the user's own: its prompt is its first message, with no
        // "From thread" header and no delegate report to end with (issue 3443).
        deliver(engine, session_id, ask.prompt.to_owned(), None);
        answer["standalone"] = json!(true);
        return Ok(answer);
    }
    let link = match record_link(
        caller,
        &caller_title,
        &session_id,
        &title,
        ask.prompt,
        &harness.id,
        &harness.name,
        model.as_deref(),
        account.as_deref(),
        ask.fan_out,
        true,
    ) {
        Ok(link) => link,
        Err(reason) => {
            // The thread is real and its task is worth running even when the
            // link could not be written; the caller can still read it.
            deliver(
                engine,
                session_id.clone(),
                delegated(ask.prompt, &caller_title),
                None,
            );
            return Err(failed(format!(
                "the thread {session_id} was started, but its link could not be recorded ({reason}); read it with read_session"
            )));
        }
    };
    deliver(
        engine,
        session_id.clone(),
        delegated(ask.prompt, &caller_title),
        Some(link.clone()),
    );
    answer["link_id"] = json!(link);
    answer["links_left"] = json!(links_left(caller));
    Ok(answer)
}

/// `message_session`: deliver a message into a thread this engine can run.
pub(crate) async fn message(caller: &str, args: &Value) -> Value {
    if let Err(remote) = super::local::local_machine(args) {
        return remote;
    }
    let target = args
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if target == caller {
        return error("not_allowed", "a thread cannot message itself");
    }
    if args
        .get("attachments")
        .and_then(Value::as_array)
        .is_some_and(|paths| !paths.is_empty())
    {
        return needs_app("message_session with attachments");
    }
    if let Err(refusal) = delegation(caller, "message_session") {
        return refusal;
    }
    let Some(meta) = crate::config::session_meta_by_id(target) else {
        return error("not_found", "no thread has that id; use list_sessions");
    };
    if !super::local::may_reach(caller, target, &meta) {
        return not_allowed(
            "that thread is in another project, and one this thread did not start; messaging across projects is off. Turn on “Let agents read threads in other projects” in Settings › Harnesses, or open the RustCode app to approve it. Nothing was sent.",
        );
    }
    let caller_title = crate::config::load_session_title(caller).unwrap_or_default();
    let text = args.get("message").and_then(Value::as_str).unwrap_or("");

    // A thread open in another RustCode process must not be opened again here:
    // two processes on one history fork it. One open in this engine is safe.
    if !crate::acp::is_acp_session(target)
        && let Some(config_dir) = crate::config::get_config_dir()
        && crate::config::session_open_by(&config_dir, target).is_some()
    {
        return unavailable(format!(
            "that thread is open in another RustCode process, and this engine will not open a second copy of it; open the RustCode app, or close it there, and retry. Nothing was sent."
        ));
    }

    let engine = match Engine::open().await {
        Ok(engine) => engine,
        Err(refusal) => return refusal,
    };
    let cwd = meta
        .workspace_cwd
        .clone()
        .unwrap_or_else(|| PathBuf::from(project_of(target, &meta)));
    if let Err(message) = engine
        .request(
            LoadSessionRequest::new(target.to_owned(), cwd),
            LOAD_TIMEOUT,
        )
        .await
    {
        return unavailable(format!(
            "that thread could not be opened in this engine without the app ({message}); open the RustCode app and retry. Nothing was sent."
        ));
    }
    if args.get("steer").and_then(Value::as_bool) == Some(true)
        && engine.steer(target, text).await == Ok(true)
    {
        // A steer joins the turn already running: the app makes no new link
        // for it, and the one the caller already waits on settles with it.
        return json!({ "session_id": target, "steered": true });
    }
    if let Some(refusal) = slot_refusal(caller, 1) {
        return refusal;
    }
    let harness = crate::harness::store::load(target)
        .map(|record| record.harness)
        .unwrap_or_else(|| crate::harness::BUILTIN_ID.to_owned());
    let harness_name = crate::harness::display_name(&harness);
    let title = crate::config::load_session_title(target).unwrap_or_default();
    let link = match record_link(
        caller,
        &caller_title,
        target,
        &title,
        text,
        &harness,
        &harness_name,
        None,
        None,
        None,
        false,
    ) {
        Ok(link) => link,
        Err(reason) => return failed(format!("the message could not be recorded: {reason}")),
    };
    deliver(
        engine,
        target.to_owned(),
        delivered(text, &caller_title),
        Some(link.clone()),
    );
    let preview: String = text.chars().take(PREVIEW_CHARS).collect();
    json!({
        "session_id": target,
        "link_id": link,
        "preview": preview,
        "links_left": links_left(caller),
    })
}

/// Run `text` as the thread's next turn, in the background, and settle `link`
/// with what it answered.
fn deliver(engine: Engine, session_id: String, text: String, link: Option<String>) {
    tokio::spawn(async move {
        let outcome = engine.prompt(&session_id, &text).await;
        let Some(link) = link else {
            return;
        };
        settle(&link, &session_id, outcome);
    });
}

/// What a turn's end writes into its link.
fn settle(link: &str, target: &str, outcome: Result<StopReason, String>) {
    let at = now();
    let worked = worked_ms(link, at);
    let change = match outcome {
        Ok(StopReason::Cancelled) => json!({
            "state": "cancelled",
            "settledAt": at,
            "workedMs": worked,
        }),
        Ok(_) => {
            let mut change = json!({
                "state": "done",
                "settledAt": at,
                "workedMs": worked,
            });
            if let Some((text, truncated)) = super::local::last_reply_cut(target) {
                change["answer"] = json!(text);
                if truncated {
                    change["truncated"] = json!(true);
                }
            }
            change
        }
        Err(reason) => json!({
            "state": "failed",
            "error": reason,
            "settledAt": at,
            "workedMs": worked,
        }),
    };
    update_link(link, change);
}

fn worked_ms(link: &str, at: i64) -> i64 {
    let started = link_book()
        .get("links")
        .and_then(Value::as_array)
        .and_then(|links| {
            links
                .iter()
                .find(|row| row.get("id").and_then(Value::as_str) == Some(link))
        })
        .and_then(|row| row.get("startedAt").and_then(Value::as_i64))
        .unwrap_or(at);
    (at - started).max(0)
}

// ------------------------------------------------------------------ links

/// A link's `state`, one of the app's `LinkState`s.
fn is_active(state: &str) -> bool {
    matches!(state, "queued" | "working" | "needs_user")
}

/// The link book is read and written under one lock, so two turns that finish
/// at once cannot lose each other's link.
fn book_lock() -> &'static Mutex<()> {
    static LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    &LOCK
}

fn empty_book() -> Value {
    json!({ "links": [], "grants": {}, "counters": {}, "fanOuts": [] })
}

/// The app's book, read from the same store it reads.
fn link_book() -> Value {
    crate::data_store::load_link_book()
        .ok()
        .and_then(|stored| stored.get("book").cloned())
        .filter(Value::is_object)
        .unwrap_or_else(empty_book)
}

fn save_book(book: &Value) -> Result<(), String> {
    crate::data_store::save_link_book(book)
}

fn update_link(id: &str, change: Value) {
    let _guard = book_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut book = link_book();
    let Some(links) = book.get_mut("links").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(row) = links
        .iter_mut()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
    else {
        return;
    };
    *row = link_after(row.clone(), &change, now());
    if let Err(reason) = save_book(&book) {
        crate::dbg_log!("local orchestration: the link book was not saved: {reason}");
    }
}

/// One link with `change`, keeping a bounded list of events as the app does.
fn link_after(mut link: Value, change: &Value, at: i64) -> Value {
    let Some(change) = change.as_object() else {
        return link;
    };
    let before = link
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    for (key, value) in change {
        link[key] = value.clone();
    }
    let state = link
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let kind = match (before == state, state.as_str()) {
        (true, _) => None,
        (_, "queued") => Some("queued"),
        (_, "working") => Some("working"),
        (_, "needs_user") => Some("asking"),
        (_, "done") => Some("answered"),
        (_, "failed") => Some("failed"),
        (_, "cancelled") => Some("stopped"),
        _ => None,
    };
    if let Some(kind) = kind {
        note_event(&mut link, kind, at, None);
    }
    link
}

fn note_event(link: &mut Value, kind: &str, at: i64, detail: Option<String>) {
    let mut events = link
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if events.iter().any(|event| {
        event.get("kind").and_then(Value::as_str) == Some(kind)
            && event.get("at").and_then(Value::as_i64) == Some(at)
    }) {
        return;
    }
    let mut event = json!({ "kind": kind, "at": at });
    if let Some(detail) = detail {
        event["detail"] = json!(detail);
    }
    events.push(event);
    if events.len() > MAX_LINK_EVENTS {
        let drop = events.len() - MAX_LINK_EVENTS;
        events.drain(0..drop);
    }
    link["events"] = Value::Array(events);
}

/// Record a link to `target` in the app's book and answer its id.
#[allow(clippy::too_many_arguments)]
fn record_link(
    caller: &str,
    caller_title: &str,
    target: &str,
    target_title: &str,
    message: &str,
    harness: &str,
    harness_name: &str,
    model: Option<&str>,
    account: Option<&str>,
    fan_out: Option<&str>,
    started: bool,
) -> Result<String, String> {
    let _guard = book_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut book = link_book();
    let at = now();
    let counter = book
        .get("counters")
        .and_then(|counters| counters.get(caller))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + 1;
    book["counters"][caller] = json!(counter);
    let id = format!("{caller}/{counter}");
    let mut link = json!({
        "id": id,
        "orchestrator": caller,
        "target": target,
        "orchestratorTitle": caller_title,
        "targetTitle": target_title,
        "message": message,
        "state": "working",
        "started": started,
        "harness": harness,
        "harnessName": harness_name,
        "createdAt": at,
        "startedAt": at,
    });
    if let Some(model) = model {
        link["model"] = json!(model);
    }
    if let Some(account) = account {
        link["account"] = json!(account);
    }
    if let Some(fan_out) = fan_out {
        link["fanOut"] = json!(fan_out);
    }
    let meta = [harness_name, model.unwrap_or("")]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ");
    let kind = if started { "started" } else { "message" };
    note_event(&mut link, kind, at, (!meta.is_empty()).then_some(meta));
    note_event(&mut link, "working", at, None);
    let Some(links) = book.get_mut("links").and_then(Value::as_array_mut) else {
        return Err("the link book is not readable".to_owned());
    };
    links.push(link);
    save_book(&book)?;
    Ok(id)
}

fn record_fan_out(caller: &str, fan_id: &str, title: &str, prompt: &str, links: &[String]) {
    let _guard = book_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut book = link_book();
    let group = json!({
        "id": fan_id,
        "orchestrator": caller,
        "title": title,
        "prompt": prompt,
        "links": links,
        "createdAt": now(),
    });
    match book.get_mut("fanOuts").and_then(Value::as_array_mut) {
        Some(groups) => groups.push(group),
        None => book["fanOuts"] = json!([group]),
    }
    if let Err(reason) = save_book(&book) {
        crate::dbg_log!("local orchestration: the fan-out was not saved: {reason}");
    }
}

/// The app's limits: one thread's delegates, and the machine's total.
fn slot_limits() -> (u64, u64) {
    let read = |key: &str, fallback: u64| {
        crate::acp::settings::value(key)
            .and_then(|value| value.as_u64())
            .unwrap_or(fallback)
    };
    (
        read("delegates-per-thread", DEFAULT_PER_THREAD).max(1),
        read("delegates-total", DEFAULT_TOTAL).max(1),
    )
}

fn active_links(caller: &str) -> (u64, u64) {
    let book = link_book();
    let links = book
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut mine = 0;
    let mut all = 0;
    for link in links {
        let state = link.get("state").and_then(Value::as_str).unwrap_or("");
        if !is_active(state) {
            continue;
        }
        all += 1;
        if link.get("orchestrator").and_then(Value::as_str) == Some(caller) {
            mine += 1;
        }
    }
    (mine, all)
}

fn links_left(caller: &str) -> u64 {
    let (per_thread, total) = slot_limits();
    let (mine, all) = active_links(caller);
    per_thread
        .saturating_sub(mine)
        .min(total.saturating_sub(all))
}

/// Refuse a call that would open more delegates than the settings allow,
/// naming how many are left so the answer is actionable.
fn slot_refusal(caller: &str, count: u64) -> Option<Value> {
    let left = links_left(caller);
    if left >= count {
        return None;
    }
    let (per_thread, total) = slot_limits();
    Some(json!({
        "error": {
            "code": "busy",
            "message": format!(
                "this thread can open {left} more delegate(s) right now (Settings › Harnesses: {per_thread} per thread, {total} at once); wait for one to finish, or ask the user to raise the limit. Nothing was sent."
            ),
            "links_left": left,
        }
    }))
}

fn fan_id() -> String {
    format!("fan-{}-{}", now(), std::process::id())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// --------------------------------------------------------------- resolving

/// The project a thread starts in: the caller's, or one the user has opened
/// before, as the app checks it.
fn project_for(
    caller: &str,
    meta: &crate::config::SessionMeta,
    asked: Option<&str>,
) -> Result<String, Value> {
    let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) else {
        return Ok(project_of(caller, meta));
    };
    let asked = clean_path(asked);
    if asked == project_of(caller, meta) || asked == reach_of(caller, meta) {
        return Ok(asked);
    }
    let known = crate::config::list_sessions().iter().any(|row| {
        let Some(id) = crate::config::session_id_from_path(&row.path) else {
            return false;
        };
        asked == project_of(&id, row) || asked == reach_of(&id, row)
    });
    if !known {
        return Err(not_allowed(
            "that folder is not a project the user has opened in RustCode; open it in the app first",
        ));
    }
    if !std::path::Path::new(&asked).is_dir() {
        return Err(not_allowed(format!("{asked} is not a folder")));
    }
    Ok(asked)
}

fn harness_for(asked: Option<&str>) -> Result<crate::harness::ResolvedHarness, Value> {
    let dir = crate::harness::config::config_dir()
        .map_err(|error| unavailable(format!("harness configuration is unavailable: {error}")))?;
    let (defaults, entries) = crate::harness::config::load_in(&dir);
    let all = crate::harness::resolve_all(&entries);
    let id = asked
        .map(str::trim)
        .filter(|asked| !asked.is_empty())
        .map(str::to_owned)
        .or_else(|| defaults.harness.clone())
        .unwrap_or_else(|| crate::harness::BUILTIN_ID.to_owned());
    let resolved = all
        .iter()
        .find(|harness| harness.id.eq_ignore_ascii_case(&id))
        .or_else(|| {
            all.iter()
                .find(|harness| harness.name.eq_ignore_ascii_case(&id))
        })
        .cloned();
    let Some(resolved) = resolved else {
        let known: Vec<&str> = all.iter().map(|harness| harness.id.as_str()).collect();
        return Err(error(
            "invalid",
            format!("no harness “{id}”; this engine knows {}", known.join(", ")),
        ));
    };
    if !resolved.enabled {
        let usable: Vec<&str> = all
            .iter()
            .filter(|harness| harness.enabled)
            .map(|harness| harness.id.as_str())
            .collect();
        return Err(not_allowed(format!(
            "{} cannot run a thread: it is turned off in Settings › Harnesses. Turn it on, or name one that can run now: {}",
            resolved.name,
            usable.join(", ")
        )));
    }
    Ok(resolved)
}

fn model_for(
    harness: &crate::harness::ResolvedHarness,
    asked: Option<&str>,
) -> Result<Option<String>, Value> {
    let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) else {
        return Ok(None);
    };
    let listed = if harness.kind == crate::harness::HarnessKind::Builtin {
        crate::harness::builtin_models()
    } else {
        crate::harness::learned_for(&harness.id)
            .models
            .unwrap_or_default()
    };
    // A harness that has never listed its models takes the id and answers for
    // itself when the thread starts.
    if listed.is_empty() {
        return Ok(Some(asked.to_owned()));
    }
    let wanted = asked.to_lowercase();
    if let Some(exact) = listed
        .iter()
        .find(|model| model.id.to_lowercase() == wanted || model.name.to_lowercase() == wanted)
    {
        return Ok(Some(exact.id.clone()));
    }
    // "deepseek-v4-pro" for "deepseek/deepseek-v4-pro": a provider prefix left
    // off, when only one model fits.
    let suffixed: Vec<&crate::harness::HarnessModel> = listed
        .iter()
        .filter(|model| model.id.to_lowercase().ends_with(&format!("/{wanted}")))
        .collect();
    if let [only] = suffixed.as_slice() {
        return Ok(Some(only.id.clone()));
    }
    let close: Vec<&str> = listed
        .iter()
        .filter(|model| {
            let tail = model
                .id
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_lowercase();
            model.id.to_lowercase().contains(&wanted) || wanted.contains(&tail)
        })
        .take(5)
        .map(|model| model.id.as_str())
        .collect();
    let offer = if close.is_empty() {
        listed
            .iter()
            .take(8)
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        format!("closest: {}", close.join(", "))
    };
    Err(error(
        "invalid",
        format!("{} has no model “{asked}”; {offer}", harness.name),
    ))
}

fn account_for(
    harness: &crate::harness::ResolvedHarness,
    asked: Option<&str>,
) -> Result<Option<String>, Value> {
    let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) else {
        return Ok(None);
    };
    let dir = crate::harness::config::config_dir()
        .map_err(|error| unavailable(format!("harness configuration is unavailable: {error}")))?;
    let accounts = crate::harness::config::load_accounts_in(&dir);
    let account = crate::harness::config::find_account(&accounts, &harness.id, asked).map_err(
        |message| {
            let known: Vec<String> = crate::harness::config::accounts_of(&accounts, &harness.id)
                .into_iter()
                .map(|account| account.id)
                .collect();
            if known.is_empty() {
                error(
                    "invalid",
                    format!(
                        "{} has no named accounts; it runs on its own sign-in, so leave account out",
                        harness.name
                    ),
                )
            } else {
                error(
                    "invalid",
                    format!("{message}; its named accounts are {}", known.join(", ")),
                )
            }
        },
    )?;
    Ok(Some(account.id))
}

// ------------------------------------------------------------- the endpoint

/// The local endpoint this very process published: the engine that runs the
/// caller, never another one, so a new thread belongs to the same engine.
fn endpoint() -> Result<String, Value> {
    let run_dir = crate::server::run_dir().map_err(|error| {
        unavailable(format!(
            "this engine has no run directory to publish a local endpoint in ({error}), so it cannot start a thread by itself"
        ))
    })?;
    let pid = std::process::id();
    if let Some(info) = crate::server::engine_endpoint::published(&run_dir)
        && info.pid == pid
    {
        return Ok(info.endpoint);
    }
    if let Some(info) = crate::server::running()
        && info.pid == pid
    {
        return Ok(info.endpoint);
    }
    Err(unavailable(
        "this engine did not publish a local endpoint, so it cannot open a thread by itself without the app; open the RustCode app and retry. Nothing was sent.",
    ))
}

/// A connection to this engine's own ACP endpoint: what the app would use.
struct Engine {
    connection: ConnectionTo<Agent>,
    close: Option<oneshot::Sender<()>>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        }
    }
}

impl Engine {
    async fn open() -> Result<Self, Value> {
        let endpoint = endpoint()?;
        let stream =
            tokio::time::timeout(CONNECT_TIMEOUT, crate::server::listen::connect(&endpoint))
                .await
                .map_err(|_| {
                    unavailable(format!(
                        "the engine's local endpoint {endpoint} did not answer in time; open the RustCode app and retry"
                    ))
                })?
                .map_err(|error| {
                    unavailable(format!(
                        "the engine's local endpoint {endpoint} could not be opened ({error}); open the RustCode app and retry"
                    ))
                })?;
        let (read, write) = tokio::io::split(stream);
        let transport = crate::acp::lines_transport(read, write);
        let (ready, ready_rx) = oneshot::channel();
        let (close, close_rx) = oneshot::channel();
        tokio::spawn(async move {
            let served = Client
                .builder()
                .on_receive_request(
                    async |request: RequestPermissionRequest, responder, _connection| {
                        responder.respond(answer_permission(&request))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async |_request: agent_client_protocol::UntypedMessage,
                           responder,
                           _connection| { responder.respond(json!({})) },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |connection| {
                    let _ = ready.send(connection);
                    let _ = close_rx.await;
                    Ok(())
                })
                .await;
            if let Err(error) = served {
                crate::dbg_log!("local orchestration: the loopback client ended: {error}");
            }
        });
        let connection = tokio::time::timeout(CONNECT_TIMEOUT, ready_rx)
            .await
            .map_err(|_| {
                unavailable("the engine's local endpoint did not complete a client handshake")
            })?
            .map_err(|_| unavailable("the engine's local endpoint closed while connecting"))?;
        let engine = Self {
            connection,
            close: Some(close),
        };
        engine
            .request(
                InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
                    .client_capabilities(ClientCapabilities::default()),
                CONNECT_TIMEOUT,
            )
            .await
            .map_err(|message| {
                unavailable(format!(
                    "the engine's local endpoint did not initialize: {message}"
                ))
            })?;
        Ok(engine)
    }

    /// Send a request and wait at most `wait` for its answer.
    async fn request<R>(&self, request: R, wait: Duration) -> Result<R::Response, String>
    where
        R: agent_client_protocol::JsonRpcRequest,
    {
        match tokio::time::timeout(wait, self.connection.send_request(request).block_task()).await {
            Err(_) => Err("the engine did not answer in time".to_owned()),
            Ok(Err(error)) => Err(acp_message(error)),
            Ok(Ok(response)) => Ok(response),
        }
    }

    /// `_rustcode/steer`: whether the running turn took the message.
    async fn steer(&self, session_id: &str, text: &str) -> Result<bool, String> {
        let request = crate::acp::steer::SteerRequest {
            session_id: session_id.to_owned(),
            text: text.to_owned(),
            prompt: None,
        };
        let answer = self.request(request, CONNECT_TIMEOUT).await?;
        Ok(answer
            .get("accepted")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// Run one turn on `session_id` and answer when it ends. The turn belongs
    /// to the engine, so a dropped connection ends only this wait.
    async fn prompt(&self, session_id: &str, text: &str) -> Result<StopReason, String> {
        let request = PromptRequest::new(
            session_id.to_owned(),
            vec![ContentBlock::Text(TextContent::new(text))],
        )
        .meta(Map::from_iter([(
            "rustcode".to_owned(),
            json!({ "access": "full" }),
        )]));
        match self.connection.send_request(request).block_task().await {
            Ok(response) => Ok(response.stop_reason),
            Err(error) => Err(acp_message(error)),
        }
    }
}

/// The answer to a permission request as Full access gives it: allow, and
/// name the option. A question (`ask_question`) is the model asking the user,
/// so it is cancelled instead of answered in the user's place.
fn answer_permission(request: &RequestPermissionRequest) -> RequestPermissionResponse {
    use agent_client_protocol::schema::v1::PermissionOptionKind;
    let is_question = request
        .meta
        .as_ref()
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("kind"))
        .and_then(Value::as_str)
        == Some("question");
    if is_question {
        return RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
    }
    let option = request
        .options
        .iter()
        .find(|option| option.kind == PermissionOptionKind::AllowAlways)
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| option.kind == PermissionOptionKind::AllowOnce)
        });
    match option {
        Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option.option_id.clone()),
        )),
        None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
    }
}

/// An ACP error's detail, which is what the engine actually said.
fn acp_message(error: agent_client_protocol::Error) -> String {
    error
        .data
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or(error.message)
}

// ---------------------------------------------------------------- the words

/// The message as the other thread receives it: who it is from, then the text.
fn delivered(text: &str, from: &str) -> String {
    format!("From thread “{from}” (agent):\n\n{text}")
}

/// A task as a new delegate receives it: attributed, with the report it ends with.
fn delegated(text: &str, from: &str) -> String {
    format!("{}\n\n{DELEGATE_REPORT}", delivered(text, from))
}

/// A title from a message's first line that says something, cut at a word and
/// marked with "…" as the app's `titleFrom` cuts it.
fn title_from(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let cut: String = line.chars().take(max - 1).collect();
    let trimmed = match cut.rfind(' ') {
        Some(at) if at > cut.len() / 2 => cut[..at].trim_end(),
        _ => cut.trim_end(),
    };
    format!("{trimmed}…")
}

/// A thread with a saved history and folder, which the store lists and
/// `session_meta_by_id` finds (a bare detached session has no row yet).
#[cfg(test)]
pub(super) fn test_thread(dir: &std::path::Path) -> String {
    let id = crate::config::create_detached_session();
    let _ = crate::config::save_session_workspace(
        &id,
        &rustcode_session::SessionWorkspace {
            cwd: dir.to_path_buf(),
            additional_directories: Vec::new(),
            task_workspace_id: None,
        },
    );
    crate::config::save_session_history(
        &id,
        &vec![crate::app::ChatMessage::new("user", "a test thread")],
    );
    crate::config::flush_history();
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_setting(key: &str, value: Value) {
        let mut values = serde_json::Map::new();
        values.insert(key.to_owned(), value);
        crate::acp::settings::write(crate::acp::settings::SetSettingsRequest { values })
            .expect("the test setting is written");
    }

    fn caller_with(access: Access) -> String {
        let caller = crate::config::create_detached_session();
        match access {
            Access::Full => crate::config::save_session_yolo(&caller, true),
            Access::Ask | Access::Edits => crate::config::save_session_yolo(&caller, false),
        }
        if access == Access::Edits {
            set_setting("default-permission-mode", json!("edits"));
        }
        caller
    }

    #[tokio::test]
    async fn a_thread_without_full_access_is_refused_and_nothing_starts() {
        let caller = caller_with(Access::Ask);
        set_setting("default-permission-mode", json!("ask"));
        for (tool, args) in [
            (super::super::START_SESSION, json!({"prompt": "go"})),
            (
                super::super::MESSAGE_SESSION,
                json!({"session_id": "thread-b", "message": "go"}),
            ),
            (
                super::super::FAN_OUT,
                json!({"prompt": "go", "harnesses": [{"harness": "codex"}, {"harness": "pi"}]}),
            ),
        ] {
            let answer = super::super::local::answer_json(
                &caller,
                &tokio_util::sync::CancellationToken::new(),
                tool,
                &args,
            )
            .await;
            let message = answer["error"]["message"].as_str().unwrap_or("");
            assert!(message.contains("Full access"), "{tool}: {message}");
            assert!(message.contains("RustCode app open"), "{tool}: {message}");
            assert!(answer["session_id"].is_null(), "{tool}: {answer}");
            assert!(answer["link_id"].is_null(), "{tool}: {answer}");
        }
        let _ =
            crate::config::get_active_session_dir(&caller).map(|dir| std::fs::remove_dir_all(dir));
    }

    #[tokio::test]
    async fn delegating_on_another_machine_is_refused_by_name() {
        let caller = caller_with(Access::Full);
        let answer = super::super::local::answer_json(
            &caller,
            &tokio_util::sync::CancellationToken::new(),
            super::super::START_SESSION,
            &json!({"prompt": "go", "machine": "studio"}),
        )
        .await;
        assert_eq!(answer["error"]["code"], json!("unavailable"), "{answer}");
        let message = answer["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains("studio"), "{message}");
        let _ =
            crate::config::get_active_session_dir(&caller).map(|dir| std::fs::remove_dir_all(dir));
    }

    #[test]
    fn a_link_is_recorded_in_the_apps_book_and_settles_with_an_answer() {
        let book = empty_book();
        assert_eq!(book["links"].as_array().map(Vec::len), Some(0));

        let link = json!({
            "id": "caller/1",
            "orchestrator": "caller",
            "target": "delegate",
            "state": "working",
            "startedAt": 1000,
        });
        let done = link_after(
            link.clone(),
            &json!({"state": "done", "answer": "the task is done", "settledAt": 5000}),
            5000,
        );
        assert_eq!(done["state"], json!("done"));
        assert_eq!(done["answer"], json!("the task is done"));
        let events = done["events"].as_array().cloned().unwrap_or_default();
        assert_eq!(events.last().unwrap()["kind"], json!("answered"));

        // A state set again is not news, and the event list is bounded.
        let same = link_after(done.clone(), &json!({"state": "done"}), 6000);
        assert_eq!(
            same["events"].as_array().map(Vec::len),
            done["events"].as_array().map(Vec::len)
        );
        let mut busy = link;
        for at in 0..40 {
            note_event(&mut busy, "working", at, None);
        }
        assert_eq!(
            busy["events"].as_array().map(Vec::len),
            Some(MAX_LINK_EVENTS)
        );
    }

    fn row(id: &str) -> Value {
        link_book()
            .get("links")
            .and_then(Value::as_array)
            .and_then(|links| {
                links
                    .iter()
                    .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
                    .cloned()
            })
            .unwrap_or(Value::Null)
    }

    #[test]
    fn a_link_the_engine_records_settles_when_the_turn_ends_with_no_app() {
        let caller = format!("caller-{}-{}", std::process::id(), now());
        let mut ids = Vec::new();
        for (n, outcome, state) in [
            (0, Ok(StopReason::EndTurn), "done"),
            (1, Ok(StopReason::Cancelled), "cancelled"),
            (2, Err("the harness exited".to_owned()), "failed"),
        ] {
            let target = format!("delegate-{n}");
            let id = record_link(
                &caller,
                "Caller",
                &target,
                "task",
                "do it",
                "rustcode",
                "RustCode",
                None,
                None,
                Some("fan-1"),
                true,
            )
            .expect("the link is recorded");
            assert_eq!(id, format!("{caller}/{}", n + 1));
            let working = row(&id);
            assert_eq!(working["state"], json!("working"), "{working}");
            assert_eq!(working["orchestrator"], json!(caller), "{working}");
            assert_eq!(working["target"], json!(target), "{working}");
            assert_eq!(working["started"], json!(true), "{working}");
            assert_eq!(working["fanOut"], json!("fan-1"), "{working}");
            assert!(is_active("working"));

            settle(&id, &target, outcome);
            let settled = row(&id);
            assert_eq!(settled["state"], json!(state), "{settled}");
            assert!(settled["settledAt"].is_i64(), "{settled}");
            assert!(!is_active(state));
            ids.push(id);
        }
        assert_eq!(
            row(&ids[2])["error"],
            json!("the harness exited"),
            "a failed turn keeps its reason"
        );
        // A settled link no longer takes a slot, and a message is not a start.
        assert_eq!(active_links(&caller).0, 0);
        let message = record_link(
            &caller,
            "Caller",
            "delegate-0",
            "",
            "again",
            "rustcode",
            "RustCode",
            None,
            None,
            None,
            false,
        )
        .expect("a message link is recorded");
        assert_eq!(row(&message)["started"], json!(false));
        assert_eq!(active_links(&caller).0, 1);
        settle(&message, "delegate-0", Ok(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn a_message_reaches_only_a_thread_the_caller_may_reach() {
        let mine = tempfile::tempdir().unwrap();
        let theirs = tempfile::tempdir().unwrap();
        let caller = test_thread(mine.path());
        crate::config::save_session_yolo(&caller, true);
        let other = test_thread(theirs.path());
        set_setting("read-across-projects", json!(false));
        let before = link_book()["links"].as_array().map(Vec::len);
        let refused = message(&caller, &json!({"session_id": other, "message": "hi"})).await;
        assert_eq!(refused["error"]["code"], json!("not_allowed"), "{refused}");
        assert!(refused["link_id"].is_null(), "{refused}");
        assert_eq!(link_book()["links"].as_array().map(Vec::len), before);

        // A thread cannot message itself, and attachments need the app.
        let itself = message(&caller, &json!({"session_id": caller, "message": "hi"})).await;
        assert_eq!(itself["error"]["code"], json!("not_allowed"), "{itself}");
        let attached = message(
            &caller,
            &json!({"session_id": other, "message": "hi", "attachments": ["/tmp/x"]}),
        )
        .await;
        assert!(
            attached["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("RustCode app open"),
            "{attached}"
        );
        set_setting("read-across-projects", json!(true));
        for id in [&caller, &other] {
            let _ =
                crate::config::get_active_session_dir(id).map(|dir| std::fs::remove_dir_all(dir));
        }
    }

    #[tokio::test]
    async fn a_start_in_a_folder_that_is_not_a_project_is_refused_before_anything_starts() {
        let dir = tempfile::tempdir().unwrap();
        let caller = test_thread(dir.path());
        crate::config::save_session_yolo(&caller, true);
        let answer = start(
            &caller,
            &json!({"prompt": "go", "project": "/nowhere/at/all"}),
        )
        .await;
        assert_eq!(answer["error"]["code"], json!("not_allowed"), "{answer}");
        assert!(answer["session_id"].is_null(), "{answer}");
        let _ =
            crate::config::get_active_session_dir(&caller).map(|dir| std::fs::remove_dir_all(dir));
    }

    #[test]
    fn a_finished_link_is_not_counted_against_the_slots() {
        let book = json!({
            "links": [
                {"id": "a/1", "orchestrator": "a", "state": "working"},
                {"id": "a/2", "orchestrator": "a", "state": "done"},
                {"id": "b/1", "orchestrator": "b", "state": "queued"},
            ],
        });
        let links = book["links"].as_array().cloned().unwrap_or_default();
        let active: Vec<&str> = links
            .iter()
            .filter(|link| is_active(link["state"].as_str().unwrap_or("")))
            .map(|link| link["id"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(active, ["a/1", "b/1"]);
    }

    #[tokio::test]
    async fn access_is_read_from_what_the_engine_can_see() {
        let caller = crate::config::create_detached_session();
        crate::config::save_session_yolo(&caller, true);
        assert_eq!(access_of(&caller), Access::Full);
        assert!(!read_needs_approval(&caller));

        crate::config::save_session_yolo(&caller, false);
        set_setting("default-permission-mode", json!("edits"));
        assert_eq!(access_of(&caller), Access::Edits);
        assert!(!read_needs_approval(&caller));

        set_setting("default-permission-mode", json!("ask"));
        assert_eq!(access_of(&caller), Access::Ask);
        assert!(read_needs_approval(&caller));
        let _ =
            crate::config::get_active_session_dir(&caller).map(|dir| std::fs::remove_dir_all(dir));
    }

    #[test]
    fn a_title_stops_at_a_word() {
        assert_eq!(
            title_from("  fix the flaky test\nand more", 60),
            "fix the flaky test"
        );
        assert_eq!(
            title_from("a very long title that keeps going", 12),
            "a very…"
        );
        assert_eq!(title_from("short", 60), "short");
    }

    #[test]
    fn a_delegate_reads_who_sent_the_task_and_what_to_report() {
        let text = delegated("do it", "Parent");
        assert!(text.starts_with("From thread “Parent” (agent):\n\ndo it"));
        assert!(text.ends_with(DELEGATE_REPORT));
        assert!(!delivered("do it", "Parent").contains(DELEGATE_REPORT));
    }

    #[test]
    fn a_question_is_not_answered_for_the_user() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
        };
        let options = vec![
            PermissionOption::new("1", "yes", PermissionOptionKind::AllowOnce),
            PermissionOption::new("2", "no", PermissionOptionKind::RejectOnce),
        ];
        let request = RequestPermissionRequest::new(
            "s",
            ToolCallUpdate::new("t", ToolCallUpdateFields::new()),
            options,
        );
        let question = request.clone().meta(Map::from_iter([(
            "rustcode".to_owned(),
            json!({ "kind": "question" }),
        )]));
        assert_eq!(
            answer_permission(&question).outcome,
            RequestPermissionOutcome::Cancelled
        );
        assert!(matches!(
            answer_permission(&request).outcome,
            RequestPermissionOutcome::Selected(_)
        ));
    }
}
