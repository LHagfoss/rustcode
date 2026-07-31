use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct DiscordRpcHandler {
    client: Option<DiscordIpcClient>,
    start_time: u64,
}

impl DiscordRpcHandler {
    pub fn new() -> Self {
        Self {
            client: None,
            start_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    pub fn connect(&mut self) {
        if self.client.is_none() {
            let client_id = "123456789012345678"; // Replace with your application's client ID
            let mut client_instance = match DiscordIpcClient::new(client_id) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to create Discord RPC client: {}", e);
                    self.client = None;
                    return;
                }
            };

            match client_instance.connect() {
                Ok(_) => {
                    self.client = Some(client_instance);
                    self.set_activity("Idle", "Waiting for input");
                }
                Err(e) => {
                    eprintln!("Failed to connect to Discord RPC: {}", e);
                    self.client = None;
                }
            }
        }
    }

    pub fn set_activity(&mut self, state: &str, details: &str) {
        if let Some(client) = &mut self.client {
            let activity = activity::Activity::new()
                .state(state)
                .details(details)
                .assets(activity::Assets::new().large_image("rustcode_logo"))
                .timestamps(activity::Timestamps::new().start(self.start_time as i64));
            if let Err(e) = client.set_activity(activity) {
                eprintln!("Failed to set Discord RPC activity: {}", e);
                self.client = None; // Disconnect on error
            }
        }
    }

    pub fn clear_activity(&mut self) {
        if let Some(client) = &mut self.client
            && let Err(e) = client.clear_activity() {
                eprintln!("Failed to clear Discord RPC activity: {}", e);
                self.client = None; // Disconnect on error
            }
    }
}
