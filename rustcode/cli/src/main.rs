// The engine's ACP router is one deeply nested future type, laid out in
// this binary; see the same limit in the engine crate.
#![recursion_limit = "1024"]

mod cli;
mod hub_command;
mod hub_member;
mod machines;
mod manage_command;
mod price_estimate;
mod run;
mod server_command;
mod up;

#[cfg(test)]
mod test_isolation;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run::run().await
}
