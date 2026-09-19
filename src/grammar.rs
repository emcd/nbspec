//! OpenSpec grammar parsing: requirement, scenario, and delta structures.
//!
//! Follows the OpenSpec 1.x parser rules (upstream `requirement-blocks.ts`
//! and `markdown-parser.ts`): requirement headers `### Requirement: <name>`,
//! scenario headers `#### Scenario: <name>`, and delta sections
//! `## ADDED|MODIFIED|REMOVED|RENAMED Requirements`, all matched
//! case-insensitively. Parsed items carry 1-indexed line numbers within the
//! source text to support note-level diagnostics.

/// A parsed `#### Scenario:` block within a requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenario {
    /// Scenario name from the header line.
    pub name: String,
    /// Content lines following the header, up to the next header.
    pub body: String,
    /// 1-indexed line number of the scenario header.
    pub line: usize,
}

/// A parsed `### Requirement:` block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    /// Requirement name from the header line.
    pub name: String,
    /// Normative text: the first non-empty line before any nested header.
    pub text: Option<String>,
    /// Scenario blocks nested under the requirement.
    pub scenarios: Vec<Scenario>,
    /// Full block including the header line, trailing whitespace trimmed.
    pub raw: String,
    /// 1-indexed line number of the requirement header.
    pub line: usize,
}

/// A `FROM:`/`TO:` pair from a `## RENAMED Requirements` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    /// Previous requirement name.
    pub from: String,
    /// New requirement name.
    pub to: String,
}

/// One requirement block located in a merge-target document, with
/// its 1-indexed line span for surgical delta application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetBlock {
    /// Requirement name from the header line.
    pub name: String,
    /// Full block including the header line, trailing whitespace trimmed.
    pub raw: String,
    /// Scenario names nested under the requirement, in source order.
    pub scenarios: Vec<String>,
    /// 1-indexed line number of the requirement header.
    pub start_line: usize,
    /// 1-indexed line number of the block's last non-blank line, inclusive.
    pub end_line: usize,
}

/// Collects the addressable requirement blocks of a merge-target
/// document: blocks under `## ADDED` / `## MODIFIED Requirements`
/// sections. Headers under other sections (removal records,
/// `## Purpose` prose) are not merge state. Spans run from the
/// requirement header through the last non-blank line before the
/// next requirement header, section header, or end of file.
pub fn parse_target_blocks(content: &str) -> Vec<TargetBlock> {
    let normalized = normalize_line_endings(content);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = mask_fenced_lines(&lines);
    let sections = split_top_level_sections(&lines, &mask);
    let mut blocks = Vec::new();
    for section_name in ["added requirements", "modified requirements"] {
        let Some(section) = find_section(&sections, section_name) else {
            continue;
        };
        let mut cursor = section.body_start;
        while cursor < section.body_end {
            let Some(name) = (!mask[cursor])
                .then(|| requirement_header_name(lines[cursor]))
                .flatten()
            else {
                cursor += 1;
                continue;
            };
            let header_index = cursor;
            cursor += 1;
            while cursor < section.body_end
                && (mask[cursor]
                    || (requirement_header_name(lines[cursor]).is_none()
                        && section_title(lines[cursor]).is_none()))
            {
                cursor += 1;
            }
            let mut end_index = cursor;
            while end_index > header_index + 1 && lines[end_index - 1].trim().is_empty() {
                end_index -= 1;
            }
            let body = &lines[header_index + 1..cursor];
            let raw = std::iter::once(lines[header_index])
                .chain(body.iter().copied())
                .collect::<Vec<_>>()
                .join("\n")
                .trim_end()
                .to_string();
            blocks.push(TargetBlock {
                name,
                raw,
                scenarios: parse_scenarios(body, &mask[header_index + 1..cursor], header_index + 1)
                    .into_iter()
                    .map(|scenario| scenario.name)
                    .collect(),
                start_line: header_index + 1,
                end_line: end_index,
            });
        }
    }
    blocks
}

/// A `FROM:`/`TO:` line in a `## RENAMED Requirements` section that
/// never formed a pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnpairedRename {
    /// 1-indexed line number of the lone label line.
    pub line: usize,
    /// Which side is present without its counterpart.
    pub side: UnpairedSide,
    /// Requirement name carried by the lone line.
    pub name: String,
}

/// Which side of a rename pair stands alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnpairedSide {
    From,
    To,
}

/// Finds lone `FROM:`/`TO:` lines in the note's `## RENAMED
/// Requirements` section: a `TO:` with no pending `FROM:`, or a
/// trailing `FROM:` the section never answers. A well-formed delta
/// writes each rename as a `FROM:` line followed immediately by its
/// `TO:` line.
pub fn find_unpaired_renames(content: &str) -> Vec<UnpairedRename> {
    let normalized = normalize_line_endings(content);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = mask_fenced_lines(&lines);
    let sections = split_top_level_sections(&lines, &mask);
    let Some(section) = find_section(&sections, "renamed requirements") else {
        return Vec::new();
    };
    let mut unpaired = Vec::new();
    let mut pending_from: Option<(usize, String)> = None;
    for (offset, line) in lines[section.body_start..section.body_end]
        .iter()
        .enumerate()
    {
        let line_number = section.body_start + offset + 1;
        if mask[section.body_start + offset] {
            continue;
        }
        if let Some(name) = labeled_requirement_name(line, "FROM:") {
            if let Some((pending_line, pending_name)) = pending_from.take() {
                unpaired.push(UnpairedRename {
                    line: pending_line,
                    side: UnpairedSide::From,
                    name: pending_name,
                });
            }
            pending_from = Some((line_number, name));
        } else if let Some(name) = labeled_requirement_name(line, "TO:")
            && pending_from.take().is_none()
        {
            unpaired.push(UnpairedRename {
                line: line_number,
                side: UnpairedSide::To,
                name,
            });
        }
    }
    if let Some((pending_line, pending_name)) = pending_from.take() {
        unpaired.push(UnpairedRename {
            line: pending_line,
            side: UnpairedSide::From,
            name: pending_name,
        });
    }
    unpaired
}

/// Extracts a `## Purpose` section body (trimmed) from note or target
/// content, or `None` when absent or blank. Matched case-insensitively
/// like every other section title.
pub fn extract_purpose_section(content: &str) -> Option<String> {
    let normalized = normalize_line_endings(content);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = mask_fenced_lines(&lines);
    let sections = split_top_level_sections(&lines, &mask);
    let section = find_section(&sections, "purpose")?;
    let body = lines[section.body_start..section.body_end].join("\n");
    let trimmed = body.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Canonical fold for requirement-name comparison: lowercase with
/// interior whitespace collapsed. Exact matches apply; folded-only
/// matches diagnose typos (a header that differs only in case or
/// spacing is a real problem to report, mirroring upstream).
pub fn fold_requirement_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Reports delta-section kinds (`added/modified/removed/renamed
/// requirements`, case-insensitive) appearing more than once in the
/// note. Repeated sections leave operations silently ignored (only
/// the first binds), so pre-validation rejects them as incoherent.
pub fn duplicate_section_kinds(content: &str) -> Vec<String> {
    const DELTA_KINDS: [&str; 4] = [
        "added requirements",
        "modified requirements",
        "removed requirements",
        "renamed requirements",
    ];
    let normalized = normalize_line_endings(content);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = mask_fenced_lines(&lines);
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for section in split_top_level_sections(&lines, &mask) {
        if let Some(kind) = DELTA_KINDS
            .iter()
            .find(|kind| **kind == section.title_lowercase)
        {
            *counts.entry(kind).or_insert(0) += 1;
        }
    }
    let mut duplicated: Vec<String> = counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(kind, _)| kind.to_string())
        .collect();
    duplicated.sort();
    duplicated
}

/// Reports whether content carries any `## ... Requirements` section
/// header: the requirement structure surgical application needs as
/// a base. Matched case-insensitively over the full section title.
pub fn has_requirements_section(content: &str) -> bool {
    let normalized = normalize_line_endings(content);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = mask_fenced_lines(&lines);
    split_top_level_sections(&lines, &mask)
        .iter()
        .any(|section| {
            section.title_lowercase == "requirements"
                || section.title_lowercase.ends_with(" requirements")
        })
}

/// Presence of each delta section, independent of section content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SectionPresence {
    pub added: bool,
    pub modified: bool,
    pub removed: bool,
    pub renamed: bool,
}

/// A parsed delta specification (the content of one delta-spec note).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeltaSpecification {
    /// Requirements under `## ADDED Requirements`.
    pub added: Vec<Requirement>,
    /// Requirements under `## MODIFIED Requirements`.
    pub modified: Vec<Requirement>,
    /// Requirement names under `## REMOVED Requirements`.
    pub removed: Vec<String>,
    /// Rename pairs under `## RENAMED Requirements`.
    pub renamed: Vec<Rename>,
    /// Which delta sections appear in the source.
    pub presence: SectionPresence,
}

/// Parses a delta specification from note content.
///
/// Unrecognized lines are skipped rather than rejected, matching the
/// upstream parser; structural validation is a separate concern.
pub fn parse_delta_specification(content: &str) -> DeltaSpecification {
    let normalized = normalize_line_endings(content);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mask = mask_fenced_lines(&lines);
    let sections = split_top_level_sections(&lines, &mask);

    let added_section = find_section(&sections, "added requirements");
    let modified_section = find_section(&sections, "modified requirements");
    let removed_section = find_section(&sections, "removed requirements");
    let renamed_section = find_section(&sections, "renamed requirements");

    DeltaSpecification {
        added: added_section
            .map(|section| {
                parse_requirement_blocks(&lines, &mask, section.body_start, section.body_end)
            })
            .unwrap_or_default(),
        modified: modified_section
            .map(|section| {
                parse_requirement_blocks(&lines, &mask, section.body_start, section.body_end)
            })
            .unwrap_or_default(),
        removed: removed_section
            .map(|section| {
                parse_removed_names(
                    &lines[section.body_start..section.body_end],
                    &mask[section.body_start..section.body_end],
                )
            })
            .unwrap_or_default(),
        renamed: renamed_section
            .map(|section| {
                parse_renamed_pairs(
                    &lines[section.body_start..section.body_end],
                    &mask[section.body_start..section.body_end],
                )
            })
            .unwrap_or_default(),
        presence: SectionPresence {
            added: added_section.is_some(),
            modified: modified_section.is_some(),
            removed: removed_section.is_some(),
            renamed: renamed_section.is_some(),
        },
    }
}

/// A `## <title>` section located within a line sequence.
struct SectionSpan {
    title_lowercase: String,
    body_start: usize,
    body_end: usize,
}

/// Marks lines inside fenced code blocks so structural scans skip
/// example content: a `## ADDED Requirements` or
/// `### Requirement:` inside a fenced example is prose, never merge
/// state. CommonMark-ish: an opening run of 3+ backticks/tildes
/// (indented at most 3 spaces) opens; a run of the same character
/// with length >= the opening run and nothing but whitespace after
/// it closes. An unclosed fence masks to end of file (fail safe:
/// example content stays inert rather than becoming operations).
/// Line indices are stable — masking never adds or removes lines,
/// so diagnostics keep pointing at the right lines.
fn mask_fenced_lines(lines: &[&str]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut open: Option<(char, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        match fence_run(line) {
            Some((ch, len, rest)) => match open {
                None => {
                    open = Some((ch, len));
                    mask[index] = true;
                }
                Some((open_ch, open_len))
                    if ch == open_ch && len >= open_len && rest.trim().is_empty() =>
                {
                    mask[index] = true;
                    open = None;
                }
                _ => {
                    mask[index] = true;
                }
            },
            None => {
                if open.is_some() {
                    mask[index] = true;
                }
            }
        }
    }
    mask
}

/// Splits one line into a fence run `(char, length, rest)` when it
/// opens or participates in a code fence: at most 3 leading spaces,
/// then a run of 3+ backticks or tildes.
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
    let leading = line.bytes().take_while(|byte| *byte == b' ').count();
    if leading > 3 {
        return None;
    }
    let indented = &line[leading..];
    let mut chars = indented.chars();
    let ch = chars.next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = 1 + chars.take_while(|c| *c == ch).count();
    if len < 3 {
        return None;
    }
    Some((ch, len, &indented[len..]))
}

fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

/// Returns the title of a level-2 section header (`## <title>`), requiring
/// at least one whitespace character after the marker.
fn section_title(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("##")?;
    if rest.starts_with('#') || !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let title = rest.trim();
    if title.is_empty() { None } else { Some(title) }
}

/// Returns the name of a requirement header (`### Requirement: <name>`),
/// matched case-insensitively; whitespace after the marker is optional.
pub(crate) fn requirement_header_name(line: &str) -> Option<String> {
    header_name(line, "###", "requirement:")
}

/// Returns the name of a scenario header (`#### Scenario: <name>`),
/// matched case-insensitively; whitespace after the marker is optional.
fn scenario_header_name(line: &str) -> Option<String> {
    header_name(line, "####", "scenario:")
}

fn header_name(line: &str, marker: &str, keyword: &str) -> Option<String> {
    let rest = line.strip_prefix(marker)?;
    if rest.starts_with('#') {
        return None;
    }
    let rest = rest.trim_start();
    let rest = strip_prefix_ignore_ascii_case(rest, keyword)?;
    if rest.is_empty() {
        return None;
    }
    Some(rest.trim().to_string())
}

fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

fn split_top_level_sections(lines: &[&str], mask: &[bool]) -> Vec<SectionSpan> {
    let mut headers: Vec<(usize, String)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if mask[index] {
            continue;
        }
        if let Some(title) = section_title(line) {
            headers.push((index, title.to_ascii_lowercase()));
        }
    }
    let mut sections = Vec::with_capacity(headers.len());
    for (position, (index, title_lowercase)) in headers.iter().enumerate() {
        let body_end = headers
            .get(position + 1)
            .map_or(lines.len(), |(next_index, _)| *next_index);
        sections.push(SectionSpan {
            title_lowercase: title_lowercase.clone(),
            body_start: index + 1,
            body_end,
        });
    }
    sections
}

fn find_section<'a>(sections: &'a [SectionSpan], title_lowercase: &str) -> Option<&'a SectionSpan> {
    sections
        .iter()
        .find(|section| section.title_lowercase == title_lowercase)
}

/// Parses requirement blocks from `lines[start..end]`, reporting 1-indexed
/// line numbers relative to the full line sequence. Headers inside
/// fenced examples never open blocks; fenced lines inside a block
/// body stay part of the block verbatim.
fn parse_requirement_blocks(
    lines: &[&str],
    mask: &[bool],
    start: usize,
    end: usize,
) -> Vec<Requirement> {
    let mut requirements = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let Some(name) = (!mask[cursor])
            .then(|| requirement_header_name(lines[cursor]))
            .flatten()
        else {
            cursor += 1;
            continue;
        };
        let header_index = cursor;
        cursor += 1;
        let body_start = cursor;
        while cursor < end
            && (mask[cursor]
                || (requirement_header_name(lines[cursor]).is_none()
                    && section_title(lines[cursor]).is_none()))
        {
            cursor += 1;
        }
        let body = &lines[body_start..cursor];
        let raw = std::iter::once(lines[header_index])
            .chain(body.iter().copied())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string();
        requirements.push(Requirement {
            name,
            text: requirement_text(body),
            scenarios: parse_scenarios(body, &mask[body_start..cursor], body_start),
            raw,
            line: header_index + 1,
        });
    }
    requirements
}

/// Extracts the requirement's normative text: the first non-empty line
/// before any nested header within the block body.
fn requirement_text(body: &[&str]) -> Option<String> {
    for line in body {
        if line.trim_start().starts_with('#') {
            return None;
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Parses scenario blocks from a requirement body, reporting 1-indexed line
/// numbers relative to the full line sequence (`body_start` is 0-indexed).
/// `mask` parallels `body`: fenced scenario headers never open blocks,
/// but fenced lines inside a scenario stay part of its body verbatim.
fn parse_scenarios(body: &[&str], mask: &[bool], body_start: usize) -> Vec<Scenario> {
    let mut scenarios = Vec::new();
    let mut cursor = 0;
    while cursor < body.len() {
        let Some(name) = (!mask[cursor])
            .then(|| scenario_header_name(body[cursor]))
            .flatten()
        else {
            cursor += 1;
            continue;
        };
        let header_index = cursor;
        cursor += 1;
        let content_start = cursor;
        while cursor < body.len() && (mask[cursor] || !ends_scenario_body(body[cursor])) {
            cursor += 1;
        }
        let content = body[content_start..cursor].join("\n").trim().to_string();
        scenarios.push(Scenario {
            name,
            body: content,
            line: body_start + header_index + 1,
        });
    }
    scenarios
}

/// Reports whether a line terminates a scenario body: another scenario
/// header, or a markdown heading of level four or less. Deeper headings
/// (`#####` and beyond) remain inside the scenario body, matching the
/// upstream section parser, which only closes a section at a heading of
/// the same or higher level.
fn ends_scenario_body(line: &str) -> bool {
    if scenario_header_name(line).is_some() {
        return true;
    }
    let hashes = line.bytes().take_while(|&byte| byte == b'#').count();
    (1..=4).contains(&hashes) && line[hashes..].starts_with(char::is_whitespace)
}

/// Parses removed-requirement names: requirement headers, or bullet items of
/// the form ``- `### Requirement: <name>` `` (backticks optional).
/// Fenced lines never contribute names.
fn parse_removed_names(lines: &[&str], mask: &[bool]) -> Vec<String> {
    let mut names = Vec::new();
    for (line, masked) in lines.iter().zip(mask.iter()) {
        if *masked {
            continue;
        }
        if let Some(name) = requirement_header_name(line) {
            names.push(name);
            continue;
        }
        if let Some(name) = bullet_requirement_name(line) {
            names.push(name);
        }
    }
    names
}

fn bullet_requirement_name(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('-')?.trim_start();
    let rest = rest.strip_prefix('`').unwrap_or(rest);
    let name = requirement_header_name(rest.trim_end())?;
    Some(name.trim_end_matches('`').trim().to_string())
}

/// Parses rename pairs from `FROM:`/`TO:` lines (leading bullet markers and
/// backticks around the requirement header are optional). Labels are
/// case-sensitive, matching the upstream parser. Fenced lines never
/// contribute pairs.
fn parse_renamed_pairs(lines: &[&str], mask: &[bool]) -> Vec<Rename> {
    let mut pairs = Vec::new();
    let mut pending_from: Option<String> = None;
    for (line, masked) in lines.iter().zip(mask.iter()) {
        if *masked {
            continue;
        }
        if let Some(name) = labeled_requirement_name(line, "FROM:") {
            pending_from = Some(name);
        } else if let Some(name) = labeled_requirement_name(line, "TO:")
            && let Some(from) = pending_from.take()
        {
            pairs.push(Rename { from, to: name });
        }
    }
    pairs
}

fn labeled_requirement_name(line: &str, label: &str) -> Option<String> {
    let rest = line.trim_start();
    let rest = rest.strip_prefix('-').map_or(rest, str::trim_start);
    let rest = rest.strip_prefix(label)?.trim_start();
    let rest = rest.strip_prefix('`').unwrap_or(rest);
    let name = requirement_header_name(rest.trim_end())?;
    Some(name.trim_end_matches('`').trim().to_string())
}
