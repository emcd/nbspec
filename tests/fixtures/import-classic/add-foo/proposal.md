# add-foo

## Why

Fixtures exercise the import → export round-trip; the proposal
text is intentionally free-form so the round-trip proof's
H1Rewrite normalization has work to do.

## What Changes

- A proposal note mapping back to itself through import/export.
- A spec with one ADDED requirement and one MODIFIED requirement
  so the import path's delta-warning code path fires.
- A design note that the export path collapses to `design.md`.
- A tasks file with both checked and unchecked items so the
  work-note parser exercises both checkbox states.
- A realistic decision record following the fleet convention
  (`clone-topology-decisions.md`) AND an arbitrary-named file
  (`20260712-foo.md`) so the name-agnostic claim is proven.