## Context

nbspec was conceived against OpenSpec 0.x (see `nb-mcp-server` notebook
`ideas/2`) and revised after examining OpenSpec 1.x/OPSX (`ideas/4`): the 1.x
line introduced customizable workflow schemas, per-change metadata, and an
action-based artifact graph, which absorb several things nbspec would
otherwise have invented. Operator review of the first draft pushed the model
further: changes are fully notebook-resident, and the repository receives
only durable documents at merge time. What remains distinctly nbspec's:
notebooks as the sole home of in-flight changes, `nb` todos as live
execution status, and repository history free of proposal churn.

The `nb-api` crate (typed Rust interface to the `nb` CLI) is being extracted
from `nb-mcp-server` under the approved `extract-nb-api` change; nbspec is
its first external consumer.

## Goals / Non-Goals

- Goals:
  - Notebook data model generalized over OpenSpec 1.x schemas — nbspec
    reads workflow schema files (TOML), it does not hardcode artifact
    types.
  - Notebook-resident changes: the repository never holds in-flight change
    trees; review runs against deterministic scratch renders.
  - Drift-protected merge: durable documents transfer to configured targets
    with provenance, and hand-edited targets are never silently overwritten.
  - Validation loop whose diagnostics point at notes, not temp files.
  - Change tiers ride OpenSpec schema selection (a "light" schema with fewer
    artifacts needs no nbspec-side special casing).

- Non-Goals (deferred to later changes):
  - Merging MODIFIED deltas into existing specifications: nbspec-owned
    requirement-level merge semantics — deliberately ours, not delegated,
    since upstream's MODIFIED semantics are internally inconsistent
    (changelog claims partial updates; bundled schema instructions demand
    full-block copies). Slice 1 `merge` handles new documents only and
    errors on MODIFIED deltas.
  - Archive search (`nbspec search --archives` or similar) and archive
    retention policy. Merge-time archive *creation* is in scope (see
    Decisions); making the archives queryable comes later.
  - ADR authoring workflow; the `decisions/<adr>` namespace and
    `documentation/decisions` target are reserved in the default schema now
    so their later arrival is additive.
  - Fleet rollout; nbspec dogfoods in this repository first.

- Fast-follow (not a distant deferral):
  - **MCP surface.** Operator direction: an MCP server will quickly be
    necessary for the same reasons it is in `nb-mcp-server` — `nb` is
    stateful about notebook selection, and an MCP server can always target
    the correct notebook for a project; agent-harness shell tools also
    mangle Markdown backticks, which are foundational to technical
    documents. Slice 1 ships the CLI only, but the core functions are built
    behind a library boundary so the MCP layer wraps them rather than
    shelling out to nbspec itself.

## Decisions

- **Notebook-resident changes.** The repository never holds an in-flight
  change tree. `nbspec render` materializes to a scratch workspace for
  review (e.g., a generated diff fed to `difit`) and validation; `nbspec
  merge` is the only operation that writes to the repository, and it writes
  only durable documents and the change archive. Consequences: per-change
  `.openspec.yaml` has no repository home — schema selection lives in the
  meta note; and in-flight proposal history lives in the notebook's own git
  repository, not the project's.
- **Merge-time change archives.** By default (configurable off), `merge`
  also writes a compressed tarball of the change — the full rendered
  artifact tree plus the meta note and a snapshot of the `work` checklist —
  to a configured directory, default `documentation/archives/
  <change-id>.tar.zst`, intended to be Git LFS-tracked (merge warns when no
  `.gitattributes` rule covers the path). This keeps the complete change
  record in the project repository — opaque to `rg`, but recoverable and
  searchable by a future `nbspec search --archives` — and gives the
  meta-note SHA linkage a repo-side counterpart that survives notebook
  loss. Tarballs are deterministic (sorted entries, normalized metadata) so
  re-merges are byte-identical.
- **Layered TOML configuration.** nbspec settings are TOML (`general.toml`),
  per fleet convention — no YAML in the nbspec-owned configuration surface.
  Sources layer, lowest to highest precedence: embedded defaults, the
  user-global settings file (platform configuration directory, e.g.
  `~/.config/nbspec/general.toml` on XDG systems), and the per-project
  settings file (`.auxiliary/configuration/nbspec/general.toml` by
  default). The per-project directory is relocatable via the
  `NBSPEC_CONFIG_DIR` environment variable or the user-global
  `project_configuration_directory` setting. Workflow schemata live beside
  the project settings as `schemata/<name>/schema.toml` — same artifact
  data model as upstream `schema.yaml`, TOML serialization; the format
  divergence is free because the conformance oracle only ever sees
  materialized scratch trees.
- **No repository `openspec/` tree.** nbspec's default schema is embedded
  in the binary; forked or overriding schemas and project configuration
  live under nbspec-owned paths (`.auxiliary/configuration/nbspec/`). The
  upstream CLI runs only inside scratch fixture trees (the conformance
  oracle renders a self-contained `openspec/` root into a temp directory
  and runs with its working directory there) — necessarily so, since
  upstream 1.5 offers no root or config path override: `openspec/` and
  `openspec/config.yaml` are hardcoded constants, discovery walks up from
  the working directory, and the only indirection (the `store:` pointer)
  belongs to the excluded stores beta. The agentsmgr-managed `openspec`
  symlink is therefore a bootstrap-only artifact of this repository,
  removed when dogfooding starts.
- **Notebook namespace `proposals/<change-id>/`.** With the repository
  `openspec/` tree gone, the `openspec` name carries no continuity value in
  the notebook either and would wrongly imply the upstream layout.
  `proposals` follows the notebook convention of Latinate top-level folder
  names (`coordination`, `issues`, `reviews`). Note names within a change
  (`meta`, `work`) stay short; the convention binds folder names, not
  notes.
- **Tasks never materialize.** The `work` todo note is the live execution
  record, surfaced through `nbspec display`; there is no generated
  `tasks.md`. The checklist ends with the change.
- **Format compatibility, not binary dependency.** nbspec keeps the OpenSpec
  1.x requirement/scenario/delta grammar (as of the 1.4 parser rules) and
  the workflow schema mechanism (artifact list, `generates` paths,
  `requires` graph — serialized as TOML `schema.toml` in nbspec; upstream
  YAML schemas are a one-time conversion away), with no runtime dependency
  on the `openspec` binary. It deliberately diverges from the `spec-driven` default layout via
  a forked default schema (below) — divergence expressed through upstream's
  own customization mechanism. Rationale: nbspec must parse the grammar
  anyway for note-level diagnostics; upstream is churning (1.5 `stores` is
  "expect breaking changes") and is not a stable base to execute; and the
  supply chain stays small — nbspec's only runtime external is `nb` itself
  (via `nb-api`), and even that boundary may someday be replaced with native
  Rust behind typed models.
- **nbspec default schema.** A fork of `spec-driven` embedded in the nbspec
  binary (a bare repository needs zero nbspec files): artifacts are
  `proposal`, `specifications/<spec-name>`, `designs/<design-name>`, and
  `decisions/<adr>` (reserved); there is no tasks artifact. `generates` paths target configurable directories,
  defaulting to `documentation/specifications`, `documentation/designs`,
  and `documentation/decisions`. This also settles where durable designs
  live: `documentation/designs`, not scattered `src/**/README.md`.
- **CI conformance oracle, pinned and sunset-able.** An optional CI job runs
  upstream `openspec validate --strict` — pinned to a fixed version, not a
  floor — against fixtures rendered into the upstream `spec-driven` layout,
  continuously proving the grammar-level compatibility claim. Layout
  compatibility is explicitly not claimed. It is dev-time scaffolding:
  failures are informational, and the oracle retires if nbspec's grammar
  deliberately diverges.
- **Schema-driven generalization.** The artifact set, `generates` paths, and
  authoring order come from the resolved schema's `schema.toml`. This makes
  tiering an emergent property of schema selection (per-change via the meta
  note) instead of an nbspec feature, and makes the default layout a schema
  choice rather than machinery.
- **Notebook is the sole source of truth.** Merged documents carry
  provenance headers with content hashes; merge refuses to overwrite a
  drifted target without `--force`. Rule of thumb: hand edits belong in
  notes; merge targets are nbspec's to write.
- **Single repository-write path, no commits.** Only `merge` writes to the
  repository — durable documents and the change archive. `create`
  scaffolds nothing on the filesystem, and `render` targets scratch
  workspaces exclusively. `merge` never creates git commits: committing
  stays a human/agent act under the existing commit conventions; a future
  opt-in flag may add it. (Both promoted from open questions with operator
  agreement.)
- **Cross-repo atomicity is impossible and acknowledged.** Notebook (its own
  git repo) and project repo cannot commit atomically. Compensation: the
  meta note records project-repo commit SHAs at status transitions; the
  linkage is recoverable, not transactional.
- **`nb-api` via git dependency** pinned to the `nb-mcp-server` repository
  until the crate's first crates.io publish (which the extract-nb-api change
  guarantees no later than the first post-split server release), then a
  version dependency.
- **CLI skeleton with clap**, subcommand layout `nbspec change <verb>` and
  top-level `nbspec render|merge|validate`. Core operations live in library
  functions the CLI wraps thinly, keeping the fast-follow MCP surface a
  second thin wrapper rather than a rework.
- **Rendering is deterministic.** Same notes in, byte-identical tree out;
  content hashes make drift detection and review diffs trivial.

## Risks / Trade-offs

- **OpenSpec upstream format drift** (stores replacing workspace model,
  schema format evolution) → no runtime coupling to chase; the pinned CI
  oracle detects grammar drift, and nbspec chooses deliberately whether to
  follow. Schema parsing stays tolerant of unknown fields.
- **nb-api API instability pre-1.0** (stringly-typed returns; typed models
  are a planned follow-up) → nbspec keeps its own thin parsing layer
  initially and sheds it as typed accessors land upstream.
- **Hand edits to merge targets between merges** → provenance headers with
  content hashes detect drift; `nbspec display` reports it; `merge`
  refuses without `--force`. Drift pressure is lower than in the old
  write-tree model since targets change only at merge time.
- **Review without repository commits** — reviewers no longer get a proposal
  branch to `git show`. Mitigation: `render` is deterministic and emits a
  reviewable tree/diff (the operator's existing `difit --clean` flow works
  on rendered output); the meta note records review outcomes.
- **Todo-note formatting variance breaking status reporting** → define and
  validate a strict `work` note grammar; fail loudly on unparseable entries
  rather than misreporting progress.

## Migration Plan

Greenfield; no migration. Dogfooding begins with this change itself: once
slice 1 is implemented, this proposal's successor changes are authored
through nbspec. (This founding change is authored as a conventional OPSX
tree — the last of its kind in this repository.) When dogfooding starts,
the OPSX artifacts and the agentsmgr-managed `openspec` symlink are removed
from this repository. Acceptance condition for that transition: agentsmgr
today creates the symlink unconditionally on every `populate`, so before
removal, agents-common must provide — and this project must enable — a
project-level opt-out for OpenSpec tree/symlink population (or an
equivalent documented mechanism); a bare deletion would otherwise be
recreated by the next populate. Tracked at `agents-common:todos/template/10`.

## Open Questions

None at this time; earlier questions (repo writes, merge commits, notebook
namespace) were resolved into Decisions with operator input.
