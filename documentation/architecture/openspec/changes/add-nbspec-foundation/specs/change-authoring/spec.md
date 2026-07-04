## ADDED Requirements

### Requirement: Notebook change namespace
nbspec SHALL store each change in its project notebook under a deterministic
folder namespace `proposals/<change-id>/`, containing a `proposal` note, a
`meta` note, a `work` todo note, and schema-defined artifact subfolders — by
default `specifications/<spec-name>`, `designs/<design-name>`, and
`decisions/<adr>` (reserved).

#### Scenario: Deterministic note layout
- **WHEN** a change `add-foo` is created with the nbspec default schema
- **THEN** the notebook contains `proposals/add-foo/` with a `proposal` note,
  a `meta` note, a `work` todo note, and `specifications/`, `designs/`, and
  `decisions/` subfolders per the schema's artifact list

#### Scenario: One change, one namespace
- **WHEN** two changes exist in the same notebook
- **THEN** their notes reside in disjoint `proposals/<change-id>/` folders and
  operations on one change never read or write the other's notes

### Requirement: Schema-driven artifact model
nbspec SHALL derive the artifact set, merge-target paths, and authoring
order for a change from an OpenSpec 1.x workflow schema (`schema.yaml`
artifact list and `requires` dependency graph), resolved in order: the
change's meta note, then project config, then the nbspec default schema.
nbspec SHALL NOT hardcode the artifact types of any particular schema.

#### Scenario: Custom schema honored
- **WHEN** a change's meta note names a custom schema whose artifact list
  omits delta specifications
- **THEN** nbspec creates and renders only the artifacts that schema
  defines, and does not require specification notes for the change

#### Scenario: Authoring order follows dependency graph
- **WHEN** an agent asks nbspec which artifacts are ready to author
- **THEN** nbspec reports artifacts whose schema `requires` dependencies are
  satisfied, in graph order

### Requirement: Meta note control plane
nbspec SHALL maintain a `meta` note per change containing a JSON object with
at minimum: change id, title, status, schema name, source notebook,
creation/update timestamps, and a schema version field for the meta format
itself. Status SHALL follow the lifecycle
`draft -> approved -> implemented -> archived`, with side states `blocked`,
`superseded`, and `abandoned`. The meta note SHALL record project-repository
commit SHAs at status transitions that correspond to repository writes.

#### Scenario: Status transitions recorded
- **WHEN** a change moves from `draft` to `approved`
- **THEN** the meta note's `status` field is updated and `updated_at` is
  refreshed, and no other note content is modified

### Requirement: Change lifecycle CLI
nbspec SHALL provide CLI verbs to create a change (`nbspec change new`),
inspect its content (`nbspec change show`), and report its artifact, todo,
and drift state (`nbspec change status`). Creating a change SHALL NOT write
to the repository working tree.

#### Scenario: Creating a change
- **WHEN** `nbspec change new add-foo --title "Add foo"` runs against a
  project notebook
- **THEN** the notebook namespace `proposals/add-foo/` is created with a
  populated `meta` note and empty artifact notes per the resolved schema,
  and the repository working tree is unmodified

#### Scenario: Status of a change
- **WHEN** `nbspec change status add-foo` runs
- **THEN** the output reports which artifact notes have content, which are
  blocked by unsatisfied dependencies, `work` todo progress, and the meta
  status
