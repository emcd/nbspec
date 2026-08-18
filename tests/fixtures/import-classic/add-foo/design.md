# main

## Context

The fixture exercises the export path's collapse of one design
note into the canonical `design.md` filename.

## Decisions

- The export path picks the first design note alphabetically
  and writes it as `design.md`. Additional notes under
  `designs/` are dropped from the round-trip.