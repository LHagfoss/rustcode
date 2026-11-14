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
use std::sync::LazyLock;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, StopReason, TextContent,
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

/// The calls that open a link run one at a time, as the app's `withLinkGate`
/// does: each checks the slots and only then records its link, so two calls
/// arriving together would both see the same free slot.
fn link_gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
    &GATE
}

/// Slots a call has checked and not yet turned into links, by caller. Counted
/// with the book's active links, so calls that start threads side by side
/// cannot overshoot a limit while the threads are still starting.
fn reservations() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static RESERVED: LazyLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        LazyLock::new(Default::default);
    &RESERVED
}

fn reserved(caller: &str) -> (u64, u64) {
    let held = reservations()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    (
        held.get(caller).copied().unwrap_or(0),
        held.values().sum::<u64>(),
    )
}

/// `count` slots held for `caller` until each becomes a link or the call ends.
#[derive(Debug)]
struct Reservation {
    caller: String,
    left: std::sync::Mutex<u64>,
}

impl Reservation {
    fn release_one(&self) {
        let mut left = self.left.lock().unwrap_or_else(|error| error.into_inner());
        if *left > 0 {
            *left -= 1;
            self.give_back(1);
        }
    }

    fn give_back(&self, count: u64) {
        let mut held = reservations()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(mine) = held.get_mut(&self.caller) {
            *mine = mine.saturating_sub(count);
            if *mine == 0 {
                held.remove(&self.caller);
            }
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let left = std::mem::take(
            self.left
                .get_mut()
                .unwrap_or_else(|error| error.into_inner()),
        );
        if left > 0 {
            self.give_back(left);
        }
    }
}

/// Check that `count` more delegates fit and hold their slots. Only this check
/// runs under the gate; starting the threads does not.
async fn reserve(caller: &str, count: u64) -> Result<Reservation, Value> {
    let _gate = link_gate().lock().await;
    if let Some(refusal) = slot_refusal(caller, count) {
        return Err(refusal);
    }
    reservations()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(caller.to_owned())
        .and_modify(|held| *held += count)
        .or_insert(count);
    Ok(Reservation {
        caller: caller.to_owned(),
        left: std::sync::Mutex::new(count),
    })
}

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
    /// The word a prompt's `_meta.rustcode.access` carries.
    fn meta(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edits => "edits",
            Self::Full => "full",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Edits => "Auto-edit",
            Self::Full => "Full access",
        }
    }
}

/// The access a thread runs at, from the records the engine keeps: the access
/// of its last prompt (`config::save_session_access`), else its own `/yolo`,
/// else Settings › General › Default access, which the app seeds into
/// `settings.json`. A thread no client has prompted since the engine began
/// keeping the access falls back to the last two, which is the gap
/// `docs/orchestrator.md` names.
fn access_of(session_id: &str) -> Access {
    // An outside agent (`outside.rs`) has no thread: its grant says what it
    // may do. Delegating needs Full here, which a delegate or manage grant is
    // the owner's approval for; a read-only grant reads and nothing more.
    if super::outside::is_caller(session_id) {
        return match super::outside::scope_of_caller(session_id) {
            Some(super::outside::Scope::ReadOnly) => Access::Edits,
            Some(_) => Access::Full,
            None => Access::Ask,
        };
    }
    // What a client last ran the thread at: the app sends it with every
    // prompt and the engine keeps it, so a thread narrowed to Ask under a Full
    // default reads as Ask here too.
    if let Some(saved) = crate::config::load_session_access(session_id) {
        return match saved.as_str() {
            "full" => Access::Full,
            "edits" => Access::Edits,
            _ => Access::Ask,
        };
    }
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

/// The links in the book that point at `target`, from any thread but `caller`,
/// and the threads above `caller`: the app's `checkMessage` rules. A thread
/// another one is working with is not messaged from a second one, and a thread
/// that already works for the caller's own ancestors is not messaged by the
/// ancestor's delegate (the loop).
fn loop_or_linked(
    caller: &str,
    target: &str,
    target_meta: &crate::config::SessionMeta,
) -> Option<Value> {
    let book = link_book();
    let links = book
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let field = |link: &Value, key: &str| link.get(key).and_then(Value::as_str).map(str::to_owned);
    // Everything above the caller: who started it, by the link book and by the
    // thread's own record.
    let mut above: Vec<String> = Vec::new();
    let mut frontier = vec![caller.to_owned()];
    while let Some(current) = frontier.pop() {
        let mut parents: Vec<String> = links
            .iter()
            .filter(|link| field(link, "target").as_deref() == Some(current.as_str()))
            .filter_map(|link| field(link, "orchestrator"))
            .collect();
        if let Some(parent) =
            crate::config::load_session_parent(&current).and_then(|parent| parent.id)
        {
            parents.push(parent);
        }
        for parent in parents {
            if !above.contains(&parent) && above.len() < 64 {
                above.push(parent.clone());
                frontier.push(parent);
            }
        }
    }
    if above.iter().any(|ancestor| ancestor == target) {
        let name = crate::config::load_session_title(target)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| target_meta.title.clone());
        return Some(not_allowed(format!(
            "“{name}” already works for this thread: messaging it would close a loop"
        )));
    }
    let linked = links.iter().any(|link| {
        field(link, "target").as_deref() == Some(target)
            && field(link, "orchestrator").as_deref() != Some(caller)
            && is_active(link.get("state").and_then(Value::as_str).unwrap_or(""))
    });
    linked.then(|| {
        error(
            "target_linked",
            "another thread is already working with that one",
        )
    })
}

/// Whether `caller` has a live link to `target` already.
fn has_active_link(caller: &str, target: &str) -> bool {
    link_book()
        .get("links")
        .and_then(Value::as_array)
        .is_some_and(|links| {
            links.iter().any(|link| {
                link.get("orchestrator").and_then(Value::as_str) == Some(caller)
                    && link.get("target").and_then(Value::as_str) == Some(target)
                    && is_active(link.get("state").and_then(Value::as_str).unwrap_or(""))
            })
        })
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
    let ask = ask_of(args);
    // The slot is checked and held under the gate, which is let go before the
    // thread starts: a harness that takes minutes to start blocks only itself.
    let reservation = if ask.standalone {
        None
    } else {
        match reserve(caller, 1).await {
            Ok(reservation) => Some(reservation),
            Err(refusal) => return refusal,
        }
    };
    match start_one(caller, &ask, reservation.as_ref()).await {
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
    if !(super::MIN_FAN_OUT..=super::MAX_FAN_OUT).contains(&entries.len()) {
        return error(
            "invalid",
            format!(
                "fan_out takes {} to {} harnesses, and this call names {}; use start_session for one",
                super::MIN_FAN_OUT,
                super::MAX_FAN_OUT,
                entries.len()
            ),
        );
    }
    let base = ask_of(args);
    let reservation = if base.standalone {
        None
    } else {
        match reserve(caller, entries.len() as u64).await {
            Ok(reservation) => Some(reservation),
            Err(refusal) => return refusal,
        }
    };
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
        match start_one(caller, &ask, reservation.as_ref()).await {
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
async fn start_one(
    caller: &str,
    ask: &Ask<'_>,
    reservation: Option<&Reservation>,
) -> Result<Value, Value> {
    if chain_refusal(caller) {
        return Err(not_allowed(format!(
            "delegation goes {MAX_DEPTH} levels deep; this thread is at the bottom of one and cannot start another"
        )));
    }
    // An outside agent has no thread record: it starts standalone threads in a
    // project it names.
    let outside = super::outside::is_caller(caller);
    let caller_meta = crate::config::session_meta_by_id(caller);
    if caller_meta.is_none() && !(outside && ask.standalone) {
        return Err(failed("this thread's own record is missing"));
    }
    let caller_title = match caller_meta.as_ref() {
        Some(meta) => crate::config::load_session_title(caller)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| meta.title.clone()),
        None => super::outside::name_of_caller(caller).unwrap_or_default(),
    };
    let project = project_for(caller, caller_meta.as_ref(), ask.project)?;
    let harness = harness_for(ask.harness)?;
    let model = model_for(&harness, ask.model)?;
    let account = account_for(&harness, ask.account)?;
    let account_dir = crate::harness::config::config_dir()
        .map_err(|error| unavailable(format!("harness configuration is unavailable: {error}")))?;
    let has_accounts = !crate::harness::config::accounts_of(
        &crate::harness::config::load_accounts_in(&account_dir),
        &harness.id,
    )
    .is_empty();
    if has_accounts {
        authorize_account(&link_book(), caller, &harness.id, account.as_deref())?;
    }
    let title = title_of(ask);

    // Everything the engine reads from `session/new` sits under `_meta.rustcode`
    // (`harness::requested`, `thread_parent::requested`, `worktrees::requested`).
    let mut meta = Map::new();
    meta.insert("harness".to_owned(), json!(harness.id));
    meta.insert("title".to_owned(), json!(title));
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

    // The work an outside agent starts runs at most at the ceiling its grant
    // sets: Auto-edit for a delegate grant, so a command that needs approval is
    // declined and nobody is asked, Full only for a manage grant.
    let thread_access = if outside {
        match super::outside::scope_of_caller(caller) {
            Some(super::outside::Scope::Manage) => Access::Full,
            _ => Access::Edits,
        }
    } else {
        Access::Full
    };
    let engine = Engine::open(thread_access).await?;
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
        "access": thread_access.meta(),
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
    // The link takes the slot from here on, so the reservation lets it go.
    if let Some(reservation) = reservation {
        reservation.release_one();
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
    // The message runs a turn on the target at the target's own access, never
    // above it. At Ask nobody is here to approve its tools.
    let target_access = access_of(target);
    if target_access == Access::Ask {
        return needs_app(
            "message_session to a thread that runs at Ask (its turn would stop at every tool)",
        );
    }
    if chain_refusal(caller) {
        return not_allowed(format!(
            "delegation goes {MAX_DEPTH} levels deep; this thread is at the bottom of one and cannot message another"
        ));
    }
    if let Some(refusal) = loop_or_linked(caller, target, &meta) {
        return refusal;
    }
    let caller_title = if super::outside::is_caller(caller) {
        super::outside::name_of_caller(caller).unwrap_or_default()
    } else {
        crate::config::load_session_title(caller).unwrap_or_default()
    };
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

    let engine = match Engine::open(target_access).await {
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
    // One turn at a time: with no queue here, a message to a thread that is
    // working joins its turn (`steer`) or is refused, never started beside it.
    if crate::thread_status::is_running(target) {
        if args.get("steer").and_then(Value::as_bool) == Some(true) {
            return match engine.steer(target, text).await {
                // A steer joins the turn already running: the app makes no new
                // link for it, and the one the caller already waits on settles with it.
                Ok(true) => json!({ "session_id": target, "steered": true }),
                Ok(false) => error(
                    "busy",
                    "that thread's turn did not take a steer (it may be waiting on the user); wait_session on it, or retry. Nothing was sent.",
                ),
                Err(reason) => error(
                    "busy",
                    format!(
                        "the steer did not reach that thread's turn ({reason}); it was not sent as a new message either. Retry."
                    ),
                ),
            };
        }
        return error(
            "busy",
            "that thread is working on a turn; send the message with steer true to join it, or wait_session on it first. Nothing was sent.",
        );
    }
    // `return_to: none` is a one-way message: no link, nothing to wait on and
    // nothing delivered back, so the caller never shows Waiting (issue 3544).
    if !super::waits_for_reply(args) {
        deliver(
            engine,
            target.to_owned(),
            delivered_from(text, &caller_title, caller, None),
            None,
        );
        let preview: String = text.chars().take(PREVIEW_CHARS).collect();
        return json!({ "session_id": target, "preview": preview, "return_to": "none" });
    }
    let _gate = link_gate().lock().await;
    // A follow-up to a thread this one already works with takes no new slot.
    if !has_active_link(caller, target)
        && let Some(refusal) = slot_refusal(caller, 1)
    {
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
        delivered_from(text, &caller_title, caller, Some(&link)),
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
    // The turn is over and the book says so: the app owns the link again.
    crate::data_store::engine_link_ended(link);
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

/// Abandon links a settled sender was waiting on, without interrupting their targets.
pub(crate) fn clear_waiting_links(sender: &str) {
    let book = link_book();
    if let Some(links) = book.get("links").and_then(Value::as_array) {
        for link in links {
            if link.get("orchestrator").and_then(Value::as_str) != Some(sender) {
                continue;
            }
            if let Some(id) = link.get("id").and_then(Value::as_str) {
                let at = now();
                let _ = crate::data_store::change_link(id, |link| {
                    if !link
                        .get("state")
                        .and_then(Value::as_str)
                        .is_some_and(is_active)
                    {
                        return link;
                    }
                    link_after(
                        link,
                        &json!({"state": "cancelled", "settledAt": at, "notifiedAt": at}),
                        at,
                    )
                });
            }
        }
    }
}

/// Change one link in the store, in one transaction of its own: the engine
/// never reads the whole book and writes it back, so it cannot undo what the
/// app saved meanwhile, and the app's whole-book save cannot undo this
/// (`data_store::save_link_book`).
fn update_link(id: &str, change: Value) {
    let at = now();
    match crate::data_store::change_link(id, |link| {
        if link.get("state").and_then(Value::as_str) == Some("cancelled") {
            link
        } else {
            link_after(link, &change, at)
        }
    }) {
        Ok(true) => {}
        Ok(false) => crate::dbg_log!("local orchestration: link {id} is no longer in the book"),
        Err(reason) => crate::dbg_log!("local orchestration: link {id} was not saved: {reason}"),
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

/// Record a link to `target` in the app's book and answer its id. The link
/// carries `"engine": true` and is live in this process until its turn ends:
/// an app that attaches meanwhile neither interrupts it nor saves over it.
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
    let at = now();
    let marked = std::cell::RefCell::new(None::<String>);
    let link = crate::data_store::open_link(caller, |counter| {
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
            "engine": true,
            "engineOwner": crate::data_store::engine_owner(),
            "engineBeat": at,
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
        // Live before it is stored, so no save of the book can see it unowned.
        crate::data_store::engine_link_started(&id);
        *marked.borrow_mut() = Some(id);
        link
    })
    .inspect_err(|_| {
        if let Some(id) = marked.borrow().as_deref() {
            crate::data_store::engine_link_ended(id);
        }
    })?;
    keep_beating();
    link.get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "the link has no id".to_owned())
}

/// While this process runs any link, stamp them every few seconds, so a reader
/// can tell them from the links of an engine that died.
fn keep_beating() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BEATING: AtomicBool = AtomicBool::new(false);
    if BEATING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        BEATING.store(false, Ordering::SeqCst);
        return;
    };
    runtime.spawn(async move {
        loop {
            tokio::time::sleep(crate::data_store::ENGINE_BEAT_EVERY).await;
            if crate::data_store::engine_live_links().is_empty() {
                BEATING.store(false, Ordering::SeqCst);
                // A link opened between the check and the store would find
                // the loop still marked running: take it up again.
                if crate::data_store::engine_live_links().is_empty()
                    || BEATING
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                {
                    break;
                }
                continue;
            }
            let _ = tokio::task::spawn_blocking(crate::data_store::beat_engine_links).await;
        }
    });
}

fn record_fan_out(caller: &str, fan_id: &str, title: &str, prompt: &str, links: &[String]) {
    let group = json!({
        "id": fan_id,
        "orchestrator": caller,
        "title": title,
        "prompt": prompt,
        "links": links,
        "createdAt": now(),
    });
    if let Err(reason) = crate::data_store::add_fan_out(group) {
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
    // Slots a start has checked and not yet recorded.
    let (held_mine, held_all) = reserved(caller);
    (mine + held_mine, all + held_all)
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
    meta: Option<&crate::config::SessionMeta>,
    asked: Option<&str>,
) -> Result<String, Value> {
    let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) else {
        return match meta {
            Some(meta) => Ok(project_of(caller, meta)),
            None => Err(not_allowed(
                "name the project to start the thread in; whoami lists the ones RustCode knows",
            )),
        };
    };
    let asked = clean_path(asked);
    if let Some(meta) = meta
        && (asked == project_of(caller, meta) || asked == reach_of(caller, meta))
    {
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

/// Full access and rate limits are not permission to spend another login.
/// Read the same account-specific user grants the desktop persists.
fn authorize_account(
    book: &Value,
    caller: &str,
    harness: &str,
    account: Option<&str>,
) -> Result<(), Value> {
    let key = json!(["delegate-account", "local", harness, account]).to_string();
    if book["grants"][caller]
        .as_array()
        .is_some_and(|keys| keys.iter().any(|value| value.as_str() == Some(&key)))
    {
        return Ok(());
    }
    Err(not_allowed(format!(
        "delegate account {} on {harness} needs the user's account-specific approval. Full access, usage pressure and rate limits do not authorize switching accounts. Open the RustCode app and approve this login; nothing was started.",
        account.unwrap_or("default")
    )))
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
    /// The access its turns run at, which decides how it answers a permission.
    access: Access,
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
    async fn open(access: Access) -> Result<Self, Value> {
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
                        responder.respond(answer_permission(&request, access))
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
            access,
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

    /// `session/cancel`: stop the turn running on `session_id`, as the app's
    /// stop button does. The engine's registry is shared by every connection,
    /// so a thread another client opened is stopped too.
    fn cancel(&self, session_id: &str) -> Result<(), String> {
        self.connection
            .send_notification(CancelNotification::new(session_id.to_owned()))
            .map_err(acp_message)
    }

    /// `_rustcode/steer`: whether the running turn took the message.
    async fn steer(&self, session_id: &str, text: &str) -> Result<bool, String> {
        let request = crate::acp::steer::SteerRequest {
            session_id: session_id.to_owned(),
            text: text.to_owned(),
            prompt: None,
            meta: None,
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
            json!({ "access": self.access.meta() }),
        )]));
        match self.connection.send_request(request).block_task().await {
            Ok(response) => Ok(response.stop_reason),
            Err(error) => Err(acp_message(error)),
        }
    }
}

/// The answer to a permission request with nobody to ask. At Full access it
/// allows (once, never "always": nothing here may add a standing rule to the
/// user's settings); at any lower access it denies, because Full is the only
/// level that would not have asked. A question (`ask_question`) is the model
/// asking the user, so it is cancelled instead of answered in the user's place.
fn answer_permission(
    request: &RequestPermissionRequest,
    access: Access,
) -> RequestPermissionResponse {
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
    let find =
        |kind: PermissionOptionKind| request.options.iter().find(|option| option.kind == kind);
    let option = if access == Access::Full {
        find(PermissionOptionKind::AllowOnce).or_else(|| find(PermissionOptionKind::AllowAlways))
    } else {
        find(PermissionOptionKind::RejectOnce)
    };
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

/// `interrupt_session` for an outside agent: stop the turn running on another
/// thread of this computer. The links under it are not walked here (an outside
/// agent has none); the answer says whether a turn was running.
pub(crate) async fn interrupt(caller: &str, args: &Value) -> Value {
    if let Err(remote) = super::local::local_machine(args) {
        return remote;
    }
    let target = args
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if target == caller {
        return error("not_allowed", "a thread cannot stop itself");
    }
    let Some(meta) = crate::config::session_meta_by_id(target) else {
        return error("not_found", "no thread has that id; use list_sessions");
    };
    if !super::local::may_reach(caller, target, &meta) {
        return not_allowed(
            "that thread is in another project; acting across projects is off. Turn on “Let agents read threads in other projects” in Settings › Harnesses. Nothing was stopped.",
        );
    }
    let was_running = crate::thread_status::is_active(target);
    let engine = match Engine::open(Access::Ask).await {
        Ok(engine) => engine,
        Err(refusal) => return refusal,
    };
    if let Err(reason) = engine.cancel(target) {
        return failed(format!("the stop did not reach the engine: {reason}"));
    }
    let mut answer = json!({
        "session_id": target,
        "interrupted": was_running,
        "status": crate::thread_status::current(target).as_str(),
    });
    if !was_running {
        answer["note"] = json!("no turn was running on that thread");
    }
    answer
}

/// The message as the other thread receives it: who it is from, then the text.
fn delivered(text: &str, from: &str) -> String {
    format!("From thread “{from}” (agent):\n\n{text}")
}

/// A message as its target receives it, with the sender's session (and the
/// link it waits on) on the line under the heading, so a reader can tell two
/// senders with the same title apart (`message_origin::delivered_sender`).
fn delivered_from(text: &str, from: &str, session: &str, link: Option<&str>) -> String {
    let link = link
        .map(|link| format!(" · link_id {link}"))
        .unwrap_or_default();
    format!("From thread “{from}” (agent):\nsession {session}{link}\n\n{text}")
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
        &vec![
            crate::app::ChatMessage::new("user", "a test thread"),
            crate::app::ChatMessage::new("assistant", "ok"),
        ],
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

    #[tokio::test]
    async fn a_message_to_a_thread_that_runs_at_ask_is_refused_not_raised_to_full() {
        let mine = tempfile::tempdir().unwrap();
        let caller = test_thread(mine.path());
        crate::config::save_session_yolo(&caller, true);
        let target = test_thread(mine.path());
        // The app's last prompt to the target said Ask, whatever the default is.
        crate::config::save_session_access(&target, "ask");
        assert_eq!(access_of(&target), Access::Ask);
        let before = link_book()["links"].as_array().map(Vec::len);
        let answer = message(&caller, &json!({"session_id": target, "message": "hi"})).await;
        assert_eq!(answer["error"]["code"], json!("unavailable"), "{answer}");
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("Ask"),
            "{answer}"
        );
        assert_eq!(link_book()["links"].as_array().map(Vec::len), before);
        for id in [&caller, &target] {
            let _ =
                crate::config::get_active_session_dir(id).map(|dir| std::fs::remove_dir_all(dir));
        }
    }

    #[test]
    fn account_approval_does_not_authorize_a_fallback() {
        let key = json!(["delegate-account", "local", "claude-code", "paral-2"]).to_string();
        let book = json!({"grants": {"parent": ["start", key]}});
        assert!(authorize_account(&book, "parent", "claude-code", Some("paral-2")).is_ok());
        let refused =
            authorize_account(&book, "parent", "claude-code", Some("personal-1")).unwrap_err();
        assert!(refused.to_string().contains("account-specific approval"));
        assert!(refused.to_string().contains("nothing was started"));
        assert!(authorize_account(&book, "parent", "claude-code", None).is_err());
        assert!(authorize_account(&book, "child", "claude-code", Some("paral-2")).is_err());
    }

    #[test]
    fn a_permission_is_allowed_once_at_full_and_denied_below() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
        };
        let request = RequestPermissionRequest::new(
            "s",
            ToolCallUpdate::new("t", ToolCallUpdateFields::new()),
            vec![
                PermissionOption::new("always", "always", PermissionOptionKind::AllowAlways),
                PermissionOption::new("once", "once", PermissionOptionKind::AllowOnce),
                PermissionOption::new("no", "no", PermissionOptionKind::RejectOnce),
            ],
        );
        let chosen = |access| match answer_permission(&request, access).outcome {
            RequestPermissionOutcome::Selected(selected) => selected.option_id.to_string(),
            other => format!("{other:?}"),
        };
        assert_eq!(chosen(Access::Full), "once");
        assert_eq!(chosen(Access::Edits), "no");
        assert_eq!(chosen(Access::Ask), "no");
    }

    #[tokio::test]
    async fn slots_checked_by_a_start_in_flight_count_until_it_records_its_link() {
        let caller = format!("caller-reserve-{}-{}", std::process::id(), now());
        let (per_thread, _) = slot_limits();
        let first = reserve(&caller, per_thread).await.expect("the slots fit");
        // Every slot is held while the threads start: a second start waits for none.
        let refused = reserve(&caller, 1).await;
        assert_eq!(
            refused
                .as_ref()
                .err()
                .map(|refusal| refusal["error"]["code"].clone()),
            Some(json!("busy")),
            "{refused:?}"
        );
        // A recorded link takes over one slot from the reservation.
        first.release_one();
        assert_eq!(reserved(&caller).0, per_thread - 1);
        drop(first);
        assert_eq!(reserved(&caller).0, 0, "the call's end gives the rest back");
        assert!(reserve(&caller, 1).await.is_ok());
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
    fn a_message_names_its_sender_by_id_and_the_reader_finds_it() {
        let waits = delivered_from("do it", "Parent", "s1", Some("s1/2"));
        assert_eq!(
            crate::message_origin::delivered_sender(&waits),
            Some(("s1".to_owned(), Some("s1/2".to_owned())))
        );
        assert_eq!(
            crate::message_origin::delivered_title(&waits),
            Some("Parent")
        );
        assert!(waits.ends_with("\n\ndo it"));
        let one_way = delivered_from("fyi", "Parent", "s1", None);
        assert_eq!(
            crate::message_origin::delivered_sender(&one_way),
            Some(("s1".to_owned(), None))
        );
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
            answer_permission(&question, Access::Full).outcome,
            RequestPermissionOutcome::Cancelled
        );
        assert!(matches!(
            answer_permission(&request, Access::Full).outcome,
            RequestPermissionOutcome::Selected(_)
        ));
    }
}
