use clap::Parser;
use musinsa_sku::{config::Cli, pipeline};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match pipeline::run(Cli::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
