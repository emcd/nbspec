//! Command-line interface definitions.
//!
//! Declares the argument grammar and terminal failure presentation;
//! command execution lives in [`crate::operations`] for the change
//! verbs and in [`crate::mcp`] for the `serve mcp` subcommand. All
//! change verbs are flat verbs operating on a change, mirroring the
//! tool vocabulary the MCP surface exposes.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::operations::OperationError;
use crate::reviews::VerdictValue;

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

    /// Records a review verdict binding the change's current content.
    ///
    /// The verdict binds the aggregate content hash of the change's
    /// full rendered artifact set: any subsequent edit stales it.
    /// Each verdict is one immutable note under the change's
    /// verdicts/ subfolder; recording never modifies existing
    /// verdicts and never transitions change lifecycle.
    Review {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,

        /// Review gate the verdict addresses.
        #[arg(long, default_value = "merge")]
        gate: String,

        /// Verdict value.
        #[arg(long, value_enum)]
        verdict: VerdictArg,

        /// Comment content, e.g. a findings note selector. Taken
        /// literally — no value is a stdin or file marker. A comment
        /// (from here or --comment-file) is REQUIRED for a revise
        /// verdict; optional for approve.
        #[arg(long, conflicts_with = "comment_file")]
        comment: Option<String>,

        /// File to read the comment from; pass - to read standard
        /// input instead. CLI-only affordance: the MCP tool takes
        /// only the literal comment string.
        #[arg(long)]
        comment_file: Option<String>,

        /// Reviewer identity; defaults to Git user.name. An explicit
        /// empty value is refused, never treated as absence.
        #[arg(long)]
        reviewer: Option<String>,
    },

    /// Brings a filesystem OpenSpec-style change tree into the
    /// notebook-resident model.
    ///
    /// Walks `<root>` for active change trees
    /// (`<root>/<change-id>/{proposal.md,specs/<cap>/spec.md,
    /// design.md,tasks.md,decisions/<adr>.md}`) and legacy archive
    /// trees (`<root>/openspec/changes/archive/<change-id>/...`).
    /// Active trees are emitted as `ActiveWrite` entries with a
    /// `paused` status: the v0.3.0 execute arm is a no-op that
    /// leaves the source filesystem tree untouched, pending an
    /// NbApi 0.3 notebook transaction/checkpoint primitive. Legacy
    /// archives become deterministic
    /// `documentation/archives/<change-id>.tar.zst` archives. All
    /// refusals are collected before any write.
    Import {
        /// Filesystem root to scan for change trees.
        root: PathBuf,

        /// Emit the plan only; do not write notes or archives and
        /// do not delete the source filesystem tree.
        #[arg(long)]
        dry_run: bool,

        /// Authorize deletion of the source filesystem tree after
        /// a clean round-trip proof. Refused absent a clean proof.
        #[arg(long)]
        delete_original: bool,

        /// Skip active change tree detection (default: detect
        /// both). In v0.3.0 the active execute arm is a no-op;
        /// this flag emits a `Skip` entry instead of a paused
        /// `ActiveWrite` entry for each detected active tree.
        #[arg(long)]
        no_active: bool,

        /// Skip legacy archive tree ingestion (default: ingest both).
        #[arg(long)]
        no_archives: bool,
    },

    /// Writes a notebook change back out as a filesystem OpenSpec
    /// tree.
    ///
    /// Inverse of `import`: walks the notebook change and writes
    /// `<target>/<change-id>/{proposal.md,specs/<cap>/spec.md,
    /// design.md,tasks.md,decisions/<adr>.md}`. The `work` todo
    /// note is reconstructed as `tasks.md`; verdicts do not export.
    /// Refuses to overwrite an existing target unless `--overwrite`
    /// is given.
    Export {
        /// Change identifier (notebook folder under `proposals/`).
        change_id: String,

        /// Filesystem directory under which to write the change tree.
        target: PathBuf,

        /// Emit the plan only; do not write the filesystem tree.
        #[arg(long)]
        dry_run: bool,

        /// Overwrite an existing `<target>/<change-id>/` filesystem
        /// tree without refusing.
        #[arg(long)]
        overwrite: bool,
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

/// Verdict values accepted by `nbspec review --verdict`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum VerdictArg {
    Approve,
    Revise,
}

impl From<VerdictArg> for VerdictValue {
    fn from(value: VerdictArg) -> Self {
        match value {
            VerdictArg::Approve => VerdictValue::Approve,
            VerdictArg::Revise => VerdictValue::Revise,
        }
    }
}

/// Long-running services exposed by `nbspec serve`.
#[derive(Debug, Subcommand)]
pub enum ServeService {
    /// Runs the Model Context Protocol server on stdio. Wraps the
    /// same operations library the change verbs dispatch to and
    /// exposes one MCP tool per CLI verb.
    Mcp,
}
