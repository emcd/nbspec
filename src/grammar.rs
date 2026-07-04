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
    let sections = split_top_level_sections(&lines);

    let added_section = find_section(&sections, "added requirements");
    let modified_section = find_section(&sections, "modified requirements");
    let removed_section = find_section(&sections, "removed requirements");
    let renamed_section = find_section(&sections, "renamed requirements");

    DeltaSpecification {
        added: added_section
            .map(|section| parse_requirement_blocks(&lines, section.body_start, section.body_end))
            .unwrap_or_default(),
        modified: modified_section
            .map(|section| parse_requirement_blocks(&lines, section.body_start, section.body_end))
            .unwrap_or_default(),
        removed: removed_section
            .map(|section| parse_removed_names(&lines[section.body_start..section.body_end]))
            .unwrap_or_default(),
        renamed: renamed_section
            .map(|section| parse_renamed_pairs(&lines[section.body_start..section.body_end]))
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
fn requirement_header_name(line: &str) -> Option<String> {
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

fn split_top_level_sections(lines: &[&str]) -> Vec<SectionSpan> {
    let mut headers: Vec<(usize, String)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
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
/// line numbers relative to the full line sequence.
fn parse_requirement_blocks(lines: &[&str], start: usize, end: usize) -> Vec<Requirement> {
    let mut requirements = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let Some(name) = requirement_header_name(lines[cursor]) else {
            cursor += 1;
            continue;
        };
        let header_index = cursor;
        cursor += 1;
        let body_start = cursor;
        while cursor < end
            && requirement_header_name(lines[cursor]).is_none()
            && section_title(lines[cursor]).is_none()
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
            scenarios: parse_scenarios(body, body_start),
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
fn parse_scenarios(body: &[&str], body_start: usize) -> Vec<Scenario> {
    let mut scenarios = Vec::new();
    let mut cursor = 0;
    while cursor < body.len() {
        let Some(name) = scenario_header_name(body[cursor]) else {
            cursor += 1;
            continue;
        };
        let header_index = cursor;
        cursor += 1;
        let content_start = cursor;
        while cursor < body.len() && !ends_scenario_body(body[cursor]) {
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
fn parse_removed_names(lines: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    for line in lines {
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
/// case-sensitive, matching the upstream parser.
fn parse_renamed_pairs(lines: &[&str]) -> Vec<Rename> {
    let mut pairs = Vec::new();
    let mut pending_from: Option<String> = None;
    for line in lines {
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
