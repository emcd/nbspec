<!-- nbspec: change=add-namespace-retirement notebook=nbspec note=proposals/add-namespace-retirement/specifications/namespace_retirement.md hash=sha256:edff0a97ba2669cd81736ead069221dfa6eb424a77eabc68d7752b36dc5d6edc -->
# Namespace retirement

## ADDED Requirements

### Requirement: Merge retires the namespace after archiving

`merge` SHALL delete the change namespace automatically once
repository materialization and the archive step have both
completed. When archiving is enabled, retirement SHALL NOT
run before the archive step succeeds and SHALL NOT run at
all when the archive step fails. The change archive
intentionally captures pre-retirement metadata (status as
observed at merge). When archiving is disabled
(`archives = false`), merge materializes but SKIPS
retirement and emits a loud warning: deletion is lossy and
the operator disabled the surviving record, so the notebook
copy stays as the record. There is no separate retirement
verb and no opt-out: every successful merge with archiving
enabled ends with the namespace deleted.

#### Scenario: Successful merge ends retired

- **WHEN** materialization and the archive write both succeed
- **THEN** the namespace is deleted in the same invocation
- **AND** the merge report names the tarball path

#### Scenario: Archive failure blocks retirement

- **WHEN** the archive step fails
- **THEN** merge reports the archive error
- **AND** the namespace remains under `proposals/` untouched

#### Scenario: Tarball holds pre-retirement metadata

- **WHEN** the archive step completes for a draft change
- **THEN** the tarball's meta snapshot shows the
  pre-retirement status
- **AND** NOT any post-merge status

#### Scenario: Disabled archives skip retirement with warning

- **WHEN** `archives = false` and materialization succeeds
- **THEN** merge writes no tarball and deletes nothing
- **AND** warns loudly that the namespace was left in place
  as the only record

### Requirement: Archive preserves the complete namespace byte-faithfully

The tarball is the sole surviving record, so it SHALL
contain a byte-faithful copy of every user file under
`proposals/<change-id>/` at original relative paths beneath
a `namespace/` prefix, alongside the existing rendered set:
proposal, specifications, designs, decisions, verdicts, work
notes, attachments, and non-Markdown files alike, stored as
raw bytes with no normalization. Notebook-managed
bookkeeping (`.gitkeep` files, `.index` files) is
intentionally excluded. Any unreadable entry fails the
merge closed before publication. After writing, merge SHALL
verify the persisted tarball by extracting it and comparing
against the captured inventory (paths and bytes); a
mismatch refuses before any deletion.

#### Scenario: Extra content survives byte-identical

- **WHEN** the namespace holds an attachment outside any
  schema artifact
- **THEN** the tarball carries it under `namespace/` with
  identical bytes
- **AND** verification passes against the inventory

#### Scenario: Unreadable entries fail closed

- **WHEN** a namespace file cannot be read during archive
  construction
- **THEN** merge errors before publication
- **AND** deletes nothing

#### Scenario: Bookkeeping is excluded by design

- **WHEN** the namespace contains `.gitkeep` or `.index`
  bookkeeping files
- **THEN** the tarball omits them
- **AND** verification still passes

#### Scenario: Verification mismatch blocks deletion

- **WHEN** the persisted tarball does not match the
  captured inventory
- **THEN** merge errors
- **AND** deletes nothing

### Requirement: One source snapshot binds the pipeline

Merge SHALL capture a source snapshot — the notebook HEAD
revision plus the per-file namespace inventory (relative
paths plus content hashes) — BEFORE review-gate evaluation,
and derive every downstream step from it: gate inputs,
rendered and materialized documents, archive inventory,
retry identity, and the deletion precondition all refer to
that snapshot. The expected revision is the ORIGINAL
pre-gate HEAD and SHALL never be refreshed within an
invocation: re-verification re-checks inventory only, and
any committed advance — related or not — refuses as
spurious-but-safe (re-archive and retry). Each invocation
captures its own snapshot; across invocations, tarball
byte-identity (not HEAD lineage) proves continuity. After
materialization and BEFORE archive construction, merge SHALL
re-verify the live namespace against the snapshot; on any
drift it refuses with a phase-aware error (repository outputs
may already exist; no archive is published, nothing is
deleted, retirement is not reported). Archive construction
SHALL read file bytes in the same pass that fixes the
inventory hashes, so the tarball cannot mix pre- and
post-change content. The deletion transaction SHALL assert
the original expected HEAD via nb-api AssertRevision;
mismatch refuses cleanly with nothing deleted. AssertRevision
covers COMMITTED changes only: dirty tracked edits and
untracked files do not advance HEAD and are NOT detected by
it. Retirement therefore assumes cooperative checkpointed
writers for the retiring namespace during the merge window;
concurrent dirty or untracked modification in that window is
an operator-accepted residual risk (accepted 2026-11-10),
stated here, not hidden. The check-to-checkpoint window
without a cross-process commit lock is likewise inherent
(nb-api 0.5.0 locking work). Folder-scoped content assertion
is the nb-api 0.5.0 follow-up.

#### Scenario: Edit after materialization but before archive refuses

- **WHEN** a namespace note changes after materialization
  but before archive construction
- **THEN** merge refuses with a phase-aware error
- **AND** no archive is published, nothing is deleted, and
  retirement is not reported

#### Scenario: Committed change before deletion refuses

- **WHEN** any commit advances the notebook HEAD after
  verification but before the deletion commits
- **THEN** the AssertRevision precondition refuses cleanly
- **AND** nothing is deleted

#### Scenario: Unrelated committed write refuses spuriously but safely

- **WHEN** an unrelated notebook commit advances HEAD after
gate evaluation
- **THEN** merge refuses rather than refreshing expected
- **AND** re-archive and retry resumes safely

#### Scenario: Dirty post-verification write is not detected

- **WHEN** an uncommitted edit or untracked file lands in
  the namespace after verification
- **THEN** AssertRevision still passes, and the delete may
  remove unarchived content
- **AND** this stands as the operator-accepted residual:
  cooperative checkpointed writers are assumed

### Requirement: Retirement deletes the namespace folder

Retirement SHALL delete the namespace folder
`proposals/<change-id>/` as one folder-level operation.
Retirement SHALL NOT delete notes piecemeal: note-by-note
deletion leaves folder skeletons and parent index entries
behind, so the deletion MUST be a transactional folder
operation provided by nb-api (explicit dependency, recorded
in the change impact). No `archives/` folder for this
namespace SHALL remain in the notebook and no user file of
this namespace SHALL survive: the on-disk tarball is the
only surviving record. (This scopes to the retired
namespace and its index entries — unrelated notebook
folders and notebook history are untouched.) The merge
report SHALL name the tarball path.

#### Scenario: Folder gone entirely

- **WHEN** retirement completes
- **THEN** `proposals/<change-id>/` no longer exists
- **AND** no user file of this namespace remains in the
  notebook

### Requirement: Merge gates without lifecycle theater

The review gate still gates merge unchanged: a binding
APPROVE verdict, or — when the review gate is
force-overridden — that recorded operator authorization
(the override notice already carried in the merge report
is the record). Merge SHALL perform no status transitions:
the namespace dies with its observed status and the tarball
preserves it; transitions followed by deletion would be
theater. Merge SHALL refuse `superseded` or `abandoned`
changes: dead changes do not merge.

#### Scenario: Forced draft merge records its authorization

- **WHEN** a draft change merges under a force-overridden
  review gate
- **THEN** merge proceeds with no transition bookkeeping
- **AND** the report carries the review-gate override notice
  as the approval record

#### Scenario: Dead changes refuse

- **WHEN** the change status is `superseded` or `abandoned`
- **THEN** merge refuses without writing or deleting anything

### Requirement: Archive identity, publication, and lookup use the configured directory

Every archive path — merge publication, already-merged
lookup, and `create`/`import` collision checks — SHALL
resolve through the configured `archive_directory`, never a
hard-coded location. Publication SHALL be atomic and
verified: write to a temporary path, verify against the
inventory, then rename into place. If the final path
exists and is byte-identical, merge keeps it (already
published, no rewrite); if it exists and differs, merge
refuses — a differing pre-existing archive is shipped
history that must never be silently overwritten. Presence
alone never proves already-merged: lookup SHALL validate
the archive (regular file, not a symlink; readable; valid
tar.zst; top-level `<change-id>/` prefix; embedded meta
snapshot naming this change id) and fail closed to
inspection-required on anything else. With archiving
disabled there is no lookup: absent means ChangeNotFound.
Only the currently configured directory is consulted; a
changed directory does not inherit old-dir tarballs.

#### Scenario: Corrupt archive fails closed

- **WHEN** a file at the expected tarball path is
  unreadable or not a valid tar.zst for this change
- **THEN** merge refuses as inspection-required
- **AND** never reports already-merged nor overwrites it

#### Scenario: Symlink archive fails closed

- **WHEN** the expected tarball path is a symlink or
  non-file
- **THEN** merge refuses as inspection-required

#### Scenario: Differing pre-existing archive refuses

- **WHEN** a tarball exists at the final path with different
  bytes than the rebuilt archive
- **THEN** merge refuses without overwriting
- **AND** directs inspection of the shipped history

#### Scenario: Identical pre-existing archive is kept

- **WHEN** a tarball exists at the final path with
  identical bytes
- **THEN** merge treats it as already published
- **AND** performs no rewrite

### Requirement: Namespace states resolve by explicit matrix

Merge SHALL resolve namespace state before loading context,
applying this matrix: present under `proposals/` with no
tarball → merge normally; present with a byte-identical
tarball → resume deletion only, skipping materialization
and archive rewrite; present with a differing tarball →
refuse (changed source); absent with a validated tarball →
report already-merged with the tarball path, performing no
writes or deletions; absent without a tarball →
ChangeNotFound; indeterminate prior outcome → refuse as
inspection-required. No persistent recovery marker is
kept: every invocation re-derives state from live
notebook and archive, so indeterminate outcomes settle on
re-read (namespace present → retry path with snapshot
match; namespace absent with valid tarball →
already-merged) and fail closed on mismatch or corruption.
Resume-by-remerge is safe only for recognized
present-with-identical-tarball states. The
tarball-collision rule binds every namespace creator:
`create` and `import` SHALL both refuse ids already
claimed by an on-disk tarball under the configured
directory, never silently overwriting a shipped record.

#### Scenario: Re-merge after retirement reports already-merged

- **WHEN** `merge` runs against a retired change id with a
  validated tarball
- **THEN** it reports already-merged with the tarball path
- **AND** it performs no writes or deletions

#### Scenario: Clean retry resumes deletion only

- **WHEN** `merge` re-runs with the namespace present and
  the tarball byte-identical to the rebuilt archive
- **THEN** it skips materialization and archive rewrite
- **AND** proceeds to snapshot-bound deletion

#### Scenario: Changed-source retry refuses

- **WHEN** `merge` re-runs with the namespace present and
  the tarball differing from the rebuilt archive
- **THEN** it refuses without writing, publishing, or
  deleting

#### Scenario: Unknown ids are not found

- **WHEN** `merge` names an id with neither namespace nor
  tarball
- **THEN** it reports ChangeNotFound

#### Scenario: Tarball-claimed ids are not reusable

- **WHEN** `create` or `import` names an id claimed by an
  on-disk tarball
- **THEN** it refuses as colliding with shipped history

### Requirement: Retirement failure reports phases and retry guidance

A retirement failure is a partial merge at operation level:
durable repository writes and the tarball (when enabled)
already remain. Merge SHALL report a distinct
retirement-failed error naming completed phases
(materialization done, tarball path or skipped) and
directing safe retry: re-running merge resumes against the
state matrix. The "namespace wholly in place" guarantee
holds only for clean rollback; indeterminate-commit or
recovery-required outcomes SHALL report
inspection-required with no rollback claim.

#### Scenario: Failed retirement names phases and retry

- **WHEN** the folder deletion fails cleanly after a
  successful archive
- **THEN** merge errors naming materialization done and the
  tarball path
- **AND** directs re-running merge to resume

#### Scenario: Indeterminate outcomes require inspection

- **WHEN** the notebook reports an indeterminate commit
- **THEN** merge errors directing inspection
- **AND** makes no claim about namespace placement

### Requirement: Retired namespaces are repo history, not notebook state

No notebook surface SHALL resolve a retired namespace:
`display`, `validate`, `render`, and `export` report
not-found for retired ids, exactly as for never-existing
ones. The inspectable record of merged work is the
repository (durable documents) plus the change tarball.
Only `merge` distinguishes retired from unknown, via the
already-merged report in the state matrix.

#### Scenario: Read verbs report retired ids as not-found

- **WHEN** `display` names a retired change id
- **THEN** it reports not-found
- **AND** does not consult the tarball

#### Scenario: Merge distinguishes retired from unknown

- **WHEN** `merge` names a retired change id
- **THEN** it reports already-merged with the tarball path
- **AND** performs no writes or deletions
