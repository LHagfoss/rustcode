use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use std::time::{SystemTime, UNIX_EPOCH};

const DISCORD_CLIENT_ID: &str = "1533154312622964970";
const DISCORD_LARGE_IMAGE: &str = "rustcode_logo";

pub struct DiscordRpcHandler {
    client: Option<DiscordIpcClient>,
    start_time: u64,
    enabled: bool,
}

impl DiscordRpcHandler {
    pub fn new() -> Self {
        Self {
            client: None,
            start_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            enabled: false,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        if enabled {
            self.enabled = true;
            self.connect();
        } else {
            self.clear_activity_internal();
            self.enabled = false;
            self.disconnect();
        }
    }

    fn disconnect(&mut self) {
        if let Some(mut client) = self.client.take()
            && let Err(e) = client.close()
        {
            eprintln!("Failed to close Discord RPC client: {}", e);
        }
    }

    fn connect(&mut self) {
        if !self.enabled || self.client.is_some() {
            return;
        }

        let mut client_instance = match DiscordIpcClient::new(DISCORD_CLIENT_ID) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to create Discord RPC client: {}", e);
                return;
            }
        };

        match client_instance.connect() {
            Ok(_) => {
                self.client = Some(client_instance);
                self.set_activity_once("Idle", "");
            }
            Err(e) => {
                eprintln!("Failed to connect to Discord RPC: {}", e);
                self.client = None;
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_idle(&mut self, model_name: Option<&str>) {
        let details = model_name.map_or("", |m| m);
        self.set_activity("Idle", details);
    }

    #[allow(dead_code)]
    pub fn set_queued(&mut self, model_name: Option<&str>) {
        let details = model_name.map_or("", |m| m);
        self.set_activity("Queued", details);
    }

    #[allow(dead_code)]
    pub fn set_thinking(&mut self, model_name: Option<&str>) {
        let details = model_name.map_or("", |m| m);
        self.set_activity("Thinking", details);
    }

    #[allow(dead_code)]
    pub fn set_streaming(&mut self, model_name: Option<&str>) {
        let details = model_name.map_or("", |m| m);
        self.set_activity("Streaming", details);
    }

    #[allow(dead_code)]
    pub fn set_running_tools(&mut self, model_name: Option<&str>) {
        let details = model_name.map_or("", |m| m);
        self.set_activity("Running Tools", details);
    }

    pub fn set_activity(&mut self, state: &str, details: &str) {
        if !self.enabled {
            return;
        }
        self.connect();
        if self.set_activity_once(state, details) {
            return;
        }

        self.disconnect();
        self.connect();
        self.set_activity_once(state, details);
    }

    fn set_activity_once(&mut self, state: &str, details: &str) -> bool {
        let Some(client) = &mut self.client else {
            return false;
        };

        let activity = activity::Activity::new()
            .state(state)
            .details(details)
            .assets(activity::Assets::new().large_image(DISCORD_LARGE_IMAGE))
            .timestamps(activity::Timestamps::new().start(self.start_time as i64));
        if let Err(e) = client.set_activity(activity) {
            eprintln!("Failed to set Discord RPC activity: {}", e);
            return false;
        }

        true
    }

    fn clear_activity_internal(&mut self) {
        if let Some(client) = &mut self.client
            && let Err(e) = client.clear_activity()
        {
            eprintln!("Failed to clear Discord RPC activity: {}", e);
        }
    }

    pub fn shutdown(&mut self) {
        self.clear_activity_internal();
        self.enabled = false;
        self.disconnect();
    }
}

impl Drop for DiscordRpcHandler {
    fn drop(&mut self) {
        self.clear_activity_internal();
        self.disconnect();
    }
}

pub(crate) fn activity_for_tools(running_tools: usize) -> &'static str {
    if running_tools == 0 {
        "Thinking"
    } else {
        "Running tools"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_activity_distinguishes_idle_tool_execution() {
        assert_eq!(activity_for_tools(0), "Thinking");
        assert_eq!(activity_for_tools(1), "Running tools");
        assert_eq!(activity_for_tools(3), "Running tools");
    }

    #[test]
    fn shutdown_is_safe_when_no_client_connected() {
        let mut handler = DiscordRpcHandler::new();
        // No client connected
        handler.shutdown();
        // Should not panic or error
        assert!(handler.client.is_none());
    }

    #[test]
    fn set_enabled_false_is_safe_when_no_client_connected() {
        let mut handler = DiscordRpcHandler::new();
        // No client connected
        handler.set_enabled(false);
        // Should not panic or error
        assert!(handler.client.is_none());
    }
}
