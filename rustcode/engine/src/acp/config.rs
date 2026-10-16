use agent_client_protocol::schema::v1::{
    SessionConfigId, SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOption,
};

pub(crate) fn build_session_config_options(
    state: &crate::app::AppState,
) -> Vec<SessionConfigOption> {
    let current_value = state
        .active_model_profile()
        .map(|p| p.name)
        .unwrap_or_else(|| state.config.default.big().to_string());

    let options: Vec<SessionConfigSelectOption> = state
        .config
        .models
        .iter()
        .map(|profile| {
            let mut opt =
                SessionConfigSelectOption::new(profile.name.clone(), profile.name.clone());
            if !profile.model.is_empty() {
                opt = opt.description(profile.model.clone());
            }
            opt
        })
        .collect();

    let model_option = SessionConfigOption::select("model", "Model", current_value, options)
        .category(SessionConfigOptionCategory::Model)
        .description("Model profile");

    let mut config = vec![model_option];
    if let Some(effort) = effort_option(state) {
        config.push(effort);
    }
    config
}

/// Reasoning effort for the active profile, with the same choices as the
/// terminal and remote pickers ("default" leaves it to the provider). Absent
/// when the profile offers no choice.
fn effort_option(state: &crate::app::AppState) -> Option<SessionConfigOption> {
    let profile = state.active_model_profile()?;
    let efforts = crate::remote::projection::reasoning_efforts(&profile);
    if efforts.len() < 2 {
        return None;
    }
    let current = crate::remote::projection::project_settings(state).reasoning_effort;
    let options = efforts
        .iter()
        .map(|effort| {
            let name = if effort == "default" {
                "Off".to_owned()
            } else {
                capitalized(effort)
            };
            SessionConfigSelectOption::new(effort.clone(), name)
        })
        .collect::<Vec<_>>();
    Some(
        SessionConfigOption::select("effort", "Effort", current, options)
            .category(SessionConfigOptionCategory::ThoughtLevel)
            .description("Reasoning effort"),
    )
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub(crate) fn handle_set_config_option(
    state: &mut crate::app::AppState,
    config_id: &SessionConfigId,
    value: &SessionConfigOptionValue,
) -> Result<Vec<SessionConfigOption>, agent_client_protocol::Error> {
    if config_id.0.as_ref() == "effort" {
        let Some(effort) = value.as_value_id().map(|v| v.0.to_string()) else {
            return Err(agent_client_protocol::Error::invalid_params()
                .data("expected value_id for select config option"));
        };
        let model = crate::remote::projection::project_settings(state).selected_model;
        crate::remote::ops::set_settings(state, &model, &effort)
            .map_err(|error| agent_client_protocol::Error::invalid_params().data(error.message))?;
        return Ok(build_session_config_options(state));
    }
    if config_id.0.as_ref() != "model" {
        return Err(agent_client_protocol::Error::invalid_params()
            .data(format!("unknown config option: {}", config_id.0)));
    }

    let Some(model_value) = value.as_value_id().map(|v| v.0.as_ref()) else {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("expected value_id for select config option"));
    };

    let matching_profile = state
        .config
        .models
        .iter()
        .find(|m| m.name == model_value)
        .cloned();

    let Some(profile) = matching_profile else {
        return Err(agent_client_protocol::Error::invalid_params()
            .data(format!("unknown model option value: {model_value}")));
    };

    state.api_base_url = profile.url;
    state.model_name = profile.model;
    state.config.default.set_big(profile.name);
    state.record_session_model();

    Ok(build_session_config_options(state))
}
