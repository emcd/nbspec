# Nbspec Instructions

This project uses **Nbspec** (notebook-resident change orchestration) for
all in-flight changes. The notebook is the source of truth for proposals;
the repository receives only durable documents at merge time
(specifications, designs, decisions, archives). The upstream `openspec`
CLI appears only as an optional, version-pinned CI conformance oracle
scoped to grammar-level fixtures — it never holds in-flight proposals.

## Authoring workflow

Proposals live in the project notebook under `proposals/<change-id>/`,
scaffolded by `nbspec create <change-id> [--title ...]`. Author the
proposal, design, specifications (one note per capability under
`specifications/<capability>`), and the `work` todo checklist as
notebook notes. Review the deterministic materialization via
`nbspec render <change-id>`; the rendered tree is the reviewable
artifact. Transfer durable documents to the configured repository
targets via `nbspec merge <change-id>` with provenance and drift
protection.

There is no `nbspec/AGENTS.md`; the `nbspec` CLI and MCP server carry
the workflow surface, and the rendered output of `nbspec render` is
the canonical review form.

## CLI surface

- `nbspec create <change-id>` — scaffold a notebook change namespace.
- `nbspec display <change-id> [--full]` — show change state and artifacts.
- `nbspec render <change-id>` — deterministic scratch-tree render.
- `nbspec merge <change-id>` — transfer durable documents with
  provenance and drift protection.
- `nbspec validate <change-id>` — OpenSpec grammar validation with
  note-level diagnostics.
- `nbspec review <change-id>` — record a verdict against a rendered set.
- `nbspec import <path>` (v0.3.0, forthcoming) — filesystem → notebook
  change interchange (and legacy archive tree conversion to
  deterministic tar.zst).
- `nbspec export <change-id> <path>` (v0.3.0, forthcoming) — notebook →
  filesystem change interchange.
- `nbspec serve mcp` — start the MCP server (one tool per verb).

## MCP surface

`nbspec serve mcp` exposes one MCP tool per CLI verb. Tool descriptions
in the server schema carry the same conventions as this file; parameters
mirror the CLI flags (with the leading `--` dropped). Notebook context
is resolved by the server from the agent's working directory plus an
optional `--notebook` parameter.

## Conventions

- Change IDs are kebab-case, verb-led (`add-`, `update-`, `remove-`,
  `refactor-`).
- Every requirement uses SHALL/MUST and carries at least one
  `#### Scenario:` (exactly four hashtags).
- The `openspec/` directory is a bootstrap-only artifact (managed by
  agentsmgr; slated for removal in section 6.2 of the foundation
  change). New proposals never land there; the `openspec` CLI runs
  only in scratch fixture trees for grammar-level conformance checks.
- For rough ideas and early-stage proposals, use the notebook
  `ideas/` folder with the `#task-proposal` tag; convert to a formal
  proposal via `nbspec create` when ready for review.

Treat Nbspec proposals like code: `nbspec display <change-id> [--full]`
is the review surface (notebook content with state, work progress, and
per-artifact readiness). For filesystem-tooling reviewers,
`nbspec render <change-id>` emits a deterministic scratch tree and
`nbspec render <change-id> --diff` emits a unified diff between the
rendered change and its current merge targets — both are suitable
for tools like `difit` (the rendered output lives in a scratch
workspace and does not land in git history, so `git show` does not
apply). Authors amend the change via notebook edits; merge happens
via the project's documented workflow.
