use std::path::Path;

use crate::interchange::plan::RoundTripProof;

/// Runs the export against the imported change and diffs the
/// resulting tree against the source tree modulo the typed
/// normalizations. The proof is the inverse composition: `import`
/// and `export` are inverses by construction, so a clean
/// round-trip authorizes `--delete-original`. Divergences are
/// named explicitly so the operator sees what was elided.
///
/// The proof exercises the export path on every gated import,
/// regression-testing the export surface. It also covers content
/// the rendered set does not include — most importantly,
/// `tasks.md` reconstruction from the `work` note.
///
/// # Fail-closed semantics (R2 / F2)
///
/// Every walk error surfaces as a divergence. A symlink in the
/// source tree is a divergence (symlinks are not part of the
/// deterministic interchange artifact). A non-UTF-8 file
/// preserves its bytes verbatim (no silent `read_to_string`
/// drop); the proof compares bytes against the bytes preserved
/// in the archive, so binary files round-trip cleanly. A file
/// missing from one side is a divergence. The proof never
/// returns `clean` on an incomplete inventory.
///
/// # Errors
///
/// Returns filesystem IO errors only when an entry cannot be
/// listed at all (e.g. permission denied on the root itself).
/// Per-file failures are surfaced as divergences, not as
/// panics, so the proof is total.
///
/// R7''' / F7 second re-review 2026-07-25: the proof now takes
/// the source change root and scratch change root directly,
/// rather than deriving them from `<root>/<change_id>/`. This
/// lets the caller point the proof at a quarantined snapshot
/// (which is renamed to a sibling path that no longer contains
/// a `<change_id>/` subdirectory) without restructuring the
/// quarantine layout.
pub fn round_trip_proof(
    imported_change_id: &str,
    source_change_root: &Path,
    scratch_change_root: &Path,
) -> RoundTripProof {
    let source_change = source_change_root;
    let scratch_change = scratch_change_root;
    let mut divergences: Vec<String> = Vec::new();
    if !source_change.is_dir() {
        divergences.push(format!(
            "{imported_change_id}: source tree not found at {}",
            source_change.display()
        ));
        return RoundTripProof {
            clean: false,
            divergences,
        };
    }
    if !scratch_change.is_dir() {
        divergences.push(format!(
            "{imported_change_id}: export tree not found at {}",
            scratch_change.display()
        ));
        return RoundTripProof {
            clean: false,
            divergences,
        };
    }

    let source_files = collect_files_for_proof(source_change, "source", &mut divergences);
    for (relative, source_bytes) in &source_files {
        let export_path = scratch_change.join(relative);
        let export_bytes = match std::fs::read(&export_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                divergences.push(format!("{}: missing from export ({})", relative, error));
                continue;
            }
        };
        let canonical_source = canonical_bytes_for_proof(relative, source_bytes);
        let canonical_export = canonical_bytes_for_proof(relative, &export_bytes);
        if canonical_source == canonical_export {
            continue;
        }
        divergences.push(format!(
            "{}: content diverges beyond typed normalizations",
            relative
        ));
    }

    // Files only in the export tree are divergences too.
    let export_files = collect_files_for_proof(scratch_change, "export", &mut divergences);
    for (relative, _) in &export_files {
        if !source_files
            .iter()
            .any(|(source_relative, _)| source_relative == relative)
        {
            divergences.push(format!(
                "{}: present in export but missing from source",
                relative
            ));
        }
    }

    RoundTripProof {
        clean: divergences.is_empty(),
        divergences,
    }
}

/// Collects every regular file under `root` as `(relative_path,
/// content_bytes)` pairs. Returns divergences via the supplied
/// accumulator (rather than swallowing them) so the proof fails
/// closed on every walk error: unreadable directory listings,
/// non-UTF-8 filenames, symlinks, IO failures, and non-`is_file`
/// entries all surface as named divergences.
///
/// R2 (F2): the previous `collect_text_files` silently skipped
/// every problematic entry (binary files, symlinks, IO errors),
/// then compared the partial inventory as if it were complete.
/// A clean proof could then authorize `remove_dir_all` on
/// inventoried-as-complete source data.
///
/// R2' (F2 re-review 2026-07-25): the walker still used
/// `ReadDir::flatten()` (silently dropping iterator errors) and
/// `into_string().ok()` (silently dropping non-UTF-8 filenames);
/// `strip_prefix` failure was also silently swallowed. An empty
/// divergences list no longer authorizes deletion unless every
/// entry was explicitly enumerated, with explicit failures surfaced.
pub fn collect_files_for_proof(
    root: &Path,
    side: &str,
    divergences: &mut Vec<String>,
) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    walk_for_proof(root, root, side, &mut files, divergences);
    files.sort();
    files
}

pub fn walk_for_proof(
    root: &Path,
    current: &Path,
    side: &str,
    files: &mut Vec<(String, Vec<u8>)>,
    divergences: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) => {
            divergences.push(format!(
                "{side}: cannot read directory {}: {error}",
                current.display()
            ));
            return;
        }
    };
    let mut names: Vec<String> = Vec::new();
    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                divergences.push(format!(
                    "{side}: read_dir entry error in {}: {error}",
                    current.display()
                ));
                continue;
            }
        };
        match entry.file_name().into_string() {
            Ok(name) => names.push(name),
            Err(os_str) => {
                divergences.push(format!(
                    "{side}: non-UTF-8 filename in {}: {:?}",
                    current.display(),
                    os_str
                ));
            }
        }
    }
    names.sort();
    for name in names {
        let path = current.join(&name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                divergences.push(format!("{side}: cannot stat {}: {error}", path.display()));
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            divergences.push(format!(
                "{side}: symlink at {} is not part of the deterministic interchange artifact",
                path.display()
            ));
            continue;
        }
        if metadata.is_dir() {
            walk_for_proof(root, &path, side, files, divergences);
        } else if metadata.is_file() {
            let relative = match path.strip_prefix(root) {
                Ok(relative) => relative,
                Err(error) => {
                    divergences.push(format!(
                        "{side}: path {} not under root {}: {error}",
                        path.display(),
                        root.display()
                    ));
                    continue;
                }
            };
            let posix = match relative.to_str() {
                Some(s) => s.replace(std::path::MAIN_SEPARATOR, "/"),
                None => {
                    divergences.push(format!(
                        "{side}: non-UTF-8 path component at {}",
                        relative.display()
                    ));
                    continue;
                }
            };
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    divergences.push(format!("{side}: cannot read {}: {error}", path.display()));
                    continue;
                }
            };
            files.push((posix, bytes));
        } else {
            divergences.push(format!(
                "{side}: unsupported entry type at {}",
                path.display()
            ));
        }
    }
}

/// Canonicalizes file bytes for round-trip proof comparison.
///
/// Text artifacts under the change tree (proposal.md, specs/*/spec.md,
/// design.md, decisions/*.md, tasks.md) are normalized by their
/// typed rules (H1 strip for proposal / spec / design / decisions,
/// tasks.md header normalization). Binary and other artifacts are
/// compared verbatim.
///
/// R2 (F2): the previous proof compared `String`s after
/// `read_to_string`; non-UTF-8 files failed the read, were
/// silently dropped, and never reached the comparison. We now
/// carry bytes end-to-end and only apply text normalizations to
/// the change-tree artifacts that the spec defines.
pub fn canonical_bytes_for_proof(relative_path: &str, bytes: &[u8]) -> Vec<u8> {
    let is_text = relative_path == "proposal.md"
        || relative_path == "tasks.md"
        || relative_path == "design.md"
        || (relative_path.starts_with("specs/") && relative_path.ends_with("/spec.md"))
        || (relative_path.starts_with("decisions/") && relative_path.ends_with(".md"));
    if !is_text {
        return bytes.to_vec();
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let normalized = canonical_for_proof(relative_path, text);
    normalized.into_bytes()
}

/// Canonicalizes a file's content for round-trip proof comparison,
/// applying the typed normalizations. Each branch corresponds to
/// one entry in the `RoundTripNormalization` enum; new categories
/// are added deliberately, not by accumulation.
pub fn canonical_for_proof(relative_path: &str, content: &str) -> String {
    if relative_path == "proposal.md" {
        return canonical_proposal_for_proof(content);
    }
    if relative_path == "tasks.md" {
        return canonical_tasks_for_proof(content);
    }
    if relative_path.starts_with("decisions/") {
        return canonical_decision_for_proof(content);
    }
    if relative_path.starts_with("specs/") && relative_path.ends_with("/spec.md") {
        return canonical_spec_for_proof(content);
    }
    if relative_path == "design.md" {
        return canonical_spec_for_proof(content);
    }
    content.to_string()
}

/// Strips a single leading H1 line (e.g. `# <title>`) from the
/// content. Used during import for spec, design, and decision
/// files whose H1 duplicates the notebook title; the round-trip
/// proof canonicalizes both sides with the same strip so the
/// missing H1 on the export side is tolerated.
pub fn strip_leading_h1(content: &str) -> String {
    let mut stripped = String::new();
    let mut skipped = false;
    for line in content.lines() {
        if !skipped && line.starts_with("# ") {
            skipped = true;
            continue;
        }
        stripped.push_str(line);
        stripped.push('\n');
    }
    stripped
}

/// Canonicalizes spec/design/decisions content by stripping a
/// leading H1 line. Symmetric with the import-side `strip_leading_h1`.
pub fn canonical_spec_for_proof(content: &str) -> String {
    strip_leading_h1(content)
}

/// Strips the proposal's leading H1 line: `H1Rewrite` is the
/// normalized form, so a divergence on the first line is the
/// expected normalization rather than a real change. Trailing
/// whitespace is also trimmed so differences in trailing newlines
/// do not register as divergences.
pub fn canonical_proposal_for_proof(content: &str) -> String {
    let mut stripped = String::new();
    for (index, line) in content.lines().enumerate() {
        if index == 0 && line.starts_with("# ") {
            continue;
        }
        stripped.push_str(line);
        stripped.push('\n');
    }
    stripped.trim_end().to_string()
}

/// Normalizes the tasks.md header: `# Tasks` (with or without the
/// `[ ]/x` checkbox title marker, blank line or no) reduces to
/// `# Tasks\n\n`. Leading blanks after the header are elided so
/// the canonical form is the same regardless of whether the source
/// separated the header from the items with a blank line. The
/// rest of the body is unchanged so per-item checkbox state is
/// compared faithfully.
pub fn canonical_tasks_for_proof(content: &str) -> String {
    let mut output = String::new();
    let mut emitted_header = false;
    let mut skipping_blanks = false;
    for line in content.lines() {
        if !emitted_header {
            let is_header = line.starts_with("# Tasks")
                || line.starts_with("# [ ] Tasks")
                || line.starts_with("# [x] Tasks");
            let is_blank = line.trim().is_empty();
            if is_header {
                output.push_str("# Tasks\n\n");
                emitted_header = true;
                skipping_blanks = true;
                continue;
            }
            if is_blank {
                continue;
            }
            // First non-blank, non-tasks-header line: emit header.
            output.push_str("# Tasks\n\n");
            emitted_header = true;
            skipping_blanks = true;
        }
        if skipping_blanks && line.trim().is_empty() {
            continue;
        }
        skipping_blanks = false;
        output.push_str(line);
        output.push('\n');
    }
    if !emitted_header {
        output.push_str("# Tasks\n\n");
    }
    output
}

/// Decision content is compared verbatim; the `DecisionSlot`
/// normalization covers filename preservation (the proof compares
/// same-named files; no body-level transformation is performed).
pub fn canonical_decision_for_proof(content: &str) -> String {
    content.to_string()
}
