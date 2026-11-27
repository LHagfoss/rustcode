mod cli;
mod inline_terminal;
mod run;
mod runtime;
mod terminal_probe;
mod ui;

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
