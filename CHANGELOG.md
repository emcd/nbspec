# Changelog

All notable changes to `nbspec` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and `nbspec` adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- **Inherited-environment hygiene**: ambient `GIT_*` env vars
  (leaked from git hooks, CI runners, or shell leakage) are
  scrubbed from every subprocess spawn site — the CLI dispatcher,
  the MCP server, and the test harness. Integration tests use
  per-test `NB_DIR`s so scratch notebooks no longer leak into the
  operator's real notebook root.

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
