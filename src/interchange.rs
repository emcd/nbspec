//! Filesystem ↔ notebook change interchange.
//!
//! Provides the `nbspec import` and `nbspec export` verbs: `import`
//! brings a filesystem OpenSpec-style change tree (active or legacy
//! archive) into the notebook-resident model, while `export` writes
//! a notebook change back out as a filesystem tree. Mapping is
//! normalized on ingest, lossless on export (modulo typed
//! normalizations), and the round-trip proof gated on `--delete-
//! original` uses `export` against the imported change as its
//! inverse composition.
//!
//! The module keeps filesystem-tree mapping, detection (active vs
//! archive), and round-trip proof logic in one place so the
//! `operations` module stays focused on CLI/MCP dispatch. The
//! existing `archives` (deterministic archive writer), `rendering`
//! (materialization), and `grammar` (validator) modules are
//! consumed by these verbs but not modified.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::archives::{ArchiveEntry, ArchiveError, build_archive};
use crate::changes::validate_change_id;
#[allow(unused_imports)]
// v0.3.0-pause: grammar imports used by #[cfg(any())]-gated pre-pause active-ingest code
use crate::grammar::{SectionPresence, parse_delta_specification};
#[allow(unused_imports)]
// v0.3.0-pause: worknotes imports used by #[cfg(any())]-gated pre-pause active-ingest code
use crate::worknotes::{WorkChecklist, WorkNoteError, parse_work_note};

/// Filename of the proposal document in an active change tree.
#[allow(dead_code)] // used by task 3.x
const ACTIVE_PROPOSAL_FILE: &str = "proposal.md";

/// Filename of the tasks document in an active change tree.
#[allow(dead_code)] // used by task 3.x
const ACTIVE_TASKS_FILE: &str = "tasks.md";

/// Filename of the design document in an active change tree.
#[allow(dead_code)] // used by task 3.x
const ACTIVE_DESIGN_FILE: &str = "design.md";

/// Subfolder holding capability specs in an active change tree.
#[allow(dead_code)] // used by task 3.x
const ACTIVE_SPECS_DIR: &str = "specs";

/// Subfolder holding decision records in an active change tree.
#[allow(dead_code)] // used by task 3.x
const ACTIVE_DECISIONS_DIR: &str = "decisions";

/// Legacy archive subpath beneath the `openspec/` directory.
const LEGACY_ARCHIVE_SUBDIR: &str = "changes/archive";

/// R7''''' / F7 third re-review 2026-07-25: process-local counter
/// for collision-safe quarantine and staging paths. Combined with
/// nanos + pid in the suffix, this closes any residual collision
/// window when nanos coincide across processes or within tight
/// loops in a single process. Exclusive creation (rather than
/// `create_dir_all`) is the final guard.
static QUARANTINE_COUNTER: AtomicU64 = AtomicU64::new(0);
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What kind of filesystem tree a path corresponds to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectedTree {
    /// `<root>/<change-id>/{proposal.md, specs/<cap>/spec.md,
    /// design.md, tasks.md, decisions/<adr>.md}` — a live, authored
    /// change in the upstream OpenSpec filesystem shape.
    Active(ActiveTree),
    /// `<root>/openspec/changes/archive/<change-id>/...` — a legacy
    /// archive tree that converts to a deterministic tar.zst under
    /// `documentation/archives/`.
    Archive(ArchiveTree),
}

/// A detected active change tree at `<root>/<change-id>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveTree {
    /// `<root>/<change-id>` — absolute path to the change folder.
    pub root: PathBuf,
    /// Change identifier (the `<change-id>` folder name).
    pub change_id: String,
}

/// A detected legacy archive tree at
/// `<root>/openspec/changes/archive/<change-id>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveTree {
    /// `<root>/openspec/changes/archive/<change-id>` — absolute path
    /// to the legacy archive folder.
    pub root: PathBuf,
    /// Change identifier (the `<change-id>` folder name).
    pub change_id: String,
}

/// A typed normalization applied when comparing the export output
/// against the source tree under the round-trip proof. The list is
/// enumerated explicitly so the proof output names every category
/// it elided; the proof never silently ignores divergence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoundTripNormalization {
    /// H1 normalization: filesystem sources may carry arbitrary H1
    /// text while the notebook model insists on selector-stable
    /// titles (per the schema's `generates` convention).
    H1Rewrite,
    /// Provenance-header normalization: durable-document writes
    /// carry provenance headers that the interchange artifact omits.
    ProvenanceHeader,
    /// Meta-note synthesis: the notebook adds a `meta` JSON
    /// control-plane note that has no filesystem analog.
    MetaSynthesis,
    /// Decision-slot reconstruction: filenames are preserved
    /// through import/export, so the proof compares the round-tripped
    /// content rather than treating the slot as opaque.
    DecisionSlotReconstruction,
}

/// Result of the round-trip proof: whether the export output is
/// equivalent to the source tree modulo the typed normalizations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundTripProof {
    /// Whether the proof is clean (the export matches the source
    /// modulo the enumerated normalizations).
    pub clean: bool,
    /// Files where the proof diverged beyond the typed
    /// normalizations (only populated when `clean` is `false`).
    pub divergences: Vec<String>,
}

/// One ingestion entry in the unified plan output. The CLI and MCP
/// surfaces consume this so the same structured payload describes
/// both ingestions and refusals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanEntry {
    /// Active change tree detected under `<root>/<change-id>/`. In
    /// v0.3.0 the execute arm is a no-op: the source filesystem tree
    /// is left untouched and no notebook mutation occurs. Active
    /// filesystem-tree ingest is paused in v0.3.0 pending an NbApi
    /// 0.3 notebook transaction/checkpoint primitive with Git/index
    /// locking; the pre-pause `import_active_tree` body is preserved
    /// in source under `#[cfg(any())]` and in git history, restored
    /// by the 0.3.0-resume cycle once NbApi 0.3 ships.
    ActiveWrite {
        change_id: String,
        source_path: PathBuf,
    },
    /// Legacy archive tree → deterministic tar.zst under
    /// `documentation/archives/`.
    ArchiveWrite {
        change_id: String,
        source_path: PathBuf,
        target_path: PathBuf,
    },
    /// Refusal: the tree is detected but cannot be ingested as-is
    /// (parse failure, change-id collision, layout anomaly). All
    /// refusals are collected before any write.
    Refusal {
        source_path: PathBuf,
        message: String,
    },
    /// Skip: the tree was eligible but the operator narrowed the
    /// scope with `--no-active` or `--no-archives`.
    Skip {
        source_path: PathBuf,
        reason: String,
    },
}

/// The unified plan output for both `import` and `export`. The plan
/// is emitted in full by `--dry-run` before any write or delete;
/// without `--dry-run`, the plan is built first, refusals abort, and
/// only the non-refused entries execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterchangePlan {
    /// Resolved target notebook for the plan.
    pub notebook: String,
    /// Per-tree plan entries in detection order.
    pub entries: Vec<PlanEntry>,
    /// Whether `--delete-original` authorizes deletion of the source
    /// filesystem tree after a clean round-trip proof.
    pub delete_original: bool,
    /// Paths that would be deleted under `--delete-original` (only
    /// populated when the flag is set).
    pub pending_deletions: Vec<PathBuf>,
    /// Whether this plan is dry-run only (no writes, no deletions).
    pub dry_run: bool,
}

/// Structured payload for the interchange plan. The CLI and MCP
/// surfaces both consume this so agents can branch on typed data.
#[derive(Clone, Debug)]
pub struct InterchangePlanStructured {
    pub plan: InterchangePlan,
    pub structured: Value,
}

impl InterchangePlanStructured {
    /// Builds the structured JSON payload from a plan.
    pub fn from_plan(plan: InterchangePlan) -> Self {
        let mut entries: Vec<Value> = Vec::with_capacity(plan.entries.len());
        for entry in &plan.entries {
            let mut object = Map::new();
            match entry {
                PlanEntry::ActiveWrite {
                    change_id,
                    source_path,
                } => {
                    // v0.3.0-pause: the active write is detected but
                    // its notebook mutation is paused pending the
                    // NbApi 0.3 notebook transaction/checkpoint
                    // primitive. Surface the pause state and the
                    // prerequisite in structured output so agents
                    // can branch on the typed payload.
                    let prerequisite = "NbApi 0.3 notebook transaction/checkpoint primitive with \
                                         Git/index locking";
                    object.insert("kind".to_string(), Value::String("active-write".into()));
                    object.insert("status".to_string(), Value::String("paused".into()));
                    object.insert("change_id".to_string(), Value::String(change_id.clone()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert(
                        "prerequisite".to_string(),
                        Value::String(prerequisite.into()),
                    );
                }
                PlanEntry::ArchiveWrite {
                    change_id,
                    source_path,
                    target_path,
                } => {
                    object.insert("kind".to_string(), Value::String("archive-write".into()));
                    object.insert("change_id".to_string(), Value::String(change_id.clone()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert(
                        "target_path".to_string(),
                        Value::String(target_path.display().to_string()),
                    );
                }
                PlanEntry::Refusal {
                    source_path,
                    message,
                } => {
                    object.insert("kind".to_string(), Value::String("refusal".into()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert("message".to_string(), Value::String(message.clone()));
                }
                PlanEntry::Skip {
                    source_path,
                    reason,
                } => {
                    object.insert("kind".to_string(), Value::String("skip".into()));
                    object.insert(
                        "source_path".to_string(),
                        Value::String(source_path.display().to_string()),
                    );
                    object.insert("reason".to_string(), Value::String(reason.clone()));
                }
            }
            entries.push(Value::Object(object));
        }
        let pending_deletions: Vec<Value> = plan
            .pending_deletions
            .iter()
            .map(|path| Value::String(path.display().to_string()))
            .collect();
        let structured = json!({
            "notebook": plan.notebook,
            "entries": entries,
            "delete_original": plan.delete_original,
            "pending_deletions": pending_deletions,
            "dry_run": plan.dry_run,
        });
        Self { plan, structured }
    }
}

/// Placeholder for the eventual `import` operation. Wired in tasks
/// 3.x (active-tree import) and 4.x (archive conversion); until
/// then it serves only as a registration point so the CLI and MCP
/// surfaces can be exercised.
pub async fn import(
    client: &nb_api::NbClient,
    notebook: Option<&str>,
    root: &std::path::Path,
    options: ImportOptions,
) -> Result<crate::operations::OperationOutcome, crate::operations::OperationError> {
    let notebook_name = resolve_notebook(notebook)?;
    let plan = build_import_plan(client, root, &notebook_name, &options).await?;
    let plan_text = render_plan_text(&plan, root);

    if plan.dry_run {
        let structured = InterchangePlanStructured::from_plan(plan).structured;
        let text = format!("{plan_text}dry-run: no writes performed");
        return Ok(crate::operations::OperationOutcome::new(text, structured));
    }

    // Refusal-collects-then-aborts: any refusal in the plan means
    // no writes happened; the operator sees the complete plan and
    // decides whether to retry.
    let refusal_count = plan
        .entries
        .iter()
        .filter(|entry| matches!(entry, PlanEntry::Refusal { .. }))
        .count();
    if refusal_count > 0 {
        let structured = InterchangePlanStructured::from_plan(plan).structured;
        let _text = format!(
            "{plan_text}refusal: {refusal_count} refusal(s) abort the import; no writes performed"
        );
        let _ = structured;
        return Err(crate::operations::OperationError::from(
            InterchangeError::ImportRefused {
                count: refusal_count,
            },
        ));
    }

    execute_import_plan(client, &plan, root, &notebook_name).await
}

/// Resolves the effective notebook for an interchange verb.
fn resolve_notebook(notebook: Option<&str>) -> Result<String, crate::operations::OperationError> {
    notebook
        .map(String::from)
        .or_else(nb_api::derive_git_notebook_name)
        .ok_or(crate::operations::OperationError::NotebookUnresolved)
}

/// Builds the import plan from a filesystem tree. Detects trees,
/// classifies each as Active / Archive / Skip / Refusal, and emits
/// a unified plan. All refusals are surfaced before any write.
async fn build_import_plan(
    _client: &nb_api::NbClient,
    root: &Path,
    notebook_name: &str,
    options: &ImportOptions,
) -> Result<InterchangePlan, crate::operations::OperationError> {
    let detected = detect_trees(root);
    let mut entries: Vec<PlanEntry> = Vec::new();

    for tree in detected {
        match tree {
            DetectedTree::Active(active) => {
                if options.no_active {
                    entries.push(PlanEntry::Skip {
                        source_path: active.root.clone(),
                        reason: "--no-active excludes active change ingestion".to_string(),
                    });
                    continue;
                }
                // v0.3.0-pause: active filesystem-tree ingest is
                // paused pending an NbApi 0.3 notebook transaction/
                // checkpoint primitive. Always emit `ActiveWrite`
                // before any collision/body validation; the v0.3.0
                // execute arm is a no-op regardless of collision, so
                // pre-checking collisions here would surface refusals
                // the binary cannot act on. The collision check and
                // body validation move into the 0.3.0-resume cycle
                // when the pre-pause `import_active_tree` body is
                // restored.
                entries.push(PlanEntry::ActiveWrite {
                    change_id: active.change_id.clone(),
                    source_path: active.root.clone(),
                });
            }
            DetectedTree::Archive(archive) => {
                if options.no_archives {
                    entries.push(PlanEntry::Skip {
                        source_path: archive.root.clone(),
                        reason: "--no-archives excludes archive ingestion".to_string(),
                    });
                    continue;
                }
                let target_path = format!("documentation/archives/{}.tar.zst", archive.change_id);
                entries.push(PlanEntry::ArchiveWrite {
                    change_id: archive.change_id.clone(),
                    source_path: archive.root.clone(),
                    target_path: PathBuf::from(target_path),
                });
            }
        }
    }

    let pending_deletions = if options.delete_original {
        entries
            .iter()
            .filter_map(|entry| match entry {
                // v0.3.0-pause: paused ActiveWrite sources are
                // never deleted (the execute arm is a no-op;
                // `active_to_prove` stays empty). Including them
                // in `pending_deletions` would advertise
                // deletions the binary cannot perform. Filter
                // them out; the per-entry paused status already
                // conveys the source's presence to the operator.
                PlanEntry::ActiveWrite { .. } => None,
                PlanEntry::ArchiveWrite { source_path, .. } => Some(source_path.clone()),
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(InterchangePlan {
        notebook: notebook_name.to_string(),
        entries,
        delete_original: options.delete_original,
        pending_deletions,
        dry_run: options.dry_run,
    })
}

/// Lists the existing change ids under `proposals/` in the given
/// notebook. Used to detect change-id collisions before any
/// Lists existing change ids under `proposals/<id>/` in the
/// notebook, used to detect change-id collisions before any
/// ingestion writes.
///
/// R6 (F6): previously swallowed every error into an empty
/// listing, which silently disabled collision protection during
/// backend failure. We now distinguish a "notebook has no
/// `proposals/` folder yet" absence (legitimate empty Vec)
/// from genuine `nb-api` failures (propagated).
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
async fn list_existing_change_ids(
    client: &nb_api::NbClient,
    notebook_name: &str,
) -> Result<Vec<String>, crate::operations::OperationError> {
    let notebook_path = client.show_notebook_path(Some(notebook_name)).await?;
    let proposals_root = notebook_path.join("proposals");
    if !proposals_root.is_dir() {
        return Ok(Vec::new());
    }
    let entries =
        std::fs::read_dir(&proposals_root).map_err(|source| InterchangeError::ListingFailed {
            notebook: notebook_name.to_string(),
            folder: "proposals".to_string(),
            message: source.to_string(),
        })?;
    let mut ids: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        if !entry.path().is_dir() {
            continue;
        }
        if validate_change_id(&name).is_err() {
            continue;
        }
        ids.push(name);
    }
    ids.sort();
    Ok(ids)
}

/// Parses the `nb ls proposals` listing into change ids. Each line
/// is a folder name; only kebab-case names are kept so non-change
/// folders under `proposals/` (e.g. `archive/`) do not collide.
///
/// R6 (F6): kept for backward compatibility with callers that
/// already have a listing string in hand; the implementation
/// here is fail-closed (unknown folder names surface as listing
/// errors, not silent skips) when used directly. Production
/// callers prefer the filesystem-walking `list_existing_change_ids`
/// helper above.
#[allow(dead_code)]
fn parse_change_id_listing(
    listing: &str,
) -> Result<Vec<String>, crate::operations::OperationError> {
    let mut ids: Vec<String> = Vec::new();
    for line in listing.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("0 items") {
            continue;
        }
        // `nb ls` may prefix entries with a counter; take the
        // last whitespace-separated token as the folder name.
        let name = trimmed.split_whitespace().last().unwrap_or(trimmed);
        if validate_change_id(name).is_ok() {
            ids.push(name.to_string());
        }
    }
    Ok(ids)
}
/// Renders the plan as a human-readable text block for the CLI's
/// `text` field. The MCP structured payload is produced by
/// `InterchangePlanStructured::from_plan`.
fn render_plan_text(plan: &InterchangePlan, root: &Path) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "Plan for notebook {} from root {}:\n",
        plan.notebook,
        root.display()
    ));
    for entry in &plan.entries {
        match entry {
            PlanEntry::ActiveWrite {
                change_id,
                source_path,
            } => {
                // v0.3.0-pause: render the pause state in the human-
                // readable plan output. The execute arm is a no-op;
                // the source filesystem tree is left untouched.
                output.push_str(&format!(
                    "  paused: active {change_id} ({source}) — \
                     v0.3.0 active ingest is no-op pending the NbApi 0.3 notebook \
                     transaction/checkpoint primitive; source is not modified\n",
                    change_id = change_id,
                    source = source_path.display()
                ));
            }
            PlanEntry::ArchiveWrite {
                change_id,
                source_path,
                target_path,
            } => {
                output.push_str(&format!(
                    "  archive-write: {change_id} ({source} -> {target})\n",
                    change_id = change_id,
                    source = source_path.display(),
                    target = target_path.display()
                ));
            }
            PlanEntry::Refusal {
                source_path,
                message,
            } => {
                output.push_str(&format!(
                    "  refusal: {source}: {message}\n",
                    source = source_path.display(),
                    message = message
                ));
            }
            PlanEntry::Skip {
                source_path,
                reason,
            } => {
                output.push_str(&format!(
                    "  skip: {source}: {reason}\n",
                    source = source_path.display(),
                    reason = reason
                ));
            }
        }
    }
    if !plan.pending_deletions.is_empty() {
        output.push_str("pending deletions:\n");
        for path in &plan.pending_deletions {
            output.push_str(&format!("  - {}\n", path.display()));
        }
    }
    output
}

/// Executes a plan that has passed the refusal gate. Three
/// phases, run in order:
///
/// 1. **Write phase.** Every entry's write is performed against
///    the notebook (active) or filesystem (archive). A write
///    failure aborts the entire batch — no partial notebook
///    state, no partial archive directory. In v0.3.0 the active
///    write arm is a no-op (active ingest is paused pending
///    NbApi 0.3).
///
/// 2. **Proof phase.** When `--delete-original` is set, the
///    round-trip proof runs against every successfully written
///    active tree, in one batch. The deletion phase runs only
///    when every proof is clean.
///
/// 3. **Deletion phase.** All source trees (active + archive)
///    are removed in a single batch only after every proof is
///    clean and every write succeeded. A dirty proof skips ALL
///    deletions, including archives whose deterministic build
///    succeeded (R3 / F3 — all-or-nothing). Archive deletion
///    honors `--delete-original` directly: the deterministic
///    tar.zst build is the implicit fidelity proof, and the source
///    tree is in version control (`git restore` is the recovery
///    story).
async fn execute_import_plan(
    client: &nb_api::NbClient,
    plan: &InterchangePlan,
    root: &Path,
    notebook_name: &str,
) -> Result<crate::operations::OperationOutcome, crate::operations::OperationError> {
    let mut output = render_plan_text(plan, root);
    let mut written: Vec<String> = Vec::new();
    let deleted: Vec<String> = Vec::new();
    let mut proof_divergences: Vec<String> = Vec::new();
    let mut write_failures: Vec<String> = Vec::new();

    // Phase 1: writes. Track active trees that need a proof
    // and archive trees whose source is eligible for deletion.
    let active_to_prove: Vec<(String, PathBuf)> = Vec::new();
    let mut archive_to_delete: Vec<(String, PathBuf)> = Vec::new();
    // R2'''' / F2 second re-review 2026-07-25: archive deletion
    // requires an explicit fidelity proof against the PERSISTED
    // archive (not the in-memory bytes). Track the persisted
    // archive path per change id so the proof phase can read it
    // back from disk.
    let mut archive_target_path_by_id: std::collections::HashMap<String, PathBuf> =
        std::collections::HashMap::new();

    for entry in &plan.entries {
        match entry {
            PlanEntry::ActiveWrite {
                change_id,
                source_path,
            } => {
                // v0.3.0-pause: the execute arm is a no-op. Source
                // filesystem tree is left untouched; no notebook
                // mutation occurs. The pre-pause `import_active_tree`
                // body that computed writes + delta warnings and
                // validated collisions is preserved in source under
                // `#[cfg(any())]` and in git history, restored by
                // the 0.3.0-resume cycle once NbApi 0.3 ships the
                // notebook transaction/checkpoint primitive.
                output.push_str(&format!(
                    "paused: active {change_id} at {source} — source untouched; \
                     active filesystem-tree ingest is paused in v0.3.0 pending the \
                     NbApi 0.3 notebook transaction/checkpoint primitive\n",
                    change_id = change_id,
                    source = source_path.display()
                ));
            }
            PlanEntry::ArchiveWrite {
                change_id,
                source_path,
                target_path,
            } => {
                let archive = ArchiveTree {
                    root: source_path.clone(),
                    change_id: change_id.clone(),
                };
                let bytes = match import_archive_tree(&archive)
                    .map_err(crate::operations::OperationError::from)
                {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        write_failures.push(format!("archive:{change_id}:{error}"));
                        output.push_str(&format!("write failure: archive {change_id}: {error}\n"));
                        continue;
                    }
                };
                if let Err(error) = write_archive_file(change_id, target_path, &bytes) {
                    write_failures.push(format!("archive:{change_id}:{error}"));
                    output.push_str(&format!("write failure: archive {change_id}: {error}\n"));
                    continue;
                }
                written.push(format!("archive:{change_id}"));
                output.push_str(&format!(
                    "wrote archive {target} ({size} bytes)\n",
                    target = target_path.display(),
                    size = bytes.len()
                ));
                if plan.delete_original {
                    archive_to_delete.push((change_id.clone(), source_path.clone()));
                    archive_target_path_by_id.insert(change_id.clone(), target_path.clone());
                }
            }
            PlanEntry::Refusal { .. } | PlanEntry::Skip { .. } => {
                // Already excluded by the gate before we got here.
            }
        }
    }

    // Phase 2: quarantine all sources (rename-to-quarantine with
    // rollback). R7''' / F7 second re-review 2026-07-25: the proof
    // must run against an immutable snapshot, not the live source;
    // if the source is renamed first, the snapshot cannot drift.
    // If any staging rename fails, every previously renamed source
    // is restored (renamed back to its original path). On staging
    // success, the next phase proves the quarantines.
    let mut staged: Vec<(String, PathBuf, PathBuf, &'static str)> = Vec::new();
    let mut retained_quarantines: Vec<(String, String, PathBuf)> = Vec::new();
    let mut rollback_failures: Vec<String> = Vec::new();

    if plan.delete_original && write_failures.is_empty() {
        // Build the pending list (every active and archive source
        // whose write succeeded).
        let pending: Vec<(String, PathBuf, &'static str)> = active_to_prove
            .iter()
            .map(|(change_id, source_path)| (change_id.clone(), source_path.clone(), "active"))
            .chain(archive_to_delete.iter().map(|(change_id, source_path)| {
                (change_id.clone(), source_path.clone(), "archive")
            }))
            .collect();

        let mut staging_failed = false;
        for (change_id, source_path, kind) in &pending {
            let quarantine = match quarantine_path_for(source_path) {
                Ok(path) => path,
                Err(error) => {
                    write_failures.push(format!(
                        "delete-source:{kind}:{change_id}:quarantine-path:{error}"
                    ));
                    output.push_str(&format!(
                        "delete failure: {kind} {change_id}: cannot derive quarantine path: {error}\n"
                    ));
                    staging_failed = true;
                    break;
                }
            };
            // R7 fourth re-review 2026-07-25: use `rename_no_replace`
            // (or its refusal fallback on non-Linux) so an existing
            // quarantine from a prior failed run is detected and
            // surfaced rather than silently absorbed by an
            // overwriting `std::fs::rename`. The staging suffix is
            // also collision-safe (counter + nanos + pid), so this
            // EEXIST is the residual `Reserved`-style protection.
            match rename_no_replace(source_path, &quarantine) {
                Ok(()) => {
                    staged.push((change_id.clone(), source_path.clone(), quarantine, kind));
                }
                Err(error) => {
                    write_failures.push(format!(
                        "delete-source:{kind}:{change_id}:quarantine-rename:{error}"
                    ));
                    output.push_str(&format!(
                        "delete failure: {kind} {change_id}: cannot rename to quarantine: {error}; \
                         reserved quarantine at {} may indicate a prior failed run",
                        quarantine.display()
                    ));
                    staging_failed = true;
                    break;
                }
            }
        }
        if staging_failed {
            // Restore every previously renamed source. The
            // restoration errors are surfaced (not silently dropped)
            // so the operator knows if any source remained
            // quarantined because rollback failed.
            for (change_id, original_path, quarantine_path, kind) in staged.iter().rev() {
                match std::fs::rename(quarantine_path, original_path) {
                    Ok(()) => {
                        output.push_str(&format!(
                            "restored {kind} {change_id} from quarantine after staging failure\n"
                        ));
                    }
                    Err(error) => {
                        rollback_failures.push(format!(
                            "{kind}:{change_id}:cannot restore from quarantine {quarantine}: {error}",
                            quarantine = quarantine_path.display()
                        ));
                        output.push_str(&format!(
                            "RESTORE FAILURE: {kind} {change_id}: source remains at quarantine {}: {error}\n",
                            quarantine_path.display()
                        ));
                    }
                }
            }
        }
    }

    // Phase 3: prove the quarantined snapshots. The active tree
    // round-trip runs against the quarantine; the archive fidelity
    // proof also runs against the quarantine (and the persisted
    // archive on disk). Both gates must be clean.
    let mut proofs_clean = write_failures.is_empty() && rollback_failures.is_empty();
    if proofs_clean && plan.delete_original && !staged.is_empty() {
        let scratch = std::env::temp_dir().join(format!(
            "nbspec-roundtrip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for (change_id, _original_path, quarantine_path, kind) in &staged {
            if *kind == "active" {
                match run_round_trip(client, change_id, quarantine_path, &scratch, notebook_name)
                    .await
                {
                    Ok(proof) if proof.clean => {}
                    Ok(proof) => {
                        proofs_clean = false;
                        for divergence in &proof.divergences {
                            proof_divergences.push(format!("{change_id}:{divergence}"));
                            output.push_str(&format!(
                                "round-trip refusal: {change_id}:{divergence}\n"
                            ));
                        }
                    }
                    Err(error) => {
                        proofs_clean = false;
                        proof_divergences.push(format!("{change_id}:round-trip:{error}"));
                        output.push_str(&format!("round-trip refusal: {change_id}:{error}\n"));
                    }
                }
            } else if let Some(archive_path) = archive_target_path_by_id.get(change_id) {
                match archive_fidelity_proof(change_id, quarantine_path, archive_path) {
                    Ok(()) => {}
                    Err(InterchangeError::ArchiveFidelity { divergences, .. }) => {
                        proofs_clean = false;
                        for divergence in &divergences {
                            proof_divergences.push(format!("{change_id}:archive:{divergence}"));
                            output.push_str(&format!(
                                "archive fidelity proof refused: {change_id}:{divergence}\n"
                            ));
                        }
                    }
                    Err(other) => {
                        proofs_clean = false;
                        proof_divergences.push(format!("{change_id}:archive:{other}"));
                        output.push_str(&format!(
                            "archive fidelity proof refused: {change_id}:{other}\n"
                        ));
                    }
                }
            } else {
                proofs_clean = false;
                proof_divergences.push(format!(
                    "{change_id}:archive:missing archive path for fidelity proof"
                ));
                output.push_str(&format!(
                    "archive fidelity proof refused: {change_id}: missing archive path\n"
                ));
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }

    // Phase 4: restore on dirty proof, retain on clean. The
    // "preserving quarantines is the safest coherent contract"
    // pattern (F7 second re-review): the auto-delete phase is
    // removed entirely. On clean proofs, the quarantines stay in
    // place and the operator decides when (and whether) to delete
    // them. On dirty proofs, the quarantines are restored to
    // their original paths; rollback errors are surfaced.
    if plan.delete_original && !staged.is_empty() {
        if proofs_clean {
            for (change_id, _original_path, quarantine_path, kind) in &staged {
                retained_quarantines.push((
                    change_id.clone(),
                    (*kind).to_string(),
                    quarantine_path.clone(),
                ));
                output.push_str(&format!(
                    "quarantined {kind} {change_id} at {}; verify and remove manually\n",
                    quarantine_path.display()
                ));
            }
        } else {
            for (change_id, original_path, quarantine_path, kind) in staged.iter().rev() {
                match std::fs::rename(quarantine_path, original_path) {
                    Ok(()) => {
                        output.push_str(&format!(
                            "restored {kind} {change_id} from quarantine after dirty proof\n"
                        ));
                    }
                    Err(error) => {
                        rollback_failures.push(format!(
                            "{kind}:{change_id}:cannot restore from quarantine {quarantine}: {error}",
                            quarantine = quarantine_path.display()
                        ));
                        output.push_str(&format!(
                            "RESTORE FAILURE: {kind} {change_id}: source remains at quarantine {}: {error}\n",
                            quarantine_path.display()
                        ));
                    }
                }
            }
        }
    } else if plan.delete_original && !proof_divergences.is_empty() {
        output.push_str(&format!(
            "round-trip refusal: {} divergence(s); no source deletion performed for any change\n",
            proof_divergences.len()
        ));
    }

    output.push_str(&format!(
        "Imported {count} entries to notebook {notebook}.\n",
        count = written.len(),
        notebook = plan.notebook
    ));
    let retained_serialized: Vec<serde_json::Value> = retained_quarantines
        .iter()
        .map(|(change_id, kind, path)| {
            serde_json::json!({
                "change_id": change_id,
                "kind": kind,
                "quarantine_path": path.display().to_string(),
            })
        })
        .collect();
    // Surface the typed per-entry payload (kind / change_id /
    // source_path / status / prerequisite) in the execution
    // structured output, mirroring `from_plan`. MCP Owner bounded
    // correction #4: previously only the dry-run path emitted
    // typed entries; the live execution path is now aligned.
    let typed_entries: Value = InterchangePlanStructured::from_plan(plan.clone())
        .structured
        .get("entries")
        .cloned()
        .unwrap_or_else(|| Value::Array(vec![]));
    let structured = serde_json::json!({
        "notebook": plan.notebook,
        "entries": typed_entries,
        "written": written,
        "deleted": deleted,
        "delete_original": plan.delete_original,
        "pending_deletions": plan.pending_deletions.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "proof_divergences": proof_divergences,
        "write_failures": write_failures,
        "rollback_failures": rollback_failures,
        "retained_quarantines": retained_serialized,
    });
    if !write_failures.is_empty() || !rollback_failures.is_empty() {
        return Err(crate::operations::OperationError::NoteRead {
            path: PathBuf::from("<import-plan>"),
            source: std::io::Error::other(format!(
                "import plan completed with {} failure(s); see structured payload",
                write_failures.len() + rollback_failures.len()
            )),
        });
    }
    Ok(crate::operations::OperationOutcome::new(output, structured))
}

/// Drives the export-based round-trip proof: runs the export of
/// the imported change against a scratch path and diffs against
/// the source tree modulo the typed normalizations.
async fn run_round_trip(
    client: &nb_api::NbClient,
    change_id: &str,
    source_path: &Path,
    scratch: &Path,
    notebook_name: &str,
) -> Result<RoundTripProof, crate::operations::OperationError> {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).map_err(|source| {
            crate::operations::OperationError::NoteRead {
                path: scratch.to_path_buf(),
                source,
            }
        })?;
    }
    std::fs::create_dir_all(scratch).map_err(|source| {
        crate::operations::OperationError::NoteRead {
            path: scratch.to_path_buf(),
            source,
        }
    })?;
    let plan = build_export_plan(
        client,
        change_id,
        scratch,
        notebook_name,
        &ExportOptions {
            dry_run: false,
            overwrite: true,
        },
    )
    .await?;
    execute_export_plan(client, &plan).await?;
    // R7''' / F7 second re-review 2026-07-25: pass the change
    // root paths directly rather than deriving them. `source_path`
    // may be a quarantine (renamed sibling, no `<change_id>/`
    // subdirectory), so the proof must use the path as-is.
    let source_change_root = source_path;
    let scratch_change_root = scratch.join(change_id);
    Ok(round_trip_proof(
        change_id,
        source_change_root,
        &scratch_change_root,
    ))
}

/// R2'''' / F2 second re-review 2026-07-25: archive fidelity
/// proof against the **persisted** archive (not the in-memory
/// bytes that may have been written to disk before the source
/// mutated). The proof extracts the persisted archive to a
/// scratch dir, builds the exact file inventory on both sides
/// (path + bytes), and refuses on any divergence. The active-tree
/// canonicalizer (`canonical_bytes_for_proof`) is not applied;
/// archive fidelity is a byte-equal contract, not a
/// normalized-equality contract.
///
/// R2'''''/ F7 third re-review 2026-07-25: the source change
/// root is now passed directly (rather than reconstructed as
/// `<source_root>/<change_id>/`). The quarantine is a renamed
/// sibling that no longer contains a `<change_id>/`
/// subdirectory, so the previous reconstruction was reading the
/// wrong path and any clean archive quarantine could not pass.
/// The empty-dir validation is re-run on the exact quarantine;
/// the build-time check at the original source no longer
/// covers state changes between build and proof.
pub fn archive_fidelity_proof(
    change_id: &str,
    source_change_root: &Path,
    archive_path: &Path,
) -> Result<(), InterchangeError> {
    let archive_bytes =
        std::fs::read(archive_path).map_err(|source| InterchangeError::SourceRead {
            path: archive_path.to_path_buf(),
            source,
        })?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let scratch = std::env::temp_dir().join(format!(
        "nbspec-archive-fidelity-{change_id}-{}-{nanos}",
        std::process::id()
    ));
    let extract_root = scratch.join(change_id);
    if let Err(error) = std::fs::create_dir_all(&scratch) {
        return Err(InterchangeError::SourceRead {
            path: scratch.clone(),
            source: error,
        });
    }
    if let Err(error) = crate::archives::extract_archive(
        &archive_bytes,
        &extract_root,
        Some(&format!("{change_id}/tree/")),
    ) {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(InterchangeError::ArchiveFidelity {
            change_id: change_id.to_string(),
            divergences: vec![format!("cannot extract persisted archive: {error}")],
        });
    }
    // R2''''' third re-review: re-run the empty-dir refusal at
    // proof time on the exact quarantine. The source-side check
    // ran at build time against the original source; an empty
    // dir could have appeared between build and proof and would
    // be captured here.
    if let Err(error) = refuse_empty_directories(source_change_root) {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(InterchangeError::ArchiveFidelity {
            change_id: change_id.to_string(),
            divergences: vec![format!("source rejected at proof time: {error}")],
        });
    }
    // Build exact file inventories (path + bytes, no canonicalization).
    let source_change = source_change_root;
    let mut source_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut divergences: Vec<String> = Vec::new();
    walk_for_proof(
        source_change,
        source_change,
        "source",
        &mut source_files,
        &mut divergences,
    );
    source_files.sort();
    let mut extracted_files: Vec<(String, Vec<u8>)> = Vec::new();
    walk_for_proof(
        &extract_root,
        &extract_root,
        "extracted",
        &mut extracted_files,
        &mut divergences,
    );
    extracted_files.sort();
    let _ = std::fs::remove_dir_all(&scratch);
    let source_map: std::collections::HashMap<String, Vec<u8>> =
        source_files.iter().cloned().collect();
    let extracted_map: std::collections::HashMap<String, Vec<u8>> =
        extracted_files.iter().cloned().collect();
    for (relative, bytes) in &source_files {
        match extracted_map.get(relative) {
            None => divergences.push(format!("{relative}: missing from archive")),
            Some(extracted) => {
                if bytes != extracted {
                    divergences.push(format!("{relative}: byte divergence"));
                }
            }
        }
    }
    for (relative, _) in &extracted_files {
        if !source_map.contains_key(relative) {
            divergences.push(format!(
                "{relative}: present in archive but missing from source"
            ));
        }
    }
    if !divergences.is_empty() {
        return Err(InterchangeError::ArchiveFidelity {
            change_id: change_id.to_string(),
            divergences,
        });
    }
    Ok(())
}

/// Staging result for an active tree's notebook writes; the caller
/// commits via `commit_active_staging` after every entry's stage
/// has succeeded (R7 fourth re-review 2026-07-25 — multi-entry
/// import all-or-nothing across entries).
pub struct ActiveStaging {
    /// Absolute path to the staging folder under the notebook.
    pub staging_full: PathBuf,
    /// Absolute path to the real proposals/<change_id>/ folder.
    pub real_full: PathBuf,
}

/// Stages the per-note actions generated by `import_active_tree` to
/// the notebook under a sibling
/// `proposals/<change_id>.staging-<counter>/` folder. Returns
/// without committing; the caller invokes
/// `commit_active_staging` after every entry's stage has
/// succeeded. On any per-write failure the staging folder is
/// removed before returning. POSIX-atomic `rename_no_replace`
/// publishes a clean batch; an EEXIST at the publish boundary is
/// the residual `Reserved`-style protection.
///
/// **v0.3.0-pause**: this function calls `client.add_note` and
/// `ensure_folder`, each of which auto-checkpoints the notebook
/// Git worktree. The notebook MCP Owner's fourth ring-1 re-review
/// called out the structural incompatibility: a notebook tree
/// created by these per-call checkpoints cannot be undone by a
/// subsequent filesystem rename, so the published state diverges
/// from the staged state. Active filesystem-tree ingest is
/// therefore paused in v0.3.0 pending an NbApi 0.3 notebook
/// transaction/checkpoint primitive with Git/index locking. The
/// source is preserved in tree under `#[cfg(any())]` and in git
/// history; with the gate off, the v0.3.0 execute path emits a
/// `paused`-state `PlanEntry::ActiveWrite` entry instead.
///
/// R7_F7 final-stack (third re-review 2026-07-25).
#[cfg(any())]
#[allow(dead_code)]
async fn stage_active_notes(
    client: &nb_api::NbClient,
    change_id: &str,
    writes: &[NoteWrite],
    notebook_name: &str,
) -> Result<ActiveStaging, crate::operations::OperationError> {
    let notebook = Some(notebook_name);
    let staging_counter = STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staging_change_id = format!("{change_id}.staging-{staging_counter}");
    let real_root = format!("proposals/{change_id}");
    let staging_root = format!("proposals/{staging_change_id}");

    ensure_folder(client, &staging_root, notebook).await?;
    let mut subfolders: Vec<String> = Vec::new();
    for write in writes {
        if !write.folder.is_empty() && !subfolders.contains(&write.folder) {
            subfolders.push(write.folder.clone());
        }
    }
    for subfolder in subfolders {
        let folder = format!("{staging_root}/{subfolder}");
        ensure_folder(client, &folder, notebook).await?;
    }

    let mut write_failures: Vec<(String, String)> = Vec::new();
    for write in writes {
        let folder = if write.folder.is_empty() {
            staging_root.clone()
        } else {
            format!("{staging_root}/{}", write.folder)
        };
        let write_result = if write.is_todo {
            write_todo_note(client, change_id, &folder, &write.content, notebook_name).await
        } else {
            client
                .add_note(
                    Some(&write.title),
                    &write.content,
                    &[],
                    Some(&folder),
                    notebook,
                )
                .await
                .map(|_| ())
                .map_err(crate::operations::OperationError::from)
        };
        if let Err(error) = write_result {
            write_failures.push((change_id.to_string(), format!("{error}")));
            break;
        }
    }

    if !write_failures.is_empty() {
        if let Ok(notebook_path) = client.show_notebook_path(notebook).await {
            let staging_full = notebook_path.join(&staging_root);
            let _ = std::fs::remove_dir_all(&staging_full);
        }
        return Err(crate::operations::OperationError::NoteRead {
            path: PathBuf::from(format!("<staging:{staging_root}>")),
            source: std::io::Error::other(format!(
                "staged notebook write failed and was rolled back: {} failure(s); \
                 first: {}",
                write_failures.len(),
                write_failures
                    .first()
                    .map(|(_, msg)| msg.clone())
                    .unwrap_or_default()
            )),
        });
    }

    let notebook_path = client.show_notebook_path(notebook).await?;
    let real_full = notebook_path.join(&real_root);
    let staging_full = notebook_path.join(&staging_root);
    Ok(ActiveStaging {
        staging_full,
        real_full,
    })
}

/// Commits a previously staged active tree by atomic-renaming the
/// staging folder into the real path. Uses `rename_no_replace` so
/// an occupant appearing between staging and commit returns
/// EEXIST atomically (Linux) or refuses (non-Linux).
/// **v0.3.0-pause**: see `stage_active_notes`. Gated behind
/// `#[cfg(any())]`; restored by the 0.3.0-resume cycle.
#[cfg(any())]
#[allow(dead_code)]
async fn commit_active_staging(
    client: &nb_api::NbClient,
    staging: ActiveStaging,
) -> Result<(), crate::operations::OperationError> {
    let notebook_name = "commit_active_staging_uses_resolved_path";
    let _ = (client, notebook_name);
    match rename_no_replace(&staging.staging_full, &staging.real_full) {
        Ok(()) => Ok(()),
        Err(error) => Err(crate::operations::OperationError::NoteRead {
            path: staging.real_full.clone(),
            source: std::io::Error::other(format!(
                "cannot publish staged notebook writes: {error}; \
                 staging folder at {} preserved for manual reconciliation",
                staging.staging_full.display()
            )),
        }),
    }
}

/// Writes the per-note actions generated by `import_active_tree` to
/// the notebook. Convenience wrapper that stages and immediately
/// commits; callers that need multi-entry atomicity should use
/// `stage_active_notes` and `commit_active_staging` directly.
///
/// **v0.3.0-pause**: see `stage_active_notes`. Gated behind
/// `#[cfg(any())]`; restored by the 0.3.0-resume cycle.
#[cfg(any())]
#[allow(dead_code)]
async fn write_active_notes(
    client: &nb_api::NbClient,
    change_id: &str,
    writes: &[NoteWrite],
    notebook_name: &str,
) -> Result<(), crate::operations::OperationError> {
    let staging = stage_active_notes(client, change_id, writes, notebook_name).await?;
    commit_active_staging(client, staging).await
}

/// Writes the work todo note directly to the notebook filesystem
/// to preserve `[x]` / `[ ]` checkbox state. `nb-api`'s
/// `add_todo` creates a fresh todo with no checked items, so the
/// only way to round-trip the source `tasks.md` faithfully is to
/// overwrite the file after creation.
///
/// **v0.3.0-pause**: see `stage_active_notes`. Gated behind
/// `#[cfg(any())]`; restored by the 0.3.0-resume cycle.
#[cfg(any())]
#[allow(dead_code)]
async fn write_todo_note(
    client: &nb_api::NbClient,
    change_id: &str,
    folder: &str,
    body: &str,
    notebook_name: &str,
) -> Result<(), crate::operations::OperationError> {
    let notebook = Some(notebook_name);
    let description = format!("Execution checklist for {change_id}.");
    client
        .add_todo(
            "work",
            Some(&description),
            &[],
            &["nbspec".to_string()],
            Some(folder),
            notebook,
        )
        .await?;
    let notebook_path = client.show_notebook_path(notebook).await?;
    let todo_path = notebook_path.join(folder).join("work.todo.md");
    std::fs::write(&todo_path, body).map_err(|source| {
        crate::operations::OperationError::NoteRead {
            path: todo_path,
            source,
        }
    })?;
    Ok(())
}

/// **v0.3.0-pause**: see `stage_active_notes`. Gated behind
/// `#[cfg(any())]`; restored by the 0.3.0-resume cycle.
#[cfg(any())]
#[allow(dead_code)]
async fn ensure_folder(
    client: &nb_api::NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> Result<(), crate::operations::OperationError> {
    let listing = client
        .list_notes(Some(folder), &[], Some(1), notebook)
        .await;
    if listing.is_ok() {
        return Ok(());
    }
    client.add_folder(folder, notebook).await?;
    Ok(())
}

/// Staging result for an archive: the staging file path and the
/// target path; the caller commits via `commit_archive_staging`
/// after every entry's stage has succeeded (R7 fourth re-review
/// 2026-07-25 — multi-entry import all-or-nothing across entries).
pub struct ArchiveStaging {
    pub staging_file: PathBuf,
    pub target_file: PathBuf,
}

/// Stages an archive at `<target>.staging-<nanos>-<pid>-<counter>`
/// without committing. The bytes are written via
/// `OpenOptions::create_new(true)` so a residual staging file
/// from a prior failed run is surfaced as `AlreadyExists` rather
/// than silently overwritten. Combined with the collision-safe
/// suffix and `rename_no_replace`, this closes every cross-process
/// path-reuse window.
///
/// R9 / F7 third re-review 2026-07-25 (final-stack blocker).
fn stage_archive_file(
    _change_id: &str,
    target: &Path,
    bytes: &[u8],
) -> Result<ArchiveStaging, crate::operations::OperationError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::operations::OperationError::ArchiveWrite {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }
    let counter = STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let staging = match target.file_name() {
        Some(name) => target.with_file_name(format!(
            "{}.staging-{}-{}-{}",
            name.to_string_lossy(),
            nanos,
            std::process::id(),
            counter,
        )),
        None => {
            return Err(crate::operations::OperationError::ArchiveWrite {
                path: target.to_path_buf(),
                source: std::io::Error::other("archive target has no file name"),
            });
        }
    };
    use std::io::Write;
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|source| crate::operations::OperationError::ArchiveWrite {
                path: target.to_path_buf(),
                source,
            })?;
        file.write_all(bytes).map_err(|source| {
            let _ = std::fs::remove_file(&staging);
            crate::operations::OperationError::ArchiveWrite {
                path: target.to_path_buf(),
                source,
            }
        })?;
        file.sync_all().ok();
    }
    Ok(ArchiveStaging {
        staging_file: staging,
        target_file: target.to_path_buf(),
    })
}

/// Commits a previously staged archive via `rename_no_replace`.
fn commit_archive_staging(
    staging: ArchiveStaging,
) -> Result<(), crate::operations::OperationError> {
    match rename_no_replace(&staging.staging_file, &staging.target_file) {
        Ok(()) => Ok(()),
        Err(error) => Err(crate::operations::OperationError::ArchiveWrite {
            path: staging.target_file.clone(),
            source: error,
        }),
    }
}

/// Writes a deterministic archive to `<target>` on disk. Convenience
/// wrapper that stages and immediately commits; callers that need
/// multi-entry atomicity should use `stage_archive_file` and
/// `commit_archive_staging` directly.
fn write_archive_file(
    change_id: &str,
    target: &Path,
    bytes: &[u8],
) -> Result<(), crate::operations::OperationError> {
    let staging = stage_archive_file(change_id, target, bytes)?;
    commit_archive_staging(staging)
}

/// Placeholder for the eventual `export` operation. Wired in task
/// 7.x; until then it serves only as a registration point.
pub async fn export(
    client: &nb_api::NbClient,
    notebook: Option<&str>,
    change_id: &str,
    target: &std::path::Path,
    options: ExportOptions,
) -> Result<crate::operations::OperationOutcome, crate::operations::OperationError> {
    // R1 (F1): reject change IDs that contain path components or
    // escape sequences before constructing any filesystem path.
    // Kebab-case validation alone is sufficient: a kebab-case
    // string contains no `.`, `/`, or `..`, so `target.join(change_id)`
    // cannot escape `target`.
    crate::changes::validate_change_id(change_id)
        .map_err(|_| InterchangeError::InvalidChangeId(change_id.to_string()))?;
    let notebook_name = resolve_notebook(notebook)?;
    let plan = build_export_plan(client, change_id, target, &notebook_name, &options).await?;
    let plan_text = render_export_plan_text(&plan);

    if plan.dry_run {
        let structured = serde_json::json!({
            "change_id": plan.change_id,
            "notebook": plan.notebook,
            "target": plan.target.display().to_string(),
            "would_overwrite": plan.would_overwrite,
            "entries": plan.entries.iter().map(|entry| serde_json::json!({"kind": "write", "path": entry.tree_path})).collect::<Vec<_>>(),
        });
        return Ok(crate::operations::OperationOutcome::new(
            plan_text, structured,
        ));
    }

    if plan.would_overwrite {
        let structured = serde_json::json!({
            "change_id": plan.change_id,
            "would_overwrite": plan.would_overwrite,
        });
        let _text = format!(
            "{plan_text}refusal: target filesystem tree already exists; pass --overwrite to replace"
        );
        let _ = structured;
        return Err(crate::operations::OperationError::from(
            InterchangeError::OverwriteRefused {
                change_id: plan.change_id.clone(),
                target: plan.target.display().to_string(),
            },
        ));
    }

    execute_export_plan(client, &plan).await
}

/// One file write in an export plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEntry {
    /// Logical path under `<target>/<change-id>/` (forward slashes).
    pub tree_path: String,
    /// On-disk path the write will land at.
    pub absolute_path: PathBuf,
    /// Note content to write.
    pub content: String,
}

#[allow(dead_code)] // used by future variants (e.g. Refusal / Skip for export)
impl ExportEntry {
    /// Convenience constructor for a write entry.
    pub fn write(
        tree_path: impl Into<String>,
        absolute_path: PathBuf,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tree_path: tree_path.into(),
            absolute_path,
            content: content.into(),
        }
    }
}

/// The export plan: per-file writes plus whether the target
/// already exists on disk (refused without `--overwrite`). The
/// plan is built once and the same `tree_path` set is used for
/// both the dry-run output and the actual writes, so the two
/// outputs never disagree.
///
/// `overwrite_authorized` records the operator's `--overwrite`
/// authorization so the execute path can re-validate the
/// destination state without re-reading CLI flags. Without this
/// field, the execute path would have no way to distinguish
/// "operator authorized an overwrite" from "no overwrite flag,
/// and the directory happened to not exist at plan time but
/// appeared at execute time" (the R7 / F7 TOCTOU window).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPlan {
    pub change_id: String,
    pub notebook: String,
    pub target: PathBuf,
    pub entries: Vec<ExportEntry>,
    pub would_overwrite: bool,
    pub overwrite_authorized: bool,
    pub dry_run: bool,
}

/// Builds the export plan: walks the notebook change and gathers
/// the writes that compose `<target>/<change-id>/`. The plan is
/// refused if `<target>/<change-id>/` already exists and
/// `--overwrite` was not given.
///
/// R6 (F6): the proposal is required; an absent proposal
/// surfaces as `RequiredNoteAbsent` (no silent empty write).
/// The meta note is required too — without it the change is
/// not lifecycle-valid (R4 / F4). All other artifacts are
/// optional.
async fn build_export_plan(
    client: &nb_api::NbClient,
    change_id: &str,
    target: &Path,
    notebook_name: &str,
    options: &ExportOptions,
) -> Result<ExportPlan, crate::operations::OperationError> {
    let change_root = target.join(change_id);
    let would_overwrite = change_root.is_dir() && !options.overwrite;

    let proposal_content =
        match read_notebook_note(client, change_id, "", "proposal", notebook_name).await? {
            Some(content) => content,
            None => {
                return Err(InterchangeError::RequiredNoteAbsent {
                    notebook: notebook_name.to_string(),
                    selector: format!("proposals/{change_id}/proposal.md"),
                }
                .into());
            }
        };
    let mut entries: Vec<ExportEntry> = Vec::new();
    entries.push(ExportEntry {
        tree_path: "proposal.md".to_string(),
        absolute_path: change_root.join("proposal.md"),
        content: proposal_content,
    });

    for (capability, content) in
        list_notebook_folder(client, change_id, "specifications", notebook_name).await?
    {
        if capability.is_empty() {
            continue;
        }
        let tree_path = format!("specs/{capability}/spec.md");
        entries.push(ExportEntry {
            tree_path,
            absolute_path: change_root.join("specs").join(&capability).join("spec.md"),
            content,
        });
    }

    if let Some((_, first_design)) =
        first_notebook_folder_entry(client, change_id, "designs", notebook_name).await?
    {
        entries.push(ExportEntry {
            tree_path: "design.md".to_string(),
            absolute_path: change_root.join("design.md"),
            content: first_design,
        });
    }

    for (name, content) in
        list_notebook_folder(client, change_id, "decisions", notebook_name).await?
    {
        let tree_path = format!("decisions/{name}.md");
        entries.push(ExportEntry {
            tree_path,
            absolute_path: change_root.join("decisions").join(format!("{name}.md")),
            content,
        });
    }

    if let Some(tasks_content) = read_notebook_work_note(client, change_id, notebook_name).await? {
        entries.push(ExportEntry {
            tree_path: "tasks.md".to_string(),
            absolute_path: change_root.join("tasks.md"),
            content: tasks_content,
        });
    }

    Ok(ExportPlan {
        change_id: change_id.to_string(),
        notebook: notebook_name.to_string(),
        target: target.to_path_buf(),
        entries,
        would_overwrite,
        overwrite_authorized: options.overwrite,
        dry_run: options.dry_run,
    })
}

/// Atomically renames `src` to `dst` without replacing an
/// existing destination. On Linux, this uses `renameat2` with
/// `RENAME_NOREPLACE`, which is atomic and concurrent-occupant
/// safe. Elsewhere, falls back to a check-then-rename sequence
/// with a documented small race window (the publish re-check
/// closes the practical exposure on non-Linux systems; the race
/// window between the check and the rename is microseconds).
///
/// R7'''' / F7 third re-review 2026-07-25: addresses the F7
/// structural blocker that "non-overwrite export still samples
/// before staging; a target appearing before publication can be
/// backed up/replaced without authorization." With this helper
/// the non-overwrite path returns `EEXIST` deterministically
/// when an occupant appears at the publish boundary.
#[cfg(target_os = "linux")]
fn rename_no_replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let src_c = CString::new(src.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    // Safety: documented `libc::renameat2` invocation with
    // `AT_FDCWD` for both directories and `RENAME_NOREPLACE` to
    // fail with EEXIST if `dst` already exists. The C strings
    // are constructed from valid Rust paths with no interior NULs.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            src_c.as_ptr(),
            libc::AT_FDCWD,
            dst_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn rename_no_replace(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    // R7_fourth re-review 2026-07-25: the previous check-then-rename
    // fallback kept a residual race window between the `dst.exists()`
    // sample and the actual `rename` call. There is no portable
    // atomic no-replace rename on non-Linux platforms without
    // `renameat2`. Refuse explicitly so the operator can fail
    // closed rather than discover the race after the fact; the
    // publish re-check at the publish boundary is the wider guard
    // and remains in place.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is not available on this platform; \
         refusing rather than risking a race",
    ))
}

/// Creates a directory at `path` exclusively: returns an error if
/// the directory already exists. This is the final collision guard
/// after the suffix uniqueness (counter + nanos + pid).
///
/// R7''''' / F7 third re-review 2026-07-25.
fn create_dir_exclusive(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

/// Derives a sibling quarantine path for an import source tree.
///
/// R7'' / F7 re-review 2026-07-25: source deletion is staged via
/// rename-to-quarantine. The quarantine is a sibling of the
/// source path (same parent directory) so `std::fs::rename` is
/// atomic on POSIX.
///
/// R7''''' / F7 third re-review 2026-07-25: the suffix appends
/// `.quarantine-<nanos>-<pid>-<counter>` so concurrent
/// invocations within the same process (where nanos may collide
/// in tight loops) and across processes both avoid collisions.
/// The exclusive creation step (`create_dir_exclusive`) closes
/// any residual collision window.
fn quarantine_path_for(source: &Path) -> Result<PathBuf, InterchangeError> {
    let parent = source
        .parent()
        .ok_or_else(|| InterchangeError::LayoutAnomaly {
            path: source.to_path_buf(),
            message: "source path has no parent; cannot derive a quarantine".to_string(),
        })?;
    let file_name = source
        .file_name()
        .ok_or_else(|| InterchangeError::LayoutAnomaly {
            path: source.to_path_buf(),
            message: "source path has no file name; cannot derive a quarantine".to_string(),
        })?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut quarantine = parent.to_path_buf();
    quarantine.push(format!(
        "{}.quarantine-{}-{}-{}",
        file_name.to_string_lossy(),
        nanos,
        std::process::id(),
        QUARANTINE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    Ok(quarantine)
}

/// Renders the export plan as text for the CLI's `text` field.
pub fn render_export_plan_text(plan: &ExportPlan) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "Export plan for change {} from notebook {} to {}:\n",
        plan.change_id,
        plan.notebook,
        plan.target.display()
    ));
    if plan.would_overwrite {
        output.push_str("  warning: target tree exists; refused without --overwrite\n");
    }
    for entry in &plan.entries {
        output.push_str(&format!("  write: {}\n", entry.tree_path));
    }
    output.push_str("verdicts are notebook-resident and are NOT exported.\n");
    output
}

/// Executes a plan that has passed the overwrite gate: writes every
/// entry to disk under `<target>/<change-id>/`. Removes any
/// pre-existing tree first when `--overwrite` was set.
async fn execute_export_plan(
    _client: &nb_api::NbClient,
    plan: &ExportPlan,
) -> Result<crate::operations::OperationOutcome, crate::operations::OperationError> {
    let change_root = plan.target.join(&plan.change_id);
    // R7' (F7 re-review 2026-07-25): every export is staged into
    // a sibling directory; the destination is renamed only after
    // every write succeeds. If the destination exists and is not
    // overwrite-authorized, refuse without touching either path.
    // If the destination exists and overwrite IS authorized, the
    // old tree is renamed to a backup sibling before the staging
    // dir is renamed into place; the backup is kept after success
    // for operator recovery.
    //
    // R7'''' (F7 second re-review 2026-07-25): the original
    // check-then-act sampled the destination at plan time, then
    // published without re-validating. A target appearing between
    // the sample and the publish rename would be backed up or
    // replaced without authorization. The publish boundary now
    // re-checks the destination; a non-overwrite export refuses
    // any execute-time occupant rather than backing it up.
    //
    // Rollback failures during the publish phase are surfaced in
    // the structured payload (`rollback_failures`) and as
    // `RESTORE FAILURE` lines in stdout rather than silently
    // dropped. The fsync call now operates on the staging
    // directory file descriptor so directory entries are durable
    // before the rename (the sentinel-file pattern only fsynced
    // a regular file, which does not flush directory metadata).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staging = plan.target.join(format!(
        ".{}.staging-{}-{}-{}",
        plan.change_id,
        nanos,
        std::process::id(),
        counter,
    ));
    let backup = plan.target.join(format!(
        ".{}.backup-{}-{}-{}",
        plan.change_id,
        nanos,
        std::process::id(),
        counter,
    ));

    // Stage every entry into the sibling staging directory. Any
    // failure here cleans up the partial staging dir and surfaces
    // the error; the destination is never touched.
    let mut written: Vec<String> = Vec::new();
    let mut rollback_failures: Vec<String> = Vec::new();
    // R7''''' / F7 third re-review: build the partial output up
    // front so rollback diagnostics can be appended before the
    // function returns. The final `output` is rebuilt below for
    // consistency; the partial one is only used to surface the
    // rollback path before the structured payload is built.
    let mut output = String::new();
    let stage_result: Result<(), crate::operations::OperationError> = (|| {
        // R7''''' / F7 third re-review: exclusive creation guards
        // against residual collision from concurrent invocations.
        if let Err(error) = create_dir_exclusive(&staging) {
            return Err(crate::operations::OperationError::NoteRead {
                path: staging.clone(),
                source: error,
            });
        }
        for entry in &plan.entries {
            let staged_path = staging.join(&entry.tree_path);
            if let Some(parent) = staged_path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| {
                    crate::operations::OperationError::NoteRead {
                        path: parent.to_path_buf(),
                        source,
                    }
                })?;
            }
            std::fs::write(&staged_path, entry.content.as_bytes()).map_err(|source| {
                crate::operations::OperationError::NoteRead {
                    path: staged_path.clone(),
                    source,
                }
            })?;
            written.push(entry.tree_path.clone());
        }
        // Best-effort directory fsync: flushes the staging
        // directory's metadata so subsequent rename sees a
        // durable source. POSIX rename is atomic on the same
        // filesystem, so a crash after this point leaves either
        // the old tree or the new tree, never a partial one.
        // (Sub-directories and the file contents they hold are
        // not separately fsynced; full checked durability would
        // require walking and syncing every file plus the parent
        // directory.)
        if let Ok(dir_file) = std::fs::File::open(&staging) {
            let _ = dir_file.sync_all();
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    // Publish phase: re-validate the destination at the publish
    // boundary and use `rename_no_replace` (Linux
    // `renameat2(RENAME_NOREPLACE)`; documented best-effort
    // window elsewhere) so an occupant appearing between this
    // check and the actual rename is rejected with EEXIST rather
    // than silently backed up or replaced. With overwrite
    // authorization, the old tree is renamed to the backup
    // sibling first.
    if change_root.is_dir() && !plan.overwrite_authorized {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(crate::operations::OperationError::from(
            InterchangeError::OverwriteRefused {
                change_id: plan.change_id.clone(),
                target: plan.target.display().to_string(),
            },
        ));
    }

    if change_root.is_dir() {
        if let Err(error) = std::fs::rename(&change_root, &backup) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(crate::operations::OperationError::NoteRead {
                path: change_root.clone(),
                source: std::io::Error::other(format!(
                    "cannot rename existing tree to backup at {}: {error}",
                    backup.display()
                )),
            });
        }
        match rename_no_replace(&staging, &change_root) {
            Ok(()) => {}
            Err(error) => {
                // R7''''' / F7 third re-review: emit a structured
                // outcome with exact recovery status rather than
                // dropping the rollback report on the floor. The
                // publish phase failed; we attempt to restore the
                // backup. Success or failure of the restore is
                // recorded with the surviving backup path.
                let restore_status = match std::fs::rename(&backup, &change_root) {
                    Ok(()) => format!(
                        "publish rename failed: {error}; restored from backup at {}",
                        backup.display()
                    ),
                    Err(restore_error) => format!(
                        "publish rename failed: {error}; restore from backup failed: {restore_error}; \
                         backup remains at {} — operator must reconcile manually",
                        backup.display()
                    ),
                };
                rollback_failures.push(restore_status.clone());
                output.push_str(&format!("ROLLBACK FAILURE: {restore_status}\n"));
                let _ = std::fs::remove_dir_all(&staging);
                // R7''''' / F7 third re-review: the rolled-back
                // error path returns Err with the exact recovery
                // status (publish error + restore outcome +
                // surviving backup path) in the error chain;
                // structured-payload surfacing of rollback_failures
                // can join later when the existing OperationOutcome
                // return type supports carrying both an error and
                // a payload.
                return Err(crate::operations::OperationError::NoteRead {
                    path: change_root.clone(),
                    source: std::io::Error::other(restore_status),
                });
            }
        }
    } else if let Err(error) = rename_no_replace(&staging, &change_root) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(crate::operations::OperationError::NoteRead {
            path: change_root.clone(),
            source: std::io::Error::other(format!("cannot publish staging tree: {error}")),
        });
    }

    let mut output = render_export_plan_text(plan);
    for tree_path in &written {
        output.push_str(&format!("wrote {tree_path}\n"));
    }
    output.push_str(&format!(
        "Exported change {} to {}\n",
        plan.change_id,
        change_root.display()
    ));
    let backup_path = if backup.exists() {
        Some(backup.display().to_string())
    } else {
        None
    };
    let structured = serde_json::json!({
        "change_id": plan.change_id,
        "notebook": plan.notebook,
        "target": change_root.display().to_string(),
        "written": written,
        "backup": backup_path,
        "rollback_failures": Vec::<String>::new(),
    });
    Ok(crate::operations::OperationOutcome::new(output, structured))
}

/// Reads a notebook note by change id and stem.
///
/// Returns:
/// - `Ok(Some(content))` when the note exists; the leading
///   title-derived H1 (`# <stem>`) is stripped so the export
///   sees only the source content.
/// - `Ok(None)` only when the note is **demonstrably absent**
///   (the `proposals/<id>/<stem>.md` file does not exist on the
///   notebook filesystem). This is the only "missing is OK"
///   case: every backend IO failure propagates as `Err`.
///
/// R6 (F6): every other `nb-api` error is propagated. The
/// previous swallow-everything heuristic silently exported
/// empty content for genuinely broken backends, which then
/// succeeded and could trigger downstream data loss.
async fn read_notebook_note(
    client: &nb_api::NbClient,
    change_id: &str,
    subfolder: &str,
    stem: &str,
    notebook_name: &str,
) -> Result<Option<String>, crate::operations::OperationError> {
    let notebook_path = client.show_notebook_path(Some(notebook_name)).await?;
    let note_path = if subfolder.is_empty() {
        notebook_path
            .join("proposals")
            .join(change_id)
            .join(format!("{stem}.md"))
    } else {
        notebook_path
            .join("proposals")
            .join(change_id)
            .join(subfolder)
            .join(format!("{stem}.md"))
    };
    if !note_path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&note_path).map_err(|source| {
        crate::operations::OperationError::NoteRead {
            path: note_path.clone(),
            source,
        }
    })?;
    let expected_h1 = format!("# {stem}");
    Ok(Some(strip_title_h1(&raw, &expected_h1)))
}

/// Strips the `# <title>` H1 that nb prepends to a note's on-disk
/// content when the note was created via `nb add --title <T>`.
/// Returns the content unchanged when the leading H1 does not
/// match the expected title (e.g. when the note predates the
/// current import path, or was edited by hand to remove the
/// duplicate).
pub fn strip_title_h1(content: &str, expected_h1: &str) -> String {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return content.to_string();
    };
    if first.trim() == expected_h1 {
        let mut out = String::new();
        // Drop the first line and any blank line that follows it
        // (nb inserts a blank between the title H1 and the body).
        let mut skipping_blanks = true;
        for line in lines {
            if skipping_blanks && line.is_empty() {
                continue;
            }
            skipping_blanks = false;
            out.push_str(line);
            out.push('\n');
        }
        return out;
    }
    content.to_string()
}

/// Lists every note under a notebook change subfolder, returning
/// `(filename_stem, content)` pairs in alphabetical order. Used
/// for the `specifications`, `designs`, and `decisions` artifact
/// folders. Returns an empty Vec when the folder is missing.
///
/// Walks the notebook filesystem directly rather than calling
/// `nb ls`: `nb ls` returns relative paths whose format varies
/// across versions, while the filesystem is authoritative.
async fn list_notebook_folder(
    client: &nb_api::NbClient,
    change_id: &str,
    subfolder: &str,
    notebook_name: &str,
) -> Result<Vec<(String, String)>, crate::operations::OperationError> {
    let notebook_path = client.show_notebook_path(Some(notebook_name)).await?;
    let folder = notebook_path
        .join("proposals")
        .join(change_id)
        .join(subfolder);
    if !folder.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    let Ok(dir_entries) = std::fs::read_dir(&folder) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<PathBuf> = dir_entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();
    for path in paths {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // The notebook stores notes with the title H1 prepended
        // by `nb add --title <T>`; strip it so the export sees
        // the source content only, matching the import's
        // round-trip fidelity.
        let stripped = strip_title_h1(&content, &format!("# {stem}"));
        entries.push((stem.to_string(), stripped));
    }
    Ok(entries)
}

/// Returns the first note under a notebook change subfolder, or
/// `Ok(None)` if the folder is absent.
async fn first_notebook_folder_entry(
    client: &nb_api::NbClient,
    change_id: &str,
    subfolder: &str,
    notebook_name: &str,
) -> Result<Option<(String, String)>, crate::operations::OperationError> {
    Ok(
        list_notebook_folder(client, change_id, subfolder, notebook_name)
            .await?
            .into_iter()
            .next(),
    )
}

/// Reads the change's `work` todo note (the `work.todo.md` file)
/// from the notebook filesystem and reconstructs it as
/// `tasks.md`. Returns `Ok(None)` when the work note is absent.
async fn read_notebook_work_note(
    client: &nb_api::NbClient,
    change_id: &str,
    notebook_name: &str,
) -> Result<Option<String>, crate::operations::OperationError> {
    let notebook_path = client.show_notebook_path(Some(notebook_name)).await?;
    let change_directory = notebook_path.join("proposals").join(change_id);
    let entries = match std::fs::read_dir(&change_directory) {
        Ok(entries) => entries,
        Err(_) => return Ok(None),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".todo.md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let open_title = format!("# [ ] {WORK_NOTE_TITLE}");
        let done_title = format!("# [x] {WORK_NOTE_TITLE}");
        if content
            .lines()
            .any(|line| line == open_title || line == done_title)
        {
            return Ok(Some(render_tasks_md(&content)));
        }
    }
    Ok(None)
}

/// Title used in the work todo note's checkbox title line.
const WORK_NOTE_TITLE: &str = "work";

/// Reconstructs `tasks.md` from a work todo note body. The work
/// note's checkbox title line `# [x] work` becomes the `# Tasks`
/// heading; the items keep their checked/unchecked state.
fn render_tasks_md(work_content: &str) -> String {
    let mut body = String::new();
    let mut in_items = false;
    for line in work_content.lines() {
        if line.starts_with("# [ ] work") || line.starts_with("# [x] work") {
            body.push_str("# Tasks\n\n");
            in_items = true;
            continue;
        }
        if in_items {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

/// Parses the `nb ls <folder>` listing into markdown filenames
/// (one per line, dropping counters and non-`.md` entries).
#[allow(dead_code)] // replaced by direct filesystem walk; retained for the next round of helpers
fn parse_markdown_filenames(listing: &str) -> Vec<String> {
    let mut names: Vec<String> = listing
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("0 items") {
                return None;
            }
            let last = trimmed.split_whitespace().last()?;
            if last.ends_with(".md") {
                Some(last.to_string())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Reports whether a `nb-api` error reflects an absent note (versus
/// an IO failure that the operator must address).
#[allow(dead_code)] // kept for the next round of read helpers; not currently called
fn is_absent_note_error(error: &nb_api::NbError) -> bool {
    // Only treat genuinely "not found" errors as absent. Any other
    // failure (selector mismatch, IO error, command failure)
    // surfaces as a real error so the export can fail loudly
    // rather than silently producing empty files.
    let message = error.to_string();
    message.contains("not found")
        || message.contains("No such")
        || message.contains("does not exist")
}

/// Runs the export against the imported change and diffs the
/// resulting tree against the source tree modulo the typed
/// normalizations. The proof is the inverse composition: `import`
/// and `export` are inverses by construction, so a clean
/// round-trip authorizes `--delete-original`. Divergences are
/// named explicitly so the operator sees what was elided.
///
/// The proof exercises the export path on every gated import,
/// regression-testing the export surface. It also covers content
/// the rendered set does not include — most importantly,
/// `tasks.md` reconstruction from the `work` note.
///
/// # Fail-closed semantics (R2 / F2)
///
/// Every walk error surfaces as a divergence. A symlink in the
/// source tree is a divergence (symlinks are not part of the
/// deterministic interchange artifact). A non-UTF-8 file
/// preserves its bytes verbatim (no silent `read_to_string`
/// drop); the proof compares bytes against the bytes preserved
/// in the archive, so binary files round-trip cleanly. A file
/// missing from one side is a divergence. The proof never
/// returns `clean` on an incomplete inventory.
///
/// # Errors
///
/// Returns filesystem IO errors only when an entry cannot be
/// listed at all (e.g. permission denied on the root itself).
/// Per-file failures are surfaced as divergences, not as
/// panics, so the proof is total.
///
/// R7''' / F7 second re-review 2026-07-25: the proof now takes
/// the source change root and scratch change root directly,
/// rather than deriving them from `<root>/<change_id>/`. This
/// lets the caller point the proof at a quarantined snapshot
/// (which is renamed to a sibling path that no longer contains
/// a `<change_id>/` subdirectory) without restructuring the
/// quarantine layout.
pub fn round_trip_proof(
    imported_change_id: &str,
    source_change_root: &Path,
    scratch_change_root: &Path,
) -> RoundTripProof {
    let source_change = source_change_root;
    let scratch_change = scratch_change_root;
    let mut divergences: Vec<String> = Vec::new();
    if !source_change.is_dir() {
        divergences.push(format!(
            "{imported_change_id}: source tree not found at {}",
            source_change.display()
        ));
        return RoundTripProof {
            clean: false,
            divergences,
        };
    }
    if !scratch_change.is_dir() {
        divergences.push(format!(
            "{imported_change_id}: export tree not found at {}",
            scratch_change.display()
        ));
        return RoundTripProof {
            clean: false,
            divergences,
        };
    }

    let source_files = collect_files_for_proof(source_change, "source", &mut divergences);
    for (relative, source_bytes) in &source_files {
        let export_path = scratch_change.join(relative);
        let export_bytes = match std::fs::read(&export_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                divergences.push(format!("{}: missing from export ({})", relative, error));
                continue;
            }
        };
        let canonical_source = canonical_bytes_for_proof(relative, source_bytes);
        let canonical_export = canonical_bytes_for_proof(relative, &export_bytes);
        if canonical_source == canonical_export {
            continue;
        }
        divergences.push(format!(
            "{}: content diverges beyond typed normalizations",
            relative
        ));
    }

    // Files only in the export tree are divergences too.
    let export_files = collect_files_for_proof(scratch_change, "export", &mut divergences);
    for (relative, _) in &export_files {
        if !source_files
            .iter()
            .any(|(source_relative, _)| source_relative == relative)
        {
            divergences.push(format!(
                "{}: present in export but missing from source",
                relative
            ));
        }
    }

    RoundTripProof {
        clean: divergences.is_empty(),
        divergences,
    }
}

/// Collects every regular file under `root` as `(relative_path,
/// content_bytes)` pairs. Returns divergences via the supplied
/// accumulator (rather than swallowing them) so the proof fails
/// closed on every walk error: unreadable directory listings,
/// non-UTF-8 filenames, symlinks, IO failures, and non-`is_file`
/// entries all surface as named divergences.
///
/// R2 (F2): the previous `collect_text_files` silently skipped
/// every problematic entry (binary files, symlinks, IO errors),
/// then compared the partial inventory as if it were complete.
/// A clean proof could then authorize `remove_dir_all` on
/// inventoried-as-complete source data.
///
/// R2' (F2 re-review 2026-07-25): the walker still used
/// `ReadDir::flatten()` (silently dropping iterator errors) and
/// `into_string().ok()` (silently dropping non-UTF-8 filenames);
/// `strip_prefix` failure was also silently swallowed. An empty
/// divergences list no longer authorizes deletion unless every
/// entry was explicitly enumerated, with explicit failures surfaced.
fn collect_files_for_proof(
    root: &Path,
    side: &str,
    divergences: &mut Vec<String>,
) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    walk_for_proof(root, root, side, &mut files, divergences);
    files.sort();
    files
}

fn walk_for_proof(
    root: &Path,
    current: &Path,
    side: &str,
    files: &mut Vec<(String, Vec<u8>)>,
    divergences: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) => {
            divergences.push(format!(
                "{side}: cannot read directory {}: {error}",
                current.display()
            ));
            return;
        }
    };
    let mut names: Vec<String> = Vec::new();
    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                divergences.push(format!(
                    "{side}: read_dir entry error in {}: {error}",
                    current.display()
                ));
                continue;
            }
        };
        match entry.file_name().into_string() {
            Ok(name) => names.push(name),
            Err(os_str) => {
                divergences.push(format!(
                    "{side}: non-UTF-8 filename in {}: {:?}",
                    current.display(),
                    os_str
                ));
            }
        }
    }
    names.sort();
    for name in names {
        let path = current.join(&name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                divergences.push(format!("{side}: cannot stat {}: {error}", path.display()));
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            divergences.push(format!(
                "{side}: symlink at {} is not part of the deterministic interchange artifact",
                path.display()
            ));
            continue;
        }
        if metadata.is_dir() {
            walk_for_proof(root, &path, side, files, divergences);
        } else if metadata.is_file() {
            let relative = match path.strip_prefix(root) {
                Ok(relative) => relative,
                Err(error) => {
                    divergences.push(format!(
                        "{side}: path {} not under root {}: {error}",
                        path.display(),
                        root.display()
                    ));
                    continue;
                }
            };
            let posix = match relative.to_str() {
                Some(s) => s.replace(std::path::MAIN_SEPARATOR, "/"),
                None => {
                    divergences.push(format!(
                        "{side}: non-UTF-8 path component at {}",
                        relative.display()
                    ));
                    continue;
                }
            };
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    divergences.push(format!("{side}: cannot read {}: {error}", path.display()));
                    continue;
                }
            };
            files.push((posix, bytes));
        } else {
            divergences.push(format!(
                "{side}: unsupported entry type at {}",
                path.display()
            ));
        }
    }
}

/// Canonicalizes file bytes for round-trip proof comparison.
///
/// Text artifacts under the change tree (proposal.md, specs/*/spec.md,
/// design.md, decisions/*.md, tasks.md) are normalized by their
/// typed rules (H1 strip for proposal / spec / design / decisions,
/// tasks.md header normalization). Binary and other artifacts are
/// compared verbatim.
///
/// R2 (F2): the previous proof compared `String`s after
/// `read_to_string`; non-UTF-8 files failed the read, were
/// silently dropped, and never reached the comparison. We now
/// carry bytes end-to-end and only apply text normalizations to
/// the change-tree artifacts that the spec defines.
fn canonical_bytes_for_proof(relative_path: &str, bytes: &[u8]) -> Vec<u8> {
    let is_text = relative_path == "proposal.md"
        || relative_path == "tasks.md"
        || relative_path == "design.md"
        || (relative_path.starts_with("specs/") && relative_path.ends_with("/spec.md"))
        || (relative_path.starts_with("decisions/") && relative_path.ends_with(".md"));
    if !is_text {
        return bytes.to_vec();
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let normalized = canonical_for_proof(relative_path, text);
    normalized.into_bytes()
}

/// Canonicalizes a file's content for round-trip proof comparison,
/// applying the typed normalizations. Each branch corresponds to
/// one entry in the `RoundTripNormalization` enum; new categories
/// are added deliberately, not by accumulation.
fn canonical_for_proof(relative_path: &str, content: &str) -> String {
    if relative_path == "proposal.md" {
        return canonical_proposal_for_proof(content);
    }
    if relative_path == "tasks.md" {
        return canonical_tasks_for_proof(content);
    }
    if relative_path.starts_with("decisions/") {
        return canonical_decision_for_proof(content);
    }
    if relative_path.starts_with("specs/") && relative_path.ends_with("/spec.md") {
        return canonical_spec_for_proof(content);
    }
    if relative_path == "design.md" {
        return canonical_spec_for_proof(content);
    }
    content.to_string()
}

/// Strips a single leading H1 line (e.g. `# <title>`) from the
/// content. Used during import for spec, design, and decision
/// files whose H1 duplicates the notebook title; the round-trip
/// proof canonicalizes both sides with the same strip so the
/// missing H1 on the export side is tolerated.
fn strip_leading_h1(content: &str) -> String {
    let mut stripped = String::new();
    let mut skipped = false;
    for line in content.lines() {
        if !skipped && line.starts_with("# ") {
            skipped = true;
            continue;
        }
        stripped.push_str(line);
        stripped.push('\n');
    }
    stripped
}

/// Canonicalizes spec/design/decisions content by stripping a
/// leading H1 line. Symmetric with the import-side `strip_leading_h1`.
fn canonical_spec_for_proof(content: &str) -> String {
    strip_leading_h1(content)
}

/// Strips the proposal's leading H1 line: `H1Rewrite` is the
/// normalized form, so a divergence on the first line is the
/// expected normalization rather than a real change. Trailing
/// whitespace is also trimmed so differences in trailing newlines
/// do not register as divergences.
fn canonical_proposal_for_proof(content: &str) -> String {
    let mut stripped = String::new();
    for (index, line) in content.lines().enumerate() {
        if index == 0 && line.starts_with("# ") {
            continue;
        }
        stripped.push_str(line);
        stripped.push('\n');
    }
    stripped.trim_end().to_string()
}

/// Normalizes the tasks.md header: `# Tasks` (with or without the
/// `[ ]/x` checkbox title marker, blank line or no) reduces to
/// `# Tasks\n\n`. Leading blanks after the header are elided so
/// the canonical form is the same regardless of whether the source
/// separated the header from the items with a blank line. The
/// rest of the body is unchanged so per-item checkbox state is
/// compared faithfully.
fn canonical_tasks_for_proof(content: &str) -> String {
    let mut output = String::new();
    let mut emitted_header = false;
    let mut skipping_blanks = false;
    for line in content.lines() {
        if !emitted_header {
            let is_header = line.starts_with("# Tasks")
                || line.starts_with("# [ ] Tasks")
                || line.starts_with("# [x] Tasks");
            let is_blank = line.trim().is_empty();
            if is_header {
                output.push_str("# Tasks\n\n");
                emitted_header = true;
                skipping_blanks = true;
                continue;
            }
            if is_blank {
                continue;
            }
            // First non-blank, non-tasks-header line: emit header.
            output.push_str("# Tasks\n\n");
            emitted_header = true;
            skipping_blanks = true;
        }
        if skipping_blanks && line.trim().is_empty() {
            continue;
        }
        skipping_blanks = false;
        output.push_str(line);
        output.push('\n');
    }
    if !emitted_header {
        output.push_str("# Tasks\n\n");
    }
    output
}

/// Decision content is compared verbatim; the `DecisionSlot`
/// normalization covers filename preservation (the proof compares
/// same-named files; no body-level transformation is performed).
fn canonical_decision_for_proof(content: &str) -> String {
    content.to_string()
}

/// Options for `nbspec import`. Carries every flag the verb accepts
/// so the eventual implementation has a single, typed entry point.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// Emit the plan only; do not write notes or archives and do
    /// not delete the source filesystem tree.
    pub dry_run: bool,
    /// Authorize deletion of the source filesystem tree after a
    /// clean round-trip proof.
    pub delete_original: bool,
    /// Skip active change tree detection (default: detect both).
    /// In v0.3.0 the active execute arm is a no-op; this flag
    /// emits a `Skip` entry instead of a paused `ActiveWrite`
    /// entry for each detected active tree.
    pub no_active: bool,
    /// Skip legacy archive tree ingestion (default: ingest both).
    pub no_archives: bool,
}

/// Options for `nbspec export`. Carries every flag the verb accepts.
#[derive(Clone, Debug, Default)]
pub struct ExportOptions {
    /// Emit the plan only; do not write the filesystem tree.
    pub dry_run: bool,
    /// Overwrite an existing `<target>/<change-id>/` filesystem
    /// tree without refusing.
    pub overwrite: bool,
}

/// Errors from interchange operations.
#[derive(Debug, Error)]
pub enum InterchangeError {
    #[error("cannot read source file {path}: {source}")]
    SourceRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("source tasks.md parse failure: {0}")]
    TasksParse(#[from] WorkNoteError),

    #[error(
        "change-id collision: notebook namespace proposals/{0}/ already exists; \
         refusing to overwrite without explicit operator intervention"
    )]
    Collision(String),

    #[error(
        "refusal: target filesystem tree {target}/{change_id}/ already exists; \
         pass --overwrite to replace"
    )]
    OverwriteRefused { change_id: String, target: String },

    #[error("refusal: import aborted by {count} refusal(s); see plan output for details")]
    ImportRefused { count: usize },

    #[error("invalid change id {0:?}: expected kebab-case (lowercase alphanumerics and hyphens)")]
    InvalidChangeId(String),

    #[error("notebook read failure for {selector} in notebook {notebook}: {message}")]
    NotebookRead {
        notebook: String,
        selector: String,
        message: String,
    },

    #[error(
        "required notebook note absent: {selector} (in notebook {notebook}); \
         refusing to export a partial change"
    )]
    RequiredNoteAbsent { notebook: String, selector: String },

    #[error("filesystem layout anomaly at {path}: {message}")]
    LayoutAnomaly { path: PathBuf, message: String },

    #[error("notebook listing failure for {folder} in notebook {notebook}: {message}")]
    ListingFailed {
        notebook: String,
        folder: String,
        message: String,
    },

    #[error("archive fidelity proof failed for {change_id}: {} divergence(s)", divergences.len())]
    ArchiveFidelity {
        change_id: String,
        divergences: Vec<String>,
    },
}

impl From<InterchangeError> for crate::operations::OperationError {
    fn from(error: InterchangeError) -> Self {
        match error {
            InterchangeError::SourceRead { path, source } => {
                crate::operations::OperationError::NoteRead { path, source }
            }
            InterchangeError::TasksParse(error) => {
                crate::operations::OperationError::WorkNote(error)
            }
            InterchangeError::Collision(change_id) => {
                crate::operations::OperationError::AlreadyExists(change_id)
            }
            InterchangeError::OverwriteRefused { .. } => {
                crate::operations::OperationError::NoteRead {
                    path: PathBuf::from("<export-refusal>"),
                    source: std::io::Error::other(error.to_string()),
                }
            }
            InterchangeError::ImportRefused { .. } => crate::operations::OperationError::NoteRead {
                path: PathBuf::from("<import-refusal>"),
                source: std::io::Error::other(error.to_string()),
            },
            InterchangeError::InvalidChangeId(_)
            | InterchangeError::NotebookRead { .. }
            | InterchangeError::RequiredNoteAbsent { .. }
            | InterchangeError::LayoutAnomaly { .. }
            | InterchangeError::ListingFailed { .. }
            | InterchangeError::ArchiveFidelity { .. } => {
                crate::operations::OperationError::NoteRead {
                    path: PathBuf::from("<interchange-refusal>"),
                    source: std::io::Error::other(error.to_string()),
                }
            }
        }
    }
}

/// One note write generated by an active-tree import. Imports
/// materialise these via the `nb-api` client during the execute
/// phase; they are also the unit tested by the active-tree
/// mapping tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteWrite {
    /// Notebook-relative folder under `proposals/<change-id>/`,
    /// mirroring the source layout (`specifications/`, `designs/`,
    /// `decisions/`, or empty for root-level notes).
    pub folder: String,
    /// Note title (also the on-disk filename stem, since `nb`
    /// derives filenames from titles).
    pub title: String,
    /// Note content.
    pub content: String,
    /// Whether to write as a `nb` todo note rather than a plain
    /// note (the `work` checklist reconstruction).
    pub is_todo: bool,
}

/// Warning surfaced by the import plan: a delta-spec section that
/// the merge path will refuse to materialise (slice 1 only adds
/// new documents). Surfaced at import time so the operator can
/// intervene before merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeltaWarning {
    /// Notebook-relative note path carrying the offending delta.
    pub note_path: String,
    /// Whether the offending section is `MODIFIED`, `REMOVED`, or
    /// `RENAMED`. All three bind at merge time, not at import time.
    pub kind: DeltaWarningKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeltaWarningKind {
    Modified,
    Removed,
    Renamed,
}

/// Maps an active change tree into the notebook namespace. Reads
/// every artifact the tree carries, normalises the proposal H1 to
/// the schema's selector-stable form, parses `tasks.md` into a
/// `work` checklist, and synthesises a `meta` control-plane note
/// with `migrated: true` provenance.
///
/// Returns the per-note writes, the warnings (MODIFIED/REMOVED
/// deltas the merge path will refuse to materialise), and the
/// synthesized meta content so the caller can attach them to the
/// plan or write them through `nb-api`.
///
/// **v0.3.0-pause**: gated behind `#[cfg(any())]`. The function
/// itself is read-only (it inspects source files and produces
/// writes + delta warnings), but it exists exclusively to feed
/// the `write_active_notes` / `write_todo_note` notebook-mutation
/// path, which is paused in v0.3.0 pending an NbApi 0.3 notebook
/// transaction/checkpoint primitive. The 0.3.0-resume cycle
/// restores this body from source (and git history) and feeds
/// the writes into the staged-then-committed `stage_active_notes`
/// flow.
///
/// # Errors
///
/// Returns [`InterchangeError::SourceRead`] when a required source
/// file is missing or unreadable, [`InterchangeError::TasksParse`]
/// when `tasks.md` is malformed, and [`InterchangeError::Collision`]
/// when the target notebook namespace already exists.
#[cfg(any())]
#[allow(dead_code)]
pub fn import_active_tree(
    tree: &ActiveTree,
    existing_namespaces: &[String],
) -> Result<(Vec<NoteWrite>, Vec<DeltaWarning>, String), InterchangeError> {
    if existing_namespaces.iter().any(|id| id == &tree.change_id) {
        return Err(InterchangeError::Collision(tree.change_id.clone()));
    }
    let proposal_source = read_source_file(&tree.root.join(ACTIVE_PROPOSAL_FILE))?;
    let proposal_content = normalize_proposal_h1(&proposal_source, &tree.change_id);
    let mut writes: Vec<NoteWrite> = Vec::new();
    writes.push(NoteWrite {
        folder: String::new(),
        title: "proposal".to_string(),
        content: proposal_content,
        is_todo: false,
    });

    let mut warnings: Vec<DeltaWarning> = Vec::new();

    let specs_root = tree.root.join(ACTIVE_SPECS_DIR);
    if specs_root.is_dir() {
        let specs = collect_capability_specs(&specs_root)?;
        for (capability, content) in specs {
            warnings.extend(delta_warnings_for_spec(
                &format!("specifications/{capability}.md"),
                &content,
            ));
            writes.push(NoteWrite {
                folder: "specifications".to_string(),
                title: capability.clone(),
                content: strip_leading_h1(&content),
                is_todo: false,
            });
        }
    }

    let design_path = tree.root.join(ACTIVE_DESIGN_FILE);
    if design_path.is_file() {
        let design_content = read_source_file(&design_path)?;
        writes.push(NoteWrite {
            folder: "designs".to_string(),
            title: "main".to_string(),
            content: strip_leading_h1(&design_content),
            is_todo: false,
        });
    }

    let decisions_root = tree.root.join(ACTIVE_DECISIONS_DIR);
    if decisions_root.is_dir() {
        let decisions = collect_decisions(&decisions_root)?;
        for (name, content) in decisions {
            writes.push(NoteWrite {
                folder: "decisions".to_string(),
                title: name.clone(),
                content: strip_leading_h1(&content),
                is_todo: false,
            });
        }
    }

    let tasks_path = tree.root.join(ACTIVE_TASKS_FILE);
    if tasks_path.is_file() {
        let tasks_content = read_source_file(&tasks_path)?;
        let checklist = parse_work_note(&tasks_content)?;
        writes.push(NoteWrite {
            folder: String::new(),
            title: "work".to_string(),
            content: render_work_note_body(&tree.change_id, &checklist),
            is_todo: true,
        });
    }

    let meta_content = render_meta_with_provenance(&tree.change_id, &tree.root)?;
    writes.push(NoteWrite {
        folder: String::new(),
        title: "meta".to_string(),
        content: meta_content.clone(),
        is_todo: false,
    });

    Ok((writes, warnings, meta_content))
}

/// Reads a filesystem source file under an active change tree,
/// surfacing a typed `InterchangeError::SourceRead` on failure.
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
fn read_source_file(path: &Path) -> Result<String, InterchangeError> {
    std::fs::read_to_string(path).map_err(|source| InterchangeError::SourceRead {
        path: path.to_path_buf(),
        source,
    })
}

/// Rewrites the proposal H1 to the schema-stable form
/// `# <change-id>`. The H1 is load-bearing: notebook selectors
/// resolve from it, so a free-form H1 in a classic tree would
/// silently break every selector downstream. When the source H1
/// is already in selector-stable form, the content is returned
/// untouched.
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
fn normalize_proposal_h1(content: &str, change_id: &str) -> String {
    let expected = format!("# {change_id}");
    let mut lines = content.lines();
    match lines.next() {
        Some(first) if first.trim() == expected => content.to_string(),
        _ => {
            let mut output = String::new();
            output.push_str(&expected);
            output.push('\n');
            for line in lines {
                output.push_str(line);
                output.push('\n');
            }
            if !content.ends_with('\n') && !output.ends_with('\n') {
                output.push('\n');
            }
            output
        }
    }
}

/// Reads the source proposal content, returning a path-anchored
/// error on failure.
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
fn render_meta_with_provenance(
    change_id: &str,
    source_root: &Path,
) -> Result<String, InterchangeError> {
    use serde_json::json;
    let source_path = source_root.display().to_string();
    let payload = json!({
        "meta_version": 1,
        "change_id": change_id,
        "title": null,
        "status": "draft",
        "schema": "nbspec-default",
        "notebook": null,
        "created_at": null,
        "updated_at": null,
        "repository_commits": [],
        "migrated": true,
        "source_path": source_path,
    });
    Ok(format!(
        "```json\n{}\n```\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    ))
}

/// Walks the `<tree>/specs/` directory and returns
/// `(capability_name, spec_content)` pairs in capability-name order.
/// Each capability is a subdirectory carrying a single `spec.md`.
/// Returns an empty Vec when the directory holds no capabilities.
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
fn collect_capability_specs(specs_root: &Path) -> Result<Vec<(String, String)>, InterchangeError> {
    let mut capabilities: Vec<(String, String)> = Vec::new();
    let entries = std::fs::read_dir(specs_root).map_err(|source| InterchangeError::SourceRead {
        path: specs_root.to_path_buf(),
        source,
    })?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    for name in names {
        let capability_root = specs_root.join(&name);
        let spec_file = capability_root.join("spec.md");
        if !spec_file.is_file() {
            // A capability without spec.md is silently skipped;
            // the source tree is loose enough that half-authored
            // capabilities are possible, and the operator's
            // intent (an empty spec) is to leave them out.
            continue;
        }
        let content = read_source_file(&spec_file)?;
        capabilities.push((name, content));
    }
    Ok(capabilities)
}

/// Walks the `<tree>/decisions/` directory and returns
/// `(decision_name, content)` pairs in name order. Decision
/// filenames are preserved because they are materialization
/// targets; arbitrary-named files are accepted to prove
/// name-agnosticism per `nbspec:ideas/7`.
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
fn collect_decisions(decisions_root: &Path) -> Result<Vec<(String, String)>, InterchangeError> {
    let mut decisions: Vec<(String, String)> = Vec::new();
    let entries =
        std::fs::read_dir(decisions_root).map_err(|source| InterchangeError::SourceRead {
            path: decisions_root.to_path_buf(),
            source,
        })?;
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        names.push((stem.to_string(), path));
    }
    names.sort();
    for (name, path) in names {
        let content = read_source_file(&path)?;
        decisions.push((name, content));
    }
    Ok(decisions)
}

/// Returns the delta warnings implied by `content`. The merge path
/// only materialises `ADDED` requirements; `MODIFIED` / `REMOVED`
/// / `RENAMED` sections are warned at import time so the operator
/// knows they will fail to materialise downstream.
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
fn delta_warnings_for_spec(note_path: &str, content: &str) -> Vec<DeltaWarning> {
    let delta = parse_delta_specification(content);
    if delta.presence == SectionPresence::default() {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    if delta.presence.modified {
        warnings.push(DeltaWarning {
            note_path: note_path.to_string(),
            kind: DeltaWarningKind::Modified,
        });
    }
    if delta.presence.removed {
        warnings.push(DeltaWarning {
            note_path: note_path.to_string(),
            kind: DeltaWarningKind::Removed,
        });
    }
    if delta.presence.renamed {
        warnings.push(DeltaWarning {
            note_path: note_path.to_string(),
            kind: DeltaWarningKind::Renamed,
        });
    }
    warnings
}

/// Renders a `work` todo note body from a parsed checklist. The
/// title is `# [ ]/x Execution checklist for <change-id>` so the
/// notebook's `display` path recognizes the todo note. Tasks are
/// emitted in their original order with checkbox state preserved.
#[cfg(any())] // v0.3.0-pause: restored by the 0.3.0-resume cycle
#[allow(dead_code)]
fn render_work_note_body(change_id: &str, checklist: &WorkChecklist) -> String {
    let title_marker = if checklist.complete { "[x]" } else { "[ ]" };
    let title = checklist
        .title
        .clone()
        .unwrap_or_else(|| format!("Execution checklist for {change_id}."));
    let mut body = String::new();
    body.push_str(&format!("# {title_marker} {title}\n"));
    for item in &checklist.items {
        let marker = if item.complete { "x" } else { " " };
        body.push_str(&format!("- [{marker}] {}\n", item.text));
    }
    body
}

/// Synthesized archive-conversion provenance: identifies the source
/// tree and records that the archive came from a legacy opsx layout
/// rather than a notebook-resident change. Stored as JSON inside
/// the archive under `<change-id>/meta.json`.
fn render_archive_meta(change_id: &str, source_root: &Path) -> Vec<u8> {
    use serde_json::json;
    let payload = json!({
        "change_id": change_id,
        "migrated": true,
        "source_path": source_root.display().to_string(),
        "kind": "legacy-archive",
    });
    serde_json::to_string_pretty(&payload)
        .unwrap_or_default()
        .into_bytes()
}

/// Walks an archive tree recursively, gathering every regular file
/// as an `ArchiveEntry` rooted at `<change-id>/tree/<relative>`.
///
/// R2 (F2): reads files as bytes (`std::fs::read`), not text.
/// The previous `read_to_string` would refuse on non-UTF-8
/// files; the spec requires the archive to be "tarred as-is"
/// with no fake normalization. Bytes round-trip verbatim
/// regardless of encoding.
///
/// R2'' (F2 re-review 2026-07-25): the walker still used
/// `ReadDir::flatten()` (silently dropping iterator errors) and
/// `into_string().ok()` (silently dropping non-UTF-8 filenames);
/// symlinks were silently skipped via `continue`. A successful
/// deterministic archive build proved only the partial inventory
/// it happened to collect, then archive source deletion could
/// discard omitted entries. The walker now fails closed on every
/// iterator error, non-UTF-8 filename, symlink, and unsupported
/// entry type. The archive conversion (and the source deletion
/// that follows) refuses unless the source inventory is
/// completely enumerable.
fn collect_archive_entries(tree: &ArchiveTree) -> Result<Vec<ArchiveEntry>, InterchangeError> {
    // R2'''' / F2 second re-review 2026-07-25: the deterministic
    // archive layout stores regular files only; empty source
    // directories would not round-trip through the archive. We
    // refuse the archive build at the source instead of producing
    // an archive whose fidelity proof would necessarily diverge.
    refuse_empty_directories(&tree.root)?;
    let mut entries: Vec<ArchiveEntry> = Vec::new();
    walk_archive_tree(&tree.root, tree.root.as_path(), &mut entries)?;
    Ok(entries)
}

/// Walks the archive source tree and refuses on any empty
/// directory. The deterministic archive layout does not preserve
/// directory entries, so an empty source dir would not round-trip
/// through the archive. Operators must populate or remove the
/// directory before importing.
fn refuse_empty_directories(root: &Path) -> Result<(), InterchangeError> {
    walk_for_empty_directories(root, root)
}

fn walk_for_empty_directories(root: &Path, current: &Path) -> Result<(), InterchangeError> {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(InterchangeError::SourceRead {
                path: current.to_path_buf(),
                source: error,
            });
        }
    };
    let mut children: Vec<PathBuf> = Vec::new();
    let mut has_file = false;
    for entry_result in entries {
        let entry = entry_result.map_err(|source| InterchangeError::SourceRead {
            path: current.to_path_buf(),
            source,
        })?;
        let name =
            entry
                .file_name()
                .into_string()
                .map_err(|os_str| InterchangeError::LayoutAnomaly {
                    path: current.to_path_buf(),
                    message: format!("non-UTF-8 archive filename: {os_str:?}"),
                })?;
        let path = current.join(&name);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|source| InterchangeError::SourceRead {
                path: path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message: "symlinks are not part of the deterministic interchange archive; refusing"
                    .to_string(),
            });
        }
        if metadata.is_dir() {
            children.push(path);
        } else if metadata.is_file() {
            has_file = true;
        } else {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message: format!("unsupported archive entry type: {:?}", metadata.file_type()),
            });
        }
    }
    if !has_file && children.is_empty() && current != root {
        return Err(InterchangeError::LayoutAnomaly {
            path: current.to_path_buf(),
            message: "empty directories are not preserved in the deterministic interchange archive; populate or remove before importing".to_string(),
        });
    }
    for child in children {
        walk_for_empty_directories(root, &child)?;
    }
    Ok(())
}

fn walk_archive_tree(
    root: &Path,
    current: &Path,
    entries: &mut Vec<ArchiveEntry>,
) -> Result<(), InterchangeError> {
    let dir_entries =
        std::fs::read_dir(current).map_err(|source| InterchangeError::SourceRead {
            path: current.to_path_buf(),
            source,
        })?;
    let mut names: Vec<String> = Vec::new();
    for entry_result in dir_entries {
        let entry = entry_result.map_err(|source| InterchangeError::SourceRead {
            path: current.to_path_buf(),
            source,
        })?;
        match entry.file_name().into_string() {
            Ok(name) => names.push(name),
            Err(os_str) => {
                return Err(InterchangeError::LayoutAnomaly {
                    path: current.to_path_buf(),
                    message: format!("non-UTF-8 archive filename: {os_str:?}"),
                });
            }
        }
    }
    names.sort();
    for name in names {
        let path = current.join(&name);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|source| InterchangeError::SourceRead {
                path: path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message:
                    "symlinks are not part of the deterministic interchange archive; refusing to silently skip"
                        .to_string(),
            });
        }
        if metadata.is_dir() {
            walk_archive_tree(root, &path, entries)?;
        } else if metadata.is_file() {
            let relative =
                path.strip_prefix(root)
                    .map_err(|error| InterchangeError::SourceRead {
                        path: path.clone(),
                        source: std::io::Error::other(error),
                    })?;
            // Use forward slashes so paths are stable across
            // platforms; tar entries are conventionally posix-style.
            let mut posix = PathBuf::from("tree");
            for component in relative.components() {
                let component_str = component.as_os_str().to_str().ok_or_else(|| {
                    InterchangeError::LayoutAnomaly {
                        path: path.clone(),
                        message: format!(
                            "non-UTF-8 archive path component: {:?}",
                            component.as_os_str()
                        ),
                    }
                })?;
                posix.push(component_str);
            }
            // R2 (F2): bytes, not text. Binary files round-trip
            // verbatim per the spec.
            let content = std::fs::read(&path).map_err(|source| InterchangeError::SourceRead {
                path: path.clone(),
                source,
            })?;
            entries.push(ArchiveEntry {
                path: posix,
                content,
            });
        } else {
            return Err(InterchangeError::LayoutAnomaly {
                path: path.clone(),
                message: format!("unsupported archive entry type: {:?}", metadata.file_type()),
            });
        }
    }
    Ok(())
}

/// Converts an archive tree into deterministic tar.zst bytes,
/// preserving the source layout under `<change-id>/tree/` and
/// appending a synthesized `<change-id>/meta.json` provenance
/// record. The result is byte-identical across runs against
/// unchanged inputs (determinism comes from the archive writer's
/// sorted-entries + zeroed-metadata contract).
///
/// # Errors
///
/// Returns [`InterchangeError::SourceRead`] on filesystem failures
/// during the walk and [`InterchangeError::Archive`] on archive
/// construction failures.
pub fn import_archive_tree(tree: &ArchiveTree) -> Result<Vec<u8>, InterchangeError> {
    let mut entries = collect_archive_entries(tree)?;
    let meta = render_archive_meta(&tree.change_id, &tree.root);
    entries.push(ArchiveEntry {
        path: PathBuf::from("meta.json"),
        content: meta,
    });
    // Move every entry under the `<change-id>/` prefix so the
    // archive root owns the change namespace, matching the
    // merge-time archive layout (one archive per change, named
    // by change id).
    let prefixed: Vec<ArchiveEntry> = entries
        .into_iter()
        .map(|mut entry| {
            let mut new_path = PathBuf::from(&tree.change_id);
            new_path.push(entry.path);
            entry.path = new_path;
            entry
        })
        .collect();
    build_archive(&prefixed).map_err(|error: ArchiveError| match error {
        ArchiveError::Build(source) => InterchangeError::SourceRead {
            path: PathBuf::from("archive"),
            source,
        },
        ArchiveError::Extract(source) => InterchangeError::SourceRead {
            path: PathBuf::from("archive"),
            source,
        },
    })
}
///
/// Detection walks `root` for both kinds of trees in a single pass,
/// skipping anything that fails structural validation (an active
/// tree without `proposal.md`, a folder whose name is not a
/// kebab-case change id, or an `openspec/changes/archive/` entry
/// that is not a directory). The result is sorted by change id so
/// plan output is deterministic across runs.
pub fn detect_trees(root: &Path) -> Vec<DetectedTree> {
    let mut trees: Vec<DetectedTree> = Vec::new();

    if !root.is_dir() {
        return trees;
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return trees,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        // The `openspec/` subdirectory is reserved for archive
        // detection; it is never itself an active tree, even when
        // it accidentally contains a `proposal.md` (e.g. a
        // misconfigured repo whose opsx root is the import root).
        if name == "openspec" {
            trees.extend(detect_archive_trees(&path));
            continue;
        }
        if let Some(active) = detect_active_tree(&path, &name) {
            trees.push(DetectedTree::Active(active));
        }
    }

    trees.sort_by(|left, right| {
        let left_id = match left {
            DetectedTree::Active(tree) => &tree.change_id,
            DetectedTree::Archive(tree) => &tree.change_id,
        };
        let right_id = match right {
            DetectedTree::Active(tree) => &tree.change_id,
            DetectedTree::Archive(tree) => &tree.change_id,
        };
        left_id.cmp(right_id)
    });
    trees
}

/// Walks `<root>/openspec/changes/archive/` for archive tree
/// candidates. Each immediate subdirectory is taken as an archive
/// tree; non-directory entries and entries with non-kebab-case
/// names are silently skipped. Returns detections in change-id
/// order.
fn detect_archive_trees(openspec_root: &Path) -> Vec<DetectedTree> {
    let archive_root = openspec_root.join(LEGACY_ARCHIVE_SUBDIR);
    let mut trees: Vec<DetectedTree> = Vec::new();
    let entries = match std::fs::read_dir(&archive_root) {
        Ok(entries) => entries,
        Err(_) => return trees,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        if validate_change_id(&name).is_err() {
            continue;
        }
        trees.push(DetectedTree::Archive(ArchiveTree {
            root: path,
            change_id: name,
        }));
    }
    trees
}

/// Detects an active change tree at `<root>/<change-id>/`.
/// Returns `None` when the folder name is not a valid kebab-case
/// change id or the folder does not carry `proposal.md`. Detection
/// is purely structural; parse failures inside `tasks.md` or
/// capability files are surfaced as plan refusals by the import
/// path (see tasks 3.x), not here.
fn detect_active_tree(folder: &Path, name: &str) -> Option<ActiveTree> {
    if validate_change_id(name).is_err() {
        return None;
    }
    let proposal = folder.join(ACTIVE_PROPOSAL_FILE);
    if !proposal.is_file() {
        return None;
    }
    Some(ActiveTree {
        root: folder.to_path_buf(),
        change_id: name.to_string(),
    })
}

use serde_json::json;
