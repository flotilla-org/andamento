# Embedding the sidebar core

`andamento-core` owns metadata, placement evaluation, template resolution and
activation decisions. `Sidebar` is the interface for a new presentation
client. Zellij's existing adapters use the extracted `ControllerState` to
retain plugin registration and rail synchronization behavior.

The terminal implementation lives in `andamento-terminal`; the plugin in
`andamento-rail` supplies its Zellij theme, cell size and event handling.
`andamento-shared` re-exports the core types for existing plugins and keeps
host filesystem/config adapters. Neither the core nor terminal crate links
Zellij or Flotilla.

## Rust interface

Create `Sidebar::new(config_kdl)` once per presentation client. Config parsing
does not perform filesystem access. A failed `configure` leaves both effective
catalogs unchanged.

The host drains incoming metadata patches into `apply(now_ms, patches)`, then
requests one `snapshot()`. Time is monotonic milliseconds local to the client;
an empty batch advances expiry, and a backwards clock input cannot revive an
expired value. Patches retain the existing producer schema and namespaces.
The legacy Zellij adapter retains its existing receipt-clock behavior during
this extraction; adopting a wall-clock expiry scheduler there is separate work.

`observe(workspaces, panes)` supplies a full local topology snapshot, including
the selected workspace. Workspace IDs are `WorkspaceId`s: 128-bit IDs the host
supplies (see [Workspace IDs](#workspace-ids)); positions retain the
existing navigation order. A host
without pane observations can start with an empty pane list. Pane observations
currently retain Zellij's terminal/plugin `u32` identity through the core's
`PaneTarget`, not just the C header. Wheelhouse's 64-bit `CFG_ID`s and general
views must not be truncated or relabelled as plugins. Use workspace observations
with an empty pane list for the fixture slice; generalise identity through the
core and adapters before integrating native panes ([#95](https://github.com/flotilla-org/andamento/issues/95)).

`dispatch(Action)` returns host effects. Focus carries a workspace ID;
materialize carries an entity, name, recipe and optional working directory.
The core chooses recipes from its resolved facts, not from frontend-provided
command text. An entity without an available presentation action returns an
inspect effect. The host decides how to show that information.

Subject activation returns `HostEffect::OpenUrl`; `Action::CopySubjectUrl`
returns `HostEffect::CopyUrl`. `Sidebar::subject_url` resolves a subject's
single `flotilla.forge` reference, reads the forge's change-request or issue
URL template, and substitutes `{web_url}`, `{scope}`, and `{number}` from the
current resolved facts. Missing facts, unknown placeholders and non-HTTP(S)
results produce no URL. Open/copy effects need no completion.

The C ABI retains its existing effect structure: `ANDAMENTO_EFFECT_OPEN_URL`
and `ANDAMENTO_EFFECT_COPY_URL` carry the URL in `recipe`. A subject row with
a resolvable URL sets `openable`; dispatch its normal activation to open it.
`AndamentoNode.copy_url` is a snapshot-scoped copy action, or
`ANDAMENTO_NONE`; dispatch it through the usual retained-snapshot validation.
`andamento_copy_subject_url` also accepts an entity directly. Hosts execute browser and clipboard operations.

These facts require a fleet generation carrying the projection from
[flotilla#2448](https://github.com/flotilla-org/flotilla/pull/2448), merge commit
`d882cd2d3a5511bbb191863fe004db181cdc986f` or a descendant. The downstream
Wheelhouse sidebar adoption is tracked in wheelhouse#137.

Complete every focus/materialize effect through `complete(request_id, result)`,
including timeouts and cancellation. Materialize success supplies the new
workspace ID; focus success supplies none. Continue supplying observations
after acknowledgement. Observation and acknowledgement can arrive in either
order. Duplicate acknowledgements are harmless. A failed materialization
remains visible with an error and can be retried.

Repeated activation while an operation is pending does not start another
operation. The core does not invent a multi-workspace chooser or MRU policy.
The host must establish the new workspace's entity identity before starting
its recipe; return the ID as part of that creation path.

## Snapshot and rendering

The new `presentation::SurfaceSnapshot` contains sections and placement nodes.
Each node carries its entity reference, placement key, ordered resolved fields,
controls, effective variables, template provenance, detail content and children.
It also carries catalog/latent/opening/live state and a host workspace ID when
live. Children remain in the snapshot when collapsed; frontends determine
which geometry to display.

Fields retain their typed metadata source as well as resolved display text.
Logical loop layout travels as intent such as `inline`; it is not a character
width or pixel rectangle. The frontend owns measurement, wrapping, drawing,
hit testing and viewport state. Terminal text and HTML are real consumers of
the same snapshot and semantic actions.

Placement variables use placement keys. Display-variable declarations and
values accompany the snapshot. `SetVariable` validates the declaration and
allowed values; invalid requests do not mutate state. Independent `Sidebar`
instances have independent UI state. Existing Zellij rail synchronization
continues through its adapter and is not exported as cross-process cooperation.

Placement is the default in every host after #71. Templates query the catalog
and resolve directly into the semantic snapshot; the legacy renderer adapter
and grouping configuration are removed. Sections may declare a placement query
or contain only static content/controls. Unmatched catalog entities are listed
in the existing Inspect surface pending #72. See the shipped
`templates/flotilla-default.kdl` and the native fixture
`crates/andamento-core/tests/fixtures/sidebar.kdl`.

## C ABI

The header is `crates/andamento-ffi/include/andamento.h`. Build the library for
the native host target, overriding this repository's WASM default:

```sh
cargo build -p andamento-ffi --target aarch64-apple-darwin
```

It produces `libandamento_ffi.a` and a dynamic library in the native target's
debug directory. The C interface is ABI version **4**, which
[The host interface](host-interface.md) maps, with what changed from ABI 3.
Each call has one form and no earlier version is accepted. It is not yet a
frozen embedding contract.

### Workspace IDs

A Workspace ID is 16 opaque bytes (`AndamentoWorkspaceId`, `WorkspaceId` in
Rust). The host generates it, a UUIDv7 in practice, when it first saves a
workspace. Andamento only compares, orders and prints IDs (as hyphenated
UUIDs, which is also their JSON form) and never generates one, so the core
stays deterministic for replay. Hosts whose workspaces are numbered tabs (the
Zellij and tmux hosts) use `tab_workspace_id(tab)`, which puts the tab number
in a UUID.

Users create workspaces that never go through MATERIALIZE. The host declares
them with `andamento_workspace_register` (`Sidebar::register_workspace`); a
successful MATERIALIZE completion registers its workspace too. Registration is
independent of topology: either may come first, and closing a workspace does
not forget it. Forget a workspace the host deleted rather than kept.
Registration changes neither the snapshot nor its revision; it gives the
workspace a record (see Records below).

### Providers

An entity is `{provider, kind, id}` (`EntityRef` in Rust). The provider is the
Dashboard's **subscription ID** (wheelhouse ADR 0012), stable and owned by the
Dashboard; the host supplies it per patch source. A producer never names it,
and the identity a provider reports is an attribute of the subscription, not
part of any key. The same kind and ID under two providers are two entities:
two rows, two placement keys, two records, two activation targets. Andamento's
own sections, groups and refs have the provider `local` (`LOCAL_PROVIDER`),
which no host subscription may use.

- **Stamping.** `apply(now_ms, provider, patches)`
  (`andamento_apply_patch_json`, `andamento_apply_entity`,
  `andamento_apply_workspace`) requires a provider and stamps it over every
  entity a patch names: its target, entity references among its values, and
  the entity a workspace's `entity.kind`/`entity.id` (or
  `.host.kind`/`.host.id`) facts name, recorded as `entity.provider`
  (`.host.provider`). Any provider the patch itself names is overwritten.
  Every entity a host passes otherwise (to `andamento_local_set`, sibling
  orders, detail lookups) names its provider.
- **Joins stay within a provider.** A placement loop that matches on a bound
  entity (`match "<key>" of="<loop>"`) only matches that entity's provider's
  entities when it is not `local`, so two subscriptions' projects named `p`
  each hold only their own vessels. Joins on local sections and groups span
  providers.
- **Activation targets.** A `local` entity's fallback activation target stays
  `kind:id`; any other provider's is `kind:id@provider`, so the same kind and
  ID never alias across providers. Explicit `action.primary.target` facts are
  producer text and are not qualified.
- **Retraction.** `retract_provider` (`andamento_provider_retract`) removes
  every fact a provider contributed in one call, as when its subscription is
  removed. It reuses the retained/ended machinery: subjects on open workspace
  paths and the targets of local refs (pins) are retained with their last
  facts, a workspace bound to its subject by that provider's facts keeps the
  binding, and nothing is marked ended. Closing the workspace or removing the
  ref lets the retained entity go.
- **Stale facts.** `set_provider_stale(provider, true)`
  (`andamento_provider_set_stale`) is for a dropped connection, instead of
  waiting for TTLs. A stale provider's facts are kept as they were when it
  became stale: those live then stay live, and no TTL expires them while it
  stays stale; facts that had already expired stay expired. Snapshot nodes
  presenting its entities (a ref presenting its target included) set
  `AndamentoNode.stale`. Marked fresh again, each TTL fact that was live
  renews its lease from that moment, as though the provider had reasserted
  it, so the reconnected stream has one TTL to reassert it before it expires.
  Facts without a TTL are unaffected either way. Retraction clears staleness.

Placement keys carry the provider. `PlacementLoopKey::encode`
(`AndamentoNode.loop_key`) is length-prefixed parts with each parent
segment's loop name, provider, kind and ID. The opaque node key
(`AndamentoNode.key`) includes each segment's provider.

### Records

Andamento owns the sidebar's logical state (wheelhouse ADR 0012) and hands it
to the host as **records**: opaque bytes (serde JSON today) that the host
stores as they are and gives back on the next start. Their form is private to
Andamento and is not versioned or migrated. The host decides where and when to
store them; Andamento never touches a filesystem, so this works in Zellij's
sandbox and under WASM too.

| Record | Holds |
|---|---|
| `dashboard` | display variables that persist, row (placement) collapse, sibling orders, placement variables set on rows, local sections, groups and refs, and the sidebar arrangement ([The sidebar arrangement](sidebar-arrangement.md)) |
| `workspace/<id>` | for each registered workspace: its subject and the entities on its path as last seen, its slots (the cached Suggested Layout baseline, the user's slots, name and mood, and what the provider removed), its arrangement document, and its slots' saved portable Target Resolutions ([Slots and arrangements](slots-and-arrangements.md), [Workspace Overlay](workspace-overlay.md)) |

Scroll offset, display variables declared `persist=false`, and section collapse
are presentation state and are not recorded.

- **Retained and ended subjects** are recorded with the least needed to draw
  their rows with no facts: identity, label, whether they ended, when they were
  last seen (the host's `now_ms` when their facts last appeared, changed or went
  away), and the facts placement reads (loop matches, `in` lists, order keys and
  visibility rules), so a row is drawn on its path, not in the fallback section.
  The full fact set is not saved: producers republish it.
- A workspace's record follows it while it is open; a closed workspace keeps
  the record it had, and forgetting the workspace drops it.
- **Generations.** `record_generation` (`andamento_record_generation`) is a
  nonzero counter that changes whenever the record does, and is cheap to poll.
  Store a record when its generation differs from the one last stored. All
  generations come from one clock per sidebar, so none is reused, not even by
  an import; they are never persisted.
- **Import** (`import_record`, `andamento_record_put`) may come before the
  first observation and needs no facts. Importing a workspace record registers
  the workspace and binds it to its subject, as a materialize completion does;
  when the workspace is observed, its recorded path is drawn until a producer
  publishes the subject again. A record that doesn't read, or names another
  record, is rejected without changing anything.

A host that shows the Dashboard's own state reads it with
`andamento_dashboard_get` rather than from the record: its local entities with
their facts, and its sibling orders by loop key.

Display variables are set directly with `set_display_variable`
(`andamento_set_display_variable`): no snapshot action, so restoring one needs
no retry. Local sections, groups and refs are set and removed with
`set_local`/`remove_local` (`andamento_local_set`/`remove`), which replace an
entity's facts in full; see Sections, groups and references.

### Update and ownership rules

Create one opaque `Andamento` handle per presentation client. Serialize calls
on that handle, usually by queueing transport events onto the UI thread.
Input arrays and UTF-8 byte strings are borrowed only during each call; they
need not be NUL-terminated. Every mutation returns success/failure and an
optional owned error string, freed with `andamento_string_free`. Invalid input
is rejected before mutation. A caught panic poisons the sidebar; recreate it.

Use typed `andamento_observe` for full workspace/pane topology and
`andamento_complete` for host outcomes. Apply incoming facts and completions,
then explicitly call `andamento_snapshot_acquire` once for rendering. Mutations
do not produce output snapshots. Action validation can still resolve state
internally; this interface does not promise incremental evaluation. Caching and
conditional resolution are tracked in [#96](https://github.com/flotilla-org/andamento/issues/96).

The snapshot owns its nodes, fields, controls and borrowed text until
`andamento_snapshot_release`. It survives updates to or destruction of the
sidebar. Nodes are in preorder with parent indices; section nodes have no
parent. Collapsed children remain present. Placement keys are opaque stable
widget identities, not strings for the host to parse. The host owns geometry,
measurement, hit testing, scrolling and drawing.

The initial native projection exposes resolved field text/class/priority,
main and detail fields, main controls, layout/form intent, collapse state,
and catalog/latent/opening/live state. Display controls include their resolved
label, icon and typed current value. Template provenance, raw facts, placement
variable editors, detail controls and chrome primitives remain available in
Rust but are not yet projected into C. Add them against actual native call
sites; do not reconstruct the template resolver in Wheelhouse.

Nodes expose activation/collapse action references; display controls expose
their own action references. Pass the snapshot and reference to
`andamento_dispatch`. References are snapshot-local, and dispatch rejects a
snapshot from another client or from before a revision-changing mutation.

Capture the action reference together with the displayed snapshot. Process
captured clicks against that snapshot before draining queued facts, ticks,
topology and completion updates, then render the next frame. If a revision-changing mutation
already intervened, dispatch rejects the click: discard it and redraw. Do not
re-hit-test the old click's coordinates against the new arrangement, since
that could activate a different entity. Only a fresh user interaction is
hit-tested against a new frame. Never reuse an action index against a different
snapshot. The conservative revision includes recipe and other fact changes even if the
visible content is unchanged. Unchanged heartbeat renewals, ticks that cross no
expiry deadline, identical topology observations, unknown completions and failed
input validation preserve actions. Retaining an older snapshot keeps its text
alive, but does not make stale actions valid.

After draining an update batch, call `andamento_snapshot_is_current` with the
currently displayed snapshot. It returns 1 when both client and revision match;
otherwise acquire a replacement. NULL or other-client snapshots return 0 without
an error; invalid or poisoned clients return 0 with an error. The query does no
snapshot construction. The shared Rust `Sidebar` retains one immutable snapshot
per revision (`snapshot_shared` borrows it; `snapshot` makes an owned clone).
Expiry lookup uses a deadline index whose entries are replaced on lease renewal.

Dispatch queues tagged host effects. `andamento_effects_take` drains them into
an independently owned batch; a second take returns an empty batch. Execute
focus/materialize locally and complete every request, including failure,
cancellation and timeout. An inspect effect needs no completion. Copy strings
needed asynchronously or retain the batch until finished. Releasing a batch
does not complete its requests, and acquiring snapshots does not drain effects.

A typical update is:

```c
/* Apply queued facts, topology and effect completions first. */
AndamentoSnapshot *frame = andamento_snapshot_acquire(sidebar, &error);
/* Read typed nodes/fields/controls and build native widgets. */
/* On a hit, dispatch the action reference from this exact frame. */
andamento_dispatch(sidebar, frame, hit_action, &error);
AndamentoEffects *effects = andamento_effects_take(sidebar, &error);
/* Execute effects, retaining/copying any data needed asynchronously. */
andamento_effects_release(effects);
andamento_snapshot_release(frame);
```

Check each result before continuing. The complete, compiled consumer is
`crates/andamento-ffi/tests/smoke.c`; the header specifies tags, pointer validity,
nullable arguments and ownership for every operation.

### Facts, encoding and transport

The semantic producer model, its encoding and its transport are separate
choices. Rust accepts typed `MetadataPatch` values. The native scalar
`andamento_apply_entity` convenience interface supports text, boolean and
integer entity facts, unsets, TTL, precedence and ordinal without encoding
those values. It intentionally does not duplicate every producer target and
value type in the C header.

`andamento_apply_patch_json` is an optional decoder for one existing producer
patch, parsed inside Rust. It accepts no local request envelope and produces
no JSON response. It is enabled by the default Cargo feature `json`; build
with `--no-default-features` to omit the entry point. Other producer targets,
lists and path values currently use that decoder or Rust's typed interface.
A future CBOR decoder can feed the same model without changing the renderer
or host-effect interface. Neither JSON nor HTTP/UDS is a core requirement.

`andamento_tick` advances expiry when no facts arrive. Supply monotonic
milliseconds scoped to this client. JSON `Request`/`Response` replay tooling
remains available in Rust/HTML, but does not define the embedding interface.

## Proof and validation

```sh
cargo run -p andamento-html --target aarch64-apple-darwin -- \
  crates/andamento-core/tests/fixtures/sidebar.kdl \
  crates/andamento-core/tests/fixtures/sidebar.jsonl > /tmp/andamento-sidebar.html
python3 scripts/check-independent-core.py
```

The HTML proof uses native HTML layout and escapes producer content. Controls
emit an `andamento-action` browser event and show its JSON. It is a snapshot
viewer, not a live host: core dispatch and receipt of the next snapshot belong
to its embedder. The optional third CLI argument is a JSONL file of core
requests to replay before rendering. The integration test feeds an action
from the terminal hit map back through the core and verifies that both
frontends reflect the resulting placement state.

The isolation script copies only the reusable crates and bundled templates
into a temporary workspace, verifies the dependency graph contains no Zellij
or Flotilla packages, runs its tests, checks the FFI without its JSON feature, then compiles and executes a C fixture
consumer. That consumer covers hierarchy, labels/status, controls, collapse,
materialize/focus, failure/retry, typed facts, expiry, stale/cross-client actions
and snapshot/effect lifetimes.
The plugin workspace itself still needs the sibling Zellij checkout for Cargo
workspace resolution. CI runs both the full plugin suite and the isolated check.

Wheelhouse's next slice is labels, status, collapse and local activation over
fixtures, then HTTP/UDS facts under wheelhouse#22. Use its existing layout and
workspace machinery. Preview implementation already exists in Wheelhouse;
declarative placement of previews remains follow-up work.

### Loop layout groups

Each Rust placement node exposes `loop_key`: the region, parent appearance and
binding of the loop invocation that produced it. Sibling items share this key;
repeated invocations under different parents do not. Sibling loop bindings must
be distinct and duplicate declarations are rejected during configuration.

C consumers read `AndamentoNode.loop_key`. The text is opaque, snapshot-owned
and empty for a section; compare it for equality only. It is also the key
`andamento_set_sibling_order` and `andamento_dashboard_get` use.
Group adjacent nodes by loop key and use the existing layout intent to choose
native geometry. Keep each node's activation/collapse reference and state when
presenting several items on one line. Width measurement stays in the frontend.

### Related entities in details

A detail template may declare `for` loops. They are evaluated for each placed
node, with the node bound under the template's name (`project` for
`project/detail`), exactly as an applied template binds its entity. Each match
renders the loop's own `field`s against its facts, and the fields are appended
to the node's detail content in loop and match order. They are detail text
only: they are not placed as rows, have no activation, and add no ABI.

Flotilla's project repository membership (flotilla#1897) uses this. Each
`project_repository` entity carries its project's `flotilla.project`, so
`match "flotilla.project" of="project"` lists a project's repositories. Its
facts are `flotilla.membership.repository_key`, optional
`flotilla.membership.repository_slug` and `flotilla.membership.subpath`, and a
`display.label` from the slug or key. Keys and slugs are identity or display
data, never template join keys. The project entity carries
`flotilla.project.repository_count`; zero means known-empty, and its absence
means the definition is unavailable.

### Workspace coverage

`Sidebar` snapshots always include a `.unplaced` section
labelled “Other workspaces”. It holds observed workspaces that have no live
placement in the resolved catalog presentation, unless a default group covers
them (see Sections, groups and references below). This section is an inventory
safety net, independent of template visibility filters. It is emitted even when
empty, because hosts anchor workspace creation to its header (Wheelhouse puts its
new-workspace button there). Frontends without such an affordance skip it while
empty (`Section::is_empty_workspace_fallback`). Collapsed descendants still count
as placements; frontends must preserve their expansion path.

Fallback keys use workspace IDs, never display names, and use the reserved
`.unplaced` loop and `.workspace` entity kind.
Renaming or selecting a workspace preserves its key. Provider removal, filtering,
and multiple workspaces sharing one entity must not hide inventory entries.

Native, HTML and terminal snapshot consumers activate rows with
`Action::ActivatePlacement`. Live placements focus their exact observed workspace;
other placements use the normal entity activation path. The C ABI exposes this
through the existing snapshot-owned activate action, with no ABI layout change.
Direct `Action::Activate` remains entity-oriented. Hovering or taking a snapshot
never materializes a workspace.

### Host entities

A host can give one of its workspaces an entity of its own: it publishes the
entity's facts like any producer, and tags the tab with `.host.kind` and
`.host.id` metadata (a patch targeting the tab). Wheelhouse does this for
local workspaces, as `.workspace` entities.

- Placements place a host entity like any other entity, so a template can home
  it, for example with `match "flotilla.project" of="project"`.
- Its rows are live for its tab, as a subject's rows are, so a placed host
  entity covers its workspace.
- An unplaced workspace with a host entity is covered under that entity rather
  than a synthetic `.workspace`, so its details resolve.
- It is not the tab's subject: closing the tab retains no path and marks nothing
  ended. The host retracts the entity's facts when the workspace goes away.
- A tab needs both keys; with only one it keeps the synthetic entity. When
  several tabs name one host entity, the first is covered under it and the
  rest under synthetic entities, so every tab keeps a distinct key.
- A tab may have both a subject and a host entity; each is live for it.

### System names

Names that start with `.` are Andamento's own system kinds and facts
(`presentation::system`). Conventions that producers share, such as
`display.label` and `flotilla.project`, keep their names.

**Renamed in this version.** These names replaced earlier ones, and the old
names are no longer read, so a host still sending them silently loses host
entity coverage:

| Old | New |
|---|---|
| `host.entity.kind`, `host.entity.id` (tab metadata) | `.host.kind`, `.host.id` |
| `wheelhouse.workspace` (host entity kind) | `.workspace` |
| `andamento.workspace` (synthetic workspace kind) | `.workspace` |
| `andamento.unplaced-workspaces` (leftover section and loop) | `.unplaced` |

### Sections, groups and references

People can make their own sections and groups, holding workspaces and references
to other entities. They are Dashboard state, so Andamento owns them: the host
sets and removes them with `set_local`/`remove_local`
(`andamento_local_set`/`andamento_local_remove`), and they are kept in the
`dashboard` record. A host may still publish them as facts, as Wheelhouse does
from its window layout until it moves; both are placed the same way. Local
workspaces' `.workspace` host entities stay host-published: they follow the
host's open workspaces.

- **`.section`**: a section someone made. A placement loop marked
  `layout="section"` makes each iteration its own section. Docking frontends
  give each its own View; others show it as a headed section. Andamento passes
  `layout` through to frontends, so this needs no special support.
- **`.group`**: a group, naming its section with an entity-reference fact,
  `.section`. A template matches it with `match ".section" of="section"`,
  where `section` is the template's own binding (its name's prefix).
- **Items**: workspaces and references name their group with `.group`, in the
  same way.
- **`.ref`**: a reference (a ghost), with `.group` and `.target`, where
  `.target` is a single entity reference. A ref presents its target:
  - it takes the target's facts, so templates, status, live state and details
    follow the target;
  - activating it activates the target;
  - it keeps its own identity, so one entity can have any number of refs, even
    in one group, and each keeps its own key and place;
  - a ref's place is its own: it keeps its own `.`-prefixed facts (`.group`,
    `.target`, `.position`) and takes none of its target's, so a ref without
    its own `.group` isn't placed in its target's, and it sorts by its own
    `.position`;
  - rules reach a ref only through its own `.group` and `.target`, never
    through its target's facts, so a query such as "needs attention" doesn't
    place the ghost as well;
  - a ref whose target is missing keeps only its own facts;
  - refs don't chain: a ref to a ref presents that ref as it is;
  - activating a placed ref (`ActivatePlacement`) activates its target;
    `Activate` with the ref's own entity activates the ref itself.
- **`.default`**: a group with `.default` set to the boolean `true` (text
  "true" doesn't count) covers workspaces that
  nothing places. They become its children in a `.unplaced` loop after its own
  items, instead of filling the `.unplaced` section, which is then emitted
  empty. With several default groups, the first placed one (in catalog order)
  covers them, under its first placement.

How a reference is shown, for example compact or expanded, is the frontend's
own state and is not published.

## Managed primary content

The optional typed content-plan interface reconciles one command terminal slot
without recreating its workspace. See [Managed primary content](managed-primary-content.md)
for producer facts, host commit tokens, expiry and retry semantics.

### Placement visibility policies

A loop can opt into `visibility="activity"`. The named policy checks the
selected entity's facts and reads existing boolean display variables:

```kdl
display-variable "show-finished" type="bool" default=false label="Finished" icon="F" persist=true
visibility "activity" {
  when kind="vessel" visible-when="show-finished" {
    match "flotilla.convoy.phase" value="landed"
  }
}
placement "work" {
  for "vessel" kind="vessel" visibility="activity" {
    field "label" key="display.label"
  }
}
```

Rules run in declaration order. The first rule whose predicates all match
uses its `visible-when` variable; an entity matching no rule remains visible.
`kind=` adds an `entity.kind` equality predicate. Policy matches support a
constant string `value=` (using the same scalar normalization as indexed
queries), or `exists=true/false`. Existence includes non-scalar facts. Policies
have no loop bindings or `of=` joins.

Policies referenced by configured loops are evaluated once per view-model
build, with cost proportional to entity count times rule count. A loop's indexed query checks the cached result
for each candidate. This preserves indexed placement selection without adding
catalog scans inside nested loops. Query predicates themselves remain equality
only. Policies cannot select an entity, change its placement key, or affect its
activation recipe.

Loops without a policy remain unfiltered, including detail loops. Apply the
same policy explicitly to Attention aliases when they should share visibility.
Children of a hidden parent are not placed. Hidden entities remain in metadata
and Inspect; observed workspaces without a placement use the existing fallback
section.

Policies merge by name using normal config-layer precedence; a higher layer
replaces the whole policy. Each config must declare the policies its loops
reference and the boolean display variables its policies reference. A higher
layer can override variable defaults. Persisted values continue to use the
existing rail UI state. If a layered override supplies a non-boolean value, the
matching rule is hidden and template diagnostics include a warning. Empty
policies allow all entities.

The typed C ABI is unchanged. Native hosts render the resulting snapshot and
use the existing display-variable actions; no host filtering is required.
