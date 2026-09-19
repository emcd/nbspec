//! Surgical delta application for merge.
//!
//! Upstream-shaped (`specs-apply.ts:buildUpdatedSpec`): the delta note
//! is pre-validated for coherence, then its operations apply in order
//! `RENAMED → REMOVED → MODIFIED → ADDED` against the target's
//! requirement-block map with exact-name matching. Every document
//! carrying delta sections applies surgically — including
//! `ADDED`-only notes, whose blocks append, no-op when identical, or
//! refuse when differing. Notes without delta sections keep the
//! legacy whole-write path (merging.rs branches before calling
//! here). Collision overwrite applies only when the caller observed
//! actually overridden drift; hash-valid collisions always refuse.

use std::collections::HashMap;

use crate::grammar::{
    UnpairedSide, duplicate_section_kinds, extract_purpose_section, find_unpaired_renames,
    fold_requirement_name, mask_fenced_lines, parse_delta_specification, parse_target_blocks,
};

pub use crate::grammar::has_requirements_section;

/// Outcome of planning one surgical document against its target.
pub struct AppliedDoc {
    /// Rebuilt durable body to stamp and write (unused when skipped).
    pub body: String,
    /// Non-blocking diagnostics for merge text + structured reporting.
    pub warnings: Vec<String>,
    /// True when no effective operation remains: warn, write nothing.
    pub skipped: bool,
}

/// Why delta planning failed. Both kinds are validation failures:
/// `--force` overrides drift, never these.
#[derive(Debug)]
pub struct ApplyFailure {
    /// True for intra-note incoherence, false for target-application faults.
    pub incoherent: bool,
    /// Human-readable diagnostic naming names and lines.
    pub message: String,
}

impl ApplyFailure {
    fn incoherent(message: String) -> Self {
        Self {
            incoherent: true,
            message,
        }
    }

    fn dangling(message: String) -> Self {
        Self {
            incoherent: false,
            message,
        }
    }
}

/// Plans one surgical document: pre-validates coherence, then applies
/// against `target_body` (`None` for an absent target).
/// `title_fallback` names the rebuilt document when the delta note
/// carries no `# ` title line. `overwrite_collisions` resolves
/// `ADDED` collisions (existing block with differing content) by
/// delta-wins replacement: callers set it only when merging with
/// `--force` against an actually drifted target, where the
/// difference may itself be the drift. Hash-valid collisions always
/// refuse, and absences never resolve under any flag.
pub fn plan_delta_merge(
    delta_content: &str,
    target_body: Option<&str>,
    title_fallback: &str,
    overwrite_collisions: bool,
) -> Result<AppliedDoc, ApplyFailure> {
    check_delta_coherence(delta_content)?;
    let delta = parse_delta_specification(delta_content);

    let added_names: Vec<String> = delta.added.iter().map(|req| req.name.clone()).collect();
    if target_body.is_none()
        && delta.modified.is_empty()
        && delta.renamed.is_empty()
        && added_names.is_empty()
    {
        return Ok(AppliedDoc {
            body: String::new(),
            warnings: vec![format!(
                "no effective operation remains ({} REMOVED requirement(s) against an absent target); skipping without creating a target",
                delta.removed.len()
            )],
            skipped: true,
        });
    }
    if target_body.is_none() && (!delta.modified.is_empty() || !delta.renamed.is_empty()) {
        return Err(ApplyFailure::dangling(format!(
            "target does not exist; only ADDED requirements are allowed for new documents ({} MODIFIED, {} RENAMED)",
            delta.modified.len(),
            delta.renamed.len()
        )));
    }

    let mut warnings = Vec::new();
    if target_body.is_none() {
        if !delta.removed.is_empty() {
            let names = delta.removed.join(", ");
            warnings.push(format!(
                "REMOVED requirement(s) ignored for a new document (nothing to remove): {names}"
            ));
        }
        return Ok(AppliedDoc {
            body: build_new_document(delta_content, &delta, title_fallback),
            warnings,
            skipped: false,
        });
    }
    let target = target_body.expect("existing target");
    apply_onto_existing(
        delta_content,
        &delta,
        target,
        &mut warnings,
        overwrite_collisions,
    )
}

/// Rejects incoherent deltas before any write: duplicate sections,
/// duplicates within a section, cross-section conflicts, unpaired
/// rename sides, and rename cycles. Target-independent: runs before
/// drift, succession, or force are consulted.
pub fn check_delta_coherence(delta_content: &str) -> Result<(), ApplyFailure> {
    let delta = parse_delta_specification(delta_content);
    prevalidate(&delta, delta_content)
}

/// Rejects incoherent deltas before any write: duplicates within a
/// section, cross-section conflicts, and unpaired rename sides.
fn prevalidate(
    delta: &crate::grammar::DeltaSpecification,
    delta_content: &str,
) -> Result<(), ApplyFailure> {
    if let Some(first) = find_unpaired_renames(delta_content).into_iter().next() {
        let missing = match first.side {
            UnpairedSide::From => "TO",
            UnpairedSide::To => "FROM",
        };
        return Err(ApplyFailure::incoherent(format!(
            "RENAMED entry on line {} has no matching {missing}: write each rename as a FROM: line followed immediately by its TO: line",
            first.line
        )));
    }
    if let Some(kind) = duplicate_section_kinds(delta_content).into_iter().next() {
        return Err(ApplyFailure::incoherent(format!(
            "duplicate \"## {kind}\" section: repeated delta sections leave operations silently ignored"
        )));
    }
    let in_section = |names: Vec<(String, String)>, section: &'static str| {
        let mut local: HashMap<String, String> = HashMap::new();
        for (folded, display) in names {
            if local.insert(folded, display.clone()).is_some() {
                return Err(ApplyFailure::incoherent(format!(
                    "duplicate requirement in {section} for header \"### Requirement: {display}\""
                )));
            }
        }
        Ok(())
    };
    in_section(
        delta
            .added
            .iter()
            .map(|req| (fold_requirement_name(&req.name), req.name.clone()))
            .collect(),
        "ADDED",
    )?;
    in_section(
        delta
            .modified
            .iter()
            .map(|req| (fold_requirement_name(&req.name), req.name.clone()))
            .collect(),
        "MODIFIED",
    )?;
    in_section(
        delta
            .removed
            .iter()
            .map(|name| (fold_requirement_name(name), name.clone()))
            .collect(),
        "REMOVED",
    )?;

    let added_names: Vec<String> = delta.added.iter().map(|req| req.name.clone()).collect();
    let modified_names: Vec<String> = delta.modified.iter().map(|req| req.name.clone()).collect();
    let added: HashMap<String, &str> = fold_map(&added_names);
    let modified: HashMap<String, &str> = fold_map(&modified_names);
    let removed: HashMap<String, &str> = fold_map(&delta.removed);
    let mut renamed_from: HashMap<String, &str> = HashMap::new();
    let mut renamed_to: HashMap<String, &str> = HashMap::new();
    for rename in &delta.renamed {
        renamed_from.insert(fold_requirement_name(&rename.from), rename.from.as_str());
        renamed_to.insert(fold_requirement_name(&rename.to), rename.to.as_str());
    }
    // Duplicate FROM / TO sides (folded).
    if renamed_from.len() != delta.renamed.len() {
        return Err(ApplyFailure::incoherent(
            "duplicate FROM in RENAMED: each rename source may appear once".to_string(),
        ));
    }
    if renamed_to.len() != delta.renamed.len() {
        return Err(ApplyFailure::incoherent(
            "duplicate TO in RENAMED: each rename destination may appear once".to_string(),
        ));
    }
    // Cycles on the folded graph (duplicate sides already rejected
    // above, so the graph is well-formed here): no application order
    // can satisfy one.
    if let Some(cycle) = find_rename_cycle(&delta.renamed) {
        return Err(ApplyFailure::incoherent(format!(
            "rename cycle through \"### Requirement: {cycle}\": write the direct rename instead"
        )));
    }
    let conflict = |name: &str, first: &str, second: &str| {
        ApplyFailure::incoherent(format!(
            "requirement present in multiple sections ({first} and {second}) for header \"### Requirement: {name}\""
        ))
    };
    for (folded, display) in &added {
        if removed.contains_key(folded.as_str()) {
            return Err(conflict(display, "ADDED", "REMOVED"));
        }
        if modified.contains_key(folded.as_str()) {
            return Err(conflict(display, "ADDED", "MODIFIED"));
        }
    }
    for (folded, display) in &modified {
        if removed.contains_key(folded.as_str()) {
            return Err(conflict(display, "MODIFIED", "REMOVED"));
        }
    }
    for rename in &delta.renamed {
        let from_folded = fold_requirement_name(&rename.from);
        let to_folded = fold_requirement_name(&rename.to);
        if removed.contains_key(from_folded.as_str()) {
            return Err(conflict(&rename.from, "RENAMED", "REMOVED"));
        }
        if modified.contains_key(from_folded.as_str()) {
            return Err(ApplyFailure::incoherent(format!(
                "when a rename exists, MODIFIED must reference the NEW header \"### Requirement: {}\"",
                rename.to
            )));
        }
        if added.contains_key(to_folded.as_str()) {
            return Err(ApplyFailure::incoherent(format!(
                "RENAMED TO header collides with ADDED for \"### Requirement: {}\"",
                rename.to
            )));
        }
    }
    Ok(())
}

/// Builds a folded-name to display-name map over owned names.
fn fold_map(names: &Vec<String>) -> HashMap<String, &str> {
    let mut map = HashMap::new();
    for name in names {
        map.entry(fold_requirement_name(name))
            .or_insert(name.as_str());
    }
    map
}

/// Finds a rename cycle by following rename destinations.
/// The graph is built on folded names — the same discipline as
/// coherence and typo checks — so `A→b, B→A` cycles refuse exactly
/// like exact-string ones. Chains (`A→B→C`) stay legal; only true
/// cycles refuse, since no application order can satisfy them.
fn find_rename_cycle(renamed: &[crate::grammar::Rename]) -> Option<String> {
    let mut edges: HashMap<String, String> = HashMap::new();
    for rename in renamed {
        edges.insert(
            fold_requirement_name(&rename.from),
            fold_requirement_name(&rename.to),
        );
    }
    for rename in renamed {
        let start = fold_requirement_name(&rename.from);
        let mut visited: Vec<String> = vec![start.clone()];
        let mut current = fold_requirement_name(&rename.to);
        while let Some(next) = edges.get(&current) {
            if visited.contains(next) {
                return Some(rename.to.clone());
            }
            visited.push(current);
            current = next.clone();
        }
    }
    None
}

/// Builds a new durable document from a delta note: the note's `# `
/// title (or `title_fallback`), its `## Purpose` section when present,
/// and its `ADDED` blocks under a fresh `## ADDED Requirements` section.
fn build_new_document(
    delta_content: &str,
    delta: &crate::grammar::DeltaSpecification,
    title_fallback: &str,
) -> String {
    let title = delta_content
        .lines()
        .find(|line| line.starts_with("# ") && !line.starts_with("## "))
        .map_or_else(
            || format!("# {title_fallback}"),
            |line| line.trim_end().to_string(),
        );
    let mut sections: Vec<String> = Vec::new();
    if let Some(purpose) = extract_purpose_section(delta_content) {
        sections.push(format!("## Purpose\n\n{purpose}"));
    }
    let mut added = String::from("## ADDED Requirements\n");
    for req in &delta.added {
        added.push('\n');
        added.push_str(req.raw.trim_end());
        added.push('\n');
    }
    sections.push(added);
    let mut body = format!("{title}\n\n{}", sections.join("\n\n"));
    while body.ends_with("\n\n") {
        body.pop();
    }
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

/// Applies a pre-validated delta onto an existing target body:
/// `RENAMED → REMOVED → MODIFIED → ADDED` against the target block
/// map with exact-name matching, then recomposes preserving title,
/// prose, and order. Collects non-blocking warnings into `warnings`.
#[allow(clippy::too_many_lines)]
fn apply_onto_existing(
    delta_content: &str,
    delta: &crate::grammar::DeltaSpecification,
    target: &str,
    warnings: &mut Vec<String>,
    overwrite_collisions: bool,
) -> Result<AppliedDoc, ApplyFailure> {
    let blocks = parse_target_blocks(target);
    // Structural refusal before application: duplicate target names
    // (exact or folded) would apply partially while the twin
    // survives, so the target itself is invalid merge state.
    let mut seen_folded: HashMap<String, &str> = HashMap::new();
    for block in &blocks {
        let folded = fold_requirement_name(&block.name);
        if let Some(first) = seen_folded.insert(folded, block.name.as_str()) {
            return Err(ApplyFailure::dangling(format!(
                "target holds duplicate requirement \"{first}\" (also spelled \"{}\"); fix the target before merging",
                block.name
            )));
        }
    }
    // Exact-name map plus insertion-order slot list (renames keep
    // their slot under the new key).
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        by_name.entry(block.name.clone()).or_insert(index);
        order.push(block.name.clone());
    }
    // Chain links (from -> to) for final-state remerge resolution.
    let chain: HashMap<&str, &str> = delta
        .renamed
        .iter()
        .map(|rename| (rename.from.as_str(), rename.to.as_str()))
        .collect();
    // Unified span edits; ties at one start line apply reheaders
    // first so MODIFIED replacements (which already hold the new
    // header) win their span.
    let mut span_edits: Vec<(usize, SpanEdit)> = Vec::new();
    // Applied renames as (from, to) for removal resolution.
    let mut renamed: Vec<(String, String)> = Vec::new();
    // Fixpoint over declaration order: each pass applies every
    // rename whose source is present and whose destination is
    // vacant, and consumes renames whose chain already resolved.
    // This makes initial, reverse-declared, partial, and final
    // states converge identically instead of depending on listing
    // order; leftovers diagnose after the fixpoint settles.
    // No-op eligibility is decided on the ORIGINAL map before any
    // application: a rename whose source never existed and whose
    // chain final already exists was consumed by an earlier run.
    // (Evaluating it on the evolving map would misread names this
    // very run created as already-synced destinations.)
    let original: std::collections::HashSet<&str> = by_name.keys().map(String::as_str).collect();
    let mut pending: Vec<bool> = vec![true; delta.renamed.len()];
    for (index, rename) in delta.renamed.iter().enumerate() {
        if !original.contains(rename.from.as_str())
            && chain_final_in(&chain, &|name| original.contains(name), rename.to.as_str())
        {
            pending[index] = false;
        }
    }
    loop {
        let mut progressed = false;
        for (index, rename) in delta.renamed.iter().enumerate() {
            if !pending[index] {
                continue;
            }
            if !by_name.contains_key(&rename.from) {
                // Postcondition already holds: the source is absent
                // and the (possibly chained) destination is present —
                // whether it got there before this run (pre-pass) or
                // earlier in it. Consuming here (rather than erroring
                // "source not found") is what lets an intermediate-only
                // target converge: B→C applies first, then A→B resolves
                // against the arrived C.
                if chain_final_in(
                    &chain,
                    &|name| by_name.contains_key(name),
                    rename.to.as_str(),
                ) {
                    pending[index] = false;
                    progressed = true;
                }
                continue;
            }
            if by_name.contains_key(&rename.to) {
                continue;
            }
            if let Some(near) = near_miss(&by_name, &order, &rename.to, Some(&rename.from)) {
                return Err(typo_error("RENAMED", &rename.to, &near));
            }
            let slot = by_name.remove(&rename.from).expect("renamed source slot");
            by_name.insert(rename.to.clone(), slot);
            if let Some(key) = order.iter_mut().find(|key| *key == &rename.from) {
                *key = rename.to.clone();
            }
            span_edits.push((
                blocks[slot].start_line,
                SpanEdit::Reheader(format!("### Requirement: {}", rename.to)),
            ));
            renamed.push((rename.from.clone(), rename.to.clone()));
            pending[index] = false;
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    for (index, rename) in delta.renamed.iter().enumerate() {
        if !pending[index] {
            continue;
        }
        if by_name.contains_key(&rename.to) {
            return Err(ApplyFailure::dangling(format!(
                "RENAMED failed for \"### Requirement: {}\": target already exists",
                rename.to
            )));
        }
        if let Some(near) = near_miss(&by_name, &order, &rename.from, None) {
            return Err(typo_error("RENAMED", &rename.from, &near));
        }
        return Err(ApplyFailure::dangling(format!(
            "RENAMED failed for \"### Requirement: {}\": source not found",
            rename.from
        )));
    }

    for name in &delta.removed {
        // The literal name wins (covers plain removals and
        // already-synced renames, whose map key is the destination);
        // otherwise resolve through applied renames to the
        // pre-rename slot. Slots index `blocks` directly.
        let slot = by_name
            .remove(name)
            .or_else(|| rename_to_from(&renamed, name).and_then(|from| by_name.remove(&from)));
        if let Some(slot) = slot {
            let block = &blocks[slot];
            span_edits.push((block.start_line, SpanEdit::Delete(block.end_line)));
        } else {
            if let Some(near) = near_miss(&by_name, &order, name, None) {
                return Err(typo_error("REMOVED", name, &near));
            }
            warnings.push(format!(
                "REMOVED requirement \"{name}\" is not in the current target; treating it as already removed"
            ));
        }
    }

    for modified in &delta.modified {
        let Some(&slot) = by_name.get(&modified.name) else {
            if let Some(near) = near_miss(&by_name, &order, &modified.name, None) {
                return Err(typo_error("MODIFIED", &modified.name, &near));
            }
            return Err(ApplyFailure::dangling(format!(
                "MODIFIED failed for \"### Requirement: {}\": not found",
                modified.name
            )));
        };
        let current = &blocks[slot];
        let missing: Vec<&str> = current
            .scenarios
            .iter()
            .map(String::as_str)
            .filter(|scenario| {
                !modified
                    .scenarios
                    .iter()
                    .any(|updated| updated.name == **scenario)
            })
            .collect();
        if !missing.is_empty() {
            return Err(ApplyFailure::dangling(format!(
                "MODIFIED failed for \"### Requirement: {}\": current target contains scenario(s) not present in the modified block: {}. Refresh the delta note before merging to avoid dropping scenarios",
                modified.name,
                missing
                    .iter()
                    .map(|scenario| format!("\"{scenario}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if current.raw.trim_end() != modified.raw.trim_end() {
            let block = &blocks[slot];
            span_edits.push((
                block.start_line,
                SpanEdit::Replace(
                    modified.raw.lines().map(str::to_string).collect(),
                    block.end_line,
                ),
            ));
        }
    }

    let mut appended: Vec<Vec<String>> = Vec::new();
    for added in &delta.added {
        if let Some(&slot) = by_name.get(&added.name) {
            if blocks[slot].raw.trim_end() == added.raw.trim_end() {
                continue;
            }
            if overwrite_collisions {
                let block = &blocks[slot];
                warnings.push(format!(
                    "ADDED collision for \"### Requirement: {}\" resolved by overwrite under --force",
                    added.name
                ));
                span_edits.push((
                    block.start_line,
                    SpanEdit::Replace(
                        added.raw.lines().map(str::to_string).collect(),
                        block.end_line,
                    ),
                ));
                continue;
            }
            return Err(ApplyFailure::dangling(format!(
                "ADDED failed for \"### Requirement: {}\": target already holds it with different content",
                added.name
            )));
        }
        if let Some(near) = near_miss(&by_name, &order, &added.name, None) {
            return Err(typo_error("ADDED", &added.name, &near));
        }
        appended.push(added.raw.lines().map(str::to_string).collect());
    }

    if let Some(delta_purpose) = extract_purpose_section(delta_content) {
        match extract_purpose_section(target) {
            Some(target_purpose) if target_purpose == delta_purpose => {}
            Some(_) => warnings.push(
                "delta Purpose ignored; the target already has one (edit the target directly to change it)"
                    .to_string(),
            ),
            None => warnings.push(
                "delta Purpose ignored; the target has none (edit the target directly to add one)"
                    .to_string(),
            ),
        }
    }

    Ok(AppliedDoc {
        body: recompose(target, span_edits, appended),
        warnings: std::mem::take(warnings),
        skipped: false,
    })
}

/// One surgical edit at a 1-indexed line span. Ties at one start
/// line apply reheaders first so MODIFIED replacements (which
/// already hold the new header) win their span.
enum SpanEdit {
    /// Delete span lines plus one adjacent blank (seam cleanup);
    /// payload is the 1-indexed inclusive end line.
    Delete(usize),
    /// Replace span lines with new content; payload is the content
    /// plus the 1-indexed inclusive end line.
    Replace(Vec<String>, usize),
    /// Re-spell one header line.
    Reheader(String),
}

/// Finds a still-present target requirement that matches `wanted`
/// only after case/whitespace folding (and is not `exempt`): a
/// mistyped header, not a missing requirement. Returns the target's
/// own spelling. Exact hits never reach here (callers check those
/// first); folded collisions across delta sections are caught
/// earlier by pre-validation, so a hit here is always a genuine typo.
fn near_miss(
    by_name: &HashMap<String, usize>,
    order: &[String],
    wanted: &str,
    exempt: Option<&str>,
) -> Option<String> {
    let folded = fold_requirement_name(wanted);
    order
        .iter()
        .find(|key| {
            key.as_str() != wanted
                && Some(key.as_str()) != exempt
                && by_name.contains_key(key.as_str())
                && fold_requirement_name(key) == folded
        })
        .cloned()
}

fn typo_error(operation: &str, wanted: &str, near: &str) -> ApplyFailure {
    ApplyFailure::dangling(format!(
        "{operation} failed for \"### Requirement: {wanted}\": not found, but \"### Requirement: {near}\" exists; fix the header to match it exactly"
    ))
}

/// Resolves a removal name through applied renames: a removal
/// naming a rename destination addresses the pre-rename block.
fn rename_to_from(renamed: &[(String, String)], name: &str) -> Option<String> {
    renamed
        .iter()
        .find(|(_, to)| to == name)
        .map(|(from, _)| from.clone())
}

/// Follows rename links from `to` to the chain's final name and
/// reports whether that name is present: a fully applied chain
/// (or single rename) re-applies as a no-op. Cycles cannot reach
/// here (pre-validation rejects them); the bound is defensive.
fn chain_final_in(
    chain: &HashMap<&str, &str>,
    is_present: &dyn Fn(&str) -> bool,
    to: &str,
) -> bool {
    let mut destination = to;
    let mut guard = 0;
    while let Some(next) = chain.get(destination) {
        guard += 1;
        if guard > chain.len() {
            return false;
        }
        destination = next;
    }
    is_present(destination)
}

/// Recomposes the durable body from the target's lines with
/// byte-preservation discipline: the target is split on `\n` only
/// (untouched lines round-trip byte-identical, including `\r` and
/// deliberate blank spacing), span edits apply bottom-up, and only
/// local seams are tidied (one adjacent blank consumed on deletion,
/// section-body trimming on append). Inserted lines (replacements,
/// appends, re-spelled headers) adopt the target's EOL style.
/// Trailing-newline state mirrors the target. No global
/// normalization ever runs here.
fn recompose(
    target: &str,
    span_edits: Vec<(usize, SpanEdit)>,
    appended: Vec<Vec<String>>,
) -> String {
    let eol = if target.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines: Vec<String> = target.split('\n').map(str::to_string).collect();
    // Bottom-up so earlier line numbers stay valid; ties at one
    // start reheader first so MODIFIED replacements (which already
    // hold the new header) win their span.
    let mut edits = span_edits;
    edits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| span_edit_order(&a.1).cmp(&span_edit_order(&b.1)))
    });
    for (start, edit) in edits {
        match edit {
            SpanEdit::Reheader(header) => {
                lines[start - 1] = eol_terminated(header, eol);
            }
            SpanEdit::Replace(replacement, end) => {
                let replacement = eol_lines(replacement, eol);
                lines.splice(start - 1..end, replacement);
            }
            SpanEdit::Delete(end) => {
                let mut from = start - 1;
                let mut to = end;
                if lines.get(to).is_some_and(|line| line.trim().is_empty()) {
                    to += 1;
                } else if from > 0 && lines[from - 1].trim().is_empty() {
                    from -= 1;
                }
                lines.drain(from..to);
            }
        }
    }
    if !appended.is_empty() {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mask = mask_fenced_lines(&refs);
        append_blocks(&mut lines, eol_lines_many(appended, eol), eol, &mask);
    }
    lines.join("\n")
}

/// Sort key for same-start edits: reheaders before replacements.
fn span_edit_order(edit: &SpanEdit) -> u8 {
    match edit {
        SpanEdit::Reheader(_) => 0,
        SpanEdit::Replace(_, _) => 1,
        SpanEdit::Delete(_) => 2,
    }
}

/// Maps LF-normalized inserted lines onto the target's EOL style.
fn eol_lines(lines: Vec<String>, eol: &str) -> Vec<String> {
    eol_terminated_many(lines, eol)
}

fn eol_lines_many(blocks: Vec<Vec<String>>, eol: &str) -> Vec<Vec<String>> {
    blocks
        .into_iter()
        .map(|block| eol_terminated_many(block, eol))
        .collect()
}

fn eol_terminated_many(lines: Vec<String>, eol: &str) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| eol_terminated(line, eol))
        .collect()
}

fn eol_terminated(line: String, eol: &str) -> String {
    if eol == "\r\n" && !line.ends_with('\r') {
        format!("{line}\r")
    } else {
        line
    }
}

/// Appends new requirement blocks at the end of the target's
/// `## ADDED Requirements` section body (trailing blanks trimmed so
/// exactly one blank line separates the last existing block), or
/// creates the section at the end of the document when absent.
/// Section scans skip fenced lines through `mask`, so an example
/// `## ADDED Requirements` never captures insertion.
/// Separators adopt the target EOL style.
fn append_blocks(lines: &mut Vec<String>, appended: Vec<Vec<String>>, eol: &str, mask: &[bool]) {
    let section_at = lines.iter().enumerate().position(|(index, line)| {
        !mask[index]
            && line
                .strip_prefix("##")
                .filter(|rest| !rest.starts_with('#'))
                .is_some_and(|rest| rest.trim().eq_ignore_ascii_case("ADDED Requirements"))
    });
    let blank = blank_line(eol);
    let mut addition: Vec<String> = Vec::new();
    for block in appended {
        addition.push(blank.clone());
        addition.extend(block);
    }
    match section_at {
        Some(header) => {
            let mut end = header + 1;
            while end < lines.len() && !(is_section_header(&lines[end]) && !mask[end]) {
                end += 1;
            }
            // Insert before the existing separator/trailing entries
            // so deliberate spacing and terminal shape survive.
            let mut at = end;
            while at > header + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            let inserted = addition.len();
            lines.splice(at..at, addition);
            if at + inserted < lines.len() && !lines[at + inserted].trim().is_empty() {
                lines.insert(at + inserted, blank);
            }
        }
        None => {
            let mut at = lines.len();
            while at > 0 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            let mut insertion: Vec<String> = Vec::new();
            if at > 0 {
                insertion.push(blank);
            }
            insertion.push(format!("## ADDED Requirements{}", crlf_suffix(eol)));
            insertion.extend(addition);
            lines.splice(at..at, insertion);
        }
    }
}

/// Blank separator line in the target's EOL style.
fn blank_line(eol: &str) -> String {
    crlf_suffix(eol).to_string()
}

/// The `\r` that terminates a CRLF line body after splitting on `\n`.
fn crlf_suffix(eol: &str) -> &str {
    if eol == "\r\n" { "\r" } else { "" }
}

/// Reports `## <title>` section headers (exactly two hashes,
/// non-blank title).
fn is_section_header(line: &str) -> bool {
    line.strip_prefix("##")
        .filter(|rest| !rest.starts_with('#'))
        .is_some_and(|rest| !rest.trim().is_empty())
}
