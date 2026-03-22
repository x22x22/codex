use clap::Parser;
use codex_executor::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    codex_executor::run_main(Cli::parse()).await
}
