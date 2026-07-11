//! Notebook-first OpenSpec orchestration.

use std::process::ExitCode;

use clap::Parser;
use nbspec::cli::{Cli, Command, ServeService, failure_report};
use nbspec::mcp::{self, McpConfiguration};
use nbspec::operations::{self, OperationError};

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = Cli::parse();
    match dispatch(arguments).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(DispatchError::ChangeVerb(error)) => {
            // Validation failures print their report verbatim — a
            // summary line followed by `note:line: [artifact]
            // message` diagnostic lines — with NO `Error:` prefix so
            // log scrapers can split on the leading summary. Every
            // other failure carries an `Error:` banner.
            eprintln!("{}", failure_report(&error));
            ExitCode::FAILURE
        }
        Err(DispatchError::Service(error)) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Top-level dispatch errors. The two arms map to two distinct
/// terminal-failure presentations: change-verb errors go through
/// [`failure_report`] (which strips the `Error:` banner for
/// validation failures), service errors go through anyhow's banner.
enum DispatchError {
    ChangeVerb(OperationError),
    Service(anyhow::Error),
}

async fn dispatch(arguments: Cli) -> Result<(), DispatchError> {
    match &arguments.command {
        Command::Serve { service } => run_service(service, arguments.notebook.as_deref())
            .await
            .map_err(DispatchError::Service),
        verb => run_change_verb(&arguments, verb).await,
    }
}

async fn run_service(service: &ServeService, notebook: Option<&str>) -> anyhow::Result<()> {
    match service {
        ServeService::Mcp => {
            let configuration = McpConfiguration {
                notebook: notebook.map(String::from),
            };
            mcp::run(configuration).await
        }
    }
}

async fn run_change_verb(arguments: &Cli, command: &Command) -> Result<(), DispatchError> {
    let config = nb_api::Config {
        notebook: arguments.notebook.clone(),
        ..nb_api::Config::default()
    };
    let client = nb_api::NbClient::new(&config)
        .map_err(|e| DispatchError::Service(anyhow::Error::msg(e.to_string())))?;
    let notebook = arguments.notebook.as_deref();
    let output = match command {
        Command::Create { change_id, title } => {
            operations::create(&client, notebook, change_id, title.as_deref()).await
        }
        Command::Display { change_id, full } => {
            operations::display(&client, notebook, change_id, *full).await
        }
        Command::Render { change_id, diff } => {
            operations::render(&client, notebook, change_id, *diff).await
        }
        Command::Merge { change_id, force } => {
            operations::merge(&client, notebook, change_id, *force).await
        }
        Command::Validate { change_id } => operations::validate(&client, notebook, change_id).await,
        Command::Review {
            change_id,
            gate,
            verdict,
            comment,
            reviewer,
        } => {
            operations::review(
                &client,
                notebook,
                change_id,
                gate,
                (*verdict).into(),
                reviewer.as_deref(),
                comment.as_deref(),
            )
            .await
        }
        Command::Serve { .. } => unreachable!("serve dispatched in dispatch()"),
    };
    match output {
        Ok(outcome) => {
            println!("{}", outcome.text);
            Ok(())
        }
        Err(error) => Err(DispatchError::ChangeVerb(error)),
    }
}
