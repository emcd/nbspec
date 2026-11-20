# Delegated Review Flow

Use this flow when an author requests review from reviewers working in other repositories — or in other clones of the same repository.

## Roles

- **Author:** implements the change, produces review packets, addresses findings with fixups, and prepares the cleaned stack for merge.
- **Tier-1 Reviewer:** first-pass reviewer and technical gate with approve-or-block disposition. Covers the whole change: design, correctness, validation evidence, and commit structure. Produces a coverage ledger — what was inspected, at what tip, with what evidence, what was not verified. The slot is usually a Reviewer General. A large project may fill it with several specialists, such as a Backend Reviewer, Frontend Reviewer, Data Models Reviewer, or Maps Reviewer.
- **Tier-2 Reviewer:** second gate per blast radius, independent architecture/security verdict. The slot is usually an Advisor. Engage only after Tier-1 approval on an explicit trigger (see Tier-2 engagement).
- **Integrator** (coordinator or tech lead): accepts the **approved, cleaned** stack for merge. Checks that the branch is based on the current `<local-integration-base>`, that the merge can proceed without conflict, and that human merge/push policy is respected. This is **not** a technical review of the change content.

The same person may wear more than one role only when the project explicitly allows it. Do not treat "send to coordinator for merge" as another round of code review.

## Review tiers

Send the initial packet to the Tier-1 Reviewer only. Do not Cc the Tier-2 Reviewer, and do not send both tiers the same packet at once. Except for cross-functional work, a Tier-1 review goes to the designated Tier-1 Reviewer for that lane, area, project, or team. Do not fan the same packet across every Tier-1 seat.

Reviews are bilateral: the author and the active reviewer only. Do not Cc anyone beyond the active pair — not the other reviewer, not the coordinator. Reviewers never message each other in either direction; all cross-tier movement arrives as owner tasking. A reviewer's disposition goes to the change owner, who owns the routing decision. The integrator enters only at the merge request, after final approval and autosquash, with cleaned hashes, tree identity against the reviewed tree, validation summary, and approval trail.

The Tier-2 aim is how we spend scarce Tier-2 credits. It is not a scope limit. Either reviewer may inspect any part of the stack, including a later fix, and may take an integrative or a pinhole view. Author concerns, suggested focus, or "please only look at X" are context, not a restriction. Do not dictate what a reviewer reviews.

### Tier-2 engagement

Default: Tier-1 approval is **final**. Engage Tier-2 only on an explicit trigger — the author may request it after Tier-1 approval; the Tier-1 Reviewer may recommend it with reason but does not require it unilaterally; the operator may require it by explicit direction, which is mandatory. "Nontrivial", "cross-repo", "public enum", "proposal", or "executable config" alone do not suffice.

The Tier-2 packet must state one line: `Tier-2 needed because [specific changed boundary or unresolved architectural decision]`, or `operator-required [instruction/reference]`. Tier-1 may ask the operator to waive a standing broad policy, but must not silently override an explicit requirement.

Good triggers: changed trust/privilege/credential boundary; new publishing/deployment authority; destructive/shared-state migration or rollback model; untrusted-input confinement/execution; significant new cross-subsystem API/precedence/ownership invariants; or an explicit operator/reviewer request naming a concern. A security referral must name WHAT authority changes or what invariant needs architecture judgment.

Usually Tier-1-only: established-pattern command flags, pins/config defaults, local/CI parity, naming/docs-only changes, regression fixes, and schema/spec carry-forward where no substantive boundary changes. These can be important or bug-prone and still belong at Tier-1. Escalate uncertainty with a concrete question, not automatic referral.

No mandatory Advisor pre-check — that moves the bottleneck. Optional one-paragraph triage (no full packet/validation duplication) only for a genuinely ambiguous trigger.

Re-reviews: state changed files/contracts plus finding addressed plus evidence identity. Autosquash-only/base-unchanged cleanups use the integrator handoff rule and do not return for technical review; semantic changes or destructive spec sync need their real delta reviewed. Related packets may share one evidence attachment with separate verdicts.

Tier-1 recommendations for escalation carry a reason; the author decides whether a second gate engages, except an explicit operator requirement, which is obeyed. Coordination-scale escalation across lanes may also be decided by the coordinator. An engaged second gate receives the change as-is. All routing is owner-dispatched; no reviewer routes to another reviewer. Tier-1 disposition states the verdict, blocking items (file:line, property, discharge), the coverage ledger, and any escalation recommendation with reason.

### Role boundaries

- Reference comparison for behavioral/control claims is a Tier-1 obligation; an engaged Tier-2 is an independent architecture/security counter-check, not the exclusive home for it and not a reason for Tier-2 on every change. Both tiers may inspect the whole change.
- Tier roles are defined by pipeline position (first pass vs independent counter-check), never by model strengths or weaknesses; a model swap must not require a guidance rewrite.
- The coordinator is not a review-state audience: operator rulings are asked of the operator directly; coordinator relay of a ruling is courtesy, not channel.

After Tier-1 approval:
- If Tier 2 is not engaged, that approval is **final**.
- If Tier 2 is engaged, send a separate packet to the Tier-2 Reviewer. Hold autosquash until that reviewer approves, so the Tier-1 hashes stay visible. Tier-2 findings are not merge authorization.
- Route a Tier-2 fix packet back to the Tier-2 Reviewer. Also route it to the Tier-1 Reviewer when the fix changes the Tier-1 contract. Routing is not a limit on what either reviewer may inspect.
- Integrator handoff waits until every engaged tier has approved. That approval is **final** for autosquash. The handoff names every engaged tier and states that each has approved.
- A non-identical rebase (conflict resolution, content edits, or a non-equivalent cumulative diff) returns to the Tier-1 Reviewer first. Re-engage Tier 2 only after that new approval, and only if Tier 2 had been engaged. Do not send that stack straight to the integrator.

## Evidence contract

- Reference-first for behavioral claims: cite file:line at the reviewed tip (version/hash), the candidate lines, and the delta. Substantive evidence, not token citation. A behavioral-comparison claim without a checked reference is asserted, not verified.
- Supplied references are untrusted discovery aids (packet summaries, control comments, author references). Check each claim at the reviewed tip; never inherit verified from the packet.
- Verify each policy conjunct separately. Check assertion values against the produced type.
- Name lifetimes and fd ownership (closer, waiter, bound).
- Absence claims need enumeration plus scope bound plus the residual channel not excluded.
- Verified requires a named basis: inspection, execution, or both, plus the exact SHA/reference or command plus receipt, plus what it establishes and does not. Execution verifies the observed result only, not historical attribution, completeness, or old-new fidelity.
- Verify the consequence, not the composition: check the effect on the quantity the contract names — a composition fitting inside a bound can still shrink what the bound permits.
- A pin must reach the claimed branch: confirm the control executes the claimed path (call counts, recorded effects, path-taken assertion) and would fail if the property were removed. A pin passes vacuously when the fake itself supplies the tested property or the exercised path bypasses production code (including early-exit paths). Production-path tests that use a fake only as a collaborator for an adjacent dependency, while the claimed path executes in production code, are valid.
- An assertion of a property is a claim, not evidence: verify comments, test names, and doc sentences against code at the boundary where the property is checked.
- When a standing rule and newer direction conflict, surface the conflict to the owner before acting on the newer direction.

## Review cycle

0. Operator pre-review. The author sends the scoped proposal to the **operator** for pre-review before any Tier-1/Tier-2 dispatch. Foundational objections are cheapest here; do not spend tier review on direction the operator would reject. Proceed on operator go-ahead.
1. Author implements the scoped change, runs validation, and creates local/private review commit(s) so the diff is hash-stable and hook-checked.
2. Author rebases onto the agreed `<local-integration-base>` and sends a review packet to the **Tier-1 Reviewer** only.
3. Tier-1 Reviewer approves or requests changes.
4. If changes are requested: author addresses them with fixup commits or first-class follow-up commits (see below), then sends an **updated** review packet to the Tier-1 Reviewer. Return to step 3.
5. When the Tier-1 Reviewer **approves**, apply the tier rule above. Autosquash only after **final** approval.
6. **Base check after final approval (required distinction):**
   - If `<local-integration-base>` is **unchanged** since the approved packet and only autosquash rewrote hashes: hand the cleaned stack to the **integrator** for merge (merge handoff with updated commit list; no repeat technical review).
   - If the base **advanced** and the author rebases onto the new base:
     - **Byte-identical stack** (cumulative change equivalent to the last approved tip): **no** dedicated technical re-review. Author verifies with `git range-diff <old-base>..<approved-tip> <new-base>..<rebased-tip>` (or an equivalent check) and confirms the output reports only equivalent (`=`) commit pairs — not an empty command result. Author **re-runs lints and tests** after the rebase. The merge handoff must name both compared ranges and the identity result. Integrator still performs merge-safety checks (current base, conflicts, multi-lane interaction).
     - **Non-identical stack:** follow the non-identical rebase rule under Review tiers. Return to step 3.
7. Integrator merges (see Integrator flow). Merge/push only after explicit human approval.

Hold the unsquashed fixup stack for the entire technical review. Autosquash only after final approval, as the step that produces the stack the integrator merges when the base has not moved.

The agreed `<local-integration-base>` is a Git ref in the current repository, such as local `master` or a local lane integration branch. It is not a filesystem path and is not a remote-tracking ref. Do not use `origin/master`, another `origin/*` ref, a path like `/path/to/repo/master`, or a raw commit hash as the rebase base unless the coordinator explicitly names that exact ref or hash. The exception is lane-clone syncing, which pulls from `origin/<integration-base>` as described below; that upstream is named by the clone topology, not by per-review direction. When in doubt, ask for the local branch/ref name before rebasing. Confirm it with `git branch --list <local-integration-base>` or `git rev-parse --verify <local-integration-base>` before running `git rebase`.

## Integrator flow

1. Confirm the stack is approved by every engaged tier and already cleaned (fixups autosquashed). If fixups are still present, send it back to the author to fold before merge — do not start a content review. This is merge-safety only (base currency, conflicts, approvals named), not content review.
2. Fetch the approved work from the author's repository by filesystem path:

   ```sh
   git fetch /path/to/author/repo <source-ref>
   ```

   Repositories in the clones-of-clone topology do not share objects. A commit hash is not resolvable in your repository until you fetch from the repository that holds it. The packet must name the author's repository filesystem path and the source branch/ref (the reviewed branch in the author's repository) — not the destination. Take `<source-ref>` from the packet's source branch/ref; do not hardcode a branch and do not substitute the destination merge target. After fetching, verify FETCH_HEAD equals the current cleaned handoff tip hash; on mismatch, stop and route back to the author. An equivalent rebase qualifies only via a refreshed handoff (new hashes, compared ranges, fresh validation) under step 6, not as an exception at fetch.
3. Confirm the fetched stack is based on the **current** `<local-integration-base>`, and that the handoff names every engaged tier and states that each has approved. If the base has advanced (stale-base stack), **refuse the merge handoff** and route the author to rebase first. After rebase: a byte-identical stack may return as a merge handoff with fresh lint/test evidence; a non-identical stack follows the non-identical rebase rule under Review tiers. Do not merge a stale-base cleaned stack.
4. If the base is current and the merge is otherwise clear, merge the fetch head with `--no-ff` when preserving a delegated-work or lane boundary; this creates a clear integration point and avoids mutually rebasing branches into increasingly long histories.
5. When the branch owner is also the project owner, the owner merges the reviewer-approved cleaned stack directly after human approval, without a coordinator handoff.
6. Merge/push only after explicit human approval.

Prefer reviewing commits by hash. Always give the path to your repository as well: its objects are not shared with the reviewer's repository, so hashes alone are not reviewable until fetched. Use patch artifacts only as a fallback when the reviewer cannot access the repository directly.

# Review Request Packet

For non-trivial delegated work, review requests should include:

- Base ref for rebase: the `<local-integration-base>` to use for `git rebase` or `git rebase -i --autosquash`.
- Intended merge target: the branch/ref where the work should eventually land (destination). This may differ from the rebase base, and may be a shared branch.
- Source branch/ref: the reviewed branch in the author's repository that the integrator fetches. Name it separately from the destination; fetching the destination name from the author's repository can fetch the wrong stack.
- Author's repository filesystem path: the path to the author's repository, required for fetching the work in topologies without shared objects.
- Complete commit list with hashes and one-line descriptions.
- Validation commands run and results, including skipped checks or known gaps.
- Intended contract: what must be true after the change lands.
- For each added test/control: claim established; referenced behavior/contract (version/hash plus location); relevant ordering/setup; assertions that actually prove it; limits. New behavior may cite the approved contract, not invent a retired implementation.
- Coverage requests: a blocking request for more coverage must name a credible failure path, material consequence, and gap in existing coverage; otherwise mark it nonblocking. Authors may answer with existing evidence, a smaller test, or a simpler design; reviewers retain full inspection scope. For new integration/smoke tests, state what contract requires that boundary, why a smaller test cannot verify it, and the material regression consequence the boundary guards. Follow the canonical test design rules in `.auxiliary/agents/standards/tests.md` rather than duplicating them here.
- Review concerns, if any: genuine uncertainty or risky areas only.
- Known risks, accepted tradeoffs, deferred items, or intentional branch staleness.

Author-provided review concerns are supplemental context, not a limit on review scope, and not an instruction the reviewer must follow. Independent inspection remains the reviewer responsibility.

Packets to the **integrator** after final approval are merge handoffs: cleaned commit list, validation status, base/merge refs, author's repository filesystem path, source branch/ref, and every engaged tier with its approval. They apply when the base is unchanged since final approval (autosquash-only hash changes are fine), or after a post-approval rebase that leaves a **byte-identical** stack with author re-validation noted. They are not a review packet. A non-identical post-approval rebase follows the non-identical rebase rule under Review tiers, not a merge handoff.

# Reviewing Stacked Commits

When feedback targets one specific commit in the current review stack, use `git commit --fixup <target-hash>`. This applies even when the review stack has only one commit. Do not directly amend reviewed commits while review is in progress; fixup commits preserve review visibility until the stack is ready for final cleanup.

A fixup is valid only when the entire fix belongs to code introduced by one target commit in the current stack. Prefer a fixup wherever one is valid: fixups vanish at fold time, leaving the target's substantive message intact, while narrative fix commits ("round 2", "review feedback") persist as noise in the folded history. Use a first-class follow-up commit when the fix touches code that is already merged to the base branch, when the fix spans or refactors code introduced by two or more in-stack commits, or when the operator requests a distinct design change. Name the review finding or rationale in the first-class commit message or review reply so reviewers know why it is not a fixup.

The author holds targeted fixups in place until **final** approval. Do not push a commit to `origin/master` (or any protected merge target) while any review of it is still in progress: autosquash rewrites hashes, which protected branches forbid, so a reviewed stack pushed mid-review strands its fixups unfusable. Push only the final cleaned stack, and only with explicit human approval.
 Each fixup stays visible on the branch in the context of its target commit, which preserves review visibility for the response. Do **not** autosquash before or during technical review — that rewrites hashes, stales the packet under review, and hides which fix addressed which finding. After final approval, the author autosquashes into the target commits.

Distinguish post-approval hash changes:
- **Autosquash only, base unchanged:** include the new commit list and validation status on the merge handoff to the integrator. No repeat technical review.
- **Rebase onto an advanced base, byte-identical stack:** merge handoff is allowed after author re-runs lints/tests and records identity verification: both `git range-diff` ranges and that the output shows only equivalent (`=`) pairs (or another specifically identified equivalent check). No dedicated technical re-review.
- **Rebase onto an advanced base, non-identical stack:** follow the non-identical rebase rule under Review tiers. Obtain that approval again before any merge handoff.

Fold the stack with `--autosquash`, which requires `-i` explicitly — `--autosquash` alone is a silent no-op. Use `<local-integration-base>` as the rebase base.

### Inspect before applying

`git log <local-integration-base>..HEAD` shows current commit order, not the autosquash-reordered todo. To print the prepared plan without applying it, point `GIT_SEQUENCE_EDITOR` at a small unique script file that prints the todo file contents to stderr and exits non-zero:

```sh
show_todo=$(mktemp)
printf '%s\n' 'cat "$1" >&2' 'rm -f "$0"' 'exit 1' > "$show_todo"
GIT_SEQUENCE_EDITOR="sh $show_todo" \
  git rebase -i --autosquash <local-integration-base>
```

The preview aborts before rewriting history, but it may leave a `.git/rebase-merge` directory behind. Check `git status` after previewing; if a rebase is in progress, run `git rebase --abort` to return to the pre-preview tip before the `Apply the fold` step. Prefer a script file over nested-quote one-liners in `GIT_SEQUENCE_EDITOR`; agent harnesses often break the latter.

### Apply the fold

In agent environments, apply the prepared plan non-interactively:

```sh
GIT_SEQUENCE_EDITOR=true git rebase -i --autosquash <local-integration-base>
```

`GIT_SEQUENCE_EDITOR=true` accepts the autosquash-prepared todo unchanged and continues. It rewrites the branch; it is not a preview.

In an interactive human terminal you may instead run `git rebase -i --autosquash <local-integration-base>` and edit the plan in your editor.

If the result is wrong, recover with `git reset --hard ORIG_HEAD` — git sets `ORIG_HEAD` to the exact pre-rebase position regardless of how far back `<local-integration-base>` was.

For example, run `git pull --rebase origin master` from the clone when the coordinator says to sync with local `master`. Do not write `git rebase /path/to/repo/master`; that is a filesystem path, not a Git ref.

If `git commit` fails because a hook rejects it, assume no commit was created unless Git clearly reports otherwise. Fix the hook finding, restage the intended files, and rerun the same `git commit` command. Do not use `git commit --amend` to recover from a failed commit attempt.
