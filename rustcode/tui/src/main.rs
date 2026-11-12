// The engine's ACP router is one deeply nested future type, laid out in
// this binary; see the same limit in the engine crate.
#![recursion_limit = "1024"]

mod cli;
mod inline_terminal;
mod machines;
mod run;
mod runtime;
mod server_command;
mod terminal_probe;
mod ui;
mod up;

#[cfg(test)]
mod test_isolation;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run::run().await
}
