//! Process-environment configuration tests.
//!
//! Unix-only: every scenario isolates the user-global configuration
//! file by redirecting home directories through the environment, which
//! is unsound on Windows where `directories` resolves Roaming AppData
//! via SHGetKnownFolderPath (no environment variable can redirect
//! it). The shared resolution logic itself is pinned on all platforms
//! by the explicit-seam unit tests in `tests/unit/schemata.rs`.
//!
//! Each scenario runs in a spawned subprocess (this same test binary,
//! filtered to the single child test) with a fully controlled
//! environment set via [`Command::env`]/[`env_remove`]. No test in
//! this binary mutates process environment in place: `unsafe`
//! in-test `set_var` cannot isolate siblings, and the real
//! user-global configuration file must never contaminate fixtures,
//! so every child gets a unique scoped home plus only the variables
//! the scenario names.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use nbspec::configuration::{
    CONFIGURATION_DIR_ENV, PROJECT_CONFIGURATION_DIR_DEFAULT,
    load_configuration,
};

/// Marker selecting the child leg: present only in spawned children.
const ISOLATED_VAR: &str = "NBSPEC_CONFIG_ENV_ISOLATED";

/// Neutral-home variables cleared or redirected in every child so the
/// real user-global configuration file cannot leak into fixtures.
const NEUTRAL_HOME_VARS: &[&str] = &[
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_DATA_HOME",
];

/// Runs the current test binary filtered to `child_test` in a child
/// process whose environment holds exactly `extra` on top of a unique
/// scoped home, then asserts the child passed. The home is unique per
/// scenario: a shared directory deleted recursively would race sibling
/// scenarios running in parallel.
fn run_isolated(child_test: &str, extra: &[(&str, Option<&str>)]) {
    let home = unique_absolute_temp_root("neutral-home");
    fs::create_dir_all(&home).unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut command = Command::new(executable);
    command
        .args(["--exact", child_test, "--nocapture"])
        .env(ISOLATED_VAR, "1")
        .env("HOME", &home)
        .env("USERPROFILE", &home);
    for variable in NEUTRAL_HOME_VARS {
        command.env_remove(variable);
    }
    for (key, value) in extra {
        match value {
            Some(text) => {
                command.env(key, text);
            },
            None => {
                command.env_remove(key);
            },
        }
    }
    let output = command.output().expect("spawn isolated child");
    assert!(
        output.status.success(),
        "isolated child {child_test} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(&home).unwrap_or(());
}

/// Reports whether this process is the isolated child leg.
fn is_child() -> bool {
    std::env::var(ISOLATED_VAR).is_ok()
}

fn unique_absolute_temp_root(label: &str) -> PathBuf {
    let unique = format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::env::temp_dir().join("nbspec-config-env").join(unique)
}

#[test]
fn declared_empty_variable_is_a_hard_refusal() {
    if !is_child() {
        run_isolated(
            "declared_empty_variable_is_a_hard_refusal",
            &[(CONFIGURATION_DIR_ENV, Some(""))],
        );
        return;
    }
    let root = unique_absolute_temp_root("config-env-declared-empty");
    fs::create_dir_all(&root).unwrap();
    let error = load_configuration(&root).unwrap_err();
    assert!(
        matches!(
            error,
            nbspec::configuration::ConfigurationError::Location(_)
        ),
        "zero-length {CONFIGURATION_DIR_ENV} must refuse, not silently vanish: {error:?}"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn relative_empty_variable_keeps_legacy_join() {
    if !is_child() {
        run_isolated(
            "relative_empty_variable_keeps_legacy_join",
            &[(CONFIGURATION_DIR_ENV, Some(""))],
        );
        return;
    }
    let root = PathBuf::from("some/relative/root");
    let configuration = load_configuration(&root).unwrap_or_else(|error| {
        panic!("relative empty variable keeps legacy behavior: {error:?}")
    });
    assert_eq!(configuration.project_directory, root);
}

#[test]
fn unusable_platform_base_falls_back_to_local_default() {
    if !is_child() {
        run_isolated(
            "unusable_platform_base_falls_back_to_local_default",
            &[
                ("HOME", Some("relative-home")),
                ("XDG_CONFIG_HOME", None),
                ("XDG_CACHE_HOME", None),
                (CONFIGURATION_DIR_ENV, None),
            ],
        );
        return;
    }
    let root = unique_absolute_temp_root("config-env-no-platform");
    fs::create_dir_all(&root).unwrap();
    let configuration = load_configuration(&root).unwrap_or_else(|error| {
        panic!(
            "unusable platform base must not block the local default: {error:?}"
        )
    });
    assert_eq!(
        configuration.project_directory,
        root.join(PROJECT_CONFIGURATION_DIR_DEFAULT)
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn declared_valid_directory_wins_over_defaults() {
    if !is_child() {
        let env_dir = unique_absolute_temp_root("config-env-elsewhere");
        fs::create_dir_all(&env_dir).unwrap();
        fs::write(
            env_dir.join(nbspec::configuration::SETTINGS_FILE),
            "schema = \"env-wins\"\n",
        )
        .unwrap();
        run_isolated(
            "declared_valid_directory_wins_over_defaults",
            &[(CONFIGURATION_DIR_ENV, Some(env_dir.to_str().unwrap()))],
        );
        fs::remove_dir_all(&env_dir).unwrap();
        return;
    }
    // Child leg: the parent pre-created the directory and passed it
    // through the environment; resolve it back out.
    let expected =
        PathBuf::from(std::env::var(CONFIGURATION_DIR_ENV).unwrap());
    let root = unique_absolute_temp_root("config-env-declared");
    fs::create_dir_all(&root).unwrap();
    let configuration = load_configuration(&root).unwrap();
    assert_eq!(configuration.schema.as_deref(), Some("env-wins"));
    assert_eq!(configuration.project_directory, expected);
    fs::remove_dir_all(&root).unwrap();
}
