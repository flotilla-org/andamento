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
without pane observations can start with an empty pane list.

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
debug directory. ABI version 1 is an initial embedding interface. Its owned
JSON messages keep Rust layouts and allocation internals out of the C contract.
Do not treat this as a frozen producer transport protocol.

1. Call `andamento_create` with UTF-8 KDL and its byte length. On failure it
   returns null and, when requested, an owned error string.
2. Serialize calls to `andamento_request` for each handle, preferably on the
   UI thread. Queue transport events onto that thread. Input buffers are
   borrowed only during each call and need not be NUL-terminated.
3. Parse the owned, NUL-terminated JSON response, then release it through
   `andamento_string_free`. Every request gets an independent response buffer.
4. Destroy the handle after all calls complete. A panic returns an error and
   poisons the handle; recreate it before further work.

Requests use the `sidebar::Request` schema. Examples:

```json
{"request":"apply","now_ms":1000,"patches":[]}
{"request":"observe","workspaces":[{"id":42,"position":0,"name":"Worker","selected":true}],"panes":[]}
{"request":"dispatch","action":{"action":"activate","entity":{"kind":"vessel","id":"v"}}}
{"request":"complete","request_id":1,"workspace_id":42,"error":null}
{"request":"snapshot"}
```

Success returns `{"ok":true,"snapshot":...,"effects":[...]}`. Errors return
`{"ok":false,"error":"..."}`. Snapshot errors are an array of entity/message
pairs, so arbitrary entity references remain valid JSON. The `revision` is
local to the instance; it is not a distributed synchronization clock.

`Configure` accepts a `kdl` string. `TogglePlacement` takes a `PlacementKey`
copied from the snapshot. `SetVariable` takes that key, a name and a string
value, or null to inherit. `ToggleDisplayVariable` takes a declared name.
Use the Rust serde definitions as the exact evolving JSON schema; do not infer
producer HTTP paths or request framing from these in-process messages.

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
or Flotilla packages, runs its tests, then compiles and executes a C caller.
The plugin workspace itself still needs the sibling Zellij checkout for Cargo
workspace resolution. CI runs both the full plugin suite and the isolated check.

Wheelhouse's next slice is labels, status, collapse and local activation over
fixtures, then HTTP/UDS facts under wheelhouse#22. Use its existing layout and
workspace machinery. Preview implementation already exists in Wheelhouse;
declarative placement of previews remains follow-up work.
