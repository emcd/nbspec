## ADDED Requirements

### Requirement: Scratch-workspace rendering
nbspec SHALL render a notebook change to a scratch workspace: each artifact
note rendered to the path its schema `generates` declares. Rendering SHALL
be deterministic (identical notes produce a byte-identical tree) and SHALL
NOT modify the repository working tree.

#### Scenario: Rendering leaves the repository untouched
- **WHEN** `nbspec render add-foo` runs
- **THEN** a complete tree is produced in a scratch workspace and the
  repository working tree is not modified

#### Scenario: Deterministic output
- **WHEN** `nbspec render add-foo` runs twice against unchanged notes
- **THEN** both renders produce byte-identical trees

### Requirement: Review output
Rendering SHALL support emitting a reviewable form of the change — the
rendered tree and a unified diff against the current merge targets —
suitable for external review tooling, without repository writes.

#### Scenario: Review diff generated
- **WHEN** `nbspec render add-foo --diff` runs
- **THEN** a unified diff between the change's rendered durable artifacts
  and their current repository targets is written to stdout or a named
  file, and the repository working tree is not modified

### Requirement: Durable artifact merge
`nbspec merge` SHALL transfer a change's durable artifacts (specifications,
designs, decisions) to their schema-configured repository target
directories. In this change's scope, merge SHALL handle artifacts that
create new documents; encountering a MODIFIED delta against an existing
document SHALL fail with a diagnostic naming the unsupported operation.
Merge SHALL be the only nbspec operation that writes to the repository, and
it SHALL NOT create git commits.

#### Scenario: New documents transferred
- **WHEN** `nbspec merge add-foo` runs for a change whose specifications are
  all new documents
- **THEN** the rendered documents are written under the configured target
  directories and no other repository paths are modified

#### Scenario: MODIFIED delta refused
- **WHEN** `nbspec merge add-foo` encounters a MODIFIED requirement delta
  targeting an existing specification
- **THEN** the merge fails with a diagnostic identifying the delta and the
  target document, and no files are written

### Requirement: Change archive at merge
Unless disabled by project configuration, `nbspec merge` SHALL write a
deterministic compressed tarball of the change — the full rendered artifact
tree, the meta note, and a snapshot of the `work` checklist — to a
configured archive directory, defaulting to
`documentation/archives/<change-id>.tar.zst`. Identical notebook content
SHALL produce a byte-identical archive. Merge SHALL warn when no
`.gitattributes` rule marks the archive path for Git LFS.

#### Scenario: Archive written at merge
- **WHEN** `nbspec merge add-foo` runs with archiving enabled
- **THEN** `documentation/archives/add-foo.tar.zst` contains the rendered
  artifact tree, meta note, and `work` checklist snapshot

#### Scenario: Archiving disabled by configuration
- **WHEN** project configuration disables merge archiving and
  `nbspec merge add-foo` runs
- **THEN** durable artifacts are transferred and no archive file is written

### Requirement: Merge provenance
Documents written by merge SHALL carry a provenance header identifying
nbspec as the generator, the source notebook and note, and a content hash of
the rendered body.

#### Scenario: Provenance stamped
- **WHEN** merge writes a specification document to the repository
- **THEN** the file begins with a comment header naming the generating
  change, source note, and content hash

### Requirement: Drift refusal
nbspec SHALL detect divergence between a merge target and what it last wrote
(via the provenance content hash) and SHALL refuse to overwrite drifted
targets unless `--force` is given. `nbspec display` SHALL report
drift.

#### Scenario: Hand-edited target protected
- **WHEN** a previously merged specification was hand-edited and
  `nbspec merge add-foo` runs without `--force`
- **THEN** the merge fails with a diagnostic naming the drifted file and no
  files are overwritten

#### Scenario: Forced overwrite
- **WHEN** the same merge runs with `--force`
- **THEN** the drifted targets are overwritten from notebook content and
  fresh provenance headers are stamped
