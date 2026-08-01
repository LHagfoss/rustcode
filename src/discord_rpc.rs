use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct DiscordRpcHandler {
    client: Option<DiscordIpcClient>,
    start_time: u64,
    enabled: bool,
    reconnect_attempts: u8,
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
            reconnect_attempts: 0,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if enabled {
            self.connect();
        } else {
            self.clear_activity();
            self.disconnect();
        }
    }

    fn disconnect(&mut self) {
        if let Some(mut client) = self.client.take()
            && let Err(e) = client.close()
        {
            eprintln!("Failed to close Discord RPC client: {}", e);
        }
        self.reconnect_attempts = 0;
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
                self.reconnect_attempts = 0;
                self.set_activity_internal("Idle", "Waiting for input");
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
        self.set_activity_internal(state, details);
    }

    fn set_activity_internal(&mut self, state: &str, details: &str) {
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
                self.client = None; // Disconnect on error, attempt reconnect on next activity update
                if self.reconnect_attempts == 0 {
                    self.reconnect_attempts += 1;
                    self.connect();
                    if self.client.is_some() {
                        // Retry setting activity after successful reconnect
                        self.set_activity_internal(state, details);
                    }
                }
            }
        } else {
            // Client is not connected, try to connect
            self.connect();
            if self.client.is_some() {
                // If connected, try setting activity
                self.set_activity_internal(state, details);
            }
        }
    }

    pub fn clear_activity(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(client) = &mut self.client
            && let Err(e) = client.clear_activity()
        {
            eprintln!("Failed to clear Discord RPC activity: {}", e);
            self.client = None; // Disconnect on error
        }
    }
}
