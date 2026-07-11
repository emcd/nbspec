<!-- nbspec: change=add-review-verb notebook=nbspec note=proposals/add-review-verb/designs/verdict-model.md hash=sha256:65b56ed511a4cc4d30f3df8fc77a079eb2b19ae7e57a51fd3d825dc4f2b0186c -->
# verdict-model

## Context

The review verb converts review approvals from delivery-time messages into
content-addressed records. The binding invariants were locked with the Nbspec
Owner on 2026-07-05; this design records them with rationale, plus the storage
and integration decisions. Decision 3 was revised 2026-07-11 after Notebook
MCP Owner's scoped storage review (nb 7.24.0, source-verified against
`/usr/local/bin/nb:15364-15457`).

## Decision 1 — Verdicts bind (gate, aggregate content hash)

A verdict names the gate it satisfies and the exact rendered content set it
evaluated. The aggregate content hash is SHA-256 over the sorted
(tree_path, body_hash) pairs of the change's full rendered artifact set.

**Why sorted pairs over the full set:** add, remove, and rename must stale a
verdict even when no body changes — a review of N documents is not a review of
N+1. Sorting makes the digest independent of enumeration order.

**Why the rendered set, not raw notes:** reviewers review renders (`nbspec
render` output, diffed via difit). The hash must cover the bytes the reviewer
saw, so provenance-stripped rendered bodies are the hashed substrate.

**New code:** provenance hashing today (`provenance.rs`) is per-document; the
aggregate helper is a distinct function and appears as an explicit work item.
It belongs beside the rendering path so both consume identical bytes.

## Decision 2 — Staleness is hash mismatch ONLY; gate mismatch is non-applicability

No time-based expiry, no lifecycle-based expiry. In particular a verdict MUST
NOT expire when the change advances through the event it gates: a guard that
expires on the event it gates is a non-guard — expiry-on-advance eats its own
license. A verdict for a different gate is simply not applicable; reporting it
as "stale" would misattribute the reason and teach users the wrong model.

## Decision 3 (REVISED 2026-07-11) — One immutable, uniquely named note per verdict under `verdicts/`

Each verdict is recorded as its own note in a `verdicts/` subfolder of the
change namespace. The note body carries one fenced JSON object (reviewer,
gate, verdict, aggregate_hash, RFC 3339 timestamp, optional comment). Once
created, a verdict note is never modified or removed by the verb. The note
name/title is collision-resistant (recorded timestamp plus a random suffix)
but the NAME IS NOT THE ORDERING KEY: evaluation orders by the recorded
RFC 3339 timestamp inside the payload, with the unique note identifier as a
deterministic tie-breaker.

**Why (Notebook MCP Owner storage review, 2026-07-11):** nb's append write
and its git checkpoint are separate and unlocked, so a shared append-only
note invites same-clone interleaving of separator/payload, git index-lock
races, and cross-clone EOF content conflicts on sync — a shared mutable
singleton, the stash-stack defect class. Unique per-verdict notes preserve
append-only semantics STRUCTURALLY: concurrency becomes additive under Git
(new files merge without conflict), and corruption is isolated to one
verdict.

**Alternatives rejected:**
- *Single shared append-only `verdicts` note* (the original Decision 3).
  Rejected: the concurrency hazards above; nb supplies neither local
  serialization nor cross-clone reconciliation.
- *Per-reviewer notes (`verdicts--<reviewer>`).* Rejected: prevents
  different-reviewer conflicts but retains same-reviewer/retry races and
  mutable append histories.
- *Entries in the `meta` note.* Rejected: meta is lifecycle state with
  replace-mode editing; mixing an immutable log into it invites the
  note-gutting failure mode observed during add-mcp-surface revisions.

The namespace-machinery cost that the original decision avoided is justified
by the fleet concurrency requirement.

## Decision 4 — Strict parsing of the verdict namespace

Evaluation reads verdict notes via resolved filesystem paths (the
rendering.rs precedent), never via short nb selectors. EVERY candidate note
in `verdicts/` is parsed strictly; a malformed or near-miss note fails
evaluation loudly, naming the note — silent skips are forbidden (a verdict
that cannot be read must block the gate, not vanish from it). Parsing
tolerates CRLF (Git checkout settings may normalize line endings) and
recognizes fences only as whole trimmed lines (backticks inside JSON strings
are harmless).

**Surfacing split (2026-07-11, Owner review item 2):** in merge, a parse
failure is a plan-phase REFUSAL kind ("verdict unparseable", naming the
note) and is `--force`-overridable like the other review-gate refusals —
force already overrides known-negative states (no-verdict,
revise-outstanding), so an unreadable note is not more sacred, and the
diagnostic names it either way. In display, a parse failure is an explicit
status, never an omission. Policy, not integrity: unsupported-delta refusals
remain the only force-proof class.

## Decision 5 — Merge-gate refusal integrates with plan-then-execute

The review-gate check contributes refusal kinds to the existing merge plan
phase: all refusals collected before any write. Four kinds: no-verdict,
stale (bound hash named), revise-outstanding, unparseable (note named).
`--force` overrides them — the human operator outranks the gate, and Eric
remains the actual merge authority — but the override is reported loudly,
and the archived change records what the verdict state was. `--force`
continues to NEVER override unsupported-delta refusals; the review gate is
policy, not integrity.

## Decision 6 — Archive inclusion is explicit

`write_change_archive` (in `operations.rs`) adds rendered artifacts,
`meta.md`, and `work.md` explicitly; nothing from the change namespace rides
the archive automatically. (Cited by function name deliberately — line-range
citations rot with every refactor above them, as the add-mcp-surface landing
already demonstrated against this document's first draft.) Verdict notes are
therefore EXPLICITLY added to the archive under `<change-id>/verdicts/`,
sorted by archive path to preserve the deterministic tar.zst output.
Verdicts never materialize to repository documentation targets.

## Decision 7 — Slice-1 policy: single approving verdict; gates fixed to `merge`

Any single current approving `merge` verdict satisfies the gate. Required
reviewers, quorums, role-scoped gates, and additional gate names are deferred;
the payload shape (reviewer recorded per verdict, gate as a string) is chosen
so those policies are storage-compatible extensions. Multi-reviewer policy was
explicitly left to proposal discovery on 2026-07-05; slice-1-minimal is the
discovered answer. Display, however, lists EVERY reviewer's latest position
per gate (Owner review item 4): supersession is an evaluation detail, and an
operator deciding whether to merge deserves to see all standing positions,
not a single winner.

## Decision 8 — Module placement: `src/reviews.rs`

Verdict creation, namespace parsing, supersession, and staleness computation
live in a new `reviews` module rather than `operations.rs`, avoiding collision
with the add-mcp-surface implementation in the same file (landed a646943; the
separation still reads well — operations.rs stays orchestration, reviews.rs
owns the verdict model). The CLI verb and merge integration touch `cli.rs`
and `merging.rs` narrowly.

## Decision 9 — Lifecycle non-interaction

Recording a verdict does not transition change lifecycle status. Verdicts
observe; lifecycle transitions remain owned by their existing verbs. The
pending-gates view (nbspec ideas/3) consumes verdict status read-only.

## Risks

- **Hash sensitivity to rendering changes.** Any change to rendering
  normalization retroactively stales every open verdict. Acceptable: rendering
  changes are rare, and conservative staleness errs toward re-review. Noted so
  a future rendering change treats mass-staleness as expected, not as a bug.
- **`--force` habituation.** If operators force past the gate routinely, the
  gate decays to noise. Mitigation: loud override reporting and the archive
  trail make habits visible in review.
- **Reviewer identity is asserted, not authenticated.** Slice 1 trusts
  `--reviewer`/Git identity, consistent with the trust level of every other
  nbspec input; an EMPTY identity (no flag, no Git user.name) is refused
  outright. The recorded ordering timestamp is likewise asserted.
  Authentication, if ever needed, arrives with the multi-reviewer policy
  change.
- **Verdict-note proliferation.** A contentious change accumulates one note
  per verdict. Acceptable: the notes are small, the trail is the point, and
  the archive preserves them when the change namespace dies.


## Decision 10 (ADDED 2026-07-11, Eric-sponsored) — Revise verdicts require findings; CLI reads comments from stdin

A `revise` verdict MUST carry a comment — typically a findings-note
selector. A revise without findings is an unactionable block: a mood, not a
protocol message. Every revise verdict issued in fleet practice already
carried a findings pointer; this makes the norm mechanical. `approve`
remains comment-optional because approval is self-contained.

On the CLI, `--comment -` reads the comment from standard input. Long
findings text through shell arguments hits the backtick-mangling and
quoting hazards that motivated the MCP surface itself; stdin is the
conventional escape (`git commit -F -` precedent). LAYER BOUNDARY: the `-`
marker is interpreted by the CLI dispatcher only. `operations::review`
receives resolved comment text, and the MCP `review` tool passes a comment
of `-` through verbatim — an MCP client has no stdin, and silently
reinterpreting its literal payload would be a spooky surprise.
