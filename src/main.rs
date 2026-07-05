//! Notebook-first OpenSpec orchestration.

use clap::Parser;
use nbspec::cli::{Cli, Command};
use nbspec::operations;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Cli::parse();
    let config = nb_api::Config {
        notebook: arguments.notebook.clone(),
        ..nb_api::Config::default()
    };
    let client = nb_api::NbClient::new(&config)?;
    let notebook = arguments.notebook.as_deref();
    let output = match &arguments.command {
        Command::Create { change_id, title } => {
            operations::create(&client, notebook, change_id, title.as_deref()).await?
        }
        Command::Display { change_id, full } => {
            operations::display(&client, notebook, change_id, *full).await?
        }
        Command::Render { change_id, diff } => {
            operations::render(&client, notebook, change_id, *diff).await?
        }
        Command::Merge { change_id, force } => {
            operations::merge(&client, notebook, change_id, *force).await?
        }
        Command::Validate { change_id } => {
            operations::validate(&client, notebook, change_id).await?
        }
    };
    println!("{output}");
    Ok(())
}
