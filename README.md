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

## Status

Implemented: change authoring (`create`, `display`), deterministic
rendering with review diffs (`render`), drift-protected merge with
provenance headers and change archives (`merge`), native grammar
validation (`validate`). Pending: integration test suite, dogfooding
transition.

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

# Transfer durable documents to their configured repository targets with
# provenance headers, and write the change archive. Hand-edited targets
# refuse without --force; a refused merge writes nothing.
nbspec merge add-foo

# Native OpenSpec-grammar validation, no external binary. Exits zero
# with a one-line summary when valid; otherwise exits nonzero with one
# "note:line: [artifact] message" diagnostic per line, each anchored
# to a notebook note rather than a filesystem path.
nbspec validate add-foo
```

Authoring happens with ordinary `nb` tooling: edit
`proposals/<change-id>/proposal`, add specification notes under
`proposals/<change-id>/specifications/`, and check off work items with
`nb tasks do`.

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
