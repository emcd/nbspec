<!-- nbspec: change=add-mcp-surface notebook=nbspec note=proposals/add-mcp-surface/specifications/mcp-tool-surface.md hash=sha256:f2d3b4de578521c6bc99e77675bc70ee0fce83f9043067a1a8984898d3a34884 -->
# mcp-tool-surface

## ADDED Requirements

### Requirement: Tool surface parity with the CLI
The system SHALL expose one MCP tool per CLI verb: `create`,
`display`, `validate`, `render`, and `merge`.

#### Scenario: Every CLI verb has a corresponding tool
- **WHEN** a client lists the server's tools
- **THEN** it sees exactly the five tools above, named
  identically to the CLI verbs (e.g., `create`, `display`)
- **AND** the tool descriptions reference the corresponding CLI
  verb by name so a human reading tool metadata can map to the CLI

### Requirement: Entry point is `nbspec serve mcp`
The system SHALL be launched as a subcommand on the existing
`nbspec` binary — `nbspec serve mcp` — rather than a separate
`nbspec-mcp` binary. The subcommand SHALL inherit the global
`--notebook` flag from the parent `nbspec` CLI.

#### Scenario: Operator starts the server from a project checkout
- **WHEN** an operator runs `nbspec serve mcp` inside a git
  repository
- **THEN** the server listens on stdio and serves the five
  tools over the MCP protocol
- **AND** the inherited `--notebook` flag, when provided, takes
  precedence over the git-derived notebook name

#### Scenario: Single binary ships the MCP surface
- **WHEN** a release is published (e.g., `cargo install nbspec`)
- **THEN** the resulting binary exposes BOTH the existing CLI
  verbs (create, display, validate, render, merge) AND the
  `serve mcp` subcommand on the same on-disk artifact
- **AND** there is no separately-installed `nbspec-mcp` binary
  to coordinate

### Requirement: Parameter parity with the CLI
The system SHALL accept the same parameter names and semantics
as the corresponding CLI verbs, except where the MCP transport
mandates a different shape (e.g., flags become boolean or string
parameters per MCP convention).

#### Scenario: `render` accepts a `diff` boolean parameter
- **WHEN** a client calls `render` with `diff: true`
- **THEN** the server returns git-format unified diff output
  rather than the rendered file tree, matching
  `nbspec render --diff`
- **AND** the parameter defaults to `false` when omitted

#### Scenario: `merge` accepts a `force` boolean parameter
- **WHEN** a client calls `merge` with `force: true`
- **THEN** the server overwrites drifted, unmanaged, or
  foreign-ownership targets, matching `nbspec merge --force`
- **AND** the parameter defaults to `false` when omitted
- **AND** `force` SHALL NOT override unsupported-delta refusals
  (MODIFIED / REMOVED / RENAMED) or non-file occupants; those
  remain hard refusals as in the CLI

### Requirement: Notebook resolution held per server lifetime
The system SHALL resolve the project notebook once at server
startup and hold that notebook name for the lifetime of the
process. There SHALL be no per-tool notebook override.

#### Scenario: Server starts in a project checkout
- **WHEN** the server is launched inside a git repository
- **THEN** the resolved notebook name is derived from the
  primary worktree's directory name (the same derivation the
  CLI uses via `nb_api::derive_git_notebook_name`)
- **AND** the resolved notebook name is logged once at startup
- **AND** every tool call uses the resolved name without
  re-deriving it
- **AND** the resolved notebook is the ONLY notebook the server
  uses for every tool call. Multi-notebook workflows require
  running multiple server instances with different startup
  configurations.

#### Scenario: Notebook resolution is not retried on failure
- **WHEN** the resolved notebook does not exist in `nb`
- **THEN** the server SHALL fail at startup with a clear error
  message naming the missing notebook, rather than repeatedly
  attempting resolution on each tool call

#### Scenario: Explicit startup configuration wins over derivation
- **WHEN** the server is launched with `notebook` configured
  via the server's startup arguments or configuration file
- **THEN** the explicit name is used regardless of the
  git-derived default
- **AND** the resolved name is logged so the operator can verify
  what the server is using

### Requirement: Tool return type is text plus structured data
The system SHALL return both a human-readable text block and a
machine-parseable structured payload for every tool call.

#### Scenario: Validate returns diagnostics as structured data
- **WHEN** a client calls `validate` and the change has
  violations
- **THEN** the response includes a `diagnostics` array where
  each entry is a serialized `nbspec::validation::Diagnostic`
  with fields `note` (notebook note path), `artifact_id`
  (schema artifact), `line` (one-indexed within the note,
  `null` for required-artifact and document-level failures),
  and `message` (the violated rule, stated for humans and
  agents alike)
- **AND** the same data appears in the text block, formatted
  exactly as `failure_report` produces it today, so existing
  log scrapers keep working
- **AND** the line field, when present, is one-indexed against
  the source note
