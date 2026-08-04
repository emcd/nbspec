# Changelog

All notable changes to `nbspec` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and `nbspec` adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-08-04

### Fixed

- **Note selectors use full filenames**: `display` and related paths
  resolve artifact notes as `proposal.md` (and peers) rather than
  bare stems, so authored notes are not reported as ready-to-author
  when the H1 diverges from the filename stem, and cross-notebook
  display no longer fails selector resolution (issues/1, issues/8).
- **Read failures surface as unreadable**: note and folder artifact
  probes map only the pinned `nb` 7.24.0 missing-item diagnostic for
  the requested selector to absence; other failures report
  `unreadable` on short and `--full` display instead of empty/ready.
- **Timestamp note materialization**: document notes with nb-timestamp
  filenames materialize to kebab-case H1-derived slugs at render and
  merge; collisions on rendered path or durable target are refused
  with source diagnostics (issue/3).
- **Stale targets after H1 rename**: merge planning detects
  provenance-owned files left behind when an H1-derived slug changes
  and refuses (or announces under `--force`) rather than creating a
  second durable document silently.
- **Durable-path confinement**: merge planning and pre-write checks
  walk every path component with non-following metadata; symlink
  ancestors and live or dangling target-file symlinks are refused
  with no external effects. Non-directory parents fail during
  planning so multi-document merges write nothing on refusal.
- **Structural validation**: authored documents require an H1 outside
  fenced code and HTML comments; schema-declared `required_sections`
  (default proposal: `## Why`) are checked with a shared
  Markdown-context scanner that continues past non-matching H2s
  (issue/2).

## [0.2.0] - 2026-07-11

### Added

- **MCP server** (`nbspec serve mcp`): one tool per CLI verb —
  `create`, `display`, `validate`, `render`, `merge`, `review`.
  Each tool returns text plus a structured payload; agents branch
  on the structured form and fall back to the text. The notebook
  resolves once at startup (the CLI `--notebook` flag is inherited)
  and holds that notebook for the server lifetime — there is no
  per-tool override.
- **Review verb** (`nbspec review`): content-bound verdicts that
  gate `merge`. Each verdict is one immutable note under
  `proposals/<change-id>/verdicts/`; a newer verdict from the same
  reviewer supersedes their older one. Slice 1 ships a single
  `merge` gate (verdict of `approve` or `revise`).
- **Clean succession of merge targets**: when merging onto a target
  owned by another change whose body matches its recorded
  provenance, the takeover proceeds without `--force` and is
  announced loudly (the change that previously owned the target is
  named in the merge output). A foreign target that has drifted
  from its recorded provenance still requires `--force`, which
  overrides loudly and records the override.
- **Inherited-environment hygiene**: ambient `GIT_*` environment
  variables (leaked from git hooks, CI runners, or shell sessions)
  are now scrubbed from every spawned subprocess. Running `nbspec`
  from inside a git hook or CI context no longer redirects the
  underlying git operations to the wrong repository.

### Changed

- **Breaking**: `--comment -` no longer reads from stdin; it
  records the literal string `-`. Use `--comment-file -` to read
  from stdin. The MCP `review` tool always records the comment
  verbatim, including any literal `-`. (A former asymmetric
  CLI-vs-MCP affordance that did not survive contact with field
  use.)

## [0.1.0] - 2026-07-05

Initial release. Foundation: change authoring (`create`, `display`),
deterministic rendering with review diffs (`render`), drift-protected
merge with provenance headers and change archives (`merge`), and
native grammar validation (`validate`). OpenSpec 1.x grammar
compatibility proven end-to-end via a pinned upstream
`openspec validate --strict`.
