use crate::app::{AppState, AppStatus, ChatMessage, SessionAction};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionTransition {
    Started {
        session_id: String,
    },
    Resumed {
        session_id: String,
    },
    Forked {
        session_id: String,
        source_session_id: String,
    },
    Cleared {
        session_id: String,
    },
    Archived {
        session_id: String,
    },
    Deleted {
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionError {
    EmptySession,
    NoSessionToResume,
    InvalidSessionId(String),
    SessionNotFound(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySession => f.write_str("the session has no conversation to save"),
            Self::NoSessionToResume => f.write_str("no saved session is available to resume"),
            Self::InvalidSessionId(id) => write!(f, "invalid session id: {id}"),
            Self::SessionNotFound(id) => write!(f, "session not found: {id}"),
        }
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SessionController;

impl SessionController {
    #[allow(dead_code)]
    pub(crate) fn active_session(&self, state: &AppState) -> String {
        state.active_session_id.clone()
    }

    pub(crate) fn start_fresh(
        &self,
        state: &mut AppState,
    ) -> Result<SessionTransition, SessionError> {
        crate::app::actions::start_new_session(state);
        state.request_clear_screen();
        Ok(SessionTransition::Started {
            session_id: state.active_session_id.clone(),
        })
    }

    pub(crate) fn resume(
        &self,
        state: &mut AppState,
        action: SessionAction,
    ) -> Result<SessionTransition, SessionError> {
        let meta = self.resolve_meta(state, action)?;
        let session_id = session_id_from_meta(&meta)
            .ok_or_else(|| SessionError::SessionNotFound(meta.title.clone()))?;
        if crate::config::load_session_file(&meta.path).is_empty() {
            return Err(SessionError::SessionNotFound(session_id));
        }

        crate::app::actions::load_session_into(state, &meta);
        Ok(SessionTransition::Resumed {
            session_id: state.active_session_id.clone(),
        })
    }

    pub(crate) fn fork(
        &self,
        state: &mut AppState,
        action: SessionAction,
    ) -> Result<SessionTransition, SessionError> {
        let (source_session_id, source_history) = match action {
            SessionAction::Latest if crate::config::session_has_content(&state.history) => {
                (state.active_session_id.clone(), state.history.clone())
            }
            SessionAction::Latest => {
                let meta = self.resolve_meta(state, SessionAction::Latest)?;
                let source_id = session_id_from_meta(&meta)
                    .ok_or_else(|| SessionError::SessionNotFound(meta.title.clone()))?;
                (source_id, crate::config::load_session_file(&meta.path))
            }
            SessionAction::Id(id) => {
                validate_session_id(&id)?;
                let meta = self.resolve_meta(state, SessionAction::Id(id.clone()))?;
                (id, crate::config::load_session_file(&meta.path))
            }
        };
        if source_history.is_empty() {
            return Err(SessionError::EmptySession);
        }

        crate::app::actions::start_new_session(state);
        let new_session_id = state.active_session_id.clone();
        state.history = source_history;
        state.history_display_start = 0;
        state.current_response.clear();
        state.current_token_usage = None;
        state.response_time = None;
        state.image_analysis_cache = crate::config::load_session_image_cache(&new_session_id);
        state.session_title_cache = None;
        state.history.push(ChatMessage::new(
            "system",
            format!("Forked session from \"{source_session_id}\""),
        ));
        crate::config::save_session_history(&new_session_id, &state.history);

        Ok(SessionTransition::Forked {
            session_id: new_session_id,
            source_session_id,
        })
    }

    pub(crate) fn clear(&self, state: &mut AppState) -> Result<SessionTransition, SessionError> {
        state.history_display_start = state.history.len();
        state.current_response.clear();
        state.current_token_usage = None;
        state.response_time = None;
        state.status = AppStatus::Idle;
        state.request_clear_screen();
        Ok(SessionTransition::Cleared {
            session_id: state.active_session_id.clone(),
        })
    }

    pub(crate) fn archive(&self, state: &mut AppState) -> Result<SessionTransition, SessionError> {
        if !crate::config::session_has_content(&state.history) {
            return Err(SessionError::EmptySession);
        }
        crate::config::save_session_history(&state.active_session_id, &state.history);
        crate::config::flush_history();
        Ok(SessionTransition::Archived {
            session_id: state.active_session_id.clone(),
        })
    }

    pub(crate) fn delete(
        &self,
        state: &mut AppState,
        action: SessionAction,
    ) -> Result<SessionTransition, SessionError> {
        let session_id = match action {
            SessionAction::Latest => state.active_session_id.clone(),
            SessionAction::Id(id) => {
                validate_session_id(&id)?;
                id
            }
        };
        validate_session_id(&session_id)?;
        let directory = crate::config::get_active_session_dir(&session_id)
            .ok_or_else(|| SessionError::SessionNotFound(session_id.clone()))?;
        let history_path = directory.join("history.json");
        if !directory.exists() {
            return Err(SessionError::SessionNotFound(session_id));
        }

        if history_path.exists() {
            crate::config::delete_session_file(&history_path);
        } else {
            let _ = std::fs::remove_dir_all(&directory);
        }
        if state.active_session_id == session_id {
            state.history.clear();
            crate::app::actions::start_new_session(state);
        }
        Ok(SessionTransition::Deleted { session_id })
    }

    fn resolve_meta(
        &self,
        state: &AppState,
        action: SessionAction,
    ) -> Result<crate::config::SessionMeta, SessionError> {
        let sessions = crate::app::actions::build_session_list(state);
        match action {
            SessionAction::Latest => sessions
                .into_iter()
                .next()
                .ok_or(SessionError::NoSessionToResume),
            SessionAction::Id(id) => {
                validate_session_id(&id)?;
                sessions
                    .into_iter()
                    .find(|meta| session_id_from_meta(meta).as_deref() == Some(id.as_str()))
                    .ok_or(SessionError::SessionNotFound(id))
            }
        }
    }
}

pub(crate) fn session_id_from_meta(meta: &crate::config::SessionMeta) -> Option<String> {
    meta.path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

fn validate_session_id(id: &str) -> Result<(), SessionError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(SessionError::InvalidSessionId(id.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SessionController, SessionError, SessionTransition};
    use crate::app::{AppState, ChatMessage, SessionAction};

    #[test]
    fn clear_preserves_history_but_hides_the_current_transcript() {
        let mut state = AppState::new();
        state.history.push(ChatMessage::new("user", "keep this"));
        state.current_response = "draft".to_owned();
        let history_len = state.history.len();

        let transition = SessionController::default()
            .clear(&mut state)
            .expect("clear should succeed");

        assert_eq!(
            transition,
            SessionTransition::Cleared {
                session_id: state.active_session_id.clone(),
            }
        );
        assert_eq!(state.history.len(), history_len);
        assert_eq!(state.history_display_start, history_len);
        assert!(state.current_response.is_empty());
    }

    #[test]
    fn fork_starts_a_new_session_with_a_copy_of_the_current_history() {
        let mut state = AppState::new();
        let source_id = state.active_session_id.clone();
        state.history.push(ChatMessage::new("user", "old task"));
        state
            .history
            .push(ChatMessage::new("assistant", "old answer"));

        let transition = SessionController::default()
            .fork(&mut state, SessionAction::Latest)
            .expect("fork should succeed");

        assert_ne!(state.active_session_id, source_id);
        assert!(matches!(
            transition,
            SessionTransition::Forked { source_session_id, .. }
                if source_session_id == source_id
        ));
        assert!(
            state
                .history
                .iter()
                .any(|message| { message.role == "user" && message.content == "old task" })
        );
        assert!(
            state
                .history
                .iter()
                .any(|message| message.content.contains("Forked session"))
        );
    }

    #[test]
    fn resume_by_id_restores_the_saved_session() {
        let mut state = AppState::new();
        let saved_id = state.active_session_id.clone();
        state.history.push(ChatMessage::new("user", "saved task"));
        state
            .history
            .push(ChatMessage::new("assistant", "saved answer"));

        SessionController::default()
            .start_fresh(&mut state)
            .expect("new session should succeed");
        assert_ne!(state.active_session_id, saved_id);

        let transition = SessionController::default()
            .resume(&mut state, SessionAction::Id(saved_id.clone()))
            .expect("saved session should resume");

        assert_eq!(
            transition,
            SessionTransition::Resumed {
                session_id: saved_id
            }
        );
        assert!(
            state
                .history
                .iter()
                .any(|message| message.content == "saved task")
        );
    }

    #[test]
    fn delete_rejects_path_like_session_ids_before_touching_disk() {
        let mut state = AppState::new();
        let error = SessionController::default()
            .delete(&mut state, SessionAction::Id("../sessions".to_owned()))
            .expect_err("path-like ids must be rejected");

        assert!(matches!(error, SessionError::InvalidSessionId(_)));
    }
}
