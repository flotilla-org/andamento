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
the selected workspace. Workspace IDs are stable for their host lifetime,
scoped to this client; positions retain the existing navigation order. A host
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

The default tree still uses legacy grouping until #71. Native snapshots report
a diagnostic for any tree/attention region lacking a placement declaration;
they do not translate legacy GroupPath rows into the native contract. Use
`crates/andamento-core/tests/fixtures/sidebar.kdl` for the first native slice.
Keep #71's temporary adapter private and finish that cutover before treating
the presentation contract as stable. The plugin can still render legacy regions.

## C ABI

The header is `crates/andamento-ffi/include/andamento.h`. Build the library for
the native host target, overriding this repository's WASM default:

```sh
cargo build -p andamento-ffi --target aarch64-apple-darwin
```

It produces `libandamento_ffi.a` and a dynamic library in the native target's
debug directory. ABI version **2** replaces the experimental JSON request
interface from version 1. It is not yet a frozen embedding contract.

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
snapshot from another client or from before a successful mutation.

Capture the action reference together with the displayed snapshot. Process
captured clicks against that snapshot before draining queued facts, ticks,
topology and completion updates, then render the next frame. If a mutation
already intervened, dispatch rejects the click: discard it and redraw. Do not
re-hit-test the old click's coordinates against the new arrangement, since
that could activate a different entity. Only a fresh user interaction is
hit-tested against a new frame. Never reuse an action index against a different
snapshot. The conservative rule also invalidates actions after a tick or a
successful dispatch that produces no effects. Unknown/duplicate completions
and failed input validation leave actions valid. Retaining an older snapshot
for drawing is always allowed. Controls with no core action expose their
host-owned intent, such as scrolling, with `ANDAMENTO_NONE` as the action.

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
