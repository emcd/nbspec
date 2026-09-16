use std::path::Path;

use nb_api::{NbError, ShowNote};

use crate::changes::{ArtifactLayout, artifact_layout, note_has_authored_content};
use crate::schemata::WorkflowSchema;
use crate::worknotes::WorkChecklist;

use super::OperationError;
use super::context::project_root;

// ── notebook helpers ────────────────────────────────────────────────

pub(crate) async fn folder_exists(
    client: &nb_api::NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> bool {
    // Filesystem-grounded (decisions/4): `nb list <folder>/` exits 1
    // with empty output on empty subfolders once nb-api 0.4.0
    // maintains `.index` files, so scraping its success for existence
    // misreports every empty folder as missing (which then collides
    // at commit when re-created). Probe the resolved notebook path
    // on disk instead; resolution failure reads as absent and the
    // caller's transaction surfaces the real error.
    match client.show_notebook_path(notebook).await {
        Ok(root) => root.join(folder).is_dir(),
        Err(_) => false,
    }
}

pub(crate) async fn folder_listing(
    client: &nb_api::NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> Result<String, String> {
    // Fast path on disk before paying for `nb list`: a missing
    // directory is `(empty)`, and so is an existing directory with
    // no note-like entries — `nb list` fails silently on exactly
    // those folders under `.index` trees, which display must report
    // as empty rather than unreadable. Non-empty directories go
    // through `nb list` passthrough as before.
    if let Ok(root) = client.show_notebook_path(notebook).await {
        let directory = root.join(folder);
        if !directory.is_dir() || !dir_has_notes(&directory) {
            return Ok("(empty)".to_string());
        }
    }
    classify_folder_listing(
        client.list_notes(Some(folder), &[], None, notebook).await,
        folder,
    )
}

/// Reports whether a notebook directory holds note-like entries:
/// subdirectories (surfaced as folder entries by `nb list`) or
/// visible files other than Git/notebook bookkeeping (`.gitkeep`,
/// `.index`, dotfiles), which `nb list` itself does not count.
fn dir_has_notes(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        if name.starts_with('.') || name == ".gitkeep" || name == ".index" {
            return false;
        }
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        kind.is_dir() || kind.is_file()
    })
}

/// Maps a `list_notes` result to display text: real listings pass
/// through, genuine absence becomes `(empty)`, and any other failure
/// becomes `Err` for the display unreadable path.
pub fn classify_folder_listing(
    result: Result<String, NbError>,
    folder: &str,
) -> Result<String, String> {
    match result {
        Ok(listing) => {
            let trimmed = listing.trim();
            if trimmed.is_empty() || trimmed.starts_with("0 items") {
                Ok("(empty)".to_string())
            } else {
                Ok(trimmed.to_string())
            }
        }
        Err(NbError::CommandFailed { stderr, .. }) if is_nb_missing_item(&stderr, folder) => {
            Ok("(empty)".to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Maps a `show_note` result to authored-content detection: genuine
/// absence is `Ok(false)`, other failures are `Err` for the display
/// unreadable path.
pub fn classify_note_content(
    result: Result<String, NbError>,
    selector: &str,
) -> Result<bool, String> {
    match result {
        Ok(content) => Ok(note_has_authored_content(&content)),
        Err(NbError::NotFound { .. }) => Ok(false),
        Err(NbError::CommandFailed { stderr, .. }) if is_nb_missing_item(&stderr, selector) => {
            Ok(false)
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Extracts the body bytes of a structured `show_note` result as a
/// lossy UTF-8 string. nb-api 0.3.0 `show_note` returns [`ShowNote`]
/// with the body as a base64 [`nb_api::ByteString`]; the display and
/// metadata paths operate on the decoded body text.
pub(crate) fn show_note_body(show: &ShowNote) -> Result<String, NbError> {
    Ok(String::from_utf8_lossy(&show.body.as_bytes()?).into_owned())
}

/// Extracts the raw source bytes of a structured `show_note` result
/// as a lossy UTF-8 string. Unlike the parsed `body` (which excludes
/// the title heading and tags prefix), `source` is the complete note
/// file, matching what `nb show` printed before nb-api returned
/// structured results.
pub(crate) fn show_note_source(show: &ShowNote) -> Result<String, NbError> {
    Ok(String::from_utf8_lossy(&show.source.as_bytes()?).into_owned())
}

/// Recognizes the pinned `nb` 7.24.0 missing-item diagnostic for a
/// requested note or folder selector.
///
/// After `nb-api` strips ANSI, the diagnostic is a line of the form
/// `!<C0>* Not found: <target>` where `<target>` is the bare selector
/// or `notebook:selector`, and folders may carry a trailing slash.
/// Compound backend output that merely embeds the `Not found:` token
/// against a different selector is not treated as absence.
pub fn is_nb_missing_item(message: &str, selector: &str) -> bool {
    let requested = selector.trim().trim_end_matches('/');
    if requested.is_empty() {
        return false;
    }
    message.lines().any(|line| {
        let Some(target) = nb_not_found_target(line) else {
            return false;
        };
        let target = target.trim_end_matches('/');
        target == requested
            || target
                .rsplit_once(':')
                .is_some_and(|(_, path)| path == requested)
    })
}

/// Extracts the target from a single pinned `! Not found: <target>`
/// line. Prefix may include C0 controls (nb emits SI after SGR reset).
fn nb_not_found_target(line: &str) -> Option<&str> {
    const MARKER: &str = "Not found: ";
    let trimmed = line.trim();
    let index = trimmed.find(MARKER)?;
    let prefix = &trimmed[..index];
    if !prefix.contains('!') {
        return None;
    }
    if !prefix
        .chars()
        .all(|c| c == '!' || c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let target = trimmed[index + MARKER.len()..].trim();
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
}

// ── work helpers ────────────────────────────────────────────────────

/// Reads the change's work todo note from the notebook filesystem:
/// the `*.todo.md` file whose checkbox title is `WORK_NOTE`.
/// Parsing the file directly avoids scraping `nb tasks` output,
/// which embeds terminal control sequences even with `--no-color`.
pub(crate) fn read_work_note(change_directory: &std::path::Path) -> Option<String> {
    use crate::changes::WORK_NOTE;
    let entries = std::fs::read_dir(change_directory).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".todo.md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let open_title = format!("# [ ] {WORK_NOTE}");
        let done_title = format!("# [x] {WORK_NOTE}");
        if content
            .lines()
            .any(|line| line == open_title || line == done_title)
        {
            return Some(content);
        }
    }
    None
}

pub(crate) fn render_work_checklist(checklist: &WorkChecklist) -> String {
    let (complete, total) = checklist.progress();
    if total == 0 {
        return "no task items yet\n".to_string();
    }
    let mut output = format!("{complete}/{total} items complete\n");
    for item in &checklist.items {
        let marker = if item.complete { "x" } else { " " };
        output.push_str(&format!("- [{marker}] {}\n", item.text));
    }
    output
}

// ── artifact helpers ────────────────────────────────────────────────

/// Reports whether an artifact has authored content: notes must have
/// body content beyond their placeholder, folders must contain notes.
pub(crate) async fn artifact_has_content(
    client: &nb_api::NbClient,
    folder: &str,
    schema: &WorkflowSchema,
    artifact_id: &str,
    notebook: Option<&str>,
) -> Result<bool, String> {
    let Some(artifact) = schema.artifact(artifact_id) else {
        return Ok(false);
    };
    match artifact_layout(artifact) {
        ArtifactLayout::Note(note) => {
            let selector = format!("{folder}/{note}.md");
            let content = client
                .show_note(&selector, notebook)
                .await
                .and_then(|show| show_note_body(&show));
            classify_note_content(content, &selector)
        }
        ArtifactLayout::Folder(subfolder) => {
            let listing =
                folder_listing(client, &format!("{folder}/{subfolder}"), notebook).await?;
            Ok(listing != "(empty)" && !listing.starts_with("0 "))
        }
    }
}

// ── metadata / configuration helpers (used by display) ──────────────

pub(crate) fn schema_for(
    metadata: &crate::changes::ChangeMetadata,
) -> Result<WorkflowSchema, OperationError> {
    let configuration = crate::configuration::load_configuration(&project_root())?;
    Ok(crate::schemata::resolve_schema(
        Some(&metadata.schema),
        &configuration,
    )?)
}

pub(crate) fn metadata_summary(metadata: &crate::changes::ChangeMetadata) -> String {
    let title = metadata.title.as_deref().unwrap_or("(untitled)");
    format!(
        "Change: {id}\nTitle: {title}\nStatus: {status}\nSchema: {schema}\nNotebook: {notebook}\nUpdated: {updated}\n",
        id = metadata.change_id,
        status = metadata.status,
        schema = metadata.schema,
        notebook = metadata.notebook,
        updated = metadata.updated_at,
    )
}

pub(crate) async fn load_metadata(
    client: &nb_api::NbClient,
    folder: &str,
    notebook: Option<&str>,
) -> Result<crate::changes::ChangeMetadata, OperationError> {
    use crate::changes::{META_NOTE, parse_meta_note};
    let show = client
        .show_note(&format!("{folder}/{META_NOTE}.md"), notebook)
        .await?;
    let content = show_note_body(&show)?;
    Ok(parse_meta_note(&content)?)
}
