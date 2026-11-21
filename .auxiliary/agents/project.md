# Project Guidance

Project-owned knowledge the generated `AGENTS.md` entrypoint must not own.
Structured tracking stays in `nb`.

## Purpose

Nbspec is a Rust CLI and MCP server for notebook-first OpenSpec orchestration. Change proposals (proposal text, specifications, designs, decisions, work checklists) live as `nb` notes; only durable documents (specifications, designs, decisions) and a compressed change archive reach the repository at merge. Review happens against deterministic scratch renders, never against the working tree. The change workflow follows the [OpenSpec 1.x](https://github.com/Fission-AI/OpenSpec) grammar; the canonical Nbspec procedure lives at `documentation/agents/nbspec.md`.

## Tech Stack

- **Language:** Rust 2024 edition, MSRV 1.98; `cargo` for build, test, and release.
- **Notebook backend:** `nb-api` 0.4.6 Rust client; `nb.sh@7.24.0` in CI.
- **Surfaces:** `clap` for the CLI grammar; `rmcp` 1.7 with `transport-io` for the MCP server; `tokio` async runtime.
- **Serialization:** `serde`, `serde_json`, `toml`, `schemars` (MCP JSON Schema).
- **Plumbing:** `anyhow`/`thiserror` (errors); `directories` (platform config); `jiff` (timestamps); `sha2` (provenance hashes); `similar` (unified diffs); `tar`/`zstd` (change archives).

## Nbspec Workflow

Consult `documentation/agents/nbspec.md` when a request touches an
existing normative specification, may require a new specification for
substantial work, explicitly mentions Nbspec or a proposal, or when the
operator requests formal planning. That file is the authoritative source
for applicability. Consulting it does not itself authorize creating a
proposal.

## Notebook Conventions (Nbspec Adaptations)

These specialize `@.auxiliary/agents/procedures/notebook.md` for this project:

- Tentative or pending design rationale lives in each change's
  `designs/<name>.md` note under `proposals/<change-id>/designs/`;
  completed (merged) designs are materialized onto the filesystem
  at `documentation/designs/<name>.md` by `nbspec merge`.
  Subsystem README files complement the materialized designs for
  hand-curated rationale. Avoid a separate top-level `designs/`
  notebook category because it creates an extra place to look that
  can go stale.
- Use `decisions/` only when the project wants optional durable
  rationale notes outside the Nbspec-managed change namespace or
  architecture README files.
- When an idea promotes to a formal Nbspec proposal, delete or
  archive the notebook draft so the change namespace under
  `proposals/` is the single source of truth for the change.
- Use **Nbspec proposals** for cross-cutting changes, contract-shaping
  work, architecture shifts, or work that needs explicit design
  discussion. When in doubt about whether work needs a Nbspec proposal
  or only `nb` execution tracking, prefer a proposal first for design
  clarity.
- Keep rolling handoff notes stable and update in place, separate from
  Nbspec-managed proposal content.
- Active change namespaces live under `proposals/<change-id>/`
  (scaffolded by `nbspec create`; carries proposal, specifications,
  designs, decisions, and `work` todos) and are the source of truth
  for in-flight changes. Durable documents materialize to
  `documentation/{specifications,designs,decisions}/` at merge time.

## Tests Development

- Bare `cargo test` is the fast hermetic suite: the live-nb tests are
  excluded by default (feature `live-nb`, never default, never
  transitively enabled). Shared xrepo gates run `--no-default-features`
  and stay ambient-tool-free by contract.
- Full maintainer validation: `cargo test-live` (alias for
  `cargo test --features live-nb --locked`) with pinned nb 7.24.0
  provisioned and `NB_API_TEST_NB` set — the same invocation as the
  project-owned CI gates. Live tests fail closed without nb by design;
  never add skip-when-absent.
- Prefer tests under `tests/unit` and `tests/integration` over inline `#[cfg(test)]` modules in `src/**`.
- Prefer tests that exercise public interfaces; avoid source-inclusion patterns used only to reach private internals.
- Inline `#[cfg(test)]` is permitted only when ALL of the following hold:
  1. The tested item is crate-private **by design** (not by oversight or laziness) and making it testable externally would require widening its visibility or adding a `#[doc(hidden)] pub` escape hatch that would itself become unintended API surface.
  2. No existing public interface exercises the same code path.
  3. The inline test block contains at most **one** `#[test]` function.
- If a candidate inline test fails any of these conditions, move it to `tests/unit` and widen visibility or restructure as needed. Do not default to inline to avoid that conversation; the friction is intentional.

## Notes

- Team org, role ownership, signoff policy, and merge workflow: `coordination/general/2`.
- Reviews go to `reviewer@notebook` unless a packet names another principal; review packets carry base/commit list/validation/contract/concerns. Release rule: CI green before tagging; version bump + changelog ride the release commit; merge --no-ff; never rewrite shared refs. Eric approves push/merge/tag.
- Custom MCP servers beyond the template set: `nbspec` (this repo's own server, via `cargo run --release -- serve mcp`). Keep it across template updates.
