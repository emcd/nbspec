# Change: Establish nbspec foundation — notebook-first change authoring

## Why

OpenSpec keeps change artifacts as loose Markdown in the repository: drafts
churn through review as whole-file rewrites, historical proposal text pollutes
`rg` scans, and live execution status depends on hand-edited `tasks.md`
checkboxes. The `nb` notebook system already solves draft collaboration,
search, and todo tracking well. nbspec makes notebooks the sole home of
in-flight changes: proposals, delta specs, and task checklists live as notes
and todos; the repository receives only the durable documents — specifications,
designs, decisions — plus a compressed change archive when a change merges.
Proposal text never enters git history as loose Markdown, so the
`rg`-pollution problem disappears, while the archive keeps the full change
record recoverable and searchable by later tooling.

## What Changes

- Define the notebook data model for changes: a per-change folder namespace
  containing a `proposal` note, a `meta` control-plane note, a `work` todo
  checklist, and schema-defined artifact folders — by default
  `specifications/<spec-name>`, `designs/<design-name>`, and
  `decisions/<adr>` (reserved).
- Derive the artifact set for a change from an OpenSpec 1.x workflow schema
  (`schema.yaml` artifact list and dependency graph) rather than hardcoding
  artifact types. Ship an nbspec default schema (forked from `spec-driven`)
  that drops the tasks artifact and targets durable documents at
  configurable directories, defaulting to `documentation/specifications`,
  `documentation/designs`, and `documentation/decisions`.
- Implement `nbspec render`: deterministic materialization of a notebook
  change to a scratch workspace for review and validation — the repository
  working tree is never touched.
- Implement `nbspec merge`: transfer of a change's durable artifacts to
  their configured repository targets, with provenance stamping and drift
  refusal. Slice 1 handles new documents; merging MODIFIED deltas into
  existing specifications is deferred.
- Archive the change at merge (configurable, default on): a deterministic
  compressed tarball of the rendered change tree, meta note, and `work`
  checklist snapshot, written to a configured directory (default
  `documentation/archives/`) and intended for Git LFS — the change record
  stays recoverable while opaque to `rg`.
- Implement `nbspec validate`: native validation of the requirement,
  scenario, and delta grammar with note-level diagnostics — no runtime
  dependency on the `openspec` binary.
- Provide the CLI skeleton (`nbspec create`, `display`, `render`,
  `merge`, `validate` — flat verbs, matching the tool vocabulary planned
  for the MCP surface) over the `nb-api` crate.

Task checklists are never materialized: the `work` todo note is the live
execution record, surfaces through `nbspec display`, and simply ends
with the change. There is no generated `tasks.md`.

## Capabilities

### New Capabilities

- `change-authoring`: notebook data model and CLI verbs for creating and
  inspecting notebook-resident changes.
- `materialization`: rendering notebook changes to scratch workspaces for
  review, and merging durable artifacts into the repository with provenance
  and drift protection.
- `validation`: native OpenSpec-grammar validation with note-level
  diagnostics, plus a development-time conformance oracle proving
  grammar-level compatibility with upstream tooling.

### Modified Capabilities

None (greenfield project).

## Impact

- Affected code: new `nbspec` binary crate (this repository); depends on the
  `nb-api` crate being extracted from `nb-mcp-server` (git dependency until
  its first crates.io publish).
- No runtime dependency on the `openspec` CLI: nbspec keeps the OpenSpec
  requirement/scenario grammar and the `schema.yaml` mechanism, but
  deliberately diverges from the `spec-driven` default layout (no
  `tasks.md`; durable documents under `documentation/`). The upstream CLI
  appears only as an optional, version-pinned CI conformance oracle scoped
  to grammar-level fixtures.
- The repository never holds in-flight change trees; review happens against
  rendered scratch trees. Only merged durable documents and the change
  archive enter git history. The repository also carries no `openspec/`
  tree once nbspec dogfoods: the default schema is embedded in the binary,
  and the upstream CLI runs only inside scratch fixture trees.
- Deferred to later changes: merging MODIFIED deltas into existing
  specifications (nbspec-owned semantics), archive search
  (`nbspec search --archives`) and retention policy, the ADR authoring
  workflow (the `decisions/` namespace is reserved now), and the MCP
  surface (a named fast-follow, not a distant deferral — see design).
