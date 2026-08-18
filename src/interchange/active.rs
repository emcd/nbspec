use std::path::{Path, PathBuf};

use crate::interchange::export::{build_export_plan, execute_export_plan};
use crate::interchange::plan::RoundTripProof;
use crate::interchange::proof::round_trip_proof;
use crate::worknotes::parse_work_note;

pub async fn execute_active_via_transaction(
    client: &nb_api::NbClient,
    notebook_name: &str,
    change_id: &str,
    source_path: &Path,
) -> Result<(), crate::operations::OperationError> {
    let tx = build_active_transaction(client, notebook_name, change_id, source_path).await?;
    let _outcome = tx.commit().await.map_err(|e| match &e {
        nb_api::NbError::DirtyBaseline { .. }
        | nb_api::NbError::PathCollision { .. }
        | nb_api::NbError::PathIgnored { .. }
        | nb_api::NbError::UnsupportedStructure { .. }
        | nb_api::NbError::DuplicateTitleHeading { .. } => {
            crate::operations::OperationError::from(e)
        }
        _ => crate::operations::OperationError::from(e),
    })?;
    Ok(())
}

async fn build_active_transaction(
    client: &nb_api::NbClient,
    notebook_name: &str,
    change_id: &str,
    source_path: &Path,
) -> Result<nb_api::Transaction, crate::operations::OperationError> {
    use crate::interchange::detect::{
        ACTIVE_DECISIONS_DIR, ACTIVE_DESIGN_FILE, ACTIVE_PROPOSAL_FILE, ACTIVE_SPECS_DIR,
        ACTIVE_TASKS_FILE,
    };
    use nb_api::NoteTarget;

    let mut tx = client
        .transaction(Some(notebook_name))
        .await
        .map_err(crate::operations::OperationError::from)?;

    // proposal.md
    let proposal_path = source_path.join(ACTIVE_PROPOSAL_FILE);
    let proposal_raw = std::fs::read_to_string(&proposal_path).map_err(|source| {
        crate::operations::OperationError::from(
            crate::interchange::plan::InterchangeError::SourceRead {
                path: proposal_path.clone(),
                source,
            },
        )
    })?;
    let proposal_content = {
        let expected = format!("# {change_id}");
        let mut lines = proposal_raw.lines();
        match lines.next() {
            Some(first) if first.trim() == expected => proposal_raw.clone(),
            _ => {
                let mut out = String::new();
                out.push_str(&expected);
                out.push('\n');
                for line in proposal_raw.lines().skip(1) {
                    out.push_str(line);
                    out.push('\n');
                }
                if !proposal_raw.ends_with('\n') && !out.ends_with('\n') {
                    out.push('\n');
                }
                out
            }
        }
    };
    tx.add_folder(&format!("proposals/{change_id}"))
        .map_err(crate::operations::OperationError::from)?;
    tx.add_note(
        &format!("proposals/{change_id}/proposal.md"),
        Some("proposal"),
        &proposal_content,
        &[],
    )
    .map_err(crate::operations::OperationError::from)?;

    // specs
    let specs_root = source_path.join(ACTIVE_SPECS_DIR);
    if specs_root.is_dir() {
        let mut caps: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&specs_root).map_err(|source| {
            crate::operations::OperationError::from(
                crate::interchange::plan::InterchangeError::SourceRead {
                    path: specs_root.clone(),
                    source,
                },
            )
        })? {
            let entry = entry.map_err(|source| {
                crate::operations::OperationError::from(
                    crate::interchange::plan::InterchangeError::SourceRead {
                        path: specs_root.clone(),
                        source,
                    },
                )
            })?;
            if !entry.path().is_dir() {
                continue;
            }
            if let Ok(name) = entry.file_name().into_string() {
                caps.push(name);
            }
        }
        caps.sort();
        if !caps.is_empty() {
            tx.add_folder(&format!("proposals/{change_id}/specifications"))
                .map_err(crate::operations::OperationError::from)?;
        }
        for cap in caps {
            let spec_file = specs_root.join(&cap).join("spec.md");
            if !spec_file.is_file() {
                continue;
            }
            let content = std::fs::read_to_string(&spec_file).map_err(|source| {
                crate::operations::OperationError::from(
                    crate::interchange::plan::InterchangeError::SourceRead {
                        path: spec_file.clone(),
                        source,
                    },
                )
            })?;
            let stripped = if content.trim_start().starts_with("# ") {
                content.lines().skip(1).collect::<Vec<_>>().join("\n")
            } else {
                content.clone()
            };
            tx.add_note(
                &format!("proposals/{change_id}/specifications/{cap}.md"),
                Some(&cap),
                &stripped,
                &[],
            )
            .map_err(crate::operations::OperationError::from)?;
        }
    }

    // design
    let design_path = source_path.join(ACTIVE_DESIGN_FILE);
    if design_path.is_file() {
        let design_raw = std::fs::read_to_string(&design_path).map_err(|source| {
            crate::operations::OperationError::from(
                crate::interchange::plan::InterchangeError::SourceRead {
                    path: design_path.clone(),
                    source,
                },
            )
        })?;
        let stripped = if design_raw.trim_start().starts_with("# ") {
            design_raw.lines().skip(1).collect::<Vec<_>>().join("\n")
        } else {
            design_raw.clone()
        };
        tx.add_folder(&format!("proposals/{change_id}/designs"))
            .map_err(crate::operations::OperationError::from)?;
        tx.add_note(
            &format!("proposals/{change_id}/designs/main.md"),
            Some("main"),
            &stripped,
            &[],
        )
        .map_err(crate::operations::OperationError::from)?;
    }

    // decisions
    let decisions_root = source_path.join(ACTIVE_DECISIONS_DIR);
    if decisions_root.is_dir() {
        let mut names: Vec<(String, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&decisions_root).map_err(|source| {
            crate::operations::OperationError::from(
                crate::interchange::plan::InterchangeError::SourceRead {
                    path: decisions_root.clone(),
                    source,
                },
            )
        })? {
            let entry = entry.map_err(|source| {
                crate::operations::OperationError::from(
                    crate::interchange::plan::InterchangeError::SourceRead {
                        path: decisions_root.clone(),
                        source,
                    },
                )
            })?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push((stem.to_string(), path));
            }
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        if !names.is_empty() {
            tx.add_folder(&format!("proposals/{change_id}/decisions"))
                .map_err(crate::operations::OperationError::from)?;
        }
        for (name, path) in names {
            let content = std::fs::read_to_string(&path).map_err(|source| {
                crate::operations::OperationError::from(
                    crate::interchange::plan::InterchangeError::SourceRead {
                        path: path.clone(),
                        source,
                    },
                )
            })?;
            let stripped = if content.trim_start().starts_with("# ") {
                content.lines().skip(1).collect::<Vec<_>>().join("\n")
            } else {
                content.clone()
            };
            tx.add_note(
                &format!("proposals/{change_id}/decisions/{name}.md"),
                Some(&name),
                &stripped,
                &[],
            )
            .map_err(crate::operations::OperationError::from)?;
        }
    }

    // tasks → work.todo.md
    let tasks_path = source_path.join(ACTIVE_TASKS_FILE);
    if tasks_path.is_file() {
        let tasks_raw = std::fs::read_to_string(&tasks_path).map_err(|source| {
            crate::operations::OperationError::from(
                crate::interchange::plan::InterchangeError::SourceRead {
                    path: tasks_path.clone(),
                    source,
                },
            )
        })?;
        let checklist =
            parse_work_note(&tasks_raw).map_err(crate::operations::OperationError::WorkNote)?;
        let todo_title = checklist
            .title
            .clone()
            .unwrap_or_else(|| format!("Execution checklist for {change_id}."));
        let task_texts: Vec<String> = checklist.items.iter().map(|it| it.text.clone()).collect();
        let todo_path = format!("proposals/{change_id}/work.todo.md");
        tx.add_todo(&todo_path, &todo_title, None, &task_texts, &[])
            .map_err(crate::operations::OperationError::from)?;
        for (idx, item) in checklist.items.iter().enumerate() {
            if item.complete {
                let task_number = (idx + 1) as u32;
                tx.mark_task_done(
                    NoteTarget::Path {
                        value: todo_path.clone(),
                    },
                    Some(task_number),
                )
                .map_err(crate::operations::OperationError::from)?;
            }
        }
    }

    // meta
    let meta_content = {
        let payload = serde_json::json!({
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
            "source_path": source_path.display().to_string(),
        });
        format!(
            "```json\n{}\n```\n",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        )
    };
    tx.add_note(
        &format!("proposals/{change_id}/meta.md"),
        Some(change_id),
        &meta_content,
        &[],
    )
    .map_err(crate::operations::OperationError::from)?;

    Ok(tx)
}

pub async fn preflight_active_source(
    client: &nb_api::NbClient,
    notebook_name: &str,
    change_id: &str,
    source_path: &Path,
) -> Result<(), crate::operations::OperationError> {
    use crate::changes::validate_change_id;

    // Validate change_id shape.
    validate_change_id(change_id).map_err(|_| {
        crate::operations::OperationError::from(
            crate::interchange::plan::InterchangeError::InvalidChangeId(change_id.to_string()),
        )
    })?;

    // Check for existing namespace collision (preflight, before any commit).
    let notebook_path = client
        .show_notebook_path(Some(notebook_name))
        .await
        .map_err(crate::operations::OperationError::from)?;
    let proposals_root = notebook_path.join("proposals");
    if let Ok(entries) = std::fs::read_dir(&proposals_root) {
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string()
                && name == change_id
                && entry.path().is_dir()
            {
                return Err(crate::operations::OperationError::from(
                    crate::interchange::plan::InterchangeError::Collision(change_id.to_string()),
                ));
            }
        }
    }

    // Build the full Transaction plan without committing — this validates
    // every spec/decision read and every add_note/add_todo path (including
    // DuplicateTitleHeading, PathCollision, etc.). Drop without commit.
    let _tx = build_active_transaction(client, notebook_name, change_id, source_path).await?;
    Ok(())
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
