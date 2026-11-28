//! Outside agents and the fleet's hub (#3957, `remote_gateway/hub_link.rs`).
//!
//! On the hub, a call whose `machine` names a member is sent to that member
//! once the hub's own gate has allowed it ([`forward`]), and `list_machines`
//! lists the members ([`with_members`]). On a member, a call the hub sent is
//! answered through the same gate as a local outside agent's, with the
//! membership in place of a grant ([`answer_for_hub`]): its access is the
//! ceiling the owner set when joining, and `ask` asks in the app here.

use super::outside::{self, EngineRunner, Grant, Runner, Scope, Settings};
use super::tool_socket::CallAnswer;
use crate::remote_gateway::hub_link::{HubCallAnswer, HubConfig, Membership};
use crate::remote_gateway::lifecycle::RemoteLifecycle;
use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// The grant id of calls a hub forwarded, followed by the hub's `host_id`.
pub const HUB_GRANT_PREFIX: &str = "hub:";

/// The membership behind the grant id `hub:<host id>`, while this machine is
/// still a member of that hub.
pub fn membership_of(grant_id: &str) -> Option<Membership> {
    let hub = grant_id.strip_prefix(HUB_GRANT_PREFIX)?;
    HubConfig::current()
        .member
        .filter(|member| member.machine == hub)
}

/// The grant a forwarded call is checked against: the membership's access
/// and `ask`, named after who asked on the hub.
fn hub_grant(membership: &Membership, caller: &str) -> Grant {
    let caller = caller.trim();
    let name = if caller.is_empty() {
        format!("the hub {}", membership.name)
    } else {
        format!("{caller} via the hub {}", membership.name)
    };
    Grant {
        expires_at: None,
        id: format!("{HUB_GRANT_PREFIX}{}", membership.machine),
        name,
        platform: "hub".to_owned(),
        scope: membership.access,
        ask: membership.ask,
        token_hash: String::new(),
        created_at: String::new(),
        last_used_at: None,
        last_tool: None,
        refresh_hash: None,
        refresh_until: None,
        oauth_client: None,
    }
}

/// Answer one call the hub sent this machine.
pub async fn answer_for_hub(
    tool: &str,
    arguments: &Value,
    caller: &str,
    cancel: &CancellationToken,
) -> CallAnswer {
    let (Some(settings), Some(config_dir)) = (Settings::machine(), crate::config::get_config_dir())
    else {
        return CallAnswer::failure(
            "error: RustCode has no configuration directory on that machine. Nothing was sent.",
        );
    };
    answer_for_hub_with(
        &settings,
        &EngineRunner,
        &config_dir,
        tool,
        arguments,
        caller,
        cancel,
    )
    .await
}

pub async fn answer_for_hub_with(
    settings: &Settings,
    runner: &dyn Runner,
    config_dir: &Path,
    tool: &str,
    arguments: &Value,
    caller: &str,
    cancel: &CancellationToken,
) -> CallAnswer {
    let Some(membership) = HubConfig::load(config_dir).member else {
        return CallAnswer::failure(
            "error: this machine has left the hub, so the hub cannot act on it. Nothing was sent.",
        );
    };
    let grant = hub_grant(&membership, caller);
    let hub = membership.machine.clone();
    let still_allowed = move |needed: Scope| {
        HubConfig::load(config_dir)
            .member
            .is_some_and(|member| member.machine == hub && member.access >= needed)
    };
    outside::answer_granted(
        settings,
        runner,
        &grant,
        tool,
        arguments,
        cancel,
        &still_allowed,
    )
    .await
}

/// The machine `args` names when it is not this computer: either an explicit
/// `machine` argument, or the `<host_id>/<session_id>` prefix of `session_id`.
fn named_machine(args: &Value) -> Option<(String, bool)> {
    if let Some(machine) = args
        .get("machine")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|machine| !machine.is_empty())
    {
        return super::local::local_machine(args)
            .is_err()
            .then(|| (machine.to_owned(), false));
    }
    let session = args.get("session_id").and_then(Value::as_str)?.trim();
    let (host, _) = session.split_once('/')?;
    let host = host.trim();
    if host.is_empty() || super::local::is_local_machine_name(host) {
        return None;
    }
    Some((host.to_owned(), true))
}

fn prepare_forwarded_args(args: &Value, strip_session_host: bool) -> Option<Value> {
    let mut arguments = args.clone();
    let object = arguments.as_object_mut()?;
    object.remove("machine");
    if strip_session_host
        && let Some(session) = object
            .get("session_id")
            .and_then(Value::as_str)
            .and_then(|value| value.split_once('/').map(|(_, id)| id.to_owned()))
    {
        object.insert("session_id".to_owned(), json!(session));
    }
    Some(arguments)
}

async fn forward_to_member(
    who: &str,
    machine: &str,
    tool: &str,
    arguments: Value,
) -> Option<CallAnswer> {
    let config_dir = crate::config::get_config_dir()?;
    if !HubConfig::load(&config_dir).hub {
        return None;
    }
    // A Loop for a member is kept here, where the clock runs while the
    // member sleeps; each run is then sent to it ([`hub_loop_machine`]).
    if tool == super::CREATE_LOOP {
        return None;
    }
    let lifecycle = RemoteLifecycle::new(&config_dir);
    match lifecycle.hub_call(machine, tool, arguments, who).await {
        Ok(Some(HubCallAnswer::Answered { success, content })) => {
            Some(CallAnswer { success, content })
        }
        Ok(Some(HubCallAnswer::NotMember)) => None,
        // No gateway, no members: a machine that is not one is refused as
        // before, and one that is says why it cannot be reached.
        Ok(None) => crate::remote_gateway::hub_link::Members::new(&config_dir)
            .knows(machine)
            .then(|| {
                CallAnswer::failure(format!(
                    "error: {machine} is reached through this hub, and the hub's remote gateway is not running. Start it with `rc remote start` on the hub. Nothing was sent."
                ))
            }),
        Err(error) => Some(CallAnswer::failure(format!(
            "error: the hub could not reach its remote gateway to forward {tool} to {machine}: {error:#}. Nothing was sent."
        ))),
    }
}

/// On the hub: send a call that names a member to it. `None` when the call
/// does not name a member of this hub, which then runs (or is refused) here
/// as it would without one.
pub(super) async fn forward(caller: &str, tool: &str, args: &Value) -> Option<CallAnswer> {
    let (machine, strip_session_host) = named_machine(args)?;
    let arguments = prepare_forwarded_args(args, strip_session_host)?;
    let who = outside::name_of_caller(caller).unwrap_or_else(|| "an outside agent".to_owned());
    forward_to_member(&who, &machine, tool, arguments).await
}

/// On the hub: send a call from a local RustCode thread (`local.rs`) to a hub
/// member when `args` targets one. Returns the JSON result shape `local.rs`
/// uses (`{ ... }` or `{ "error": { ... } }`), or `None` when the target is
/// not a member of this hub.
pub(super) async fn forward_from_thread(
    session_id: &str,
    tool: &str,
    args: &Value,
) -> Option<Value> {
    let (machine, strip_session_host) = named_machine(args)?;
    let arguments = prepare_forwarded_args(args, strip_session_host)?;
    let who = crate::sessions::load_session_title(session_id)
        .filter(|title| !title.trim().is_empty())
        .map(|title| format!("thread “{title}”"))
        .unwrap_or_else(|| format!("thread {session_id}"));
    let answer = forward_to_member(&who, &machine, tool, arguments).await?;
    if answer.success {
        if let Ok(value) = serde_json::from_str::<Value>(&answer.content) {
            return Some(value);
        }
        return Some(json!({ "output": answer.content }));
    }
    let message = answer
        .content
        .strip_prefix("error: ")
        .unwrap_or(&answer.content)
        .to_owned();
    Some(json!({ "error": { "code": "failed", "message": message } }))
}

/// On the hub: the member a call's `machine` names, when it names one. A
/// Loop saved for it runs on this hub's schedule and is sent there.
pub(super) fn hub_loop_machine(args: &Value) -> Option<String> {
    let (machine, _) = named_machine(args)?;
    let config_dir = crate::config::get_config_dir()?;
    (HubConfig::load(&config_dir).hub
        && crate::remote_gateway::hub_link::Members::new(&config_dir).knows(&machine))
    .then_some(machine)
}

/// On the hub: add linked and remembered members to a `list_machines` JSON
/// value in place.
pub(super) async fn with_members_value(listed: &mut Value) {
    let Some(config_dir) = crate::config::get_config_dir() else {
        return;
    };
    if !HubConfig::load(&config_dir).hub {
        return;
    }
    let Ok(Some(members)) = RemoteLifecycle::new(&config_dir).hub_members().await else {
        return;
    };
    add_members(listed, &members);
}

/// On the hub: `list_machines` with the members added, each saying whether
/// it is linked now, when it was last seen and the access it grants the hub.
pub(super) async fn with_members(content: String) -> String {
    let Ok(mut listed) = serde_json::from_str::<Value>(&content) else {
        return content;
    };
    with_members_value(&mut listed).await;
    listed.to_string()
}

fn add_members(listed: &mut Value, members: &[crate::remote_gateway::hub_link::HubMember]) {
    let Some(rows) = listed.get_mut("machines").and_then(Value::as_array_mut) else {
        return;
    };
    for member in members {
        let hub = json!({
            "linked": member.online,
            "last_seen": member.last_seen_at,
            "access": member.access.as_str(),
            "asks": member.ask,
            "queued": member.queued,
        });
        match rows
            .iter_mut()
            .find(|row| row.get("id").and_then(Value::as_str) == Some(member.host_id.as_str()))
        {
            Some(row) => {
                row["hub"] = hub;
                if member.online {
                    row["state"] = json!("connected");
                }
            }
            None => rows.push(json!({
                "id": member.host_id,
                "name": member.name,
                "state": if member.online { "connected" } else { "offline" },
                "hub": hub,
            })),
        }
    }
    listed["hub_note"] = json!(
        "This machine is the fleet's hub. Pass a machine's id or name as `machine` to act there: it runs at most at the access it grants the hub, and starting or messaging a thread on an offline machine waits on the hub until it connects."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::outside::BoxFuture;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<(String, String)>>,
    }

    impl Runner for Recorder {
        fn run<'a>(
            &'a self,
            caller: &'a str,
            _cancel: &'a CancellationToken,
            tool: &'a str,
            _args: &'a Value,
        ) -> BoxFuture<'a, CallAnswer> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((caller.to_owned(), tool.to_owned()));
                CallAnswer {
                    success: true,
                    content: "ran".into(),
                }
            })
        }

        fn context<'a>(
            &'a self,
            _caller: &'a str,
            _cancel: &'a CancellationToken,
        ) -> BoxFuture<'a, Value> {
            Box::pin(async { json!({}) })
        }
    }

    fn member(dir: &Path, access: Scope, ask: bool) {
        HubConfig {
            hub: false,
            member: Some(Membership {
                machine: "vps-id".into(),
                name: "vps".into(),
                access,
                ask,
            }),
        }
        .save(dir)
        .unwrap();
    }

    fn settings(dir: &Path) -> Settings {
        Settings {
            grants: dir.join("outside-agents.json"),
            data: None,
            approval_wait: Duration::from_millis(50),
            needs_approver: true,
        }
    }

    async fn call(dir: &Path, runner: &Recorder, tool: &str, args: Value) -> CallAnswer {
        answer_for_hub_with(
            &settings(dir),
            runner,
            dir,
            tool,
            &args,
            "ChatGPT",
            &CancellationToken::new(),
        )
        .await
    }

    #[tokio::test]
    async fn the_hub_reaches_a_member_only_up_to_the_access_it_granted() {
        let dir = tempfile::tempdir().unwrap();
        member(dir.path(), Scope::ReadOnly, false);
        let runner = Recorder::default();
        let read = call(
            dir.path(),
            &runner,
            crate::orchestration::LIST_SESSIONS,
            json!({}),
        )
        .await;
        assert!(read.success, "{}", read.content);
        let start = call(
            dir.path(),
            &runner,
            crate::orchestration::START_SESSION,
            json!({"prompt": "go", "project": "/tmp"}),
        )
        .await;
        assert!(!start.success);
        assert!(
            start.content.contains("needs Threads access")
                && start.content.contains("ChatGPT via the hub vps"),
            "{}",
            start.content
        );
        let calls = runner.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![(
                "outside:hub:vps-id".to_owned(),
                crate::orchestration::LIST_SESSIONS.to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn a_member_that_asks_refuses_a_change_when_no_app_can_ask() {
        let dir = tempfile::tempdir().unwrap();
        member(dir.path(), Scope::Delegate, true);
        outside::set_approvers(|| false);
        let runner = Recorder::default();
        let start = call(
            dir.path(),
            &runner,
            crate::orchestration::START_SESSION,
            json!({"prompt": "go", "project": "/tmp"}),
        )
        .await;
        assert!(!start.success);
        assert!(start.content.contains("approval"), "{}", start.content);
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_machine_that_left_the_hub_refuses_what_the_hub_sends() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Recorder::default();
        let read = call(
            dir.path(),
            &runner,
            crate::orchestration::LIST_SESSIONS,
            json!({}),
        )
        .await;
        assert!(!read.success && read.content.contains("left the hub"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn list_machines_on_the_hub_adds_members_and_marks_saved_ones() {
        let now = chrono::Utc::now();
        let mut listed = json!({
            "current": "local",
            "machines": [
                {"id": "local", "name": "vps", "state": "connected"},
                {"id": "desk-id", "name": "desk", "state": "offline"},
            ],
        });
        let members = vec![
            crate::remote_gateway::hub_link::HubMember {
                host_id: "desk-id".into(),
                name: "desk".into(),
                access: Scope::Delegate,
                ask: true,
                online: true,
                last_seen_at: now,
                queued: 0,
                deliveries: Vec::new(),
            },
            crate::remote_gateway::hub_link::HubMember {
                host_id: "laptop-id".into(),
                name: "laptop".into(),
                access: Scope::ReadOnly,
                ask: false,
                online: false,
                last_seen_at: now,
                queued: 2,
                deliveries: Vec::new(),
            },
        ];
        add_members(&mut listed, &members);
        let rows = listed["machines"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["state"], "connected");
        assert_eq!(rows[1]["hub"]["access"], "delegate");
        assert_eq!(rows[2]["id"], "laptop-id");
        assert_eq!(rows[2]["state"], "offline");
        assert_eq!(rows[2]["hub"]["queued"], 2);
        assert!(listed["hub_note"].is_string());
    }
}
