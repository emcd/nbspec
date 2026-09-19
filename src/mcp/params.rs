//! Tool argument structs for the MCP server.
//!
//! Each struct derives `Deserialize` (so rmcp can deserialize MCP tool
//! arguments), `JsonSchema` (so rmcp can publish the JSON schema to
//! clients), and `Default` (so an empty arg object is treated as
//! "no overrides"). `#[serde(deny_unknown_fields)]` mirrors the
//! strictness policy of the existing CLI: silent acceptance of
//! unknown keys is rejected, so a misspelled parameter surfaces as
//! a schema-validation error rather than a confusing runtime
//! misbehavior.
//!
//! Per-tool notebook overrides are intentionally absent: the
//! specification pins notebook resolution to the server lifetime
//! (startup `--notebook` wins; otherwise git-derived), so exposing a
//! `notebook` parameter on each tool would invite callers to override
//! a value that the server has committed to ignore.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::reviews::VerdictValue;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    /// Change identifier (becomes the folder name under `proposals/`).
    pub change_id: String,

    /// Human-readable change title.
    #[serde(default)]
    #[schemars(with = "String")]
    pub title: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisplayArgs {
    /// Change identifier (notebook folder under `proposals/`).
    pub change_id: String,

    /// Includes artifact note contents and folder listings.
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateArgs {
    /// Change identifier (notebook folder under `proposals/`).
    pub change_id: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderArgs {
    /// Change identifier (notebook folder under `proposals/`).
    pub change_id: String,

    /// Emits a unified diff against current merge targets rather than
    /// the rendered file tree. Matches `nbspec render --diff`; pipes
    /// cleanly into review tooling such as difit.
    #[serde(default)]
    pub diff: bool,
}

fn merge_gate() -> String {
    crate::reviews::MERGE_GATE.to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewArgs {
    /// Change identifier (notebook folder under `proposals/`).
    pub change_id: String,

    /// Review gate the verdict addresses; defaults to `merge`, the
    /// only slice-1 gate.
    #[serde(default = "merge_gate")]
    pub gate: String,

    /// Verdict value: `approve` or `revise`.
    pub verdict: VerdictValue,

    /// Comment, e.g. a findings note selector. REQUIRED for a revise
    /// verdict; optional for approve. Recorded verbatim.
    #[serde(default)]
    #[schemars(with = "String")]
    pub comment: Option<String>,

    /// Reviewer identity; defaults to Git user.name. An explicit
    /// empty value is refused, never treated as absence.
    #[serde(default)]
    #[schemars(with = "String")]
    pub reviewer: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeArgs {
    /// Change identifier (notebook folder under `proposals/`).
    pub change_id: String,

    /// Overwrites merge targets that drifted since the last merge.
    /// Overrides target-state refusals (drift, unmanaged, foreign
    /// ownership) but never delta incoherence, dangling names, or
    /// non-file occupants. Force adopts an unmanaged surgical base
    /// only with addressable blocks. ADDED collisions against
    /// drifted text resolve delta-wins under force; hash-valid
    /// collisions refuse regardless.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportArgs {
    /// Filesystem root to scan for change trees.
    pub root: std::path::PathBuf,

    /// Emit the plan only; do not write notes or archives and do
    /// not delete the source filesystem tree.
    #[serde(default)]
    pub dry_run: bool,

    /// Authorize deletion of the source filesystem tree after a
    /// clean round-trip proof. Refused absent a clean proof.
    #[serde(default)]
    pub delete_original: bool,

    /// Skip active change tree detection (default: detect both).
    /// In v0.3.0 the active execute arm is a no-op; this flag
    /// emits a `Skip` entry instead of a paused `ActiveWrite`
    /// entry for each detected active tree.
    #[serde(default)]
    pub no_active: bool,

    /// Skip legacy archive tree ingestion (default: ingest both).
    #[serde(default)]
    pub no_archives: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    /// Change identifier (notebook folder under `proposals/`).
    pub change_id: String,

    /// Filesystem directory under which to write the change tree.
    pub target: std::path::PathBuf,

    /// Emit the plan only; do not write the filesystem tree.
    #[serde(default)]
    pub dry_run: bool,

    /// Overwrite an existing `<target>/<change-id>/` filesystem
    /// tree without refusing.
    #[serde(default)]
    pub overwrite: bool,
}
