//! First-create and first-review transaction-failure atomicity.
//!
//! Regression for `nbspec:reviews/7` P2: `create` and `review` must
//! enqueue their parent root folder (`proposals/`, `verdicts/`) into
//! the SAME transaction as the namespace / verdict note, so an injected
//! commit failure leaves no durable partial folder and no new
//! checkpoint.
//!
//! **Unix-only**: drives a real `nb` notebook through
//! [`nb_api::testing::NbTestEnv`] with the `NB_API_FAIL_AFTER_STAGE`
//! staging-failure injection (testing feature). The in-process
//! `operations::create` / `operations::review` calls spawn `nb` with
//! the inherited process environment, so the fixture's `NB_DIR`,
//! `HOME`, and `PATH` must be applied process-wide for the duration of
//! each test.

use std::ffi::OsString;
use std::process::Command;
use std::sync::Mutex;

use nb_api::testing::{NbTestEnv, fixture_child_path};
use nb_api::{Config, NbClient};

use nbspec::operations;
use nbspec::reviews::VerdictValue;

/// Serializes env-mutating tests: `nb` inherits process env, and these
/// tests temporarily point `NB_DIR`/`HOME`/`PATH` at the fixture.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const GIT_AUTHOR_NAME: &str = "nb-api tests";
const GIT_AUTHOR_EMAIL: &str = "nb-api@localhost";

/// Applies the fixture's environment (plus the failure-injection
/// switch, removed unless re-added by the test) to the process and
/// restores every touched variable on drop.
///
/// Every key is saved exactly ONCE (deduped) before any mutation, so
/// a key that `leaked_git_names` lists and that the explicit set also
/// names (e.g. `GIT_CONFIG_COUNT`) cannot be saved twice and then
/// wrongly removed on restore.
struct FixtureEnv {
    saved: Vec<(String, Option<OsString>)>,
}

impl FixtureEnv {
    fn apply(env: &NbTestEnv) -> Self {
        // Enumerate the full key set BEFORE any mutation, deduped:
        // all inherited `GIT_*` (mirrors `configure_std`'s scrub) plus
        // the fixture-controlled vars that may or may not exist in the
        // parent environment.
        let mut keys: Vec<String> = nbspec::git_env::leaked_git_names();
        keys.extend(
            [
                "NB_DIR",
                "HOME",
                "PATH",
                "NB_API_FAIL_AFTER_STAGE",
                "GIT_AUTHOR_NAME",
                "GIT_AUTHOR_EMAIL",
                "GIT_COMMITTER_NAME",
                "GIT_COMMITTER_EMAIL",
            ]
            .map(String::from),
        );
        keys.sort();
        keys.dedup();

        let saved: Vec<(String, Option<OsString>)> = keys
            .iter()
            .map(|key| (key.clone(), std::env::var_os(key)))
            .collect();

        // SAFETY: serialized under ENV_LOCK; test-only process env.
        unsafe {
            for key in &keys {
                std::env::remove_var(key);
            }
            std::env::set_var("PATH", fixture_child_path());
            std::env::set_var("NB_DIR", env.nb_dir());
            std::env::set_var("HOME", env.home_dir());
            std::env::set_var("GIT_AUTHOR_NAME", GIT_AUTHOR_NAME);
            std::env::set_var("GIT_AUTHOR_EMAIL", GIT_AUTHOR_EMAIL);
            std::env::set_var("GIT_COMMITTER_NAME", GIT_AUTHOR_NAME);
            std::env::set_var("GIT_COMMITTER_EMAIL", GIT_AUTHOR_EMAIL);
            std::env::set_var("GIT_CONFIG_COUNT", "2");
            std::env::set_var("GIT_CONFIG_KEY_0", "commit.gpgsign");
            std::env::set_var("GIT_CONFIG_VALUE_0", "false");
            std::env::set_var("GIT_CONFIG_KEY_1", "tag.gpgsign");
            std::env::set_var("GIT_CONFIG_VALUE_1", "false");
            std::env::remove_var("NB_API_FAIL_AFTER_STAGE");
        }
        FixtureEnv { saved }
    }
}

impl Drop for FixtureEnv {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            // SAFETY: serialized under ENV_LOCK; test-only process env.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Regression for reviews/10 P2: `FixtureEnv` must restore an inherited
/// `GIT_CONFIG_*` value exactly, even though `GIT_CONFIG_*` names both
/// appear in `leaked_git_names()` (and are removed by the scrub) and in
/// the explicit fixture key set. The deduped save list guarantees each
/// key is saved once and restored once, so a pre-existing
/// `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n` survives the guard.
#[test]
#[cfg(unix)]
fn fixture_env_restores_inherited_git_config() {
    let _lock = ENV_LOCK.lock().unwrap();
    let env = NbTestEnv::new().expect("NbTestEnv");

    let inherited_count = std::env::var("GIT_CONFIG_COUNT").ok();
    let inherited_key = std::env::var("GIT_CONFIG_KEY_0").ok();
    let inherited_value = std::env::var("GIT_CONFIG_VALUE_0").ok();
    // SAFETY: serialized under ENV_LOCK; test-only process env.
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "3");
        std::env::set_var("GIT_CONFIG_KEY_0", "inherited.setting");
        std::env::set_var("GIT_CONFIG_VALUE_0", "inherited-value");
    }

    let guard = FixtureEnv::apply(&env);
    assert_eq!(
        std::env::var("GIT_CONFIG_KEY_0").unwrap(),
        "commit.gpgsign",
        "fixture config must be active while the guard is held"
    );
    drop(guard);

    assert_eq!(
        std::env::var("GIT_CONFIG_COUNT").unwrap(),
        "3",
        "inherited GIT_CONFIG_COUNT must be restored exactly"
    );
    assert_eq!(
        std::env::var("GIT_CONFIG_KEY_0").unwrap(),
        "inherited.setting",
        "inherited GIT_CONFIG_KEY_0 must be restored exactly"
    );
    assert_eq!(
        std::env::var("GIT_CONFIG_VALUE_0").unwrap(),
        "inherited-value",
        "inherited GIT_CONFIG_VALUE_0 must be restored exactly"
    );

    // Restore the pre-test environment so a parallel run of the
    // lifecycle tests sees a clean slate.
    // SAFETY: serialized under ENV_LOCK; test-only process env.
    unsafe {
        match inherited_count {
            Some(value) => std::env::set_var("GIT_CONFIG_COUNT", value),
            None => std::env::remove_var("GIT_CONFIG_COUNT"),
        }
        match inherited_key {
            Some(value) => std::env::set_var("GIT_CONFIG_KEY_0", value),
            None => std::env::remove_var("GIT_CONFIG_KEY_0"),
        }
        match inherited_value {
            Some(value) => std::env::set_var("GIT_CONFIG_VALUE_0", value),
            None => std::env::remove_var("GIT_CONFIG_VALUE_0"),
        }
    }
}

/// Client bound to the fixture notebook by name.
fn client(env: &NbTestEnv) -> NbClient {
    NbClient::new(&Config {
        notebook: Some(env.notebook().to_string()),
        ..Config::default()
    })
    .expect("client construction is pure configuration")
}

/// Current `HEAD` of the fixture notebook's git repository, or `None`
/// when the repository has no commits yet.
fn notebook_head(env: &NbTestEnv) -> Option<String> {
    let mut command = Command::new("git");
    nbspec::git_env::scrub_git_env(&mut command);
    let output = command
        .current_dir(env.nb_dir().join(env.notebook()))
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A failed first create must leave no `proposals/` folder and must not
/// advance the notebook's checkpoint history.
#[test]
#[cfg(unix)]
fn first_create_failure_leaves_no_proposals_folder_or_checkpoint() {
    let _lock = ENV_LOCK.lock().unwrap();
    let env = NbTestEnv::new().expect("NbTestEnv");
    let _guard = FixtureEnv::apply(&env);
    let client = client(&env);
    let before = notebook_head(&env);

    // SAFETY: serialized under ENV_LOCK; test-only process env.
    unsafe { std::env::set_var("NB_API_FAIL_AFTER_STAGE", "1") };

    let result = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(operations::create(
            &client,
            Some(env.notebook()),
            "add-demo",
            None,
        ));
    assert!(
        result.is_err(),
        "create must fail under injected commit failure"
    );

    let proposals = env.nb_dir().join(env.notebook()).join("proposals");
    assert!(
        !proposals.exists(),
        "failed first-create must not leave proposals/ behind: {}",
        proposals.display()
    );
    assert_eq!(
        notebook_head(&env),
        before,
        "failed first-create must not create a new checkpoint"
    );
}

/// A failed first review must leave no `verdicts/` folder and must not
/// advance the notebook's checkpoint history.
#[test]
#[cfg(unix)]
fn first_review_failure_leaves_no_verdicts_folder_or_checkpoint() {
    let _lock = ENV_LOCK.lock().unwrap();
    let env = NbTestEnv::new().expect("NbTestEnv");
    let _guard = FixtureEnv::apply(&env);
    let client = client(&env);

    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(operations::create(
            &client,
            Some(env.notebook()),
            "add-demo",
            None,
        ))
        .expect("first create must succeed without injection");
    let before = notebook_head(&env);

    // SAFETY: serialized under ENV_LOCK; test-only process env.
    unsafe { std::env::set_var("NB_API_FAIL_AFTER_STAGE", "1") };

    let result = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(operations::review(
            &client,
            Some(env.notebook()),
            "add-demo",
            "merge",
            VerdictValue::Approve,
            Some("itest"),
            Some("ok"),
        ));
    assert!(
        result.is_err(),
        "review must fail under injected commit failure"
    );

    let verdicts = env
        .nb_dir()
        .join(env.notebook())
        .join("proposals")
        .join("add-demo")
        .join("verdicts");
    assert!(
        !verdicts.exists(),
        "failed first-review must not leave verdicts/ behind: {}",
        verdicts.display()
    );
    assert_eq!(
        notebook_head(&env),
        before,
        "failed first-review must not create a new checkpoint"
    );
}
