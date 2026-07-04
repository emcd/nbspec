//! Notebook-first OpenSpec orchestration.

use clap::Parser;
use nbspec::cli::{ChangeCommand, Cli, Command};
use nbspec::operations;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Cli::parse();
    let config = nb_api::Config {
        notebook: arguments.notebook.clone(),
        ..nb_api::Config::default()
    };
    let client = nb_api::NbClient::new(&config)?;
    let output = match &arguments.command {
        Command::Change(change_command) => match change_command {
            ChangeCommand::New { change_id, title } => {
                operations::change_new(&client, change_id, title.as_deref()).await?
            }
            ChangeCommand::Show { change_id } => {
                operations::change_show(&client, change_id).await?
            }
            ChangeCommand::Status { change_id } => {
                operations::change_status(&client, change_id).await?
            }
        },
        Command::Render { change_id, diff } => {
            operations::render(&client, change_id, *diff).await?
        }
        Command::Merge { change_id, force } => {
            operations::merge(&client, change_id, *force).await?
        }
        Command::Validate { change_id } => operations::validate(&client, change_id).await?,
    };
    println!("{output}");
    Ok(())
}
