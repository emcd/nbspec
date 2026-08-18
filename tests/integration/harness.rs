//! Shared test harness for integration tests.
//!
//! The harness is built on nb-api's shipped [`NbTestEnv`] fixture
//! (`NB_DIR`, notebook, `HOME`, deterministic git identity, signing and
//! line-ending config) instead of hand-rolled isolation. On top of it,
//! each test gets a scratch project repository in the fixture's
//! `working_dir` — the cwd every spawned nbspec binary runs from — so
//! the resolved project root, and therefore every merge write, stays
//! inside the sandbox.
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
//!    any of these variables redirect every git call inside `nb` (and
//!    inside the nbspec binary, which resolves the project root via
//!    git) away from the intended repository. [`NbTestEnv`]'s
//!    `configure_std` / `configure_tokio` scrub them from every spawned
//!    command's environment; the same scrub is applied to the scratch
//!    repo's `git init`. See `src/git_env.rs` for the rationale.

use std::path::{Path, PathBuf};
use std::process::Command;

use nb_api::testing::NbTestEnv;

pub use nbspec::git_env::scrub_git_env;

/// Combines nb-api's hermetic [`NbTestEnv`] notebook fixture with a
/// scratch project repository rooted at the fixture's working
/// directory. Spawned nbspec binaries (and their internal `nb`
/// subprocesses) run with the fixture environment applied and
/// `working_dir` as cwd, so the project root resolves inside the
/// sandbox and merge writes never touch the operator's repository.
///
/// Drop removes the fixture's root tempdir (data store, notebook, and
/// working directory), so the operator's real notebook list and repo
/// are never touched.
pub struct Fixture {
    env: NbTestEnv,
    project_root: PathBuf,
}

impl Fixture {
    /// Builds the fixture: constructs the [`NbTestEnv`], then
    /// initializes a scratch git repository with a pinned project
    /// configuration at the fixture's working directory.
    pub fn new() -> Self {
        let env = NbTestEnv::new().expect("NbTestEnv");
        let project_root = env.working_dir().to_path_buf();
        init_scratch_project(&project_root);
        Self { env, project_root }
    }

    /// Applies the fixture's environment to a `std::process::Command`
    /// (scrubs `GIT_*`, sets `NB_DIR`/`HOME`/`PATH`, git identity,
    /// signing + line-ending config, and `current_dir` to the scratch
    /// project root).
    pub fn configure_std(&self, cmd: &mut Command) {
        self.env.configure_std(cmd);
    }

    /// Async counterpart to [`configure_std`](Self::configure_std).
    pub fn configure_tokio(&self, cmd: &mut tokio::process::Command) {
        self.env.configure_tokio(cmd);
    }

    /// The scratch notebook's name.
    pub fn notebook(&self) -> &str {
        self.env.notebook()
    }

    /// Filesystem path of the scratch notebook directory inside the
    /// fixture's isolated `NB_DIR`.
    pub fn notebook_path(&self) -> PathBuf {
        self.env.nb_dir().join(self.env.notebook())
    }

    /// Root of the scratch project repository (the fixture's
    /// `working_dir`).
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// The sandbox configuration directory, pinned through
    /// `NBSPEC_CONFIG_DIR` so a user-global
    /// `project_configuration_directory` cannot redirect it.
    pub fn configuration_directory(&self) -> PathBuf {
        self.project_root.join(".auxiliary/configuration/nbspec")
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

/// Initializes the scratch project: a git repository at the fixture's
/// working directory with a `general.toml` pinning every setting the
/// test depends on at the highest-precedence layer, so an operator's
/// user-global configuration cannot change test behavior.
fn init_scratch_project(root: &Path) {
    let mut command = Command::new("git");
    scrub_git_env(&mut command);
    let output = command
        .args(["init", "--quiet"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success(), "cannot initialize scratch repo");
    let configuration_directory = root.join(".auxiliary/configuration/nbspec");
    std::fs::create_dir_all(&configuration_directory).unwrap();
    std::fs::write(
        configuration_directory.join("general.toml"),
        "schema = \"nbspec-default\"\n\
         scratch_directory = \".auxiliary/temporary/renders\"\n\
         archives = true\n\
         archive_directory = \"documentation/archives\"\n",
    )
    .unwrap();
}
