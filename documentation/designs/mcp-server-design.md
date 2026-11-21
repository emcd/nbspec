<!-- nbspec: change=add-mcp-surface notebook=nbspec note=proposals/add-mcp-surface/designs/mcp-server-design.md hash=sha256:82f82b63de80583ba0e05fde60686df61b21ce203e4bc867b3a1e12cd7ae8a95 -->
# mcp-server-design

## Entry Point and Binary Layout

The MCP server launches as `nbspec serve mcp` — a subcommand on
the existing `nbspec` binary — rather than as a separately
shipped `nbspec-mcp` binary. Three reasons drove the choice:

1. **One implementation, one release artifact.** The MCP surface
   is a thin wrapper over the same operations library the CLI
   dispatches to; a second binary buys nothing technically and
   costs an extra release artifact plus the PATH-version skew
   that comes from coordinating two on-disk files. The release
   workflow packages `nbspec`; that single artifact covers
   everything.

2. **Verb-led register.** nbspec's CLI surface is imperative
   Latinate verbs (`create`, `display`, `render`, `validate`,
   `merge`). The MCP server is also imperative — it serves a
   long-running protocol session. The agentmux precedent uses
   `host mcp`; nbspec chose `serve mcp` because the verb-led
   register is the surface-level convention here. The pattern
   itself — long-running services nest under a service verb on
   the main binary — stays consistent with the fleet.

3. **Discoverability.** `nbspec --help` lists every verb
   including `serve`. Operators and harness authors discover the
   MCP surface from the same help output they already use; no
   separate `nbspec-mcp --help` exists to forget.

The `serve` subcommand inherits the global `--notebook` flag
from the parent `nbspec` CLI. Operators pass
`nbspec serve mcp --notebook <name>` to set the explicit
startup notebook; without the flag, the server falls back to
the git-derived name (see "Notebook Resolution Sources" below).
The MCP surface itself does NOT honor per-tool notebook
overrides — the resolved name is held for the server lifetime.

## Notebook Resolution Sources

Resolution happens once at startup. The configuration sources,
in priority order:

1. Explicit `notebook` configuration from server startup
   arguments or configuration file. This wins.
2. Git-derived default: the notebook name is the primary
   worktree's directory name, exactly as `operations.rs`
   resolves it today via `nb_api::derive_git_notebook_name`
   (which reads `git rev-parse --git-common-dir` and takes the
   parent). The server logs the derivation so the operator can
   see what happened.

Resolution is NOT re-attempted on tool call failure. If the
resolved notebook does not exist in `nb`, the server fails at
startup with a clear error. The resolved name is held for the
server lifetime; there is no per-tool override in v0.2.0.

Multi-notebook workflows run multiple server instances. This
is a deliberate boundary, not a limitation to grow out of:
one MCP client session operates against one notebook, and
operator tooling that needs to coordinate across notebooks
spawns additional server instances with different
configurations.

## Tool Handler Pattern

Each tool handler is a small function that takes a `&NbClient`
(plus the resolved notebook context) and calls the operations-
library entry point the CLI itself dispatches to. The CLI's
top-level verbs map directly to MCP tools:

- `create` tool → `operations::create(client, notebook, change_id, 
title)`
- `display` tool → `operations::display(client, notebook, change_id, 
full)`
- `validate` tool → `operations::validate(client, notebook, change_id)`
- `render` tool → `operations::render(client, notebook, change_id, 
diff)`
- `merge` tool → `operations::merge(client, notebook, change_id, 
force)`

These five entry points (operations.rs:106/171/275/314/436) are
the user-facing surface the CLI dispatches to; the lower-level
library functions (`validation::validate_change`,
`merging::merge_documents`, etc.) stay internal. The MCP layer
does not call the lower-level functions directly — that keeps
the operations library authoritative across both surfaces and
means future CLI/MCP shared logic (history formatting, drift
reporting, etc.) only has to land in one place.

## Structured Diagnostic Return

The `validate` tool's structured return carries each diagnostic
as a serialized `nbspec::validation::Diagnostic` (now
`#[derive(Serialize)]`). The wire-format field names are
`note`, `artifact_id`, `line`, `message` — identical to the
Rust struct fields, so drift between the structured payload
and the underlying data is structurally impossible.

`line` is `null` for required-artifact and document-level
failures where no line anchor exists. The text block in
`failure_report` is unchanged, so existing log scrapers keep
working.

Stable categorical codes (a fixed enumeration like
`required-artifact-empty`, `missing-normative-text`, etc.) are
tracked as a follow-up change at `nbspec:todos/3`. Until that
lands, MCP clients branch on `note`, `artifact_id`, and `line`
(which are stable) but not on diagnostic kind (which would
require parsing the `message` prose).

## Argument Validation Boundary

`clap` already validates the CLI args. The MCP server validates
independently (using `schemars` to derive JSON schemas from the
Rust types) so a malformed tool call fails with a structured
error at the protocol boundary, not deep inside operations.

## Out-of-Scope Surfaces

- MCPB bundle and MCP registry publish: handled by the same
  release workflow that ships the CLI binary, added to
  `releaser.yaml` as additional matrix legs.
- HTTP/SSE transport: stdio only for v0.2.0. SSE is a natural
  follow-up but requires MCP-side rate-limiting and
  request-routing work that is out of scope here.
- New operations beyond the five CLI verbs: `review` and
  `suggest` (from `ideas/1`) ship in later cycles.
