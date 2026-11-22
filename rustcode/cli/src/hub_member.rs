//! A member's side of the hub link (#3957, `remote_gateway/hub_link.rs`).
//!
//! A machine that joined a hub (`rc machines hub join <machine>`) keeps
//! one outbound connection to it from its own gateway process: it connects
//! to the hub as the paired computer it already is, over whichever saved
//! route answers first, switches the connection to the hub link and answers
//! each call the hub sends through this machine's outside-agent gate at the
//! access it granted the hub. It reconnects with a backoff while the hub is
//! down, and lets go when the membership changes or ends.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rustcode::remote_gateway::handshake::HandshakeResponse;
use rustcode::remote_gateway::hub_link::{HubConfig, HubFrame, Membership, result_frame};
use rustcode::remote_gateway::lifecycle::RemoteLifecycle;
use rustcode_client::{AcpChannel, ClientKind, Connector, MachineRegistry, TokenStore};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// How often a machine that is in no hub looks again, and how often a linked
/// one checks its membership and the hub's pulse.
const CHECK: Duration = Duration::from_secs(30);
/// The hub pings every 30 s; this long without a word is a dead link.
const SILENCE: Duration = Duration::from_secs(95);
const FIRST_RETRY: Duration = Duration::from_secs(5);
const LAST_RETRY: Duration = Duration::from_secs(300);

/// Keep this machine linked to its hub until `stop` is cancelled. A machine
/// in no hub only rereads its role file now and then.
pub async fn run(config_dir: PathBuf, stop: CancellationToken) {
    let mut retry = FIRST_RETRY;
    loop {
        let ours = RemoteLifecycle::new(&config_dir).running_pid() == Some(std::process::id());
        let wait = match HubConfig::load(&config_dir).member {
            None => CHECK,
            Some(_) if !ours => FIRST_RETRY,
            Some(membership) => match link_once(&config_dir, &membership, &stop, answer).await {
                Ok(()) => {
                    retry = FIRST_RETRY;
                    Duration::ZERO
                }
                Err(error) => {
                    rustcode::dbg_log!(
                        "hub link to {}: {error:#}; trying again in {}s",
                        membership.name,
                        retry.as_secs()
                    );
                    let wait = retry;
                    retry = (retry * 2).min(LAST_RETRY);
                    wait
                }
            },
        };
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// This machine's own answer to a call from its hub.
async fn answer(tool: String, arguments: Value, caller: String) -> (bool, String) {
    let answer =
        rustcode::answer_hub_call(&tool, &arguments, &caller, &CancellationToken::new()).await;
    (answer.success, answer.content)
}

/// Connect to the hub once and serve its calls until the link ends. `Ok`
/// when it ended because the membership changed or `stop` was cancelled.
pub async fn link_once<F, Fut>(
    config_dir: &Path,
    membership: &Membership,
    stop: &CancellationToken,
    answer: F,
) -> Result<()>
where
    F: Fn(String, Value, String) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = (bool, String)> + Send + 'static,
{
    let registry = MachineRegistry::load(config_dir)?;
    let machine = registry.get(&membership.machine).with_context(|| {
        format!(
            "the hub {} is no longer a saved machine; join it again after adding it",
            membership.name
        )
    })?;
    let tokens = TokenStore::open(config_dir)?;
    let connector = Connector::new(config_dir, ClientKind::Desktop)?;
    let connected = connector.connect_raced(machine, &tokens).await?;
    let channel = connected
        .session
        .hub_join(membership.access.as_str(), membership.ask)
        .await?;
    rustcode::dbg_log!(
        "hub link to {} up over {}",
        membership.name,
        connected.route.address
    );
    serve(channel, config_dir, membership, stop, answer).await
}

/// Answer the hub's calls on `channel`, each on its own task, until the link
/// ends.
pub async fn serve<F, Fut>(
    mut channel: AcpChannel,
    config_dir: &Path,
    membership: &Membership,
    stop: &CancellationToken,
    answer: F,
) -> Result<()>
where
    F: Fn(String, Value, String) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = (bool, String)> + Send + 'static,
{
    let (results, mut finished) = mpsc::channel::<Value>(64);
    let mut check = tokio::time::interval_at(tokio::time::Instant::now() + CHECK, CHECK);
    loop {
        tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            frame = channel.next() => {
                let Some(frame) = frame? else {
                    bail!("the hub closed the link");
                };
                match serde_json::from_value::<HubFrame>(frame.clone()) {
                    Ok(HubFrame::HubCall { call_id, tool, arguments, caller }) => {
                        let answer = answer.clone();
                        let results = results.clone();
                        tokio::spawn(async move {
                            let (success, content) = answer(tool, arguments, caller).await;
                            let frame = result_frame(call_id, success, content);
                            if let Ok(frame) = serde_json::to_value(frame) {
                                let _ = results.send(frame).await;
                            }
                        });
                    }
                    Ok(HubFrame::HubJoined) => {}
                    Err(_) => {
                        if let Ok(HandshakeResponse::Error { message, .. }) =
                            serde_json::from_value::<HandshakeResponse>(frame)
                        {
                            bail!("the hub ended the link: {message}");
                        }
                    }
                }
            }
            Some(result) = finished.recv() => channel.send(result).await?,
            _ = check.tick() => {
                if HubConfig::load(config_dir).member.as_ref() != Some(membership) {
                    return Ok(());
                }
                if channel.last_heard().elapsed() >= SILENCE {
                    bail!("the hub stopped answering");
                }
                channel.ping().await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The link end to end against the real gateway: a member pairs as a
    //! desktop, joins, answers a call the hub forwards, gets the work that
    //! waited while it was away, and sees the link end when the hub goes.
    use super::*;
    use rustcode::remote_gateway::address::plan_listen;
    use rustcode::remote_gateway::gateway::{Gateway, GatewayConfig, Limits};
    use rustcode::remote_gateway::hub_link::{HubCallAnswer, HubMember, Scope};
    use rustcode::remote_gateway::session_registry::{HubLimits, SessionHub};
    use rustcode_client::machine::machine_from_pairing;
    use rustcode_client::{Machine, Route};
    use serde_json::json;

    const WAIT: Duration = Duration::from_secs(15);

    async fn start_hub(
        dir: &Path,
    ) -> (
        RemoteLifecycle,
        String,
        CancellationToken,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        HubConfig {
            hub: true,
            member: None,
        }
        .save(dir)
        .unwrap();
        let lifecycle = RemoteLifecycle::new(dir);
        let sessions = SessionHub::start(HubLimits::default());
        let gateway = Gateway::bind(
            &lifecycle,
            GatewayConfig {
                replan: None,
                plan: plan_listen("127.0.0.1", None, &[]).unwrap(),
                port: 0,
                router: sessions.clone(),
                sessions: Some(sessions),
                limits: Limits::default(),
            },
        )
        .await
        .expect("the gateway binds");
        let address = gateway.local_addr().to_string();
        let shutdown = gateway.shutdown_token();
        let task = tokio::spawn(gateway.run());
        (lifecycle, address, shutdown, task)
    }

    /// Pair the member with the hub the way the app does, accepting on the hub.
    async fn pair(
        lifecycle: &RemoteLifecycle,
        address: &str,
        connector: &Connector,
        member_dir: &Path,
    ) -> (Machine, TokenStore) {
        let ask = connector.ask(address).await.expect("the request is taken");
        let deadline = tokio::time::Instant::now() + WAIT;
        let code = loop {
            let status = lifecycle.status().await.unwrap().expect("running");
            if let Some(request) = status.connect_requests.first() {
                let answer = lifecycle
                    .answer_connect_request(&request.id, true)
                    .await
                    .unwrap();
                break answer.code.expect("a code").expose().to_owned();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no connection request arrived"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let approved = ask.wait(WAIT).await.unwrap().expect("approved");
        let paired = approved.pair(&code).await.expect("the code is accepted");
        let machine = machine_from_pairing(
            paired.session.welcome(),
            Route::new(address.to_owned()),
            None,
        );
        let tokens = TokenStore::in_file(member_dir).unwrap();
        paired.keep(&tokens, &machine.host_id).unwrap();
        (machine, tokens)
    }

    async fn members(lifecycle: &RemoteLifecycle, online: bool) -> Vec<HubMember> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let members = lifecycle.hub_members().await.unwrap().unwrap_or_default();
            if members
                .first()
                .is_some_and(|member| member.online == online)
            {
                return members;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the member never became online={online}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_member_answers_the_hub_gets_queued_work_and_sees_the_hub_go() {
        let hub_dir = tempfile::tempdir_in("/tmp").unwrap();
        let member_dir = tempfile::tempdir_in("/tmp").unwrap();
        let (lifecycle, address, shutdown, gateway) = start_hub(hub_dir.path()).await;
        let connector = Connector::new(member_dir.path(), ClientKind::Desktop).unwrap();
        let (machine, tokens) = pair(&lifecycle, &address, &connector, member_dir.path()).await;
        let membership = Membership {
            machine: machine.host_id.clone(),
            name: machine.name.clone(),
            access: Scope::Delegate,
            ask: false,
        };
        HubConfig {
            hub: false,
            member: Some(membership.clone()),
        }
        .save(member_dir.path())
        .unwrap();
        let (calls, mut seen) = mpsc::unbounded_channel::<(String, Value)>();
        let answer = move |tool: String, arguments: Value, caller: String| {
            let calls = calls.clone();
            async move {
                let content = format!("{tool} for {caller}");
                let _ = calls.send((tool, arguments));
                (true, content)
            }
        };
        let link = |stop: CancellationToken| {
            let connector = Connector::new(member_dir.path(), ClientKind::Desktop).unwrap();
            let machine = machine.clone();
            let tokens = TokenStore::in_file(member_dir.path()).unwrap();
            let dir = member_dir.path().to_owned();
            let membership = membership.clone();
            let answer = answer.clone();
            tokio::spawn(async move {
                let connected = connector.connect_raced(&machine, &tokens).await?;
                let channel = connected
                    .session
                    .hub_join(membership.access.as_str(), membership.ask)
                    .await?;
                serve(channel, &dir, &membership, &stop, answer).await
            })
        };
        drop(tokens);

        let stop = CancellationToken::new();
        let first = link(stop.clone());
        let joined = members(&lifecycle, true).await;
        assert_eq!(joined[0].access, Scope::Delegate);
        let member = joined[0].host_id.clone();
        let answer = lifecycle
            .hub_call(&member, "list_sessions", json!({}), "ChatGPT")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            answer,
            HubCallAnswer::Answered {
                success: true,
                content: "list_sessions for ChatGPT".into()
            }
        );
        assert_eq!(seen.recv().await.unwrap().0, "list_sessions");

        stop.cancel();
        first.await.unwrap().unwrap();
        members(&lifecycle, false).await;
        let HubCallAnswer::Answered { success, .. } = lifecycle
            .hub_call(&member, "list_sessions", json!({}), "ChatGPT")
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("the hub knows the member")
        };
        assert!(!success);
        let HubCallAnswer::Answered { success, content } = lifecycle
            .hub_call(
                &member,
                rustcode_start_session(),
                json!({"prompt": "go"}),
                "ChatGPT",
            )
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("the hub knows the member")
        };
        assert!(success && content.contains("\"queued\":true"), "{content}");

        let second = link(CancellationToken::new());
        let (tool, arguments) = tokio::time::timeout(WAIT, seen.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tool, rustcode_start_session());
        assert_eq!(arguments["prompt"], "go");

        shutdown.cancel();
        let ended = tokio::time::timeout(WAIT, second).await.unwrap().unwrap();
        assert!(ended.is_err());
        gateway.await.unwrap().unwrap();
    }

    fn rustcode_start_session() -> &'static str {
        rustcode::remote_gateway::hub_link::QUEUEABLE[0]
    }
}
