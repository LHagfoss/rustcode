use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use std::time::{SystemTime, UNIX_EPOCH};

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

        let client_id = "1533154312622964970";
        let mut client_instance = match DiscordIpcClient::new(client_id) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to create Discord RPC client: {}", e);
                return;
            }
        };

        match client_instance.connect() {
            Ok(_) => {
                self.client = Some(client_instance);
            }
            Err(e) => {
                eprintln!("Failed to connect to Discord RPC: {}", e);
                self.client = None;
            }
        }
    }

    pub fn set_activity(&mut self, state: &str, details: &str) {
        if !self.enabled {
            return;
        }
        self.set_activity_internal(state, details, true);
    }

    fn set_activity_internal(&mut self, state: &str, details: &str, allow_reconnect: bool) {
        if !self.enabled {
            return;
        }

        if let Some(client) = &mut self.client {
            let activity = activity::Activity::new()
                .state(state)
                .details(details)
                .assets(activity::Assets::new().large_image("rustcode_logo"))
                .timestamps(activity::Timestamps::new().start(self.start_time as i64));
            if let Err(e) = client.set_activity(activity) {
                eprintln!("Failed to set Discord RPC activity: {}", e);
                self.disconnect();
                if allow_reconnect {
                    self.connect();
                    if self.client.is_some() {
                        self.set_activity_internal(state, details, false);
                    }
                }
            }
        } else {
            self.connect();
            if self.client.is_some() {
                self.set_activity_internal(state, details, false);
            }
        }
    }

    fn clear_activity_internal(&mut self) {
        if let Some(client) = &mut self.client
            && let Err(e) = client.clear_activity()
        {
            eprintln!("Failed to clear Discord RPC activity: {}", e);
            self.disconnect();
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
    use super::activity_for_tools;

    #[test]
    fn tool_activity_distinguishes_idle_tool_execution() {
        assert_eq!(activity_for_tools(0), "Thinking");
        assert_eq!(activity_for_tools(1), "Running tools");
        assert_eq!(activity_for_tools(3), "Running tools");
    }
}
