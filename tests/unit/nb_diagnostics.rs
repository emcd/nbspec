//! Observes pinned nb 7.24.0 diagnostics through nb-api's hermetic
//! [`NbTestEnv`] harness. Locks the absence shapes that
//! `operations::is_nb_missing_item` depends on, without mutating
//! process-global environment (which races under cargo test).

use nb_api::testing::NbTestEnv;
use nbspec::operations::is_nb_missing_item;

/// Strips CSI/Fe ANSI sequences the way operators see after nb-api
/// sanitization. Leaves C0 controls (nb emits SI after SGR reset).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(stdout));
    text.push_str(&String::from_utf8_lossy(stderr));
    strip_ansi(&text)
}

#[test]
fn show_note_missing_emits_pinned_not_found_for_selector() {
    let env = NbTestEnv::new().expect("NbTestEnv");
    let notebook = env.notebook();
    let selector = "proposals/add-demo/proposal.md";
    let qualified = format!("{notebook}:{selector}");

    let output = env
        .nb_command()
        .args(["show", &qualified, "--no-color"])
        .output()
        .expect("spawn nb show");
    assert!(!output.status.success(), "missing note must fail");
    let msg = combined_output(&output.stdout, &output.stderr);
    assert!(
        is_nb_missing_item(&msg, selector),
        "classifier must accept pinned diagnostic: {msg:?}"
    );
    assert!(is_nb_missing_item(&msg, &qualified));
}

#[test]
fn list_notes_missing_folder_emits_pinned_not_found() {
    let env = NbTestEnv::new().expect("NbTestEnv");
    let notebook = env.notebook();
    let folder = "proposals/add-demo/specifications";
    let qualified = format!("{notebook}:{folder}/");

    let output = env
        .nb_command()
        .args(["ls", &qualified, "--no-color"])
        .output()
        .expect("spawn nb ls");
    assert!(!output.status.success(), "missing folder must fail");
    let msg = combined_output(&output.stdout, &output.stderr);
    assert!(
        is_nb_missing_item(&msg, folder),
        "classifier must accept pinned folder diagnostic: {msg:?}"
    );
    assert!(is_nb_missing_item(&msg, &format!("{folder}/")));
}

#[test]
fn show_note_on_directory_is_not_classified_as_missing() {
    let env = NbTestEnv::new().expect("NbTestEnv");
    let notebook = env.notebook();

    // Materialize a directory where a note file is expected.
    let dir = env
        .nb_dir()
        .join(notebook)
        .join("proposals/demo/proposal.md");
    std::fs::create_dir_all(&dir).expect("create dir-as-note");

    let selector = "proposals/demo/proposal.md";
    let qualified = format!("{notebook}:{selector}");
    let output = env
        .nb_command()
        .args(["show", &qualified, "--no-color"])
        .output()
        .expect("spawn nb show");
    let msg = combined_output(&output.stdout, &output.stderr);
    assert!(
        !output.status.success() || msg.contains("folder") || msg.contains("directory"),
        "directory selector should not look like a normal note: status={} msg={msg:?}",
        output.status
    );
    assert!(
        !is_nb_missing_item(&msg, selector),
        "directory occupant must not classify as missing: {msg:?}"
    );
}

#[test]
fn compound_prose_with_embedded_token_is_not_absence() {
    let msg = "backend exploded: ! Not found: other.md while scanning";
    assert!(!is_nb_missing_item(msg, "proposals/x/proposal.md"));
    assert!(!is_nb_missing_item(
        "! Not found in scratch: query terms\n",
        "query terms"
    ));
}
