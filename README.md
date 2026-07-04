# nbspec

Notebook-first OpenSpec orchestration for spec-driven development.

`nbspec` keeps OpenSpec change authoring and collaboration in [nb](https://github.com/xwmx/nb)
notebooks — notes as proposal artifacts, todo checklists as canonical execution
status — while materializing a fully compatible filesystem tree for the
[OpenSpec](https://github.com/Fission-AI/OpenSpec) CLI to validate, sync, and
archive.

## Status

Pre-implementation. The founding change proposal lives under
`openspec/changes/`; the shared `nb-api` crate it builds on is being extracted
from [nb-mcp-server](https://github.com/emcd/nb-mcp-server).

## Motivation

- Proposal drafting and review generate heavy token churn when historical
  text lives loose in the repository; notebooks keep drafts searchable and
  structured without polluting `rg` scans.
- `nb` todo checklists are a better live execution tracker than hand-edited
  `tasks.md` checkboxes, but OpenSpec tooling expects the files; `nbspec`
  bridges the two.
- OpenSpec 1.x workflow schemas define what artifacts a change carries;
  `nbspec` generalizes over schemas rather than hardcoding artifact types.

## License

Apache-2.0
