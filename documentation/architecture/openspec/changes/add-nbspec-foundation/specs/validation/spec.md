## ADDED Requirements

### Requirement: Native change validation
nbspec SHALL validate notebook changes directly — checking requirement
headers, scenario blocks, delta operations, and schema-required artifacts
against the OpenSpec 1.x grammar — without invoking any external binary.

#### Scenario: Valid change passes
- **WHEN** `nbspec validate add-foo` runs for a well-formed notebook change
- **THEN** the command exits zero and reports the change valid, without
  modifying the repository working tree

#### Scenario: Grammar violation detected
- **WHEN** a delta-spec note contains a requirement without a
  `#### Scenario:` block
- **THEN** validation fails and the diagnostic identifies the violated rule

### Requirement: Note-level diagnostics
Validation diagnostics SHALL identify the notebook note (and artifact id)
containing each violation, with a line reference within the note where
feasible. Diagnostics SHALL NOT reference temporary or materialized file
paths.

#### Scenario: Validation error names the note
- **WHEN** the specification note `proposals/add-foo/specifications/user-auth`
  omits a scenario and `nbspec validate add-foo` runs
- **THEN** the reported error names
  `proposals/add-foo/specifications/user-auth` rather than any filesystem
  path

### Requirement: Upstream grammar conformance
nbspec's requirement, scenario, and delta grammar SHALL remain syntactically
compatible with OpenSpec 1.x tooling. The project's conformance suite SHALL
exercise this by rendering grammar fixtures into the upstream `spec-driven`
layout and running an upstream `openspec validate --strict`, pinned to a
fixed version, against them. Layout conformance is not claimed: nbspec's
default schema deliberately diverges from the `spec-driven` layout. The
conformance oracle SHALL be a development-time concern only — never a
runtime dependency.

#### Scenario: Conformance oracle passes
- **WHEN** the conformance suite renders fixture changes into the upstream
  layout and runs the pinned upstream validator against them
- **THEN** the upstream validator reports the fixtures valid

#### Scenario: No runtime binary requirement
- **WHEN** `nbspec validate` runs on a machine without the `openspec` CLI
  installed
- **THEN** validation completes normally using nbspec's native validator
