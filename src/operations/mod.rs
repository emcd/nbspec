//! Core change operations shared by the CLI and future MCP surface.
//!
//! Each public function corresponds to one user-facing verb. All
//! notebook access flows through [`nb_api::NbClient`]; only `merge`
//! may write to the repository working tree. Operations resolve the
//! effective notebook themselves — the explicit per-call argument, or
//! the Git-derived project notebook when `None` — and pass the
//! resolved name to every client call, so recorded metadata and
//! notebook writes always agree. The client's own configured default
//! is never consulted, because [`nb_api::NbClient`] does not expose
//! it; callers targeting a non-derived notebook must pass it
//! explicitly. Project configuration resolves against the Git
//! repository root, so operations behave identically from any
//! subdirectory.

use std::path::PathBuf;

use serde_json::Value;
use thiserror::Error;

use crate::archives::ArchiveError;
use crate::changes::ChangeError;
use crate::configuration::ConfigurationError;
use crate::merging::MergeError;
use crate::rendering::RenderError;
use crate::reviews::VerdictError;
use crate::schemata::SchemaError;
use crate::validation::ValidationFailure;
use crate::worknotes::WorkNoteError;

pub mod context;
pub mod create;
pub mod display;
pub mod helpers;
pub mod merge;
pub mod render;
pub mod review;
pub mod validate;

// Re-export the primary verbs so `crate::operations::create` etc. keep
// working as before the directory split.
pub use create::create;
pub use display::display;
pub use helpers::{classify_folder_listing, classify_note_content, is_nb_missing_item};
pub use merge::merge;
pub use render::render;
pub use review::review;
pub use validate::validate;

/// Tag applied to nbspec-managed control-plane notes.
pub(crate) const META_TAG: &str = "nbspec";

/// Errors from nbspec core operations.
#[derive(Debug, Error)]
pub enum OperationError {
    #[error("change already exists: {0}")]
    AlreadyExists(String),

    #[error("change not found in notebook {notebook}: {change_id}")]
    ChangeNotFound { notebook: String, change_id: String },

    #[error("cannot read note file {path}: {source}")]
    NoteRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error(
        "notebook not configured; pass --notebook or run within a Git repository \
         with a derivable notebook name"
    )]
    NotebookUnresolved,

    #[error("nb invocation failed: {0}")]
    Nb(#[from] nb_api::NbError),

    #[error(transparent)]
    Change(#[from] ChangeError),

    #[error(transparent)]
    Configuration(#[from] ConfigurationError),

    #[error(transparent)]
    Schema(#[from] SchemaError),

    #[error(transparent)]
    WorkNote(#[from] WorkNoteError),

    #[error(transparent)]
    Render(#[from] RenderError),

    #[error(transparent)]
    Merge(#[from] MergeError),

    #[error(transparent)]
    Archive(#[from] ArchiveError),

    #[error(transparent)]
    Invalid(#[from] ValidationFailure),

    #[error("cannot write archive {path}: {source}")]
    ArchiveWrite {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("unknown review gate {gate:?}; known gates: {known}")]
    GateUnknown { gate: String, known: String },

    #[error("reviewer identity unresolved; pass --reviewer or set git config user.name")]
    ReviewerUnresolved,

    #[error(
        "a revise verdict requires a comment naming the findings; pass --comment \
         (or --comment - on the CLI to read standard input)"
    )]
    ReviseCommentMissing,

    #[error(transparent)]
    Verdict(#[from] VerdictError),

    #[error("cannot encode verdict payload: {0}")]
    VerdictEncode(#[from] serde_json::Error),

    #[error("import failed with {failures} failure(s)")]
    ImportFailed {
        outcome: Box<OperationOutcome>,
        failures: usize,
    },
}

/// Result alias for core operations.
pub type OperationResult = Result<OperationOutcome, OperationError>;

/// The outcome of a successful operation: the same text the CLI prints,
/// plus a structured payload covering the operation's natural data. Both
/// surfaces consume this — the CLI prints `text`, the MCP server returns
/// `text` and `structured` together so clients can branch on typed data
/// instead of scraping prose.
#[derive(Clone, Debug)]
pub struct OperationOutcome {
    pub text: String,
    pub structured: Value,
}

impl OperationOutcome {
    /// Wraps `text` and `structured` as a successful outcome.
    pub fn new(text: impl Into<String>, structured: Value) -> Self {
        Self {
            text: text.into(),
            structured,
        }
    }
}
