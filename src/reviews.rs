//! Review verdicts: content-bound, immutable, per-verdict notes.
//!
//! Each verdict is one note under a change's `verdicts/` subfolder,
//! carrying a fenced JSON payload that binds (gate, aggregate content
//! hash). Staleness is hash mismatch ONLY; a verdict for another gate
//! is non-applicable, never expired. Notes are never modified after
//! creation: supersession means a newer note wins, ordered by the
//! recorded timestamp with the note identifier as tie-breaker, so
//! concurrent recordings stay additive under Git.
//!
//! This module is pure model, parsing, and evaluation. Note creation
//! (nb client orchestration) lives in [`crate::operations`], matching
//! the layering of every other verb.

use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Change-namespace subfolder holding verdict notes.
pub const VERDICTS_FOLDER: &str = "verdicts";

/// Errors from reading or parsing the verdict namespace.
#[derive(Debug, Error)]
pub enum VerdictError {
    /// A candidate note failed strict parsing. An unreadable verdict
    /// blocks gate evaluation; it never silently vanishes from it.
    #[error("verdict note {note} is malformed: {reason}")]
    Malformed { note: String, reason: String },
    #[error("IO failure at {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

/// Verdict value a reviewer records for a gate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictValue {
    Approve,
    Revise,
}

impl std::fmt::Display for VerdictValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerdictValue::Approve => formatter.write_str("approve"),
            VerdictValue::Revise => formatter.write_str("revise"),
        }
    }
}

/// Payload of one verdict note (the fenced JSON object).
///
/// Unknown JSON fields are tolerated deliberately: future
/// multi-reviewer policies extend the payload without a storage
/// migration, so an older parser must not reject a newer note.
/// Required fields and types remain strict.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerdictRecord {
    pub reviewer: String,
    pub gate: String,
    pub verdict: VerdictValue,
    /// Aggregate content hash of the rendered set the verdict
    /// evaluated (see [`crate::rendering::aggregate_content_hash`]).
    pub aggregate_hash: String,
    /// RFC 3339 recording time; the semantic ordering key for
    /// supersession. Asserted, like reviewer identity.
    pub timestamp: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// One parsed verdict: payload plus its note identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verdict {
    pub record: VerdictRecord,
    /// Note filename within `verdicts/` (collision-resistant). The
    /// supersession TIE-BREAKER, never the primary ordering key.
    pub note: String,
}

/// A reviewer's standing (latest) verdict for one gate.
#[derive(Clone, Debug)]
pub struct ReviewerPosition<'a> {
    pub verdict: &'a Verdict,
    /// Whether the bound aggregate hash matches the current one.
    pub current: bool,
}

/// Slice-1 gate evaluation outcome: any single current approving
/// verdict satisfies the gate. When unsatisfied, the reported state
/// prefers the stalest-but-closest condition: a stale approval (a
/// re-review away from satisfied) over an outstanding revise, over
/// absence.
#[derive(Clone, Debug)]
pub enum GateEvaluation<'a> {
    Satisfied(&'a Verdict),
    StaleApproval(&'a Verdict),
    ReviseOutstanding(&'a Verdict),
    NoVerdict,
}

/// Gates the slice-1 review policy defines.
pub const KNOWN_GATES: &[&str] = &["merge"];

/// Resolves the reviewer identity: an explicit value wins when
/// non-empty; an explicit EMPTY value resolves to nothing (explicit
/// is never absence — the same contract the MCP notebook resolution
/// settled at 1d1468d); otherwise Git `user.name` is consulted.
/// `None` means the caller must refuse: verdicts never record an
/// empty identity.
pub fn resolve_reviewer(explicit: Option<&str>) -> Option<String> {
    if let Some(value) = explicit {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }
    let output = std::process::Command::new("git")
        .args(["config", "user.name"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Builds a collision-resistant verdict note name: the recorded
/// timestamp in compact form plus process/time entropy. The name is
/// unique for concurrency (new files merge additively under Git); it
/// is NOT the semantic ordering key.
pub fn verdict_note_name(timestamp: &Timestamp) -> String {
    static SEQUENCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let compact = timestamp.strftime("%Y%m%d%H%M%S");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "{compact}-{:x}-{:06x}-{sequence:x}",
        std::process::id(),
        nanos & 0xff_ffff
    )
}

/// Renders a verdict note body: selector-stable H1 matching the note
/// name, then the fenced JSON payload.
///
/// # Errors
///
/// Returns a serialization error when the record cannot be encoded
/// (not expected for well-formed records).
pub fn render_verdict_note(
    name: &str,
    record: &VerdictRecord,
) -> Result<String, serde_json::Error> {
    let json = serde_json::to_string_pretty(record)?;
    Ok(format!("# {name}\n\n```json\n{json}\n```\n"))
}

/// Reads and strictly parses every verdict note under a change
/// directory. An absent `verdicts/` folder means no verdicts. Every
/// `.md` file is a candidate (dotfiles such as nb's `.index` are
/// not); ANY malformed candidate fails the whole read loudly, naming
/// the note.
///
/// # Errors
///
/// Returns [`VerdictError::Malformed`] for the first unparseable
/// candidate and [`VerdictError::Io`] for filesystem failures.
pub fn read_verdicts(change_directory: &Path) -> Result<Vec<Verdict>, VerdictError> {
    let directory = change_directory.join(VERDICTS_FOLDER);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&directory).map_err(|source| VerdictError::Io {
        path: directory.display().to_string(),
        source,
    })?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| VerdictError::Io {
            path: directory.display().to_string(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !name.ends_with(".md") {
            continue;
        }
        names.push(name);
    }
    names.sort();
    let mut verdicts = Vec::new();
    for name in names {
        let path = directory.join(&name);
        let content = std::fs::read_to_string(&path).map_err(|source| VerdictError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let record = parse_verdict_note(&content).map_err(|reason| VerdictError::Malformed {
            note: format!("{VERDICTS_FOLDER}/{name}"),
            reason,
        })?;
        verdicts.push(Verdict { record, note: name });
    }
    Ok(verdicts)
}

/// Latest verdict per reviewer for `gate`, with currency against
/// `current_hash`. Ordered by reviewer name for deterministic output.
/// Supersession: newest recorded timestamp wins; the note identifier
/// breaks exact ties deterministically.
pub fn reviewer_positions<'a>(
    verdicts: &'a [Verdict],
    gate: &str,
    current_hash: &str,
) -> Vec<ReviewerPosition<'a>> {
    let mut latest: std::collections::BTreeMap<&'a str, &'a Verdict> =
        std::collections::BTreeMap::new();
    for verdict in verdicts.iter().filter(|v| v.record.gate == gate) {
        latest
            .entry(verdict.record.reviewer.as_str())
            .and_modify(|incumbent| {
                if supersedes(verdict, incumbent) {
                    *incumbent = verdict;
                }
            })
            .or_insert(verdict);
    }
    latest
        .into_values()
        .map(|verdict| ReviewerPosition {
            current: verdict.record.aggregate_hash == current_hash,
            verdict,
        })
        .collect()
}

/// Evaluates the slice-1 gate policy over reviewer positions.
pub fn evaluate_gate<'a>(positions: &[ReviewerPosition<'a>]) -> GateEvaluation<'a> {
    if let Some(position) = positions
        .iter()
        .find(|p| p.current && p.verdict.record.verdict == VerdictValue::Approve)
    {
        return GateEvaluation::Satisfied(position.verdict);
    }
    if let Some(position) = positions
        .iter()
        .filter(|p| p.verdict.record.verdict == VerdictValue::Approve)
        .max_by(|a, b| recency(a.verdict).cmp(&recency(b.verdict)))
    {
        return GateEvaluation::StaleApproval(position.verdict);
    }
    if let Some(position) = positions
        .iter()
        .filter(|p| p.verdict.record.verdict == VerdictValue::Revise)
        .max_by(|a, b| recency(a.verdict).cmp(&recency(b.verdict)))
    {
        return GateEvaluation::ReviseOutstanding(position.verdict);
    }
    GateEvaluation::NoVerdict
}

fn recency(verdict: &Verdict) -> (Timestamp, &str) {
    (verdict.record.timestamp, verdict.note.as_str())
}

fn supersedes(candidate: &Verdict, incumbent: &Verdict) -> bool {
    recency(candidate) > recency(incumbent)
}

/// Strictly parses one verdict note body: exactly one fenced JSON
/// block (fences recognized as whole trimmed lines only, so CRLF
/// endings and backticks inside JSON strings are harmless), whose
/// object deserializes to a [`VerdictRecord`] with non-empty
/// reviewer, gate, and aggregate hash.
fn parse_verdict_note(content: &str) -> Result<VerdictRecord, String> {
    let json = extract_single_fenced_json(content)?;
    let record: VerdictRecord =
        serde_json::from_str(json).map_err(|error| format!("payload is not a verdict: {error}"))?;
    if record.reviewer.trim().is_empty() {
        return Err("reviewer is empty".to_string());
    }
    if record.gate.trim().is_empty() {
        return Err("gate is empty".to_string());
    }
    if record.aggregate_hash.trim().is_empty() {
        return Err("aggregate_hash is empty".to_string());
    }
    Ok(record)
}

fn extract_single_fenced_json(content: &str) -> Result<&str, String> {
    let mut block: Option<(usize, usize)> = None;
    let mut open_start: Option<usize> = None;
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        match open_start {
            None => {
                if let Some(language) = trimmed.strip_prefix("```") {
                    let language = language.trim();
                    if language.is_empty() || language == "json" {
                        if block.is_some() {
                            return Err("more than one fenced block".to_string());
                        }
                        open_start = Some(offset + line.len());
                    }
                }
            }
            Some(start) => {
                if trimmed == "```" {
                    block = Some((start, offset));
                    open_start = None;
                }
            }
        }
        offset += line.len();
    }
    if open_start.is_some() {
        return Err("unterminated fenced block".to_string());
    }
    match block {
        Some((start, end)) => Ok(&content[start..end]),
        None => Err("no fenced JSON block found".to_string()),
    }
}
