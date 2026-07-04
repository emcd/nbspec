//! Deterministic scratch-workspace rendering.
//!
//! Renders a notebook change to the file tree its schema `generates`
//! paths declare: artifact notes are read directly from the notebook
//! directory (notebooks are plain git-backed folders; `nb-api`
//! exposes their paths) and copied byte-for-byte, so identical notes
//! always produce a byte-identical tree. Rendering writes only to
//! scratch destinations — never to the repository working tree.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::changes::{ArtifactLayout, artifact_layout};
use crate::schemata::WorkflowSchema;

/// Errors from rendering.
#[derive(Debug, Error)]
pub enum RenderError {
    #[error("IO failure at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl RenderError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        RenderError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// One document of a rendered change tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedDocument {
    /// Schema artifact the document belongs to.
    pub artifact_id: String,
    /// Path within the rendered tree (matches the artifact's
    /// `generates` pattern), for example
    /// `specifications/change-authoring.md`.
    pub tree_path: PathBuf,
    /// Repository-relative merge destination, when the artifact
    /// declares a `target`; `None` marks render-only documents.
    pub target_path: Option<PathBuf>,
    /// Notebook-relative source note file, for provenance.
    pub source_note: String,
    /// Verbatim note content.
    pub content: String,
}

/// Renders a change's artifact notes into an ordered document list:
/// artifacts in schema declaration order, documents within a folder
/// artifact sorted by path. Notes the change has not authored yet
/// (absent files) are skipped; control-plane files (`meta`, the
/// `work` todo note) belong to no artifact and never render.
///
/// # Errors
///
/// Returns [`RenderError::Io`] when the notebook directory cannot be
/// read.
pub fn render_documents(
    change_directory: &Path,
    change_folder: &str,
    schema: &WorkflowSchema,
) -> Result<Vec<RenderedDocument>, RenderError> {
    let mut documents = Vec::new();
    for artifact in &schema.artifacts {
        match artifact_layout(artifact) {
            ArtifactLayout::Note(stem) => {
                let file = change_directory.join(format!("{stem}.md"));
                if !file.is_file() {
                    continue;
                }
                let content = std::fs::read_to_string(&file)
                    .map_err(|error| RenderError::io(&file, error))?;
                let tree_path = PathBuf::from(&artifact.generates);
                documents.push(RenderedDocument {
                    artifact_id: artifact.id.clone(),
                    target_path: artifact
                        .target
                        .as_deref()
                        .map(|target| Path::new(target).join(&tree_path)),
                    source_note: format!("{change_folder}/{stem}.md"),
                    tree_path,
                    content,
                });
            }
            ArtifactLayout::Folder(name) => {
                let directory = change_directory.join(&name);
                if !directory.is_dir() {
                    continue;
                }
                for relative in walk_markdown(&directory)? {
                    let file = directory.join(&relative);
                    let content = std::fs::read_to_string(&file)
                        .map_err(|error| RenderError::io(&file, error))?;
                    let tree_path = Path::new(&name).join(&relative);
                    documents.push(RenderedDocument {
                        artifact_id: artifact.id.clone(),
                        target_path: artifact
                            .target
                            .as_deref()
                            .map(|target| Path::new(target).join(&relative)),
                        source_note: format!("{change_folder}/{}", tree_path.to_string_lossy()),
                        tree_path,
                        content,
                    });
                }
            }
        }
    }
    Ok(documents)
}

/// Writes a rendered document list as a file tree, replacing any
/// previous contents of `destination` so stale files never survive a
/// re-render.
///
/// # Errors
///
/// Returns [`RenderError::Io`] when the destination cannot be
/// (re)created or written.
pub fn write_tree(documents: &[RenderedDocument], destination: &Path) -> Result<(), RenderError> {
    if destination.exists() {
        std::fs::remove_dir_all(destination)
            .map_err(|error| RenderError::io(destination, error))?;
    }
    std::fs::create_dir_all(destination).map_err(|error| RenderError::io(destination, error))?;
    for document in documents {
        let path = destination.join(&document.tree_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| RenderError::io(parent, error))?;
        }
        std::fs::write(&path, &document.content).map_err(|error| RenderError::io(&path, error))?;
    }
    Ok(())
}

/// Builds a unified diff between a change's durable documents (those
/// with merge targets) and the current contents of their repository
/// targets, in `git diff` format suitable for external review
/// tooling. Target provenance headers are stripped before comparing,
/// so the diff shows document changes rather than header churn.
/// Unchanged targets are omitted; absent targets diff from
/// `/dev/null`.
///
/// # Errors
///
/// Returns [`RenderError::Io`] when an existing target cannot be
/// read.
pub fn review_diff(
    documents: &[RenderedDocument],
    project_root: &Path,
) -> Result<String, RenderError> {
    let mut output = String::new();
    for document in documents {
        let Some(target_path) = &document.target_path else {
            continue;
        };
        let absolute = project_root.join(target_path);
        let current = if absolute.is_file() {
            let raw = std::fs::read_to_string(&absolute)
                .map_err(|error| RenderError::io(&absolute, error))?;
            let (_, body) = crate::provenance::split_document(&raw);
            Some(body.to_string())
        } else {
            None
        };
        if current.as_deref() == Some(document.content.as_str()) {
            continue;
        }
        let path = target_path.to_string_lossy();
        output.push_str(&format!("diff --git a/{path} b/{path}\n"));
        let (old_content, old_header) = match &current {
            Some(content) => (content.as_str(), format!("a/{path}")),
            None => {
                output.push_str("new file mode 100644\n");
                ("", "/dev/null".to_string())
            }
        };
        let text_diff = similar::TextDiff::from_lines(old_content, document.content.as_str());
        output.push_str(&format!(
            "{}",
            text_diff
                .unified_diff()
                .header(&old_header, &format!("b/{path}"))
        ));
    }
    Ok(output)
}

/// Collects Markdown files under a directory recursively, as sorted
/// directory-relative paths. Todo notes and hidden files are not
/// documents and are skipped.
fn walk_markdown(directory: &Path) -> Result<Vec<PathBuf>, RenderError> {
    let mut files = Vec::new();
    collect_markdown(directory, Path::new(""), &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_markdown(
    directory: &Path,
    prefix: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), RenderError> {
    let entries =
        std::fs::read_dir(directory).map_err(|error| RenderError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| RenderError::io(directory, error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, &prefix.join(name.as_ref()), files)?;
        } else if name.ends_with(".md") && !name.ends_with(".todo.md") {
            files.push(prefix.join(name.as_ref()));
        }
    }
    Ok(())
}
