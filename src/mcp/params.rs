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

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeArgs {
    /// Change identifier (notebook folder under `proposals/`).
    pub change_id: String,

    /// Overwrites merge targets that drifted since the last merge.
    /// Does NOT override unsupported-delta refusals (MODIFIED /
    /// REMOVED / RENAMED) or non-file occupants; those remain
    /// unimplemented and require explicit operator intervention.
    #[serde(default)]
    pub force: bool,
}
