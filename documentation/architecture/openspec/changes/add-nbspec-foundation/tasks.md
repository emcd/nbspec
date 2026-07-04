## 1. Foundation

- [x] 1.1 Add `nb-api` dependency (crates.io 0.1.0 — published before
      implementation began, superseding the planned pinned-rev git
      dependency) and `clap` to Cargo.toml
- [x] 1.2 Implement CLI skeleton over library-boundary core functions:
      `nbspec create`, `nbspec display`, `nbspec render`, `nbspec merge`,
      `nbspec validate`
- [x] 1.3 Implement the OpenSpec grammar module: requirement/scenario/delta
      parsing (shared by validation and diagnostic mapping)

## 2. Notebook Data Model

- [x] 2.1 Implement schema resolution (meta note → project config → nbspec
      default) and workflow schema parsing (TOML `schema.toml`; artifacts,
      `generates`, `requires`)
- [x] 2.2 Author the nbspec default schema, embedded in the binary:
      proposal, specifications, designs, decisions (reserved); no tasks
      artifact; `generates` targets defaulting to
      `documentation/{specifications,designs,decisions}`
- [x] 2.3 Implement change namespace conventions (`proposals/<change-id>/`
      folders with `specifications/`, `designs/`, `decisions/` subfolders,
      note naming, tags)
- [x] 2.4 Implement meta note read/write with JSON schema, status lifecycle,
      and schema selection
- [x] 2.5 Implement `nbspec create` (scaffold namespace + meta +
      artifact notes; no filesystem writes)
- [x] 2.6 Implement `nbspec display` (short and `--full` forms)
      (artifact readiness per `requires` graph, `work` todo progress)

## 3. Rendering and Merge

- [ ] 3.1 Implement deterministic scratch-workspace rendering to schema
      `generates` paths (repository never touched)
- [ ] 3.2 Implement review output: rendered tree plus unified diff suitable
      for external review tooling (e.g., difit)
- [ ] 3.3 Implement `work` todo note grammar and parser for status reporting
- [ ] 3.4 Implement `nbspec merge`: transfer durable artifacts (new
      documents) to configured targets with provenance headers and content
      hashes; error on MODIFIED deltas (deferred capability)
- [ ] 3.5 Implement drift detection on merge targets with `--force`
      override; surface drift in `nbspec display`
- [ ] 3.6 Implement merge-time change archives: deterministic tar + zstd of
      rendered tree, meta, and `work` snapshot to the configured archive
      directory; configuration toggle; warn when `.gitattributes` lacks an
      LFS rule for the archive path

## 4. Validation

- [ ] 4.1 Implement native validation rules over the grammar module
      (requirement structure, scenario presence, delta operations,
      schema-required artifacts)
- [ ] 4.2 Implement note-level diagnostics (note name, artifact id, line
      reference where feasible)
- [ ] 4.3 Exit-code and output contract for agent consumption
- [ ] 4.4 Add optional CI conformance oracle: pinned upstream
      `openspec validate --strict` against grammar fixtures rendered into
      the upstream `spec-driven` layout (informational, dev-time only)

## 5. Quality

- [ ] 5.1 Unit tests: schema parsing, todo grammar, provenance/drift, meta
      lifecycle
- [ ] 5.2 Integration tests: end-to-end create → author → render →
      validate → merge against a fixture notebook
- [ ] 5.3 Update README status section; document CLI usage
- [ ] 5.4 `cargo clippy --all-targets --all-features -- -D warnings` and
      full test suite pass

## 6. Dogfooding Transition

- [ ] 6.1 Gate: agents-common provides, and this project enables, a
      project-level opt-out for OpenSpec tree/symlink population (tracked
      at agents-common:todos/template/10)
- [ ] 6.2 Remove OPSX artifacts and the bootstrap `openspec` symlink;
      author successor changes through nbspec
