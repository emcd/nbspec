use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::changes::validate_change_id;
use crate::interchange::archive::archive_fidelity_proof;
use crate::interchange::archive::{import_archive_tree, write_archive_file};
use crate::interchange::detect::{
    ArchiveTree, DetectedTree, detect_trees, quarantine_path_for, rename_no_replace,
};
use crate::interchange::export::{build_export_plan, execute_export_plan};
use crate::interchange::plan::{
    ImportOptions, InterchangeError, InterchangePlan, InterchangePlanStructured, PlanEntry,
    RoundTripProof, render_plan_text, resolve_notebook,
};
use crate::interchange::proof::round_trip_proof;

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

/// Builds the import plan from a filesystem tree. Detects trees,
/// classifies each as Active / Archive / Skip / Refusal, and emits
/// a unified plan. All refusals are surfaced before any write.
pub async fn build_import_plan(
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
pub async fn list_existing_change_ids(
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
pub fn parse_change_id_listing(
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
pub async fn execute_import_plan(
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
pub async fn run_round_trip(
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
        &crate::interchange::plan::ExportOptions {
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
