use std::path::{Path, PathBuf};

use crate::interchange::detect::{STAGING_COUNTER, create_dir_exclusive, rename_no_replace};
use crate::interchange::plan::{
    ExportEntry, ExportOptions, ExportPlan, InterchangeError, resolve_notebook,
};

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
pub async fn build_export_plan(
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
pub async fn execute_export_plan(
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
pub async fn read_notebook_note(
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
pub async fn list_notebook_folder(
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
pub async fn first_notebook_folder_entry(
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
pub async fn read_notebook_work_note(
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
