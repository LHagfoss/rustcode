//! `_rustcode/session/harness` with `harness: "rustcode"`: an idle external
//! thread goes on with the built-in harness. Its process stops, its
//! `harness.json` is removed so it opens as a built-in thread, and the switch
//! marker in `history.json` carries the conversation so far for the first
//! built-in request (`harness::handoff::builtin`). The client opens the thread
//! again with `session/load` to run on it.

use serde_json::Value;

use crate::harness::{self, store};

fn failure(message: impl Into<String>) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(message.into())
}

/// Whether `request` moves its thread to the built-in harness.
pub(super) fn wanted(request: &super::harnesses::SwitchHarnessRequest) -> bool {
    request.harness.trim() == harness::BUILTIN_ID
}

pub(super) async fn switch(
    server: &super::AcpServer,
    request: super::harnesses::SwitchHarnessRequest,
) -> Result<Value, agent_client_protocol::Error> {
    let session_id = request.session_id.clone();
    if crate::thread_status::is_running(&session_id) {
        return Err(failure("stop the running turn before switching harness"));
    }
    let current = super::harnesses::get(&server.external, &session_id).await;
    let record = match &current {
        Some(session) => session.record(),
        None => store::load(&session_id).ok_or_else(|| {
            failure(if crate::sessions::session_id_has_content(&session_id) {
                "the thread already runs on RustCode".to_owned()
            } else {
                format!("unknown session '{session_id}'")
            })
        })?,
    };
    let history = match &current {
        Some(session) => session.recorded_history(),
        None => super::harnesses::handoff_history(&session_id)?,
    };
    let dir = harness::config::config_dir().map_err(failure)?;
    let accounts = harness::config::load_accounts_in(&dir);
    let from = record.harness.clone();
    let from_account = record.account.as_deref().and_then(|id| {
        accounts
            .iter()
            .find(|known| known.harness == from && known.id == id)
            .map(|known| known.display_name().to_owned())
    });
    let from_label = record
        .handoff_from
        .clone()
        .unwrap_or_else(|| harness::handoff::label(&from, from_account.as_deref()));

    let mut state =
        super::builtin_open::build_loaded_app_state(&session_id, &record.cwd, history.clone());
    super::builtin_open::apply_requested_model_and_effort(
        &mut state,
        &harness::HarnessRequest {
            harness: Some(harness::BUILTIN_ID.to_owned()),
            account: None,
            model: request.model.clone(),
            effort: request.effort.clone(),
        },
    );
    let config_options = super::config::build_session_config_options(&state);
    let model = state.active_model_profile().map(|profile| profile.name);
    drop(state);

    if current.is_some() {
        super::harnesses::close(&server.external, &session_id, &server.remote).await;
    }
    hand_over(
        &session_id,
        history,
        Origin {
            harness: &from,
            account: from_account.as_deref(),
            label: &from_label,
        },
        model.as_deref(),
    );
    if request.reason.as_deref() == Some("limit") {
        crate::logger::operational_event(
            "thread.limit_switch",
            serde_json::json!({
                "session_id": session_id,
                "from_harness": from,
                "from_model": record.model,
                "to_harness": harness::BUILTIN_ID,
                "to_model": model,
                "reason": "limit",
            }),
        );
    }
    Ok(serde_json::json!({
        "configOptions": config_options,
        "_meta": harness::builtin_session_meta(),
    }))
}

/// The harness a thread leaves, as its divider and preamble name it.
struct Origin<'a> {
    harness: &'a str,
    account: Option<&'a str>,
    label: &'a str,
}

/// Record the switch in `history.json`, its marker carrying the conversation
/// so far, and drop `harness.json`: from now on the thread opens as built-in.
fn hand_over(
    session_id: &str,
    mut history: Vec<rustcode_core::ChatMessage>,
    from: Origin<'_>,
    model: Option<&str>,
) {
    let context =
        harness::handoff::context_for(&history, from.label, harness::BUILTIN_ID, Some(session_id));
    let marker =
        harness::handoff::marker_with(from.harness, from.account, harness::BUILTIN_ID, None, model);
    history.push(harness::handoff::builtin::with_context(marker, context));
    crate::sessions::save_session_history(session_id, &history);
    crate::sessions::flush_history();
    store::remove(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode_core::ChatMessage;

    #[test]
    fn a_handed_over_thread_opens_built_in_and_its_first_request_has_the_context() {
        let id = crate::sessions::create_detached_session();
        let cwd = tempfile::tempdir().unwrap();
        store::save(
            &id,
            &store::HarnessRecord::new("claude-code", cwd.path().to_path_buf()),
        );
        let history = vec![
            ChatMessage::new("user", "Fix the login bug"),
            ChatMessage::new("assistant", "Found it in auth.rs"),
        ];
        crate::sessions::save_session_history(&id, &history);
        assert!(store::load_for_restore(&id).unwrap().is_some());

        hand_over(
            &id,
            history,
            Origin {
                harness: "claude-code",
                account: None,
                label: "Claude Code",
            },
            None,
        );

        assert!(store::load_for_restore(&id).unwrap().is_none());
        let (_, mut saved) = crate::sessions::load_session_by_id(&id).unwrap();
        assert_eq!(saved.len(), 3);
        assert!(harness::handoff::divider(&saved[2]).is_some());
        saved.push(ChatMessage::new("user", "Now add a test"));
        let view = harness::handoff::builtin::request_view(&saved);
        assert_eq!(view.len(), 1);
        assert!(view[0].content.contains("Found it in auth.rs"));
        assert!(view[0].content.ends_with("Now add a test"));
    }

    #[test]
    fn only_a_switch_to_rustcode_comes_here() {
        let request = |harness: &str| super::super::harnesses::SwitchHarnessRequest {
            session_id: "s".into(),
            harness: harness.into(),
            account: None,
            model: None,
            effort: None,
            reason: None,
        };
        assert!(wanted(&request(" rustcode ")));
        assert!(!wanted(&request("codex")));
    }
}
