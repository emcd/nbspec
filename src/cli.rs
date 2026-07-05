//! Command-line interface definitions.
//!
//! Declares the argument grammar only; command execution lives in
//! [`crate::operations`]. All commands are flat verbs operating on a
//! change, mirroring the tool vocabulary planned for the MCP surface.

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
}
