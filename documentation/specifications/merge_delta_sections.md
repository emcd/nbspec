<!-- nbspec: change=add-delta-merge notebook=nbspec note=proposals/add-delta-merge/specifications/merge_delta_sections.md hash=sha256:89068c47ddfcf342c1bc1a2b59f420869452b4d3280f9f9e25dd0a1fcf3f6c8e -->
# Merge delta sections

## ADDED Requirements

### Requirement: Delta operations apply against the target block map

`merge` SHALL NOT refuse a document for carrying `MODIFIED` /
`REMOVED` / `RENAMED` delta sections. For hash-valid targets it
SHALL decompose the target into requirement blocks and apply delta
operations in order `RENAMED → REMOVED → MODIFIED → ADDED`,
matching requirement names exactly: `REMOVED` deletes the named
block, `MODIFIED` replaces it with the delta block, `RENAMED`
re-headers it in place preserving position, `ADDED` appends new
blocks under the target's `## ADDED Requirements` section.
Notes carrying only an `ADDED Requirements` section apply the same
way (new blocks append, identical blocks no-op, differing blocks
refuse) — there is no whole-write exemption.
Recomposition SHALL preserve the target title, non-requirement
prose, and existing block order. Untouched lines round-trip
byte-identical, including line-ending style and deliberate blank
spacing; inserted lines adopt the target's EOL. Headers inside
fenced code blocks (backtick/tilde runs of 3+, length-matched
closing) are prose, never merge state. Drifted targets refuse;
`--force` applies onto the drifted body anyway.

#### Scenario: REMOVED deletes the named block

- **WHEN** the target contains `### Requirement: Old thing`
- **AND** the delta names it under `REMOVED Requirements`
- **THEN** the merged target no longer contains that block
- **AND** all other blocks keep their order and content

#### Scenario: ADDED-only note appends

- **WHEN** the delta carries only `ADDED Requirements` with a new
  `### Requirement: Fresh` block
- **AND** the hash-valid target holds other requirements
- **THEN** the merged target appends the fresh block after the
  existing ones
- **AND** no existing block is duplicated or rewritten

#### Scenario: Fenced headers stay prose

- **WHEN** the target shows `### Requirement: Ghost` only inside a
  fenced example
- **AND** the delta removes `### Requirement: Ghost`
- **THEN** merge warns the removal as already applied
- **AND** the fenced example is byte-identical afterwards

#### Scenario: Drifted target refuses, force applies

- **WHEN** the target body no longer matches its provenance hash
- **THEN** merge refuses with `Drifted` even though the delta
  sections are well-formed
- **AND** `--force` applies the delta operations onto the drifted
  body

### Requirement: Absent targets and name mismatches fail safe

Against an absent target only `ADDED` SHALL apply; `REMOVED`
SHALL warn and be ignored; `MODIFIED` / `RENAMED` SHALL refuse.
When no effective `ADDED` block remains (e.g. a `REMOVED`-only
note against an absent target), merge SHALL warn, skip the
document, and create no durable target — an empty specification
SHALL NOT materialize accidentally.
Name mismatches SHALL fail safe: a `REMOVED` name with no exact
target match warns as already-removed; `MODIFIED` / `RENAMED`
with no exact match refuse; `ADDED` over differing content on a
hash-valid target refuses (never silent duplication); identical
content is a no-op. Under `--force`, an `ADDED` collision against
drifted text resolves delta-wins (the difference may itself be
the drift); absences never resolve under force.
A case/whitespace near-miss present in the target SHALL refuse as
a typo in every operation. `MODIFIED` SHALL NOT drop scenarios
present in the current block.

#### Scenario: REMOVED-missing warns on an existing target

- **WHEN** the delta removes `### Requirement: Gone` and no exact
  target block matches
- **AND** no case/whitespace variant is present
- **THEN** merge warns the removal as already applied and continues

#### Scenario: REMOVED-only note against an absent target skips

- **WHEN** the delta carries only a `REMOVED Requirements` section
- **AND** the merge target does not exist
- **THEN** merge warns that no effective operation remains
- **AND** no durable target file is created

#### Scenario: ADDED over differing content refuses

- **WHEN** the delta adds `### Requirement: X` with new body text
- **AND** a hash-valid target already holds `### Requirement: X`
  with different body text
- **THEN** merge refuses with a collision diagnostic
- **AND** rerunning with `--force` still refuses

#### Scenario: Force resolves drift-induced ADDED collision

- **WHEN** the delta adds `### Requirement: X` with new body text
- **AND** a drifted target holds `### Requirement: X` with
  different body text
- **THEN** merge without `--force` refuses with `Drifted`
- **AND** `--force` replaces the block with the delta content

### Requirement: Delta incoherence refuses without force override

`merge` SHALL reject incoherent deltas before any write:
duplicate names within one section; repeated delta sections;
one name in two of `ADDED` / `MODIFIED` / `REMOVED`; `RENAMED`
unpaired `FROM:` / `TO:`; rename cycles; `RENAMED`-to colliding
with an `ADDED` name; `MODIFIED` naming a `RENAMED`-from side; a
`RENAMED`-from name also named under `REMOVED` (the rename would
consume the old header while the removal silently no-ops as
already-applied). Rename chains (`A→B→C`) stay legal and resolve
to final-state on remerge. These refusals SHALL NOT be
force-overridable: force overrides drift, never validation.

#### Scenario: Cross-section duplicate refuses with force

- **WHEN** a note names one requirement under both `ADDED` and
  `REMOVED Requirements`
- **THEN** merge refuses with an incoherence diagnostic
- **AND** rerunning with `--force` still refuses

#### Scenario: Rename-plus-remove refuses with force

- **WHEN** a note renames `### Requirement: A` to `B` and also
  names `A` under `REMOVED Requirements`
- **THEN** merge refuses with an incoherence diagnostic
- **AND** rerunning with `--force` still refuses

#### Scenario: Repeated sections refuse

- **WHEN** a note carries two `## ADDED Requirements` sections
- **THEN** merge refuses with an incoherence diagnostic
- **AND** rerunning with `--force` still refuses

#### Scenario: Rename cycle refuses

- **WHEN** a note renames `A→B` and `B→A`
- **THEN** merge refuses with an incoherence diagnostic
- **AND** rerunning with `--force` still refuses

#### Scenario: Applied chain remerges silently

- **WHEN** a delta renames `A→B` and `B→C`
- **AND** the target already holds only `### Requirement: C`
- **THEN** merge succeeds without writing
- **AND** no warning is emitted

### Requirement: Surgical merge composes with target state and reporting

`--force` against an unmanaged existing target (no provenance
header) SHALL require a populated requirement-block map: when the
target holds addressable blocks, merge applies the delta
operations, stamps the result, and adopts the target; when it
holds none, merge refuses regardless of `--force` (no base to
apply onto — this narrows the old whole-overwrite behavior
deliberately, since whole-writing a sparse delta note would
corrupt the target). A target holding duplicate requirement
names (exact or folded) likewise refuses as structurally invalid,
since application would proceed partially while the twin
survives.
Target-state comparison (`target_status`, remerge detection)
SHALL run against the rebuilt durable body, not the raw delta
note, so idempotent no-op merges report `Current` instead of
perpetual `UpdatePending` rewrites. All merge warnings (already-
removed, ignored delta `Purpose` including targets without one,
skipped vacuous documents) SHALL surface in merge text output
and in a structured `warnings` report field alongside
`successions` / `drift_overrides`, on every merge including
idempotent remerges.

#### Scenario: Force adopts a parseable unmanaged target

- **WHEN** an unmanaged file at the target parses into
  requirement blocks
- **AND** merge runs with `--force`
- **THEN** the delta operations apply onto that body
- **AND** the written file carries the change's provenance stamp

#### Scenario: Duplicate target names refuse

- **WHEN** the target holds two `### Requirement: Same` blocks
- **AND** the delta modifies `### Requirement: Same`
- **THEN** merge refuses with a structural diagnostic
- **AND** rerunning with `--force` still refuses

#### Scenario: Idempotent remerge reports current

- **WHEN** every delta operation is an already-applied no-op
- **AND** the target body equals the rebuilt body
- **THEN** merge reports the target `unchanged`
- **AND** no rewrite occurs
