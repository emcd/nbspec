//! Command-line interface definitions.
//!
//! Declares the argument grammar and terminal failure presentation;
//! command execution lives in [`crate::operations`] for the change
//! verbs and in [`crate::mcp`] for the `serve mcp` subcommand. All
//! change verbs are flat verbs operating on a change, mirroring the
//! tool vocabulary the MCP surface exposes.

use clap::{Parser, Subcommand};

use crate::operations::OperationError;

/// Formats a failed operation for the terminal. A validation failure
/// prints its report verbatim — a summary line followed by
/// `note:line: [artifact] message` diagnostic lines that agents
/// parse — while every other failure carries an `Error:` banner.
pub fn failure_report(error: &OperationError) -> String {
    match error {
        OperationError::Invalid(failure) => failure.to_string(),
        other => format!("Error: {other}"),
    }
}

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
    /// Creates a change namespace in the project notebook.
    Create {
        /// Change identifier (becomes the folder name under `proposals/`).
        change_id: String,

        /// Human-readable change title.
        #[arg(long)]
        title: Option<String>,
    },

    /// Displays a change: status summary by default, note contents
    /// with --full.
    Display {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,

        /// Includes artifact note contents and folder listings.
        #[arg(long)]
        full: bool,
    },

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

    /// Validates a change against the OpenSpec grammar and its schema.
    ///
    /// Exits zero with a one-line summary when the change is valid.
    /// Otherwise exits nonzero with a summary line followed by one
    /// diagnostic per line in `note:line: [artifact] message` form,
    /// each anchored to a notebook note.
    Validate {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,
    },

    /// Runs a long-running service exposed by nbspec.
    ///
    /// Long-running protocol servers nest under this verb so they
    /// share the parent binary's release artifact, configuration
    /// surface, and operator help output (`nbspec --help`). v0.2.0
    /// ships the `mcp` service; later cycles may add others.
    Serve {
        #[command(subcommand)]
        service: ServeService,
    },
}

/// Long-running services exposed by `nbspec serve`.
#[derive(Debug, Subcommand)]
pub enum ServeService {
    /// Runs the Model Context Protocol server on stdio. Wraps the
    /// same operations library the change verbs dispatch to and
    /// exposes one MCP tool per CLI verb.
    Mcp,
}
