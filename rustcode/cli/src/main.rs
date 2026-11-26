mod cli;
mod hub_command;
mod hub_member;
mod machines;
mod machines_revoke;
mod manage_command;
mod price_estimate;
mod run;
mod server_command;
mod sync_from;
mod up;

#[cfg(test)]
mod test_isolation;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run::run().await
}
