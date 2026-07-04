use nbspec::worknotes::{WorkNoteError, parse_work_note};

const WORK_NOTE: &str = "\
# [ ] work

## Description

Execution checklist for add-demo.

## Tags

#nbspec

## Tasks

- [x] Design the thing.
- [ ] Build the thing.
- [ ] Test the thing.
";

#[test]
fn parses_title_and_items() {
    let checklist = parse_work_note(WORK_NOTE).unwrap();
    assert_eq!(checklist.title.as_deref(), Some("work"));
    assert!(!checklist.complete);
    assert_eq!(checklist.items.len(), 3);
    assert_eq!(checklist.progress(), (1, 3));
    assert_eq!(checklist.items[0].text, "Design the thing.");
    assert!(checklist.items[0].complete);
    assert_eq!(checklist.items[1].text, "Build the thing.");
    assert!(!checklist.items[1].complete);
}

#[test]
fn records_item_line_numbers() {
    let checklist = parse_work_note(WORK_NOTE).unwrap();
    let lines: Vec<usize> = checklist.items.iter().map(|item| item.line).collect();
    assert_eq!(lines, vec![13, 14, 15]);
}

#[test]
fn parses_completed_title() {
    let checklist = parse_work_note("# [x] work\n\n- [x] Everything.\n").unwrap();
    assert_eq!(checklist.title.as_deref(), Some("work"));
    assert!(checklist.complete);
    assert_eq!(checklist.progress(), (1, 1));
}

#[test]
fn accepts_indented_items() {
    let checklist = parse_work_note("- [ ] Parent.\n  - [x] Nested.\n").unwrap();
    assert_eq!(checklist.progress(), (1, 2));
    assert_eq!(checklist.items[1].text, "Nested.");
}

#[test]
fn accepts_bare_checkbox_without_text() {
    let checklist = parse_work_note("- [ ]\n").unwrap();
    assert_eq!(checklist.items.len(), 1);
    assert_eq!(checklist.items[0].text, "");
}

#[test]
fn ignores_prose_headers_and_plain_bullets() {
    let content = "\
# [ ] work

Some prose about [x] markers in running text.

## Section

- plain bullet without checkbox
* [x] star bullets are not nb tasks
";
    let checklist = parse_work_note(content).unwrap();
    assert!(checklist.items.is_empty());
}

#[test]
fn rejects_empty_brackets() {
    let error = parse_work_note("- [] broken\n").unwrap_err();
    let WorkNoteError::MalformedItem { line, content } = error;
    assert_eq!(line, 1);
    assert_eq!(content, "- [] broken");
}

#[test]
fn rejects_uppercase_check_marker() {
    let error = parse_work_note("- [ ] fine\n- [X] shouting\n").unwrap_err();
    let WorkNoteError::MalformedItem { line, .. } = error;
    assert_eq!(line, 2);
}

#[test]
fn rejects_missing_space_after_checkbox() {
    let error = parse_work_note("- [x]glued\n").unwrap_err();
    let WorkNoteError::MalformedItem { line, .. } = error;
    assert_eq!(line, 1);
}

#[test]
fn tolerates_missing_title() {
    let checklist = parse_work_note("- [ ] Untitled work.\n").unwrap();
    assert_eq!(checklist.title, None);
    assert_eq!(checklist.progress(), (0, 1));
}

#[test]
fn handles_crlf_line_endings() {
    let checklist = parse_work_note("# [ ] work\r\n\r\n- [x] Done.\r\n").unwrap();
    assert_eq!(checklist.title.as_deref(), Some("work"));
    assert_eq!(checklist.progress(), (1, 1));
    assert_eq!(checklist.items[0].text, "Done.");
}
