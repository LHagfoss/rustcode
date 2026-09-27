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

    vec![model_option]
}

pub(crate) fn handle_set_config_option(
    state: &mut crate::app::AppState,
    config_id: &SessionConfigId,
    value: &SessionConfigOptionValue,
) -> Result<Vec<SessionConfigOption>, agent_client_protocol::Error> {
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

    Ok(build_session_config_options(state))
}
