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
