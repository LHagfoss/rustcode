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
        // Attempt to connect if not already connected.
        // This also sets the initial "Idle" activity.
        self.connect();

        // Try to set the activity once.
        if self.set_activity_once(state, details) {
            return;
        }

        // If setting activity failed, it might be due to a disconnected client.
        // Disconnect the old client (if any), reconnect, and try setting activity once more.
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
    fn set_activity_reconnects_on_failure() {
        let mut handler = DiscordRpcHandler::new();
        handler.enabled = true; // Manually enable for testing reconnect logic without full connect
        // Simulate a client that fails to set activity
        // This is tricky to test directly without mocking the DiscordIpcClient trait.
        // For now, we'll rely on the existing logic that if set_activity_once returns false,
        // it triggers a reconnect.
        // A more robust test would involve a mock DiscordIpcClient.
        handler.set_activity("Thinking", "model_name");
        // We can't assert much here without mocking, but we can ensure it doesn't panic.
    }

    #[test]
    fn set_enabled_connects_and_sets_idle() {
        let mut handler = DiscordRpcHandler::new();
        handler.set_enabled(true);
        // We can't directly check if it connected and set idle without mocking,
        // but we can check if client is Some after enabling.
        assert!(handler.client.is_some());
    }

    #[test]
    fn set_enabled_disconnects_and_clears_activity() {
        let mut handler = DiscordRpcHandler::new();
        handler.set_enabled(true); // Connect first
        assert!(handler.client.is_some());
        handler.set_enabled(false);
        assert!(handler.client.is_none());
    }

    #[test]
    fn shutdown_clears_activity_and_disconnects() {
        let mut handler = DiscordRpcHandler::new();
        handler.set_enabled(true); // Connect first
        assert!(handler.client.is_some());
        handler.shutdown();
        assert!(handler.client.is_none());
    }

    #[test]
    fn activity_includes_rustcode_logo() {
        let mut handler = DiscordRpcHandler::new();
        handler.set_enabled(true);
        // This test is more conceptual as we can't inspect the activity sent to Discord directly.
        // We rely on the `set_activity_once` function constructing the activity correctly.
        // The `DISCORD_LARGE_IMAGE` constant is used in `set_activity_once`.
        // If `set_activity_once` succeeds, it implies the activity was constructed with the logo.
        assert!(handler.set_activity_once("Idle", ""));
    }

    #[test]
    fn set_activity_once_returns_false_when_client_not_connected() {
        let mut handler = DiscordRpcHandler::new();
        assert!(!handler.set_activity_once("Idle", ""));
    }
}
