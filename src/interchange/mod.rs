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

pub mod active;
pub mod archive;
pub mod detect;
pub mod export;
pub mod import;
pub mod plan;
pub mod proof;
pub mod staging;

// Re-export the public surface so `crate::interchange::PlanEntry` etc
// continue to work.
pub use active::run_round_trip;
pub use archive::{ArchiveStaging, archive_fidelity_proof};
pub use archive::{collect_archive_entries, import_archive_tree, walk_archive_tree};
pub use detect::{
    ACTIVE_DECISIONS_DIR, ACTIVE_DESIGN_FILE, ACTIVE_PROPOSAL_FILE, ACTIVE_SPECS_DIR,
    ACTIVE_TASKS_FILE, LEGACY_ARCHIVE_SUBDIR, QUARANTINE_COUNTER, STAGING_COUNTER,
};
pub use detect::{ActiveTree, ArchiveTree, DetectedTree, detect_trees};
pub use detect::{create_dir_exclusive, quarantine_path_for, rename_no_replace};
pub use export::{build_export_plan, execute_export_plan, export, render_export_plan_text};
pub use export::{read_notebook_note, strip_title_h1};
pub use import::{build_import_plan, execute_import_plan, import};
pub use plan::{
    DeltaWarning, DeltaWarningKind, ExportEntry, ExportOptions, ExportPlan, ImportOptions,
    InterchangeError, InterchangePlan, InterchangePlanStructured, NoteWrite, PlanEntry,
    RoundTripNormalization, RoundTripProof, render_plan_text, resolve_notebook,
};
pub use proof::{
    canonical_bytes_for_proof, canonical_decision_for_proof, canonical_for_proof,
    canonical_proposal_for_proof, canonical_spec_for_proof, canonical_tasks_for_proof,
    collect_files_for_proof, round_trip_proof, strip_leading_h1, walk_for_proof,
};
pub use staging::ActiveStaging;
#[cfg(any())]
pub use staging::{
    commit_active_staging, ensure_folder, stage_active_notes, write_active_notes, write_todo_note,
};

// `import_active_tree` is gated behind `#[cfg(any())]` (v0.3.0-pause)
// so it is only available when explicitly enabled.
#[cfg(any())]
pub use staging::import_active_tree;
