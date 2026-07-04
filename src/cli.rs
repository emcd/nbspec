//! Command-line interface definitions.
//!
//! Declares the argument grammar only; command execution lives in
//! [`crate::operations`].

use clap::{Parser, Subcommand};

/// Notebook-first OpenSpec orchestration.
#[derive(Debug, Parser)]
#[command(name = "nbspec", version, about)]
pub struct Cli {
    /// Notebook holding project changes (defaults to a Git-derived name).
    #[arg(long, global = true)]
    pub notebook: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

/// Top-level nbspec commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manages notebook-resident changes.
    #[command(subcommand)]
    Change(ChangeCommand),

    /// Renders a change to a scratch workspace for review.
    Render {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,

        /// Emits a unified diff against current merge targets.
        #[arg(long)]
        diff: bool,
    },

    /// Transfers a change's durable artifacts into the repository.
    Merge {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,

        /// Overwrites merge targets that drifted since the last merge.
        #[arg(long)]
        force: bool,
    },

    /// Validates a change against the OpenSpec grammar.
    Validate {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,
    },
}

/// Change lifecycle subcommands.
#[derive(Debug, Subcommand)]
pub enum ChangeCommand {
    /// Creates a change namespace in the project notebook.
    New {
        /// Change identifier (becomes the folder name under `proposals/`).
        change_id: String,

        /// Human-readable change title.
        #[arg(long)]
        title: Option<String>,
    },

    /// Shows a change's notes.
    Show {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,
    },

    /// Reports a change's artifact, todo, and drift state.
    Status {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,
    },
}
