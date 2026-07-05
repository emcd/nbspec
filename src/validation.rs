//! Native change validation over the OpenSpec grammar.
//!
//! Applies structural rules to a change's rendered documents without
//! invoking any external binary: schema-required artifacts must carry
//! authored content, and documents of delta-specification artifacts
//! must satisfy the OpenSpec 1.x requirement/scenario/delta grammar.
//! Diagnostics identify notebook notes and artifact ids, with
//! 1-indexed line numbers within the note where feasible — never
//! scratch or repository file paths.

use std::collections::HashSet;
use std::fmt;

use serde::Serialize;

use crate::changes::{ArtifactLayout, artifact_layout, note_has_authored_content};
use crate::grammar::{Requirement, parse_delta_specification};
use crate::rendering::RenderedDocument;
use crate::schemata::{ArtifactGrammar, WorkflowSchema};

/// One validation violation, anchored to a notebook note.
///
/// Serialized verbatim into the MCP `validate` tool's structured
/// return — the field names below are the wire-format contract.
/// `line` is null when the failure is required-artifact or
/// document-level rather than line-anchored.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    /// Notebook note path, for example
    /// `proposals/add-foo/specifications/user-auth.md`.
    pub note: String,
    /// Schema artifact the note belongs to.
    pub artifact_id: String,
    /// 1-indexed line within the note content, where feasible.
    pub line: Option<usize>,
    /// The violated rule, stated for humans and agents alike.
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(
                formatter,
                "{note}:{line}: [{artifact}] {message}",
                note = self.note,
                artifact = self.artifact_id,
                message = self.message,
            ),
            None => write!(
                formatter,
                "{note}: [{artifact}] {message}",
                note = self.note,
                artifact = self.artifact_id,
                message = self.message,
            ),
        }
    }
}

/// A failed validation: the change id and every diagnostic found.
///
/// Displays as a summary line followed by one diagnostic per line in
/// `note:line: [artifact] message` form, so agents can split lines
/// and map each violation back to its notebook note.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationFailure {
    /// The validated change.
    pub change_id: String,
    /// Violations in document order.
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for ValidationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.diagnostics.len();
        let noun = if count == 1 {
            "violation"
        } else {
            "violations"
        };
        write!(
            formatter,
            "change {change_id} is invalid: {count} {noun}",
            change_id = self.change_id,
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n{diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationFailure {}

/// Validates a change's rendered documents against its schema.
///
/// Reports missing required artifacts first, in schema declaration
/// order, then grammar violations per document in document order.
/// An empty result means the change is valid.
pub fn validate_change(
    documents: &[RenderedDocument],
    schema: &WorkflowSchema,
    change_folder: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for artifact in &schema.artifacts {
        if !artifact.required {
            continue;
        }
        // A file within a folder artifact exists only because someone
        // authored it; a root note also has to outgrow its scaffold
        // placeholder to count.
        let layout = artifact_layout(artifact);
        let authored = documents.iter().any(|document| {
            document.artifact_id == artifact.id
                && match layout {
                    ArtifactLayout::Note(_) => note_has_authored_content(&document.content),
                    ArtifactLayout::Folder(_) => true,
                }
        });
        if authored {
            continue;
        }
        let note = match layout {
            ArtifactLayout::Note(stem) => format!("{change_folder}/{stem}.md"),
            ArtifactLayout::Folder(folder) => format!("{change_folder}/{folder}/"),
        };
        diagnostics.push(Diagnostic {
            note,
            artifact_id: artifact.id.clone(),
            line: None,
            message: "required artifact has no authored content".to_string(),
        });
    }
    for document in documents {
        let Some(artifact) = schema.artifact(&document.artifact_id) else {
            continue;
        };
        if artifact.grammar != Some(ArtifactGrammar::DeltaSpecification) {
            continue;
        }
        // A scaffolded root note awaiting authorship is already covered
        // by the required-artifact rule; a file within a folder artifact
        // exists only because someone authored it, so it always faces
        // the grammar.
        let authored = match artifact_layout(artifact) {
            ArtifactLayout::Note(_) => note_has_authored_content(&document.content),
            ArtifactLayout::Folder(_) => true,
        };
        if !authored {
            continue;
        }
        validate_delta_document(document, &mut diagnostics);
    }
    diagnostics
}

/// Applies the delta-specification grammar rules to one document.
fn validate_delta_document(document: &RenderedDocument, diagnostics: &mut Vec<Diagnostic>) {
    let delta = parse_delta_specification(&document.content);
    let presence = delta.presence;
    if !(presence.added || presence.modified || presence.removed || presence.renamed) {
        diagnostics.push(document_diagnostic(
            document,
            None,
            "no delta sections (ADDED, MODIFIED, REMOVED, or RENAMED Requirements)".to_string(),
        ));
        return;
    }
    if presence.added && delta.added.is_empty() {
        diagnostics.push(document_diagnostic(
            document,
            None,
            "ADDED Requirements section declares no requirements".to_string(),
        ));
    }
    if presence.modified && delta.modified.is_empty() {
        diagnostics.push(document_diagnostic(
            document,
            None,
            "MODIFIED Requirements section declares no requirements".to_string(),
        ));
    }
    if presence.removed && delta.removed.is_empty() {
        diagnostics.push(document_diagnostic(
            document,
            None,
            "REMOVED Requirements section names no requirements".to_string(),
        ));
    }
    if presence.renamed && delta.renamed.is_empty() {
        diagnostics.push(document_diagnostic(
            document,
            None,
            "RENAMED Requirements section contains no FROM:/TO: pairs".to_string(),
        ));
    }
    validate_requirements(&delta.added, "ADDED", document, diagnostics);
    validate_requirements(&delta.modified, "MODIFIED", document, diagnostics);
}

/// Applies the requirement-block rules to one delta section: unique
/// names, normative text, and at least one scenario with content.
fn validate_requirements(
    requirements: &[Requirement],
    section: &str,
    document: &RenderedDocument,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen: HashSet<&str> = HashSet::new();
    for requirement in requirements {
        if !seen.insert(requirement.name.as_str()) {
            diagnostics.push(document_diagnostic(
                document,
                Some(requirement.line),
                format!(
                    "duplicate requirement name in {section} Requirements: {}",
                    requirement.name
                ),
            ));
        }
        if requirement.text.is_none() {
            diagnostics.push(document_diagnostic(
                document,
                Some(requirement.line),
                format!("requirement {} has no normative text", requirement.name),
            ));
        }
        if requirement.scenarios.is_empty() {
            diagnostics.push(document_diagnostic(
                document,
                Some(requirement.line),
                format!(
                    "requirement {} has no #### Scenario: block",
                    requirement.name
                ),
            ));
        }
        for scenario in &requirement.scenarios {
            if scenario.body.is_empty() {
                diagnostics.push(document_diagnostic(
                    document,
                    Some(scenario.line),
                    format!("scenario {} has no WHEN/THEN content", scenario.name),
                ));
            }
        }
    }
}

fn document_diagnostic(
    document: &RenderedDocument,
    line: Option<usize>,
    message: String,
) -> Diagnostic {
    Diagnostic {
        note: document.source_note.clone(),
        artifact_id: document.artifact_id.clone(),
        line,
        message,
    }
}
