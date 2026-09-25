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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelMenuChoice {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

pub(crate) fn model_menu_choices(
    models: &[ModelChoice],
    selected: Option<&str>,
) -> Vec<ModelMenuChoice> {
    models
        .iter()
        .map(|model| ModelMenuChoice {
            id: model.id.clone(),
            label: model.label.clone(),
            selected: selected == Some(model.id.as_str()),
        })
        .collect()
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

    pub fn approval_mode_label(&self) -> &'static str {
        if !self.has_session {
            "Unavailable"
        } else if self.auto_approve {
            "Auto approve"
        } else {
            "Ask first"
        }
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
    fn settings_approval_label_tracks_the_persisted_controller_value() {
        let ask_first = SettingsState::from_snapshot(Some(&snapshot()));
        let auto_approve = SettingsState::from_snapshot(Some(&ControllerSnapshot {
            auto_approve: true,
            ..snapshot()
        }));

        assert_eq!(ask_first.approval_mode_label(), "Ask first");
        assert_eq!(auto_approve.approval_mode_label(), "Auto approve");
        assert!(ask_first.has_session);
        assert!(auto_approve.has_session);
    }

    #[test]
    fn settings_without_a_session_marks_permissions_unavailable() {
        let state = SettingsState::from_snapshot(None);

        assert!(state.models.is_empty());
        assert_eq!(state.selected_model_label(), None);
        assert!(!state.has_session);
        assert_eq!(state.approval_mode_label(), "Unavailable");
    }

    #[test]
    fn settings_snapshot_without_an_active_session_marks_permissions_unavailable() {
        let state = SettingsState::from_snapshot(Some(&ControllerSnapshot {
            session_id: None,
            ..snapshot()
        }));

        assert!(!state.has_session);
        assert_eq!(state.approval_mode_label(), "Unavailable");
    }

    #[test]
    fn model_menu_choices_preserve_large_profile_lists_and_profile_aliases() {
        // These two distinct profile names intentionally stand in for aliases
        // targeting the same underlying provider/model.
        let models = (0..31)
            .map(|index| match index {
                0 => ModelChoice {
                    id: "qwen-primary".into(),
                    label: "Qwen 3.5 · Primary".into(),
                },
                1 => ModelChoice {
                    id: "qwen-backup".into(),
                    label: "Qwen 3.5 · Backup".into(),
                },
                30 => ModelChoice {
                    id: "profile-30".into(),
                    label: "Final configured profile".into(),
                },
                _ => ModelChoice {
                    id: format!("profile-{index}"),
                    label: format!("Configured profile {index}"),
                },
            })
            .collect::<Vec<_>>();

        let snapshot = ControllerSnapshot {
            models,
            ..snapshot()
        };
        let state = SettingsState::from_snapshot(Some(&snapshot));
        let choices = model_menu_choices(&state.models, Some("qwen-backup"));

        assert_eq!(choices.len(), 31);
        assert_eq!(choices[0].id, "qwen-primary");
        assert_eq!(choices[0].label, "Qwen 3.5 · Primary");
        assert!(!choices[0].selected);
        assert_eq!(choices[1].id, "qwen-backup");
        assert_eq!(choices[1].label, "Qwen 3.5 · Backup");
        assert!(choices[1].selected);
        assert_eq!(choices[30].id, "profile-30");
        assert_eq!(choices[30].label, "Final configured profile");
        assert!(!choices[30].selected);
    }
}
