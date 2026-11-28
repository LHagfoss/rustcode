mod agent_command;
mod cli;
mod hub_command;
mod hub_member;
mod machines;
mod machines_revoke;
mod manage_command;
mod price_estimate;
mod relay_command;
mod run;
mod server_command;
mod sync_from;
mod terminal_command;
mod up;

#[cfg(test)]
mod test_isolation;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if rustcode::computer_worker::requested() {
        rustcode::computer_worker::run();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run::run())
}
