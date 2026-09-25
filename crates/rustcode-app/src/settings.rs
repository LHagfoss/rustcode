use rustcode::controller::{ControllerSnapshot, ModelChoice};

/// The native settings surface only presents preferences the controller can
/// currently change. Both values are scoped to the running app/session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SettingsState {
    pub models: Vec<ModelChoice>,
    pub selected_model: Option<String>,
    pub auto_approve: bool,
    pub has_session: bool,
}

impl SettingsState {
    pub fn from_snapshot(snapshot: Option<&ControllerSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self {
                auto_approve: true,
                ..Self::default()
            };
        };
        Self {
            models: snapshot.models.clone(),
            selected_model: snapshot.selected_model.clone(),
            auto_approve: snapshot.auto_approve,
            has_session: snapshot.session_id.is_some(),
        }
    }

    pub fn selected_model_label(&self) -> Option<&str> {
        self.models
            .iter()
            .find(|model| self.selected_model.as_deref() == Some(model.id.as_str()))
            .map(|model| model.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode::controller::{ControllerSnapshot, ModelChoice};

    fn snapshot() -> ControllerSnapshot {
        ControllerSnapshot {
            generation: 1,
            workspace: None,
            session_id: Some("session-1".into()),
            sessions: Vec::new(),
            models: vec![
                ModelChoice {
                    id: "local".into(),
                    label: "Local model".into(),
                },
                ModelChoice {
                    id: "remote".into(),
                    label: "Remote model".into(),
                },
            ],
            selected_model: Some("remote".into()),
            transcript: Vec::new(),
            live_response: String::new(),
            queued_count: 0,
            turn_active: false,
            auto_approve: false,
            pending_question: None,
            pending_approval: None,
        }
    }

    #[test]
    fn settings_state_uses_only_controller_supported_preferences() {
        let state = SettingsState::from_snapshot(Some(&snapshot()));

        assert_eq!(state.models.len(), 2);
        assert_eq!(state.selected_model_label(), Some("Remote model"));
        assert!(!state.auto_approve);
        assert!(state.has_session);
    }

    #[test]
    fn settings_without_a_session_does_not_invent_model_choices() {
        let state = SettingsState::from_snapshot(None);

        assert!(state.models.is_empty());
        assert_eq!(state.selected_model_label(), None);
        assert!(!state.has_session);
        assert!(state.auto_approve);
    }
}
