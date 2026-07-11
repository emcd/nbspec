<!-- nbspec: change=add-clean-succession notebook=nbspec note=proposals/add-clean-succession/specifications/target-ownership.md hash=sha256:6bd668342edbbae541d785ab44ebb2d1949675cb8f75565af1d39d78f4b94dd5 -->
# target-ownership

## ADDED Requirements

### Requirement: Provenance ownership of merge targets

Every repository target written by `nbspec merge` MUST carry a provenance header recording the materializing change, source notebook and note, and the content hash of the materialized body. The change named in a target's provenance header is the target's OWNER. Ownership transfers whenever a merge writes the target: the header always names the most recent materializing change.

#### Scenario: Provenance header written on merge

- **WHEN** a change materializes a target
- **THEN** the written file begins with a provenance header naming that change and the content hash of the written body

#### Scenario: Ownership transfers on succession

- **WHEN** a second change later materializes the same target
- **THEN** the target's provenance header afterward names the second change

### Requirement: Clean succession

When a merge plans to write a target owned by a DIFFERENT change, and the target's current content matches the content hash recorded in its own provenance header, the merge MUST proceed without `--force` and MUST announce the takeover in merge output, naming the previous owning change and the succeeding change. The succession test compares the target's CURRENT content against the hash in its OWN provenance header — the existing drift computation — never the proposed new content against the old header.

#### Scenario: Clean succession proceeds without force

- **WHEN** a change with a satisfied review gate merges onto a target owned by another change whose current content matches its recorded provenance hash
- **THEN** the merge proceeds without `--force`
- **AND THEN** the merge output announces the takeover, naming both changes

#### Scenario: Succession is never silent

- **WHEN** any clean succession occurs
- **THEN** the merge output states the previous owner and the new owner (operators see the inheritance rather than guessing why no force was needed)

### Requirement: Drifted takeover remains force-gated

When a merge plans to write a target owned by a different change and the target's current content does NOT match the hash in its provenance header, the plan MUST refuse before any write, stating that the target has drifted from its recorded provenance and naming the owning change; `--force` MUST override, loudly, per existing force semantics.

#### Scenario: Drifted takeover refused

- **WHEN** a change merges onto another change's target that has been modified since its materialization
- **THEN** the plan reports a drift refusal naming the owning change and nothing is written

#### Scenario: Force override of drifted takeover is loud

- **WHEN** the same merge reruns with `--force`
- **THEN** the merge proceeds and the output states that a drifted target was overwritten

### Requirement: Succession scope

Clean succession MUST relax only the ownership refusal. Review-gate refusals and unsupported-delta refusals MUST retain their existing semantics and force requirements unchanged.

#### Scenario: Unreviewed successor still refused

- **WHEN** a change without a current approving merge verdict merges onto a cleanly successible target
- **THEN** the plan reports the review-gate refusal and nothing is written
- **AND THEN** the clean-succession recognition does not bypass the review gate

