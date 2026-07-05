//! Notebook-first OpenSpec orchestration.

use std::process::ExitCode;

use clap::Parser;
use nbspec::cli::{Cli, Command, failure_report};
use nbspec::operations::{self, OperationError};

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = Cli::parse();
    let config = nb_api::Config {
        notebook: arguments.notebook.clone(),
        ..nb_api::Config::default()
    };
    let client = match nb_api::NbClient::new(&config) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::FAILURE;
        }
    };
    match run(&client, &arguments).await {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", failure_report(&error));
            ExitCode::FAILURE
        }
    }
}

async fn run(client: &nb_api::NbClient, arguments: &Cli) -> Result<String, OperationError> {
    let notebook = arguments.notebook.as_deref();
    match &arguments.command {
        Command::Create { change_id, title } => {
            operations::create(client, notebook, change_id, title.as_deref()).await
        }
        Command::Display { change_id, full } => {
            operations::display(client, notebook, change_id, *full).await
        }
        Command::Render { change_id, diff } => {
            operations::render(client, notebook, change_id, *diff).await
        }
        Command::Merge { change_id, force } => {
            operations::merge(client, notebook, change_id, *force).await
        }
        Command::Validate { change_id } => operations::validate(client, notebook, change_id).await,
    }
}
