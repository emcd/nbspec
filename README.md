# nbspec

Notebook-first OpenSpec orchestration for spec-driven development.

`nbspec` makes [nb](https://github.com/xwmx/nb) notebooks the sole home of
in-flight change proposals: proposal text, delta specifications, and design
notes live as notes; execution status lives as a todo checklist. The
repository never holds an in-flight change tree — review happens against
deterministic scratch renders, and only durable documents (specifications,
designs, decisions) plus a compressed change archive enter git history at
merge. Proposal churn never pollutes `rg` scans of the repository.

`nbspec` keeps the [OpenSpec](https://github.com/Fission-AI/OpenSpec)
requirement/scenario grammar and workflow-schema mechanism (serialized as
TOML) with no runtime dependency on the `openspec` binary.

## Installation

`nbspec` is published to crates.io as a binary crate:

```sh
cargo install nbspec
```

Prerequisite: [`nb`](https://github.com/xwmx/nb) installed and on
`$PATH`. `nbspec` orchestrates around `nb` but does not replace it;
the project notebook holds proposal artifacts (proposal text,
specifications, designs, decisions, verdicts) and `nbspec` reads and
writes them through the `nb` CLI. CI uses `nb.sh@7.24.0`; older and
newer 7.x releases should work but are not exercised in our test
matrix.

For unreleased versions (from `master` or a branch), install from the
repository:

```sh
cargo install --git https://github.com/emcd/nbspec
```

## Status

**[v0.2.0](CHANGELOG.md#020---2026-07-11)** released 2026-07-11 — see
[CHANGELOG.md](CHANGELOG.md) for full release notes.

The verbs in the [Usage](#usage) section below are all implemented:
change authoring, rendering with review diffs, drift-protected
merge, native grammar validation, content-bound review gating, and
an MCP server mirroring the CLI.

OpenSpec 1.x grammar compatibility is verified end-to-end against a
pinned upstream `openspec validate --strict`; no runtime dependency
on the `openspec` binary.

## Usage

All commands operate on the project notebook, derived from the git remote
by default or named explicitly with `--notebook <name>`.

```sh
# Scaffold a change namespace (proposals/add-foo/) in the notebook:
# meta control-plane note, work todo checklist, artifact notes and folders.
nbspec create add-foo --title "Add the foo capability"

# Status view: metadata, artifact readiness against the schema dependency
# graph, work checklist progress, merge-target drift.
nbspec display add-foo
nbspec display add-foo --full   # adds note contents and folder listings

# Render the change to a scratch tree (never the repository working tree).
nbspec render add-foo

# Emit only a git-format diff against current merge targets — pipes
# straight into review tooling.
nbspec render add-foo --diff | difit --clean

# Record a review verdict bound to the change's CURRENT rendered
# content: any subsequent edit stales it. Each verdict is one immutable
# note under proposals/add-foo/verdicts/; a newer verdict from the same
# reviewer supersedes their older one. Revise verdicts require a
# findings comment: --comment takes literal content; --comment-file
# reads a file, or stdin when the path is - (the two conflict).
nbspec review add-foo --verdict approve
nbspec review add-foo --verdict revise --comment "findings at reviews/9"
nbspec review add-foo --verdict revise --comment-file findings.md
nbspec render add-foo --diff | summarize | nbspec review add-foo --verdict revise --comment-file -

# Transfer durable documents to their configured repository targets with
# provenance headers, and write the change archive. Merge REFUSES
# without a current approving verdict (no verdict, stale approval,
# outstanding revise, or an unparseable verdict note all refuse;
# --force overrides the review gate, loudly). Hand-edited targets
# refuse without --force; a refused merge writes nothing. A target
# inherited from an earlier change succeeds cleanly when it still
# matches its recorded provenance (the takeover is announced, never
# silent); a foreign target that drifted refuses without --force.
# Verdict notes ride the change archive and never materialize to the
# repository.
nbspec merge add-foo

# Native OpenSpec-grammar validation, no external binary. Exits zero
# with a one-line summary when valid; otherwise exits nonzero, with a
# summary line and one "note:line: [artifact] message" diagnostic per
# line on stderr, each anchored to a notebook note rather than a
# filesystem path.
nbspec validate add-foo
```

Authoring happens with ordinary `nb` tooling: edit
`proposals/<change-id>/proposal`, add specification notes under
`proposals/<change-id>/specifications/`, and check off work items with
`nb tasks do`.

## MCP Server

`nbspec serve mcp` starts a Model Context Protocol server on stdio that
exposes one tool per CLI verb (`create`, `display`, `validate`,
`render`, `merge`, `review`). The notebook resolves once at startup (the
`--notebook` flag is inherited from the parent CLI) and holds that
notebook for the server lifetime — there is no per-tool override.

```sh
# Start the MCP server. Notebook resolves from --notebook, falling back
# to the git-derived project name.
nbspec serve mcp --notebook myproject

# Or, when run inside the project's git checkout, let the server
# derive the notebook name from the working directory.
nbspec serve mcp
```

Register the server with an MCP-aware client (Claude Desktop, OpenCode,
etc.) by adding an entry to its `mcpServers` configuration:

```json
{
  "mcpServers": {
    "nbspec": {
      "command": "nbspec",
      "args": ["serve", "mcp"]
    }
  }
}
```

The `validate` tool returns text plus structured diagnostics: on
success, `{ valid: true, change_id, documents_checked, schema }`; on
failure, `{ valid: false, change_id, diagnostics: [...] }` where each
entry carries `note`, `artifact_id`, `line` (nullable), and `message`.
Clients branch on the structured payload; agents that prefer text can
still scrape the conventional `note:line: [artifact] message` lines.

## Configuration

Settings are TOML (`general.toml`) and layer, lowest to highest
precedence: embedded defaults, the user-global file (platform
configuration directory, e.g. `~/.config/nbspec/general.toml`), and the
per-project file (`.auxiliary/configuration/nbspec/general.toml`; the
directory is relocatable via `NBSPEC_CONFIG_DIR` or the user-global
`project_configuration_directory` setting).

| Setting | Default | Purpose |
|---------|---------|---------|
| `schema` | embedded `nbspec-default` | Workflow schema for changes that do not name one |
| `scratch_directory` | platform cache directory | Where rendered change trees land |
| `archives` | `true` | Whether merge writes a change archive |
| `archive_directory` | `documentation/archives` | Repository directory receiving archives (Git LFS recommended) |

Workflow schemata (artifact sets, dependency graphs, merge targets)
follow the OpenSpec 1.x data model as `schemata/<name>/schema.toml`
beside the project settings. The default schema ships proposal,
specifications, designs, and reserved decisions artifacts targeting
`documentation/{specifications,designs,decisions}` — and no tasks
artifact: the `work` todo note is the live execution record and ends
with the change.

## Motivation

- Proposal drafting and review generate heavy token churn when historical
  text lives loose in the repository; notebooks keep drafts searchable and
  structured without polluting `rg` scans.
- `nb` todo checklists are a better live execution tracker than
  hand-edited `tasks.md` checkboxes; `nbspec` surfaces them through
  `display` and never materializes them.
- OpenSpec 1.x workflow schemas define what artifacts a change carries;
  `nbspec` generalizes over schemas rather than hardcoding artifact types.

## License

Apache-2.0
