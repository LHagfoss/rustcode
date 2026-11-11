//! A thread a paired device asked for, started here (`docs/remote.md`,
//! *Threads started from a device*).
//!
//! The gateway routes `create_session` to this engine while a share of it
//! advertises `host_threads` — the desktop app, with Remote access on. The
//! thread is created in the running engine, not by a detached session owner,
//! so it runs on any harness, appears in the app's sidebar and is shared like
//! any other thread of this app.

use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{Meta, NewSessionRequest};
use serde_json::json;

use super::AcpServer;
use super::clients::{Capability, Peer};
use super::remote::{RemoteSession, ThreadStart};
use crate::harness::{self, HarnessKind};
use crate::remote::{RemoteError, RemoteErrorCode, RemoteSessionInfo};

/// The harness (and model) a requested thread starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub harness: String,
    pub model: Option<String>,
}

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(RemoteErrorCode::InvalidRequest, message)
}

fn internal(message: impl Into<String>) -> RemoteError {
    RemoteError::new(RemoteErrorCode::Internal, message)
}

/// The harness and model a device asked for, checked against what this host
/// has. `Err` is what the device is told: an unknown harness or model names
/// the value and nothing is started.
pub(crate) fn choice(ask: &ThreadStart) -> Result<Choice, RemoteError> {
    let dir = harness::config::config_dir()
        .map_err(|error| internal(format!("harness configuration is unavailable: {error}")))?;
    let (defaults, entries) = harness::config::load_in(&dir);
    let asked = ask
        .harness
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let id = match asked {
        Some(id) => id.to_owned(),
        // A device that names no harness gets the one a new thread starts on.
        None => defaults
            .harness
            .clone()
            .unwrap_or_else(|| harness::BUILTIN_ID.to_owned()),
    };
    let resolved = harness::resolve(&entries, &id).ok_or_else(|| {
        let all = harness::resolve_all(&entries);
        let known: Vec<&str> = all.iter().map(|harness| harness.id.as_str()).collect();
        invalid(format!(
            "unknown harness '{id}'; this host has {}",
            known.join(", ")
        ))
    })?;
    if !resolved.enabled {
        return Err(invalid(format!(
            "the harness '{id}' is turned off on this host; turn it on in Settings › Harnesses"
        )));
    }
    let model = ask
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned);
    if let Some(model) = &model {
        // Only a harness whose models this host already knows can be checked;
        // one that has never listed them takes the value and answers for
        // itself when the thread starts.
        let listed = if resolved.kind == HarnessKind::Builtin {
            harness::builtin_models()
        } else {
            harness::learned_for(&id).models.unwrap_or_default()
        };
        if !listed.is_empty() && !listed.iter().any(|known| known.id == *model) {
            return Err(invalid(format!(
                "unknown model '{model}' for the harness '{id}'; this host lists {}",
                listed
                    .iter()
                    .map(|known| known.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    Ok(Choice { harness: id, model })
}

/// The folder the thread runs in. The gateway resolved and checked it already;
/// this re-reads it so a folder that went away says so.
fn directory(path: Option<&str>) -> Result<PathBuf, RemoteError> {
    let Some(path) = path.map(PathBuf::from) else {
        return Err(invalid(
            "no folder was named for the new thread; choose one in the app or on the device",
        ));
    };
    let canonical = path
        .canonicalize()
        .map_err(|error| invalid(format!("{} cannot be read: {error}", path.display())))?;
    if !canonical.is_dir() {
        return Err(invalid(format!("{} is not a folder", canonical.display())));
    }
    // The thread's folder is stored and listed as people write it: the verbatim
    // prefix a canonical Windows path carries is a Win32 detail, not a project (#3425).
    Ok(rustcode_session::strip_verbatim(&canonical))
}

/// The `_meta.rustcode` a `session/new` would carry for this choice.
fn meta(choice: &Choice) -> Meta {
    let mut rustcode = json!({"harness": choice.harness});
    if let Some(model) = &choice.model
        && let Some(object) = rustcode.as_object_mut()
    {
        object.insert("model".to_owned(), json!(model));
    }
    let mut meta = Meta::new();
    meta.insert("rustcode".to_owned(), rustcode);
    meta
}

fn acp_failure(error: agent_client_protocol::Error) -> RemoteError {
    let detail = error
        .data
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or(error.message);
    internal(detail)
}

/// Start the thread `ask` names, share it, and answer with its row. The
/// newest connected client that lists threads has it open.
pub(crate) async fn start(
    server: &AcpServer,
    ask: ThreadStart,
) -> Result<RemoteSessionInfo, RemoteError> {
    let Some(peer) = server.clients.newest_with(Capability::Threads) else {
        return Err(internal(
            "no RustCode app is connected to open the new thread".to_owned(),
        ));
    };
    let choice = choice(&ask)?;
    let cwd = directory(ask.path.as_deref())?;
    let session_id = if choice.harness == harness::BUILTIN_ID {
        builtin(server, &peer, &cwd, &choice).await?
    } else {
        external(server, &peer, &cwd, &choice).await?
    };
    let info = server
        .remote
        .thread_info(&session_id)
        .ok_or_else(|| internal(format!("the new thread {session_id} is not shared")))?;
    // The app lists its threads from the store, which has no row for a thread
    // before its first turn: this is what puts it in the sidebar now.
    server.clients.broadcast_to(
        Capability::Threads,
        super::ThreadCreatedNotification {
            session_id: info.session_id.clone(),
            cwd: cwd.clone(),
            title: (!info.title.trim().is_empty()).then(|| info.title.clone()),
            harness: info.harness.clone(),
            model: (!info.model.trim().is_empty()).then(|| info.model.clone()),
        },
    );
    Ok(info)
}

/// A thread on the built-in harness: the same session `session/new` creates,
/// shared from its `AppState`, so a prompt from the device runs a turn here.
async fn builtin(
    server: &AcpServer,
    peer: &Peer,
    cwd: &Path,
    choice: &Choice,
) -> Result<String, RemoteError> {
    let mut request = NewSessionRequest::new(cwd.to_path_buf());
    request.meta = Some(meta(choice));
    let response = super::create_builtin_session(server, peer, request)
        .await
        .map_err(acp_failure)?;
    let session_id = response.session_id.to_string();
    let session = {
        let sessions = server.sessions.lock().await;
        sessions.get(&session_id).map(|session| {
            RemoteSession::from_acp(
                session_id.clone(),
                session,
                server.clients.session(&session_id),
                std::sync::Arc::clone(&server.client),
                server.auto_approve,
                server.presence.clone(),
                server.remote.clone(),
            )
        })
    };
    let Some(session) = session else {
        return Err(internal(format!(
            "the new thread {session_id} is not open in this app"
        )));
    };
    server
        .remote
        .start(session, false)
        .await
        .map_err(|error| acp_failure(error.into_acp()))?;
    Ok(session_id)
}

/// A thread on an external harness: `session/new`'s own path, which starts the
/// harness in the thread's folder, then the share of its updates.
async fn external(
    server: &AcpServer,
    peer: &Peer,
    cwd: &Path,
    choice: &Choice,
) -> Result<String, RemoteError> {
    let mut request = NewSessionRequest::new(cwd.to_path_buf());
    request.meta = Some(meta(choice));
    let asked = harness::HarnessRequest {
        harness: Some(choice.harness.clone()),
        account: None,
        model: choice.model.clone(),
        effort: None,
    };
    let response =
        super::harnesses::new_session(&server.external, request, asked, peer, &server.remote)
            .await
            .map_err(acp_failure)?;
    let session_id = response.session_id.to_string();
    let session = super::harnesses::get(&server.external, &session_id)
        .await
        .ok_or_else(|| {
            internal(format!(
                "the new thread {session_id} is not open in this app"
            ))
        })?;
    server
        .remote
        .start_harness(
            session_id.clone(),
            session,
            Some(server.clients.session(&session_id)),
            false,
        )
        .await
        .map_err(|error| acp_failure(error.into_acp()))?;
    Ok(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> PathBuf {
        harness::config::config_dir().expect("a configuration directory")
    }

    #[test]
    fn an_unknown_harness_is_named_and_nothing_starts() {
        let error = choice(&ThreadStart {
            harness: Some("claude-kode".into()),
            ..ThreadStart::default()
        })
        .unwrap_err();
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);
        assert!(
            error.message.contains("unknown harness 'claude-kode'"),
            "{error:?}"
        );
    }

    #[test]
    fn a_harness_that_is_turned_off_is_refused() {
        let dir = configured();
        harness::config::edit_in(&dir, |_, entries| {
            entries.push(harness::config::HarnessEntry {
                id: "codex".into(),
                enabled: Some(false),
                ..harness::config::HarnessEntry::default()
            });
            Ok(())
        })
        .unwrap();
        let error = choice(&ThreadStart {
            harness: Some("codex".into()),
            ..ThreadStart::default()
        })
        .unwrap_err();
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);
        assert!(error.message.contains("'codex' is turned off"), "{error:?}");
    }

    #[test]
    fn a_model_the_harness_does_not_list_is_named() {
        harness::config::edit_in(&configured(), |_, entries| {
            entries.push(harness::config::HarnessEntry {
                id: "claude-code".into(),
                ..harness::config::HarnessEntry::default()
            });
            Ok(())
        })
        .unwrap();
        harness::learn("claude-code", |learned| {
            learned.models = Some(vec![harness::HarnessModel {
                id: "sonnet".into(),
                name: "Sonnet".into(),
                efforts: Vec::new(),
                default_effort: None,
                efforts_unknown: false,
            }]);
        });
        let picked = choice(&ThreadStart {
            harness: Some("claude-code".into()),
            model: Some("sonnet".into()),
            ..ThreadStart::default()
        })
        .unwrap();
        assert_eq!(picked.model.as_deref(), Some("sonnet"));
        let error = choice(&ThreadStart {
            harness: Some("claude-code".into()),
            model: Some("sonnet-5".into()),
            ..ThreadStart::default()
        })
        .unwrap_err();
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);
        assert!(
            error.message.contains("unknown model 'sonnet-5'"),
            "{error:?}"
        );
        harness::forget_learned("claude-code");
    }

    #[test]
    fn the_hosts_default_harness_is_used_when_none_is_named() {
        let dir = configured();
        harness::config::edit_in(&dir, |defaults, entries| {
            defaults.harness = Some("codex".into());
            entries.push(harness::config::HarnessEntry {
                id: "codex".into(),
                ..harness::config::HarnessEntry::default()
            });
            Ok(())
        })
        .unwrap();
        let picked = choice(&ThreadStart::default()).unwrap();
        assert_eq!(picked.harness, "codex");
    }
}
