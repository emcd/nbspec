//! Provenance headers for merge-written documents.
//!
//! Every document `merge` writes begins with a one-line Markdown
//! comment naming the generating change, source notebook and note,
//! and a SHA-256 hash of the document body. The hash is the drift
//! detector: a target whose body no longer matches its recorded hash
//! was edited by hand since nbspec last wrote it.

use sha2::{Digest, Sha256};

/// Leading marker of a provenance header line.
pub const HEADER_PREFIX: &str = "<!-- nbspec:";

/// Provenance of one merge-written document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// Generating change identifier.
    pub change_id: String,
    /// Source notebook name.
    pub notebook: String,
    /// Notebook-relative source note file.
    pub note: String,
    /// SHA-256 hex digest of the document body.
    pub hash: String,
}

/// Computes the SHA-256 hex digest of a document body.
pub fn content_hash(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    format!("{digest:x}")
}

/// Renders a provenance header line (without trailing newline).
pub fn render_header(provenance: &Provenance) -> String {
    format!(
        "{HEADER_PREFIX} change={change} notebook={notebook} note={note} \
         hash=sha256:{hash} -->",
        change = provenance.change_id,
        notebook = provenance.notebook,
        note = provenance.note,
        hash = provenance.hash,
    )
}

/// Stamps a document body with a provenance header for `change_id`,
/// hashing the body as given.
pub fn stamp(body: &str, change_id: &str, notebook: &str, note: &str) -> String {
    let provenance = Provenance {
        change_id: change_id.to_string(),
        notebook: notebook.to_string(),
        note: note.to_string(),
        hash: content_hash(body),
    };
    format!("{}\n{body}", render_header(&provenance))
}

/// Splits a document into its provenance and body. Documents without
/// a parseable header line return `None` and the full content as
/// body.
pub fn split_document(content: &str) -> (Option<Provenance>, &str) {
    let Some((first_line, rest)) = content.split_once('\n') else {
        return (parse_header(content), "");
    };
    match parse_header(first_line) {
        Some(provenance) => (Some(provenance), rest),
        None => (None, content),
    }
}

/// Reports whether a document body matches the hash its provenance
/// recorded.
pub fn body_matches(provenance: &Provenance, body: &str) -> bool {
    content_hash(body) == provenance.hash
}

/// Parses a provenance header line.
fn parse_header(line: &str) -> Option<Provenance> {
    let inner = line
        .trim_end()
        .strip_prefix(HEADER_PREFIX)?
        .strip_suffix("-->")?;
    let mut change_id = None;
    let mut notebook = None;
    let mut note = None;
    let mut hash = None;
    for token in inner.split_whitespace() {
        let (key, value) = token.split_once('=')?;
        match key {
            "change" => change_id = Some(value.to_string()),
            "notebook" => notebook = Some(value.to_string()),
            "note" => note = Some(value.to_string()),
            "hash" => hash = Some(value.strip_prefix("sha256:")?.to_string()),
            _ => {}
        }
    }
    Some(Provenance {
        change_id: change_id?,
        notebook: notebook?,
        note: note?,
        hash: hash?,
    })
}
