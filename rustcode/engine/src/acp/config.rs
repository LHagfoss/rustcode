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
    if let Some(fast) = fast_option(state) {
        config.push(fast);
    }
    if let Some(protocol) = tool_protocol_option(state) {
        config.push(protocol);
    }
    config
}

/// The ids, names and descriptions of the tool protocols, as the client
/// shows them.
const TOOL_PROTOCOLS: [(crate::config::ToolProtocol, &str, &str, &str); 3] = [
    (
        crate::config::ToolProtocol::ApiNative,
        "apinative",
        "API",
        "The server's own function calling",
    ),
    (
        crate::config::ToolProtocol::Native,
        "native",
        "Text tags",
        "Calls written as [TOOL_CALLS] text",
    ),
    (
        crate::config::ToolProtocol::Json,
        "json",
        "Text blocks",
        "Calls written as ```tool JSON blocks",
    ),
];

/// How the active model sends tool calls, for a model the user configured
/// themselves: a ChatGPT plan always uses the API's calls. Set on the
/// profile, so it lasts beyond the session.
fn tool_protocol_option(state: &crate::app::AppState) -> Option<SessionConfigOption> {
    if state.active_profile_uses_chatgpt() || state.active_model_profile_index().is_none() {
        return None;
    }
    let active = state.active_tool_protocol();
    let current = TOOL_PROTOCOLS
        .iter()
        .find(|(protocol, ..)| *protocol == active)
        .map_or("apinative", |(_, id, ..)| id);
    let options = TOOL_PROTOCOLS
        .iter()
        .map(|(_, id, name, description)| {
            SessionConfigSelectOption::new(*id, *name).description(*description)
        })
        .collect::<Vec<_>>();
    Some(
        SessionConfigOption::select("tool_protocol", "Tool calls", current, options)
            .description("How the model writes tool calls"),
    )
}

/// Fast mode, offered only while the session's model runs on a ChatGPT plan:
/// no other transport has the priority tier it asks for (#2582).
fn fast_option(state: &crate::app::AppState) -> Option<SessionConfigOption> {
    state.active_profile_uses_chatgpt().then(|| {
        SessionConfigOption::boolean("fast", "Fast", state.fast_mode)
            .description("Faster responses; uses your plan's usage faster")
    })
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
            let name = crate::remote::projection::effort_label(effort);
            SessionConfigSelectOption::new(effort.clone(), name)
        })
        .collect::<Vec<_>>();
    Some(
        SessionConfigOption::select("effort", "Effort", current, options)
            .category(SessionConfigOptionCategory::ThoughtLevel)
            .description("Reasoning effort"),
    )
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
    if config_id.0.as_ref() == "fast" {
        // A boolean, or `on`/`off` from a client that only sends value ids.
        let enabled = value
            .as_bool()
            .or_else(|| match value.as_value_id().map(|v| v.0.as_ref()) {
                Some("on" | "true") => Some(true),
                Some("off" | "false") => Some(false),
                _ => None,
            });
        let Some(enabled) = enabled else {
            return Err(agent_client_protocol::Error::invalid_params()
                .data("expected a boolean for the fast option"));
        };
        state.set_fast_mode(enabled);
        return Ok(build_session_config_options(state));
    }
    if config_id.0.as_ref() == "tool_protocol" {
        let id = value.as_value_id().map(|v| v.0.to_string());
        let Some((protocol, ..)) = TOOL_PROTOCOLS
            .iter()
            .find(|(_, known, ..)| Some(*known) == id.as_deref())
        else {
            return Err(agent_client_protocol::Error::invalid_params()
                .data("expected apinative, native or json for the tool_protocol option"));
        };
        state.set_active_tool_protocol(*protocol);
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
    state.config.use_profile_for_session(profile.name);
    state.record_session_model();

    Ok(build_session_config_options(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(profile: crate::config::ModelProfile) -> crate::app::AppState {
        let mut state = crate::app::AppState::new();
        state.active_session_id = rustcode_session::next_session_id();
        let name = profile.name.clone();
        state.api_base_url = profile.url.clone();
        state.model_name = profile.model.clone();
        state.config.models = vec![profile];
        state.config.use_profile_for_session(name);
        state
    }

    fn chatgpt() -> crate::config::ModelProfile {
        crate::config::ModelProfile {
            name: "chatgpt/gpt-6-luna".into(),
            url: "https://api.openai.com/v1/responses".into(),
            model: "gpt-6-luna".into(),
            api_protocol: Some(crate::config::ApiProtocol::Responses),
            credential: Some(crate::provider_auth::CredentialRef {
                provider: "openai".into(),
                account: "test-account".into(),
                method: crate::provider_auth::AuthMethod::ChatGpt,
            }),
            ..Default::default()
        }
    }

    fn fast_value(options: &[SessionConfigOption]) -> Option<serde_json::Value> {
        let options = serde_json::to_value(options).unwrap();
        options
            .as_array()
            .unwrap()
            .iter()
            .find(|option| option["id"] == "fast")
            .cloned()
    }

    // #2582: the fast option is a boolean offered only for ChatGPT plans,
    // and setting it round-trips into the advertised options.
    #[test]
    fn fast_option_round_trips_for_chatgpt_profiles_only() {
        let mut state = state_with(chatgpt());
        let fast = fast_value(&build_session_config_options(&state)).expect("offered");
        assert_eq!(fast["type"], "boolean");
        assert_eq!(fast["currentValue"], false);

        let id = SessionConfigId::new("fast");
        let options =
            handle_set_config_option(&mut state, &id, &SessionConfigOptionValue::boolean(true))
                .unwrap();
        assert!(state.fast_mode);
        assert_eq!(fast_value(&options).unwrap()["currentValue"], true);

        // A client that only sends value ids can say `off`.
        let options =
            handle_set_config_option(&mut state, &id, &SessionConfigOptionValue::value_id("off"))
                .unwrap();
        assert!(!state.fast_mode);
        assert_eq!(fast_value(&options).unwrap()["currentValue"], false);
        assert!(
            handle_set_config_option(
                &mut state,
                &id,
                &SessionConfigOptionValue::value_id("maybe")
            )
            .is_err()
        );

        // The exact request the macOS app sends for its toggle.
        let request: agent_client_protocol::schema::v1::SetSessionConfigOptionRequest =
            serde_json::from_value(serde_json::json!({
                "sessionId": "s", "configId": "fast", "type": "boolean", "value": true
            }))
            .unwrap();
        handle_set_config_option(&mut state, &request.config_id, &request.value).unwrap();
        assert!(state.fast_mode);

        let api_key = state_with(crate::config::ModelProfile {
            name: "openai/gpt-6-luna".into(),
            url: "https://api.openai.com/v1/responses".into(),
            model: "gpt-6-luna".into(),
            ..Default::default()
        });
        assert!(fast_value(&build_session_config_options(&api_key)).is_none());
    }

    fn option_value(options: &[SessionConfigOption], id: &str) -> Option<serde_json::Value> {
        let options = serde_json::to_value(options).unwrap();
        options
            .as_array()
            .unwrap()
            .iter()
            .find(|option| option["id"] == id)
            .cloned()
    }

    // Session 01a1874e…: profiles on one server URL each keep their own
    // protocol, and the client can change the active one's.
    #[test]
    fn tool_protocol_follows_the_active_profile_and_round_trips() {
        use crate::config::{ModelProfile, ToolProtocol};
        let url = "https://tokmax.example/v1/chat/completions";
        let mut state = state_with(ModelProfile {
            name: "lfm3".into(),
            url: url.into(),
            model: "LFM3-1.7B".into(),
            ..Default::default()
        });
        state.config.tool_protocol = ToolProtocol::Json;
        state.config.models.insert(
            0,
            ModelProfile {
                name: "bonsai".into(),
                url: url.into(),
                model: "bonsai-27b".into(),
                tool_protocol: Some(ToolProtocol::Native),
                ..Default::default()
            },
        );
        assert_eq!(
            state.active_tool_protocol(),
            ToolProtocol::Json,
            "not bonsai's pin"
        );

        let option = option_value(&build_session_config_options(&state), "tool_protocol")
            .expect("offered for a configured model");
        assert_eq!(option["currentValue"], "json");
        assert_eq!(option["options"].as_array().unwrap().len(), 3);

        let id = SessionConfigId::new("tool_protocol");
        let options = handle_set_config_option(
            &mut state,
            &id,
            &SessionConfigOptionValue::value_id("apinative"),
        )
        .unwrap();
        assert_eq!(
            option_value(&options, "tool_protocol").unwrap()["currentValue"],
            "apinative"
        );
        assert_eq!(
            state.config.models[1].tool_protocol,
            Some(ToolProtocol::ApiNative)
        );
        assert_eq!(
            state.config.models[0].tool_protocol,
            Some(ToolProtocol::Native)
        );
        assert!(
            handle_set_config_option(&mut state, &id, &SessionConfigOptionValue::value_id("xml"))
                .is_err()
        );

        assert!(
            option_value(
                &build_session_config_options(&state_with(chatgpt())),
                "tool_protocol"
            )
            .is_none()
        );
    }
}
