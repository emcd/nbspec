use nbspec::cli::failure_report;
use nbspec::operations::OperationError;
use nbspec::validation::{Diagnostic, ValidationFailure};

#[test]
fn validation_failure_reports_without_error_banner() {
    let error = OperationError::Invalid(ValidationFailure {
        change_id: "add-demo".to_string(),
        diagnostics: vec![
            Diagnostic {
                note: "proposals/add-demo/specifications/user-auth.md".to_string(),
                artifact_id: "specifications".to_string(),
                line: Some(5),
                message: "requirement User authentication has no #### Scenario: block".to_string(),
            },
            Diagnostic {
                note: "proposals/add-demo/proposal.md".to_string(),
                artifact_id: "proposal".to_string(),
                line: None,
                message: "required artifact has no authored content".to_string(),
            },
        ],
    });
    let report = failure_report(&error);
    assert!(!report.contains("Error:"));
    let lines: Vec<&str> = report.lines().collect();
    assert_eq!(lines[0], "change add-demo is invalid: 2 violations");
    assert_eq!(
        lines[1],
        "proposals/add-demo/specifications/user-auth.md:5: [specifications] \
         requirement User authentication has no #### Scenario: block"
    );
    assert_eq!(
        lines[2],
        "proposals/add-demo/proposal.md: [proposal] required artifact has no authored content"
    );
}

#[test]
fn other_failures_carry_the_error_banner() {
    let error = OperationError::AlreadyExists("add-demo".to_string());
    assert_eq!(
        failure_report(&error),
        "Error: change already exists: add-demo"
    );
}

#[test]
fn review_verb_parses_with_gate_default() {
    use clap::Parser;
    use nbspec::cli::{Cli, Command, VerdictArg};
    let cli = Cli::parse_from(["nbspec", "review", "add-demo", "--verdict", "approve"]);
    match cli.command {
        Command::Review {
            change_id,
            gate,
            verdict,
            comment,
            reviewer,
        } => {
            assert_eq!(change_id, "add-demo");
            assert_eq!(gate, "merge", "gate defaults to the slice-1 gate");
            assert_eq!(verdict, VerdictArg::Approve);
            assert_eq!(comment, None);
            assert_eq!(reviewer, None);
        }
        other => panic!("expected Review, got {other:?}"),
    }
}

#[test]
fn review_verb_requires_a_verdict() {
    use clap::Parser;
    use nbspec::cli::Cli;
    let result = Cli::try_parse_from(["nbspec", "review", "add-demo"]);
    assert!(result.is_err(), "verdict is a required argument");
}

#[test]
fn review_verb_accepts_comment_and_reviewer() {
    use clap::Parser;
    use nbspec::cli::{Cli, Command, VerdictArg};
    let cli = Cli::parse_from([
        "nbspec",
        "review",
        "add-demo",
        "--verdict",
        "revise",
        "--comment",
        "findings at reviews/9",
        "--reviewer",
        "Advisor",
    ]);
    match cli.command {
        Command::Review {
            verdict,
            comment,
            reviewer,
            ..
        } => {
            assert_eq!(verdict, VerdictArg::Revise);
            assert_eq!(comment.as_deref(), Some("findings at reviews/9"));
            assert_eq!(reviewer.as_deref(), Some("Advisor"));
        }
        other => panic!("expected Review, got {other:?}"),
    }
}
