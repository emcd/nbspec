//! Environment hygiene for spawning git-aware subprocesses.
//!
//! When a process is invoked from inside a Git hook (pre-commit,
//! pre-push, post-checkout, ...) or from a CI runner, Git exports a
//! set of repository-routing environment variables into the hook's
//! environment: `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`,
//! `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`,
//! `GIT_ALTERNATE_OBJECT_DIRECTORIES`. Every subprocess the hook
//! spawns inherits them.
//!
//! Any of these variables redirect every Git call inside the
//! subprocess away from the subprocess's expected repository.
//! Downstream tools layered over Git — for our purposes, `nb`, which
//! is a bash script wrapping Git — then act on the wrong repo:
//! `nb notebooks add` writes a scratch notebook into the parent
//! repo, the subsequent `nb notebooks` listing reads from a
//! different root, and the test fails in ways that depend on the
//! hook environment (CI vs. local vs. the same tests run outside
//! any hook).
//!
//! The fix is mechanical: before invoking any Git-aware subprocess
//! from inside a context that may be hooked or CI-driven, remove
//! every `GIT_*` variable from the child's environment. The child
//! then starts from a clean slate and resolves the repository from
//! its own `cwd` / its own arguments.
//!
//! See `nbspec:issues/4` for the original CI failure analysis and
//! the local repro (`GIT_DIR=<repo> GIT_INDEX_FILE=<repo>/index
//! cargo test --test integration`).

use std::process::Command;

/// Returns the names of every environment variable in the current
/// process whose name starts with `GIT_`. Exposed so other call
/// sites (e.g., a future tokio-process variant in `git_env`, or
/// any caller that wants the list without the scrub) share one
/// enumeration policy.
///
/// **Blast vs. selective — deliberate decision.** The `GIT_` prefix
/// blast also removes intent vars (`GIT_CONFIG_GLOBAL`,
/// `GIT_SSH_COMMAND`, `GIT_TERMINAL_PROMPT`, ...). Today no nbspec
/// code path consumes those; the only vars that redirect Git's
/// view of the repository are `GIT_DIR`, `GIT_INDEX_FILE`,
/// `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`, and
/// `GIT_ALTERNATE_OBJECT_DIRECTORIES`. A more selective policy
/// could enumerate exactly those. The blast is chosen for two
/// reasons: (1) any future `GIT_*` redirect that lands in this
/// range gets caught by default rather than requiring a code
/// change; (2) keeping the predicate to a prefix check is the
/// minimum surface to audit. Revisit if a container identity
/// mechanism ever routes through `GIT_CONFIG_GLOBAL` — at that
/// point a selective enumeration belongs here.
pub fn leaked_git_names() -> Vec<String> {
    std::env::vars()
        .filter_map(|(name, _)| {
            if name.starts_with("GIT_") {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

/// Removes every environment variable whose name starts with `GIT_`
/// from the given command's environment. The spawned process
/// inherits every other variable from the parent (PATH, HOME,
/// LANG, ...), just not the ones that redirect Git's view of the
/// repository.
///
/// Pass the command BEFORE chaining `.args(...)` or `.env(...)` so
/// later `.env(name, value)` calls are not also removed.
///
/// # Example
///
/// ```no_run
/// use std::process::Command;
/// nbspec::git_env::scrub_git_env(&mut Command::new("nb"));
/// ```
pub fn scrub_git_env(command: &mut Command) {
    for name in leaked_git_names() {
        command.env_remove(&name);
    }
}
