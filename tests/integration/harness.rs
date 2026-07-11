//! Shared test harness for integration tests.
//!
//! Concerns handled here, both about ambient state leaking into
//! spawned subprocesses and breaking the test contract in ways that
//! depend on the test runner:
//!
//! 1. **Git environment variables.** When the test runs inside a hook
//!    (pre-commit, pre-push) or a CI runner, git exports
//!    `GIT_DIR` / `GIT_INDEX_FILE` / `GIT_COMMON_DIR` /
//!    `GIT_WORK_TREE` / `GIT_OBJECT_DIRECTORY` /
//!    `GIT_ALTERNATE_OBJECT_DIRECTORIES` into the environment of every
//!    subprocess it spawns. `nb` is a bash script layered over git;
//!    any of these variables redirect every git call inside `nb`
//!    away from the notebook's repository and into the project
//!    repository (or a bare-repo error). The shared `scrub_git_env`
//!    helper (in `crate::git_env`) strips them from every spawned
//!    command's environment. Production code uses the same helper;
//!    see `src/git_env.rs` for the rationale.
//!
//! 2. **NB directory isolation.** Without per-test `NB_DIR`, scratch
//!    notebooks accumulate in the operator's real notebook list
//!    (filed as `nbspec:issues/5`). Each test creates a fresh temp
//!    `NB_DIR`; the scratch notebook lives there; the temp dir is
//!    removed on Drop. Tests never touch the real notebook root.
//!    Each `NB_DIR` is primed against nb's first-run interactive
//!    banner before any real subcommand runs.
//!
//! Per the diagnostic in `nbspec:issues/4` and Eric's directive on
//! `nbspec:issues/5`. See also: the hook-environment repro that
//! Advisor surfaced (pre-commit `cargo test` invocations fail
//! identically because hooks inherit the parent repo's `GIT_*`).

use std::path::{Path, PathBuf};
use std::process::Command;

pub use nbspec::git_env::scrub_git_env;

/// Removes every environment variable whose name starts with `GIT_`
/// from a tokio command's environment. See `scrub_git_env` (the
/// std-process counterpart in `crate::git_env`) for the rationale.
/// When nbspec grows a tokio-spawned git pathway in production,
/// move this function to `crate::git_env` and re-export from here.
pub fn scrub_git_env_async(command: &mut tokio::process::Command) {
    for name in nbspec::git_env::leaked_git_names() {
        command.env_remove(name);
    }
}

/// Returns a fresh per-test `NB_DIR` path. The directory is created;
/// the caller is responsible for cleanup (or wrap with
/// `IsolatedNbDir` to drop-clean). Lives under `std::env::temp_dir()`
/// (deliberate: see `prime_nb_dir` for the rationale on temp vs
/// `.auxiliary/temporary`).
///
/// Concurrency-safe: the path includes a per-process atomic sequence
/// so two calls within the same nanosecond from the same process
/// still get distinct paths. Without the sequence, the only entropy
/// was pid + nanoseconds, which can collide when two threads call
/// `IsolatedNbDir::new()` back-to-back. Mirrors the pattern in
/// `reviews::verdict_note_name`.
pub fn isolated_nb_dir_path() -> PathBuf {
    static SEQUENCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "nbspec-itest-nbdir-{}-{}-{sequence:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&path).expect("create isolated nb dir");
    path
}

/// Primes `nb` against a fresh `NB_DIR` by running one non-interactive
/// list command. Without this priming, the first interactive-aware
/// subcommand (`notebooks add`) is silently eaten by nb's first-run
/// banner — `nb` writes the welcome banner + REPL prompt to stdout
/// and exits 0 without executing the subcommand. Discovered via the
/// diagnostic in `nbspec:issues/4`; reproducible with any fresh
/// `NB_DIR` on `nb 7.24.0`.
///
/// **Ambient-state caveat.** On first invocation against any NB_DIR,
/// `nb` 7.24.0 also touches `${HOME}/.nbrc` — sourcing it if present,
/// or creating it via `_init_create_nb_dir` if absent. The prime
/// therefore mutates the operator's home directory exactly once per
/// test process that runs first against a fresh NB_DIR. CI's HOME
/// is ephemeral so this is a non-issue there; on a developer's
/// workstation, `.nbrc` will appear after the first test run. The
/// alternative — leaving `.nbrc` uncreated — leaves the notebook's
/// git config in a half-initialized state that breaks subsequent
/// commands, so this is the lesser evil. Long-term the right fix
/// is in nb: a `--no-rc-init` flag, or a `NB_RC_PATH=/dev/null`
/// escape hatch.
fn prime_nb_dir(path: &Path) {
    let mut command = Command::new("nb");
    scrub_git_env(&mut command);
    // The prime is best-effort: any exit code is acceptable, the
    // important thing is that nb touches the directory and lays
    // down its first-run config files.
    let _ = command
        .env("NB_DIR", path)
        .args(["notebooks", "--no-color"])
        .output();
}

/// RAII wrapper around an isolated `NB_DIR`. Drop best-effort removes
/// the directory; the per-test scratch notebook inside it is also
/// removed (the spawn sites do this explicitly so they can retry on
/// transient nb failures).
#[derive(Debug)]
pub struct IsolatedNbDir {
    path: PathBuf,
}

impl IsolatedNbDir {
    pub fn new() -> Self {
        let path = isolated_nb_dir_path();
        prime_nb_dir(&path);
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Default for IsolatedNbDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IsolatedNbDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
