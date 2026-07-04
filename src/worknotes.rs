//! Grammar for the `work` todo note.
//!
//! `nb` todo notes carry a checkbox title line (`# [ ] <title>`) and
//! task items as dash bullets (`- [ ] <text>` / `- [x] <text>`,
//! lowercase `x`, exact spacing — the forms `nb tasks` itself
//! recognizes). Status reporting parses the raw note rather than
//! scraping `nb tasks` output, which embeds terminal control
//! sequences even with `--no-color`. Lines that resemble task items
//! but do not match the exact grammar fail parsing loudly: `nb`
//! would silently skip them, and a silently skipped item misreports
//! progress.

use thiserror::Error;

/// Errors from work note parsing.
#[derive(Debug, Error)]
pub enum WorkNoteError {
    #[error(
        "malformed task item at line {line}: {content:?} \
         (expected \"- [ ] <text>\" or \"- [x] <text>\")"
    )]
    MalformedItem { line: usize, content: String },
}

/// One task item in a work checklist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItem {
    /// Whether the item is checked (`[x]`).
    pub complete: bool,
    /// Item text following the checkbox.
    pub text: String,
    /// 1-indexed line number within the note.
    pub line: usize,
}

/// A parsed work todo note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkChecklist {
    /// Title from the checkbox title line, when present.
    pub title: Option<String>,
    /// Whether the title checkbox is checked.
    pub complete: bool,
    /// Task items in note order.
    pub items: Vec<WorkItem>,
}

impl WorkChecklist {
    /// Returns `(complete, total)` item counts.
    pub fn progress(&self) -> (usize, usize) {
        let complete = self.items.iter().filter(|item| item.complete).count();
        (complete, self.items.len())
    }
}

/// Parses a work todo note.
///
/// # Errors
///
/// Returns [`WorkNoteError::MalformedItem`] for dash-bullet checkbox
/// lines that do not match the exact `nb` task grammar.
pub fn parse_work_note(content: &str) -> Result<WorkChecklist, WorkNoteError> {
    let mut checklist = WorkChecklist::default();
    for (index, line) in content.lines().enumerate() {
        let number = index + 1;
        if checklist.title.is_none()
            && let Some((complete, title)) = title_line(line)
        {
            checklist.title = Some(title.trim().to_string());
            checklist.complete = complete;
            continue;
        }
        let stripped = line.trim_start();
        if let Some((complete, text)) = item_line(stripped) {
            checklist.items.push(WorkItem {
                complete,
                text: text.trim().to_string(),
                line: number,
            });
        } else if stripped.starts_with("- [") {
            return Err(WorkNoteError::MalformedItem {
                line: number,
                content: line.trim_end().to_string(),
            });
        }
    }
    Ok(checklist)
}

/// Matches a checkbox title line: `# [ ] <title>` or `# [x] <title>`.
fn title_line(line: &str) -> Option<(bool, &str)> {
    let rest = line.strip_prefix("# ")?;
    checkbox(rest)
}

/// Matches a task item after indentation: `- [ ] <text>` or
/// `- [x] <text>`.
fn item_line(stripped: &str) -> Option<(bool, &str)> {
    let rest = stripped.strip_prefix("- ")?;
    checkbox(rest)
}

/// Matches a checkbox marker and returns `(complete, remainder)`.
/// Accepts a bare marker at end of line (empty remainder).
fn checkbox(rest: &str) -> Option<(bool, &str)> {
    for (marker, complete) in [("[ ]", false), ("[x]", true)] {
        if let Some(after) = rest.strip_prefix(marker)
            && (after.is_empty() || after.starts_with(' '))
        {
            return Some((complete, after));
        }
    }
    None
}
