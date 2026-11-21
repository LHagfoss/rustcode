mod cli;
mod inline_terminal;
mod run;
mod runtime;
mod terminal_probe;
mod ui;

#[cfg(test)]
mod test_isolation;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run::run().await
}
