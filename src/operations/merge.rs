use std::path::{Path, PathBuf};

use nb_api::{NbClient, NoteTarget};
use serde_json::{Value, json};

use crate::archives::{ArchiveEntry, build_archive, gitattributes_covers_lfs};
use crate::changes::{
    ChangeStatus, META_NOTE, WORK_NOTE, change_folder, validate_change_id,
};
use crate::configuration::load_configuration;
use crate::interchange::archive::{
    commit_archive_staging, stage_archive_file,
};
use crate::merging::merge_documents;
use crate::rendering::aggregate_content_hash;
use crate::retirement::{
    ArchiveOccupant, NamespaceSnapshot, capture_notebook_head,
    capture_snapshot, namespace_entries, read_archive_occupant,
    refuse_invalid_occupant, validate_archive_identity,
    verify_persisted_archive, verify_snapshot,
};
use crate::reviews::{
    MERGE_GATE, VERDICTS_FOLDER, VerdictError, evaluate_gate,
    gate_refusal_state, read_verdicts, reviewer_positions,
};

use super::context::{
    ChangeContext, load_change_context, project_root, read_metadata,
    resolve_notebook_name,
};
use super::helpers::read_work_note;
use super::{OperationError, OperationOutcome, OperationResult};

/// Transfers a change's durable artifacts into the repository.
///
/// Renders the change from its notes and writes the target-bearing
/// documents to their configured repository destinations with
/// provenance headers. Notes carrying delta operations apply
/// surgically against their targets (see `merging`); notes without
/// delta sections write whole. Planning collects every violation
/// before any write, so a refused merge modifies nothing; `force`
/// overrides target-state refusals (drift, unmanaged files, foreign
/// ownership) but never delta incoherence, dangling names, or
/// non-file occupants. `force` adopts an unmanaged surgical base
/// only when it holds addressable requirement blocks. `ADDED`
/// collisions against drifted text resolve delta-wins under
/// `--force`; hash-valid collisions refuse regardless of `--force`.
/// This is the only nbspec operation that writes to the repository,
/// and it creates no git commits.
///
/// With archiving enabled, a successful merge ends retired: the
/// change archive is published atomically and verified, then the
/// source namespace is deleted in one snapshot-bound transaction
/// (see `retirement`). With archiving disabled, merge materializes
/// and warns loudly, leaving the namespace in place as the only
/// record.
///
/// # Errors
///
/// Returns [`OperationError::ChangeNotFound`] when the change
/// namespace is absent and no tarball claims the id,
/// [`OperationError::Merge`] refusal listing every violating
/// target, retirement refusals and failures per the namespace
/// retirement contract, and notebook, configuration, schema, or IO
/// errors otherwise.
pub async fn merge(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
    force: bool,
    strict: bool,
) -> OperationResult {
    validate_change_id(change_id)?;
    let notebook_name = resolve_notebook_name(notebook)?;
    let root = project_root();
    let configuration = load_configuration(&root)?;
    let folder = change_folder(change_id);
    let notebook_path = client
        .show_notebook_path(Some(notebook_name.as_str()))
        .await?;
    let change_directory = notebook_path.join(&folder);

    if !configuration.archives {
        let context = load_change_context(client, notebook, change_id).await?;
        refuse_dead_status(read_metadata(&context.change_directory)?.status)?;
        let (mut output, mut structured) =
            materialize(&context, change_id, force, strict, None)?;
        let retention_warning = "warning: archives disabled; the namespace is left in place as \
            the only record; retirement skipped\n";
        output.push_str(retention_warning);
        if let Some(warnings) =
            structured.get_mut("warnings").and_then(Value::as_array_mut)
        {
            warnings.push(json!(retention_warning.trim()));
        }
        structured["archived"] = Value::Null;
        return Ok(OperationOutcome::new(output, structured));
    }

    let tarball_relative = configuration
        .archive_directory
        .join(format!("{change_id}.tar.zst"));
    let tarball_path = root.join(&tarball_relative);
    let tarball_bytes = match read_archive_occupant(&tarball_path)? {
        ArchiveOccupant::Absent => None,
        ArchiveOccupant::File(bytes) => Some(bytes),
        ArchiveOccupant::Invalid(detail) => {
            return Err(refuse_invalid_occupant(&tarball_relative, &detail));
        },
    };

    if !change_directory.is_dir() {
        return match tarball_bytes {
            None => Err(OperationError::ChangeNotFound {
                notebook: notebook_name,
                change_id: change_id.to_string(),
            }),
            Some(bytes) => {
                match validate_archive_identity(&bytes, change_id) {
                    Ok(()) => Ok(already_merged(change_id, &tarball_relative)),
                    Err(error) => Err(error),
                }
            },
        };
    }
    // Dead changes never merge: the status gate precedes every
    // repository effect, the review gate, and any deletion. Force
    // does not bypass it.
    refuse_dead_status(read_metadata(&change_directory)?.status)?;
    if let Some(bytes) = tarball_bytes.as_ref() {
        validate_archive_identity(bytes, change_id)?;
    }

    // The source snapshot binds everything downstream: gate inputs,
    // rendered documents, archive inventory, and the deletion
    // precondition all refer to the state captured here, before the
    // review gate runs.
    let expected_head = capture_notebook_head(&notebook_path)?;
    let snapshot = capture_snapshot(&change_directory)?;

    let context = load_change_context(client, notebook, change_id).await?;
    // Render inputs are bound here: anything that changed after the
    // snapshot refuses before the gate or any repository effect.
    verify_snapshot(&change_directory, &snapshot).map_err(|error| match error {
        OperationError::RetirementRefused { reason, .. } => OperationError::RetirementRefused {
            reason: format!("{reason} (before review gate)"),
            guidance: "Nothing was written, published, moved, or deleted; resolve the \
                    drift and re-run merge."
                .to_string(),
        },
        other => other,
    })?;
    let rebuilt = build_full_archive_bytes(&context, change_id, &snapshot)?;

    if let Some(existing) = tarball_bytes.as_ref() {
        if *existing == rebuilt {
            // Clean resume: the tarball already proves materialization
            // and the archive, so skip both and go to snapshot-bound
            // deletion. Byte-identity bridges the snapshot across
            // invocations; the fresh snapshot below binds this run.
            verify_snapshot(&change_directory, &snapshot)?;
            let retired = retire_namespace(
                client,
                &notebook_name,
                &folder,
                &expected_head,
                &tarball_relative,
            )
            .await?;
            let text = format!(
                "Change {change_id} already archived; {retired}",
                retired = retired.trim_end(),
            );
            return Ok(OperationOutcome::new(
                text,
                json!({
                    "change_id": change_id,
                    "already_merged": true,
                    "retired": true,
                    "archived": tarball_relative.display().to_string(),
                    "tarball": tarball_relative.display().to_string(),
                    "written": [],
                    "unchanged": [],
                    "warnings": [],
                    "diagnostics": [],
                }),
            ));
        }
        return Err(OperationError::RetirementRefused {
            reason: format!(
                "live namespace diverges from the published tarball {}",
                tarball_path.display()
            ),
            guidance: "Inspect the shipped history; this merge will not overwrite it, \
                publish over it, or delete the namespace."
                .to_string(),
        });
    }

    let (mut output, mut structured) =
        materialize(&context, change_id, force, strict, Some(&snapshot))?;

    // Private test seam for the deterministic post-materialization
    // drift regression. Compiled ONLY with the non-default
    // `test-support` feature, so default builds contain no
    // environment-activated mutator. When present the seam names a
    // namespace-relative file to append drift to after
    // materialization and before publication; paths are confined to
    // the namespace (no absolute paths, no parent escapes) and IO
    // failures refuse loudly rather than being ignored.
    #[cfg(feature = "test-support")]
    {
        use std::path::Component;
        if let Ok(marker) =
            std::env::var("NBSPEC_TEST_DRIFT_AFTER_MATERIALIZE")
        {
            let relative = Path::new(&marker);
            if relative.is_absolute()
                || relative.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir
                    )
                })
            {
                return Err(OperationError::RetirementFailed {
                    phases: "materialization complete; archive not published"
                        .to_string(),
                    guidance: format!(
                        "test seam path escapes the namespace: {marker}; refusing"
                    ),
                });
            }
            let drift_path = change_directory.join(relative);
            if let Some(parent) = drift_path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| {
                    OperationError::RetirementFailed {
                        phases: "materialization complete; archive not published".to_string(),
                        guidance: format!(
                            "test seam setup failed ({source}); re-run merge without the seam"
                        ),
                    }
                })?;
            }
            let mut existing = match std::fs::read(&drift_path) {
                Ok(bytes) => bytes,
                Err(source)
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    Vec::new()
                },
                Err(source) => {
                    return Err(OperationError::RetirementFailed {
                        phases:
                            "materialization complete; archive not published"
                                .to_string(),
                        guidance: format!(
                            "test seam setup failed ({source}); re-run merge without the seam"
                        ),
                    });
                },
            };
            existing.extend_from_slice(b"\nUnarchived test drift.\n");
            std::fs::write(&drift_path, existing).map_err(|source| {
                OperationError::RetirementFailed {
                    phases: "materialization complete; archive not published".to_string(),
                    guidance: format!(
                        "test seam setup failed ({source}); re-run merge without the seam"
                    ),
                }
            })?;
        }
    }

    // Post-materialization barrier: anything that changed after the
    // snapshot -- including during materialization itself -- refuses
    // here with completed-phase context. Repository outputs may
    // already exist; no archive is published, nothing is deleted,
    // retirement is not reported. ALL verification errors take this
    // path: an unreadable namespace is as unretireable as a drifted
    // one.
    if let Err(error) = verify_snapshot(&change_directory, &snapshot) {
        let detail = error.to_string();
        return Err(OperationError::RetirementFailed {
            phases: "materialization complete; archive not published"
                .to_string(),
            guidance: format!(
                "{detail}; repository outputs may already exist; resolve the drift and re-run \
                 merge"
            ),
        });
    }

    publish_archive(change_id, &tarball_path, &rebuilt, &snapshot)?;
    output.push_str(&format!("archived {}\n", tarball_relative.display()));
    if !gitattributes_covers_lfs(&root, &tarball_relative) {
        output.push_str(&format!(
            "warning: no .gitattributes rule marks {} for Git LFS\n",
            tarball_relative.display()
        ));
    }
    let retired = retire_namespace(
        client,
        &notebook_name,
        &folder,
        &expected_head,
        &tarball_relative,
    )
    .await?;
    output.push_str(&retired);
    output.push_str(&format!(
        "Merged change {change_id} and retired its namespace.",
    ));
    structured["retired"] = json!(true);
    structured["already_merged"] = json!(false);
    structured["archived"] = json!(tarball_relative.display().to_string());
    structured["tarball"] = json!(tarball_relative.display().to_string());
    Ok(OperationOutcome::new(output, structured))
}

/// Renders the review gate, applies deltas, and formats the shared
/// materialization report. Used by every merge path that reaches the
/// repository. The snapshot binds the gate inputs: verdicts are read
/// only after the live namespace still matches it.
fn materialize(
    context: &ChangeContext,
    change_id: &str,
    force: bool,
    strict: bool,
    snapshot: Option<&NamespaceSnapshot>,
) -> Result<(String, Value), OperationError> {
    if let Some(snapshot) = snapshot {
        verify_snapshot(&context.change_directory, snapshot).map_err(|error| match error {
            OperationError::RetirementRefused { reason, .. } => OperationError::RetirementRefused {
                reason: format!("{reason} (before review gate)"),
                guidance: "Nothing was written, published, moved, or deleted; resolve the \
                        drift and re-run merge."
                    .to_string(),
            },
            other => other,
        })?;
    }
    let aggregate = aggregate_content_hash(&context.documents);
    let review_gate_state = match read_verdicts(&context.change_directory) {
        Ok(verdicts) => {
            let positions =
                reviewer_positions(&verdicts, MERGE_GATE, &aggregate);
            gate_refusal_state(&evaluate_gate(&positions), &aggregate)
        },
        // An unparseable verdict is a plan-phase POLICY refusal
        // (force-overridable), not a hard error: it blocks the gate
        // while naming the note, but never hides behind an abort.
        Err(VerdictError::Malformed { note, reason }) => {
            Some(format!("verdict unparseable: {note}: {reason}"))
        },
        Err(error @ VerdictError::Io { .. }) => return Err(error.into()),
    };
    let report = merge_documents(
        &context.documents,
        &context.root,
        change_id,
        &context.notebook_name,
        review_gate_state.as_deref(),
        force,
        strict,
    )?;

    let mut output = String::new();
    if let Some(state) = &report.review_gate_overridden {
        output
            .push_str(&format!("REVIEW GATE OVERRIDDEN (--force): {state}\n"));
    }
    for takeover in &report.drift_overrides {
        output.push_str(&format!(
            "DRIFTED TARGET OVERRIDDEN (--force): {target} (was owned by {previous})\n",
            target = takeover.target,
            previous = takeover.previous_owner,
        ));
    }
    for path in &report.stale_target_overrides {
        output.push_str(&format!(
            "STALE TARGET (--force): {path} left behind by H1-slug rename; remove manually\n"
        ));
    }
    for succession in &report.successions {
        output.push_str(&format!(
            "TAKEOVER (clean succession): {target}: {previous} -> {change_id}\n",
            target = succession.target,
            previous = succession.previous_owner,
        ));
    }
    for diagnostic in &report.diagnostics {
        let mut identity = String::new();
        if let Some(target) = &diagnostic.target {
            identity.push_str(&format!(" target={target}"));
        }
        if let Some(requirement) = &diagnostic.requirement {
            identity.push_str(&format!(" requirement=\"{requirement}\""));
        }
        if let Some(scenario) = &diagnostic.scenario {
            identity.push_str(&format!(" scenario=\"{scenario}\""));
        }
        output.push_str(&format!(
            "warning: [{}]{identity} {}\n",
            diagnostic.code, diagnostic.message
        ));
    }
    for path in &report.written {
        output.push_str(&format!("wrote {path}\n"));
    }
    for path in &report.unchanged {
        output.push_str(&format!("unchanged {path}\n"));
    }
    if report.written.is_empty() && report.unchanged.is_empty() {
        output.push_str("no durable documents to merge\n");
    }
    let successions: Vec<Value> = report
        .successions
        .iter()
        .map(|succession| {
            json!({
                "target": succession.target,
                "previous_owner": succession.previous_owner,
            })
        })
        .collect();
    let drift_overrides: Vec<Value> = report
        .drift_overrides
        .iter()
        .map(|takeover| {
            json!({
                "target": takeover.target,
                "previous_owner": takeover.previous_owner,
            })
        })
        .collect();
    let diagnostics: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "message": diagnostic.message,
                "target": diagnostic.target,
                "requirement": diagnostic.requirement,
                "scenario": diagnostic.scenario,
            })
        })
        .collect();
    let structured = json!({
        "change_id": change_id,
        "written": report.written,
        "unchanged": report.unchanged,
        "review_gate_overridden": report.review_gate_overridden,
        "successions": successions,
        "drift_overrides": drift_overrides,
        "stale_target_overrides": report.stale_target_overrides,
        "warnings": report.warnings,
        "diagnostics": diagnostics,
        "retired": false,
        "already_merged": false,
        "archived": Value::Null,
        "tarball": Value::Null,
    });
    Ok((output, structured))
}

/// Builds the complete deterministic archive bytes: the rendered
/// artifact tree plus `meta.md`, the work snapshot, explicit
/// verdicts, and the byte-faithful `namespace/` copy of the live
/// namespace. Determinism (sorted entries, normalized headers)
/// makes byte-identity a valid already-published proof.
fn build_full_archive_bytes(
    context: &ChangeContext,
    change_id: &str,
    snapshot: &NamespaceSnapshot,
) -> Result<Vec<u8>, OperationError> {
    let prefix = PathBuf::from(change_id);
    let mut entries: Vec<ArchiveEntry> = context
        .documents
        .iter()
        .map(|document| ArchiveEntry {
            path: prefix.join(Path::new(&document.tree_path)),
            content: document.content.clone().into_bytes(),
        })
        .collect();
    let meta_path = context.change_directory.join(format!("{META_NOTE}.md"));
    let meta_content = std::fs::read(&meta_path).map_err(|source| {
        OperationError::NoteRead {
            path: meta_path,
            source,
        }
    })?;
    entries.push(ArchiveEntry {
        path: prefix.join(format!("{META_NOTE}.md")),
        content: meta_content,
    });
    if let Some(work_content) = read_work_note(&context.change_directory) {
        entries.push(ArchiveEntry {
            path: prefix.join(format!("{WORK_NOTE}.md")),
            content: work_content.into_bytes(),
        });
    }
    // Verdict notes ride the archive EXPLICITLY: nothing from the
    // change namespace is included automatically, and the review
    // trail must survive the change. Files are copied raw — the
    // archive preserves even a malformed verdict rather than
    // validating it away. (build_archive sorts entries by path, so
    // determinism holds regardless of push order.)
    let verdicts_directory = context.change_directory.join(VERDICTS_FOLDER);
    if verdicts_directory.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&verdicts_directory)
            .map_err(|source| OperationError::NoteRead {
                path: verdicts_directory.clone(),
                source,
            })?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.') && name.ends_with(".md"))
            .collect();
        names.sort();
        for name in names {
            let path = verdicts_directory.join(&name);
            let content = std::fs::read(&path).map_err(|source| {
                OperationError::NoteRead {
                    path: path.clone(),
                    source,
                }
            })?;
            entries.push(ArchiveEntry {
                path: prefix.join(VERDICTS_FOLDER).join(&name),
                content,
            });
        }
    }
    entries.extend(namespace_entries(change_id, snapshot));
    // Namespace entries ride snapshot bytes directly; liveness of
    // every other input is enforced by the post-materialization
    // barrier before publication, which carries the completed-phase
    // context this pre-materialization point cannot claim.
    Ok(build_archive(&entries)?)
}

/// Publishes the archive atomically: stages to a temporary path,
/// verifies the persisted bytes against the snapshot inventory, then
/// renames into place with no-clobber semantics. A byte-identical
/// pre-existing tarball is kept without rewrite; a differing one
/// refuses — shipped history is never silently overwritten. (The
/// merge flow only reaches publication when no tarball was present
/// at matrix time, so any pre-existing file here raced in and the
/// no-clobber rename is the backstop.)
fn publish_archive(
    change_id: &str,
    tarball_path: &Path,
    bytes: &[u8],
    snapshot: &NamespaceSnapshot,
) -> Result<(), OperationError> {
    match read_archive_occupant(tarball_path)? {
        ArchiveOccupant::Absent => {},
        ArchiveOccupant::File(existing) if existing == *bytes => return Ok(()),
        ArchiveOccupant::File(_) => {
            return Err(OperationError::RetirementRefused {
                reason: format!(
                    "pre-existing tarball {} differs from the rebuilt archive",
                    tarball_path.display()
                ),
                guidance: "Inspect the shipped history; this merge will not overwrite it, \
                    publish over it, or delete the namespace."
                    .to_string(),
            });
        },
        ArchiveOccupant::Invalid(detail) => {
            return Err(refuse_invalid_occupant(tarball_path, &detail));
        },
    }
    let staging = stage_archive_file(change_id, tarball_path, bytes)?;
    verify_persisted_archive(&staging.staging_file, change_id, snapshot)?;
    commit_archive_staging(staging)
}

/// Deletes the change namespace in one snapshot-bound transaction:
/// the original pre-gate HEAD is asserted before anything is
/// materialized, then the whole folder goes recursively. Clean
/// failures leave the namespace wholly in place (safe to resume);
/// indeterminate or recovery-required outcomes report
/// inspection-required with no rollback claim.
async fn retire_namespace(
    client: &NbClient,
    notebook_name: &str,
    folder: &str,
    expected_head: &str,
    tarball_path: &Path,
) -> Result<String, OperationError> {
    let phases = format!(
        "materialization complete; tarball published at {}",
        tarball_path.display()
    );
    let mut transaction = client
        .transaction(Some(notebook_name))
        .await
        .map_err(|source| OperationError::RetirementFailed {
            phases: phases.clone(),
            guidance: format!(
                "retirement not attempted ({source}); re-run merge to resume"
            ),
        })?;
    transaction
        .assert_revision(expected_head)
        .map_err(|source| OperationError::RetirementFailed {
            phases: phases.clone(),
            guidance: format!(
                "retirement precondition rejected ({source}); namespace untouched; re-run merge \
                 to resume"
            ),
        })?;
    transaction
        .delete_folder(NoteTarget::path(folder), true)
        .map_err(|source| OperationError::RetirementFailed {
            phases: phases.clone(),
            guidance: format!(
                "retirement not attempted ({source}); namespace untouched; re-run merge to resume"
            ),
        })?;
    let outcome = transaction.commit().await;
    match crate::retirement::classify_commit_outcome(
        outcome.map(|_| ()),
        &phases,
    ) {
        Ok(()) => Ok(format!("retired {folder}/\n")),
        Err(error) => Err(error),
    }
}

/// Refuses terminal changes: superseded and abandoned changes are
/// dead and never merge, under force or otherwise.
fn refuse_dead_status(status: ChangeStatus) -> Result<(), OperationError> {
    match status {
        ChangeStatus::Superseded | ChangeStatus::Abandoned => {
            Err(OperationError::RetirementRefused {
                reason: format!(
                    "change is {status}; dead changes do not merge"
                ),
                guidance:
                    "A superseded or abandoned change cannot be merged; \
                    create a successor change instead."
                        .to_string(),
            })
        },
        _ => Ok(()),
    }
}

/// Reports an already-retired change: namespace absent, validated
/// tarball present. No writes, no moves, no deletions.
fn already_merged(
    change_id: &str,
    tarball_relative: &Path,
) -> OperationOutcome {
    OperationOutcome::new(
        format!(
            "Change {change_id} already merged (retired; tarball {tarball}).",
            tarball = tarball_relative.display()
        ),
        json!({
            "change_id": change_id,
            "already_merged": true,
            "retired": true,
            "archived": tarball_relative.display().to_string(),
            "tarball": tarball_relative.display().to_string(),
            "written": [],
            "unchanged": [],
            "warnings": [],
            "diagnostics": [],
        }),
    )
}
