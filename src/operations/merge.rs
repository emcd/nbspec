use std::path::{Path, PathBuf};

use nb_api::NbClient;
use serde_json::{Value, json};

use crate::archives::{ArchiveEntry, build_archive, gitattributes_covers_lfs};
use crate::changes::{META_NOTE, WORK_NOTE};
use crate::merging::merge_documents;
use crate::rendering::aggregate_content_hash;
use crate::reviews::{
    MERGE_GATE, VerdictError, evaluate_gate, gate_refusal_state, read_verdicts, reviewer_positions,
};

use super::context::load_change_context;
use super::helpers::read_work_note;
use super::{OperationError, OperationOutcome, OperationResult};

/// Transfers a change's durable artifacts into the repository.
///
/// Renders the change from its notes and writes the target-bearing
/// documents to their configured repository destinations with
/// provenance headers. `ADDED`-only notes write whole; notes carrying
/// delta operations apply surgically (see `merging`). Planning
/// collects every violation before any write, so a refused merge
/// modifies nothing; `force` overrides target-state refusals (drift,
/// unmanaged files, foreign ownership) but never delta incoherence,
/// dangling names, or non-file occupants.
/// This is the only nbspec operation that writes to the repository,
/// and it creates no git commits. Archive writing happens after the
/// documents transfer: an archive IO failure therefore leaves
/// already-merged documents in place — an accepted trade-off, since
/// rerunning merge is idempotent and completes the archive.
///
/// # Errors
///
/// Returns [`OperationError::ChangeNotFound`] when the change
/// namespace is absent, [`MergeError::Refused`] (wrapped) listing
/// every violating target, and notebook, configuration, schema, or
/// IO errors otherwise.
pub async fn merge(
    client: &NbClient,
    notebook: Option<&str>,
    change_id: &str,
    force: bool,
) -> OperationResult {
    let context = load_change_context(client, notebook, change_id).await?;
    let aggregate = aggregate_content_hash(&context.documents);
    let review_gate_state = match read_verdicts(&context.change_directory) {
        Ok(verdicts) => {
            let positions = reviewer_positions(&verdicts, MERGE_GATE, &aggregate);
            gate_refusal_state(&evaluate_gate(&positions), &aggregate)
        }
        // An unparseable verdict is a plan-phase POLICY refusal
        // (force-overridable), not a hard error: it blocks the gate
        // while naming the note, but never hides behind an abort.
        Err(VerdictError::Malformed { note, reason }) => {
            Some(format!("verdict unparseable: {note}: {reason}"))
        }
        Err(error @ VerdictError::Io { .. }) => return Err(error.into()),
    };
    let report = merge_documents(
        &context.documents,
        &context.root,
        change_id,
        &context.notebook_name,
        review_gate_state.as_deref(),
        force,
    )?;

    let mut output = String::new();
    if let Some(state) = &report.review_gate_overridden {
        output.push_str(&format!("REVIEW GATE OVERRIDDEN (--force): {state}\n"));
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
    for warning in &report.warnings {
        output.push_str(&format!("warning: {warning}\n"));
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
    let archived_path = if context.configuration.archives {
        let archive_output = write_change_archive(
            &context.configuration,
            &context.root,
            &context.change_directory,
            change_id,
            &context.documents,
        )?;
        output.push_str(&archive_output);
        // Parse the "archived <path>" line for structured reporting;
        // warnings are kept in text only.
        archive_output
            .lines()
            .find_map(|line| line.strip_prefix("archived ").map(|rest| rest.to_string()))
    } else {
        None
    };
    output.push_str(&format!(
        "Merged change {change_id}: {written} written, {unchanged} unchanged.",
        written = report.written.len(),
        unchanged = report.unchanged.len(),
    ));
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
    let structured = json!({
        "change_id": change_id,
        "written": report.written,
        "unchanged": report.unchanged,
        "archived": archived_path,
        "review_gate_overridden": report.review_gate_overridden,
        "successions": successions,
        "drift_overrides": drift_overrides,
        "stale_target_overrides": report.stale_target_overrides,
        "warnings": report.warnings,
    });
    Ok(OperationOutcome::new(output, structured))
}

/// Writes the merge-time change archive: the rendered artifact tree
/// plus `meta.md` and a `work.md` checklist snapshot, packed
/// deterministically under a top-level `<change-id>/` prefix.
/// Returns report lines, including a warning when no `.gitattributes`
/// rule marks the archive path for Git LFS.
fn write_change_archive(
    configuration: &crate::configuration::Configuration,
    root: &std::path::Path,
    change_directory: &std::path::Path,
    change_id: &str,
    documents: &[crate::rendering::RenderedDocument],
) -> Result<String, OperationError> {
    use crate::reviews::VERDICTS_FOLDER;
    let prefix = PathBuf::from(change_id);
    let mut entries: Vec<ArchiveEntry> = documents
        .iter()
        .map(|document| ArchiveEntry {
            path: prefix.join(Path::new(&document.tree_path)),
            content: document.content.clone().into_bytes(),
        })
        .collect();
    let meta_path = change_directory.join(format!("{META_NOTE}.md"));
    let meta_content = std::fs::read(&meta_path).map_err(|source| OperationError::NoteRead {
        path: meta_path,
        source,
    })?;
    entries.push(ArchiveEntry {
        path: prefix.join(format!("{META_NOTE}.md")),
        content: meta_content,
    });
    if let Some(work_content) = read_work_note(change_directory) {
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
    let verdicts_directory = change_directory.join(VERDICTS_FOLDER);
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
            let content = std::fs::read(&path).map_err(|source| OperationError::NoteRead {
                path: path.clone(),
                source,
            })?;
            entries.push(ArchiveEntry {
                path: prefix.join(VERDICTS_FOLDER).join(&name),
                content,
            });
        }
    }
    let bytes = build_archive(&entries)?;

    let archive_path = configuration
        .archive_directory
        .join(format!("{change_id}.tar.zst"));
    let absolute = root.join(&archive_path);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).map_err(|source| OperationError::ArchiveWrite {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&absolute, &bytes).map_err(|source| OperationError::ArchiveWrite {
        path: absolute.clone(),
        source,
    })?;

    let mut output = format!("archived {}\n", archive_path.display());
    if !gitattributes_covers_lfs(root, &archive_path) {
        output.push_str(&format!(
            "warning: no .gitattributes rule marks {} for Git LFS\n",
            archive_path.display()
        ));
    }
    Ok(output)
}
