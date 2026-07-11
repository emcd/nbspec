<!-- nbspec: change=add-review-verb notebook=nbspec note=proposals/add-review-verb/specifications/review-gating.md hash=sha256:ae92e9b13992dcc29f3c74f2642ab31e76fa5ae176c0a4c2b7b1b52a2a3235e6 -->
# review-gating

## ADDED Requirements

### Requirement: Verdict recording

The `review` verb MUST record each verdict as ONE new immutable note under `proposals/<change-id>/verdicts/`, whose body carries a fenced JSON object containing: reviewer identity, gate name, verdict value (`approve` or `revise`), the change's aggregate content hash at recording time, an RFC 3339 timestamp, and a comment (REQUIRED for `revise` — the findings reference that makes the revision actionable; optional for `approve`). The note name MUST be collision-resistant (recorded timestamp plus a random suffix); the name MUST NOT serve as the semantic ordering key. Existing verdict notes MUST never be modified or removed by the verb. Reviewer identity resolves from an explicit `--reviewer` argument when given, else from Git identity (`user.name`); when neither yields a non-empty identity, the verb MUST refuse with a clear diagnostic and create no note. On the CLI, `--comment -` reads the comment from standard input; this is a CLI-only affordance — the MCP tool takes the comment string verbatim and MUST NOT interpret `-` as a stdin marker.

#### Scenario: Approving verdict recorded

- **WHEN** a reviewer runs `nbspec review <change-id> --gate merge --verdict approve`
- **THEN** a new note is created under `proposals/<change-id>/verdicts/` with verdict `approve`, gate `merge`, the current aggregate content hash, and the resolved reviewer identity
- **AND THEN** no existing verdict note is modified or removed

#### Scenario: Revise verdict recorded with comment

- **WHEN** a reviewer runs `nbspec review <change-id> --gate merge --verdict revise --comment "findings at <selector>"`
- **THEN** the created note carries verdict `revise` and the comment text
- **AND THEN** the change's lifecycle status is unchanged (verdicts observe; they do not transition lifecycle)

#### Scenario: Revise without a comment refused

- **WHEN** a reviewer runs `nbspec review <change-id> --verdict revise` with no comment (or a whitespace-only comment)
- **THEN** the verb refuses with a diagnostic explaining that a revise verdict must name its findings
- **AND THEN** no note is created

#### Scenario: Comment read from standard input

- **WHEN** a reviewer runs `nbspec review <change-id> --verdict revise --comment -` with the findings text piped on standard input
- **THEN** the recorded comment is the standard-input contents
- **AND THEN** the MCP `review` tool given a comment of `-` records the literal string `-` (no stdin semantics outside the CLI)

#### Scenario: Unknown gate refused

- **WHEN** a reviewer names a gate that the change's schema does not define (slice 1 defines only `merge`)
- **THEN** the verb refuses with a diagnostic naming the known gates
- **AND THEN** no note is created

#### Scenario: Missing reviewer identity refused

- **WHEN** no `--reviewer` argument is given and Git `user.name` is unset or empty
- **THEN** the verb refuses with a diagnostic explaining how to supply an identity
- **AND THEN** no note is created

#### Scenario: Concurrent verdicts do not conflict

- **WHEN** two recordings occur near-simultaneously — whether from one reviewer or many, possibly in concurrent invocations
- **THEN** each verdict lands as its own uniquely named note
- **AND THEN** no shared file is contended and Git treats the outcome as additive (no content conflict)

### Requirement: Aggregate content hash

The system MUST compute a change's aggregate content hash as the SHA-256 digest over the sorted sequence of (tree_path, body_hash) pairs of the change's FULL rendered artifact set — the same byte-for-byte rendered content that `nbspec render` produces. Adding, removing, or renaming an artifact MUST change the aggregate hash even when no artifact body changes.

#### Scenario: Deterministic for unchanged content

- **WHEN** the aggregate hash is computed twice for a change whose notes have not changed
- **THEN** both computations yield the same hash

#### Scenario: Body edit changes the aggregate

- **WHEN** any artifact note's body changes
- **THEN** the aggregate hash changes

#### Scenario: Set membership changes the aggregate

- **WHEN** an artifact note is added, removed, or renamed without any body edit to other artifacts
- **THEN** the aggregate hash changes

### Requirement: Staleness and gate applicability

A verdict MUST be considered current if and only if its stored aggregate hash equals the change's current aggregate hash; hash mismatch is the ONLY staleness condition. A verdict whose gate differs from the gate under evaluation MUST be treated as non-applicable, never as expired or stale.

#### Scenario: Verdict goes stale on content change

- **WHEN** an approving `merge` verdict exists and any artifact subsequently changes
- **THEN** the verdict is reported stale, identifying the hash it bound

#### Scenario: Gate mismatch is non-applicability

- **WHEN** a verdict exists for a gate other than the one being evaluated
- **THEN** the verdict is excluded from that evaluation as non-applicable
- **AND THEN** it is not reported as stale or expired

### Requirement: Supersession

When one reviewer has recorded multiple verdicts for the same gate, the newest MUST supersede that reviewer's older verdicts for that gate. Recency is determined by the recorded RFC 3339 timestamp inside the verdict payload, with the collision-resistant note identifier as a deterministic tie-breaker. Verdicts from distinct reviewers MUST coexist independently. Superseded verdict notes remain in the namespace as history.

#### Scenario: Newer same-gate verdict supersedes

- **WHEN** a reviewer records `revise` and later records `approve` for the same gate
- **THEN** evaluation considers only the `approve` verdict for that reviewer
- **AND THEN** the superseded verdict note remains in the namespace as history

#### Scenario: Distinct reviewers coexist

- **WHEN** two reviewers each record a verdict for the same gate
- **THEN** each reviewer's latest verdict is evaluated independently

#### Scenario: Timestamp tie broken deterministically

- **WHEN** two verdicts from the same reviewer for the same gate carry identical recorded timestamps
- **THEN** evaluation breaks the tie by the collision-resistant note identifier
- **AND THEN** repeated evaluations yield the same winner

### Requirement: Strict verdict parsing

Evaluation MUST read verdict notes via resolved filesystem paths and MUST parse every candidate note in the `verdicts/` namespace strictly. A malformed or near-miss note MUST fail the evaluation loudly, naming the offending note; silent skips are forbidden (an unreadable verdict blocks the gate rather than vanishing from it). In merge, a parse failure surfaces as a plan-phase refusal (see Merge gate enforcement); in display, as an explicit parse-failure status. Parsing MUST tolerate CRLF line endings and MUST recognize fences only as whole trimmed lines.

#### Scenario: Malformed verdict note blocks evaluation

- **WHEN** a note in `verdicts/` cannot be parsed as a well-formed verdict
- **THEN** gate evaluation fails with a diagnostic naming that note
- **AND THEN** the malformed note is NOT silently excluded from evaluation

#### Scenario: CRLF-normalized note still parses

- **WHEN** a verdict note's line endings have been normalized to CRLF by Git checkout settings
- **THEN** the note parses identically to its LF form

### Requirement: Merge gate enforcement

`nbspec merge` MUST refuse to execute when no current approving `merge`-gate verdict exists, reporting the refusal in the plan phase alongside existing refusal kinds (all refusals collected before any write). The refusal MUST distinguish four kinds: "no verdict", "verdict stale" (naming the bound hash), "latest verdict is revise", and "verdict unparseable" (naming the offending note). All four are plan-phase POLICY refusals: `--force` MUST override them, and the override MUST be reported in merge output. (`--force` continues to never override unsupported-delta refusals — integrity, not policy.) In slice 1, any single current approving verdict satisfies the gate.

#### Scenario: Merge refused without verdict

- **WHEN** `nbspec merge <change-id>` runs and the change has no `merge`-gate verdicts
- **THEN** the plan reports a review-gate refusal ("no verdict") and nothing is written

#### Scenario: Merge refused on stale approval

- **WHEN** the only approving `merge` verdict binds a superseded aggregate hash
- **THEN** the plan reports the verdict as stale, naming the bound hash and the current hash, and nothing is written

#### Scenario: Merge refused when latest verdict is revise

- **WHEN** the evaluating reviewer's latest `merge`-gate verdict is `revise`
- **THEN** the plan reports a review-gate refusal ("latest verdict is revise", with the reviewer and comment reference) and nothing is written

#### Scenario: Merge refused on unparseable verdict

- **WHEN** any note in `verdicts/` fails strict parsing
- **THEN** the plan reports a review-gate refusal ("verdict unparseable", naming the note) and nothing is written
- **AND THEN** the refusal is `--force`-overridable like the other review-gate refusal kinds

#### Scenario: Merge proceeds with current approval

- **WHEN** a current approving `merge`-gate verdict exists
- **THEN** the merge proceeds per the existing merge contract

#### Scenario: Force override is loud

- **WHEN** `nbspec merge <change-id> --force` runs without a current approving verdict
- **THEN** the merge proceeds and the output states that the review gate was overridden

### Requirement: Verdict visibility

`nbspec display` MUST surface verdict status per gate AND per reviewer: for each known gate, each reviewer's latest verdict is listed separately (current approval, stale approval, or revise outstanding, with reviewer and timestamp), or absence when no verdicts exist. Supersession is an evaluation detail; the display summary shows every reviewer's latest position. Verdict notes MUST never materialize to repository documentation targets. The merge-time change archive MUST include each verdict note EXPLICITLY (archive inclusion is not automatic for change-namespace content), placed under `<change-id>/verdicts/` and sorted by archive path to preserve deterministic archive output.

#### Scenario: Display shows current approval

- **WHEN** a current approving `merge` verdict exists and `nbspec display <change-id>` runs
- **THEN** the output includes the gate, verdict, reviewer, and timestamp

#### Scenario: Display lists each reviewer's latest position

- **WHEN** two reviewers have recorded `merge`-gate verdicts (one approve, one revise)
- **THEN** the display output lists both reviewers' latest verdicts separately
- **AND THEN** neither reviewer's position hides the other's

#### Scenario: Display shows staleness

- **WHEN** the only approving verdict is stale
- **THEN** the display output marks it stale rather than omitting it

#### Scenario: Archive preserves the review trail

- **WHEN** a change with verdicts is merged and archived
- **THEN** the archive contains each `verdicts/` note alongside `meta` and `work`, sorted by archive path
- **AND THEN** no verdict content is written into the repository tree
