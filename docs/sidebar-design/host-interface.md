# The host interface (ABI 7)

What a host such as Wheelhouse uses to drive Andamento through its C ABI.
`crates/andamento-ffi/include/andamento.h` is generated from the FFI crate
by cbindgen (`scripts/generate-header.sh`; CI fails if it is stale), so each
declaration's comment there is its contract. This page is the map. The model
behind it is in [Slots and arrangements](slots-and-arrangements.md),
[The sidebar arrangement](sidebar-arrangement.md) and the
[Workspace Overlay](workspace-overlay.md).

`andamento_abi_version()` returns 4. Each call has one form: there is no
compatibility with earlier versions, and a host requires exactly the version
it was built against.

## Conventions

- **Text** is `AndamentoText {data, len}`: UTF-8, length-delimited. Input is
  borrowed for the call; output is borrowed from whatever returned it, and
  empty output text has a non-NULL pointer.
- **Errors.** A call that mutates returns 1 (or a status) on success, and
  changes nothing on invalid input. `error_out`, when not NULL, is cleared on
  entry and receives a message to free with `andamento_string_free`. A panic
  poisons the sidebar: destroy and recreate it.
- **Getters** that fill an `out` struct return 0 for NULL handles or output
  and invalid indices, leaving `out` untouched.
- **Documents** (arrangements, slots, plans, the overlay, the Dashboard) are
  returned as one struct in one allocation, with every pointer and string in
  it valid until its `_free` call. `_free(NULL)` is harmless. A struct the
  host passes in is only read during the call.
- **Compare-and-set** calls return `ANDAMENTO_COMMITTED`, `ANDAMENTO_STALE`
  (no error, nothing changed: read again) or fail with an error
  (`ANDAMENTO_INVALID`). Generations come from one clock per sidebar: they
  are never reused (not even across an import) and never persisted, so a
  host never keeps one across sessions.
- `ANDAMENTO_NONE` (`SIZE_MAX`) is an index that names nothing.

## Identity

- **Workspace IDs** are 16 bytes (`AndamentoWorkspaceId`) the host supplies,
  UUIDv7 in practice. Andamento only compares and prints them (as a
  hyphenated UUID, which is also their JSON form).
- **Entities** are `AndamentoEntity {provider, kind, id}`. The provider is
  the Dashboard subscription the facts came from; the same kind and ID under
  two providers are two entities. `"local"` is Andamento's own provider
  (local sections, groups and refs); a host's subscription is never called
  that. Every entity a host passes names its provider.

## Facts and topology

| Call | |
|---|---|
| `andamento_apply_patch_json(h, now_ms, provider, json)` | One producer patch, from `provider` (required, not `"local"`), stamped over every entity it names. |
| `andamento_apply_entity`, `andamento_apply_workspace` | Scalar facts (`AndamentoFact`) on an entity, or on a workspace (a tab target), from `provider`. |
| `andamento_tick(h, now_ms)` | Advance time without facts. |
| `andamento_provider_retract`, `andamento_provider_set_stale` | Drop a subscription's facts; mark one stale or fresh. |
| `andamento_observe(h, workspaces, n, panes, m)` | The whole topology: `AndamentoWorkspace {id, position, name, selected}`, `AndamentoPane`. |
| `andamento_observe_workdirs` | Directory associations, replaced whole. |
| `andamento_workspace_register` / `_forget` / `_registered` | Workspaces the host made itself; a forgotten one's record goes. |

## The snapshot and effects

`andamento_snapshot_acquire` (or `_acquire_details`, with every catalog
card) returns the sidebar as nodes in preorder; `andamento_snapshot_is_current`
checks it cheaply. An `AndamentoNode` carries what ABI 2 kept in side calls:
its entity with provider, `stale`, `loop_key`, `workspace_id` (16 bytes, LIVE
nodes), a section's `default_host`/`has_order`/`order`, and `copy_url`. An
`AndamentoControl` carries its display variable's name and `persist`. Fields,
diagnostics and details are read by index as before;
`andamento_snapshot_detail_find` and `_request` take an `AndamentoEntity`.

`andamento_dispatch(h, snapshot, action)` queues effects;
`andamento_effects_take` drains them. An `AndamentoEffect` carries
`workspace_id` (FOCUS), `entity` with provider (MATERIALIZE, INSPECT), the
recipe or URL, and `has_primary_target`/`primary_target` (MATERIALIZE).
`andamento_complete(h, request_id, outcome, workspace_id, message)` completes
every FOCUS and MATERIALIZE request. `andamento_copy_subject_url` takes an
entity.

## Records

Andamento owns the Dashboard's and each workspace's logical state, and hands
it to the host as **records** to store as they are: their bytes (JSON, today)
are private to Andamento and may change from one build to the next. The host
decides where to keep them; Andamento never touches a filesystem.

| Call | |
|---|---|
| `andamento_record_names` | `dashboard`, then `workspace/<id>` per registered workspace, one per line. |
| `andamento_record_generation(h, name)` | A counter: nonzero, changes whenever the record does. Cheap to poll; store the record when it differs from the one last stored. |
| `andamento_record_get` / `_put` | The bytes to store, and restoring them before the first observe. A record that doesn't read is rejected without change; putting `workspace/<id>` registers the workspace. |

There are no record versions or migrations: a Dashboard directory from before
ABI 4 is not imported.

## The Dashboard

| Call | |
|---|---|
| `andamento_set_display_variable` | Set or reset a declared display variable. |
| `andamento_local_set` / `_remove` | A local section, group or ref with its facts (`AndamentoLocalFact`; an `ENTITY` fact names its provider). |
| `andamento_set_sibling_order(h, loop_key, entities)` | The host's order for one sibling run, keyed by a node's `loop_key`. |
| `andamento_dashboard_get` / `_free` | `AndamentoDashboard`: the dashboard record's `generation`, the local entities with their facts (in the form `local_set` takes), and each saved sibling order with its `loop_key` (the same text nodes carry) and entities. Read again when the generation moves. |

## Workspaces

| Call | |
|---|---|
| `andamento_slots_get` / `_free` | `AndamentoSlots`: each `AndamentoSlot` with its View Spec, rebind policy, `in_baseline`, `detached`, `ANDAMENTO_SLOT_FLAG_*` flags and saved portable resolution (`has_saved`, `saved`). |
| `andamento_slot_set` / `_remove` / `_reattach` | The user's own slots (`u:<id>`) and edits of baseline slots. |
| `andamento_slot_plan(h, ws, key, applied)` / `_plan_free` | `AndamentoSlotContent`: state, token, resolution identity, target, recipe, rebind, previous instance and the saved resolution. |
| `andamento_slot_valid` / `_complete` / `_retry` / `_release_previous` | The plan protocol. |
| `andamento_slot_resolution_set` / `_clear` | Compare-and-set of a slot's saved portable resolution, at the slot's generation (0 for none). |
| `andamento_workspace_set_name` / `_set_mood` | The user's name and mood over the provider's. |
| `andamento_overlay_get` / `_free` | `AndamentoOverlay`: baseline version, the user's name and mood, the subject's name (its display label now), `owned`, and TOMBSTONED and DEPARTED notes. A workspace is named by the user's name if it has one, else the subject's. |
| `andamento_workspace_content_revision` | Poll to learn when to read slots, plans and arrangements again. |

## Arrangements

Both arrangements cross as the same document: an array of
`AndamentoDocPanel`, each naming its parent by index.

```c
typedef struct {
    size_t parent;               /* an earlier SPLIT panel, or ANDAMENTO_NONE for a root */
    AndamentoText id;            /* stable, nonempty, unique in the document */
    double weight;               /* relative to siblings; finite and positive */
    uint32_t kind, axis;         /* ANDAMENTO_PANEL_SPLIT/TABS, ANDAMENTO_AXIS_ROW/COLUMN */
    const AndamentoText *tabs;   /* a TABS panel's keys, in order */
    size_t tab_count;
    size_t selected;             /* an index into tabs, or ANDAMENTO_NONE */
} AndamentoDocPanel;
typedef struct { const AndamentoDocPanel *panels; size_t panel_count; } AndamentoArrangementDoc;
```

A split's children are the panels naming it as parent, in array order. A
host writes a document by walking its tree in preorder, and reads one in a
single pass, appending each panel to its parent.

- **A workspace's** (`andamento_arrangement_get`, `_commit(h, ws, doc,
  expected)`, `_free`): at most one root; tabs are slot keys. Commit and get
  return `AndamentoWorkspaceArrangement` with `status`, `generation`,
  `owned`, `flags` (PROVIDER_CHANGED, SOFT, UNRESOLVED) and notes (PLACED,
  GONE, UNRESOLVED). `andamento_arrangement_resolve` KEEPs or FOLLOWs after a
  provider change.
- **The sidebar's** (`andamento_sidebar_arrangement_get`, `_commit(h, doc,
  expected)`, `andamento_sidebar_restore_section`, `andamento_sidebar_reset`,
  `_free`, and `andamento_sidebar_arrangement_generation` to poll): an
  `AndamentoSidebarDoc {dock}`, the dock with at most one root; tabs are
  section keys. Every call
  returns `AndamentoSidebarArrangement` with notes CLOSED, UNRESOLVED,
  PLACED, RESTORED and DUPLICATE. UNRESOLVED also lists saved dashboard keys
  the template no longer resolves, as `display:`, `collapse:`, `order:` or
  `variable:` and the key.

Committing an arrangement never changes the snapshot's revision.

## From ABI 6

ABI 7 removes floating panels (wheelhouse#380): `AndamentoSidebarDoc` loses
`floating`, so it and `AndamentoSidebarArrangement`, which embeds it, change
layout, and a host built against ABI 6 must be rebuilt. Every section docks; a
region's `default-host` no longer places one. A stored record's floating
panels are dropped on import, and the sections only they held are docked;
host views (`u:<id>`) only they held are dropped.

## From ABI 5

ABI 6 removes `has_ttl` and `ttl_ms` from `AndamentoDetailField`: an entity's
facts have no lease (andamento#173), so they were always 0. `stale` now
follows the provider alone. The struct's layout changed, so a host built
against ABI 5 must be rebuilt.

## From ABI 4

ABI 5 adds `has_subject_name` and `subject_name` to `AndamentoOverlay`,
after `mood`: the change is additive, but the struct's layout changed, so a
host built against ABI 4 must be rebuilt.

## From ABI 3

| ABI 3 | ABI 4 |
|---|---|
| `andamento_observe3`, `_observe_workdirs3`, `_complete3`, `*3` structs | `andamento_observe`, `_observe_workdirs`, `_complete`, plain structs with 16-byte IDs |
| `andamento_apply_patch_json_from`, `_apply_entity_from`, `_apply_workspace_from`, `andamento_set_default_provider` | `andamento_apply_patch_json`, `_apply_entity`, `_apply_workspace`, each with a required provider |
| `AndamentoEntity3`, `andamento_set_sibling_order3`, `_local_set3`, `_copy_subject_url3`, `_snapshot_detail_find3`/`_request3` | `AndamentoEntity` (with provider) and the plain calls |
| `andamento_snapshot_node_workspace`, `_node_provider`, `_node_loop_key`, `_region_hints`, `_copy_url_action`, `_control_variable` | `AndamentoNode` and `AndamentoControl` fields |
| `andamento_snapshot_detail_workspace`, `_detail_provider`, `andamento_effects_workspace`, `_effects_provider`, `_effects_primary_target` | `AndamentoDetail` and `AndamentoEffect` fields |
| `andamento_content_get`/`_plan`/`_plan_entity`/`_valid`/`_complete`/`_retry`/`_release` (and their `3` forms) | the `primary` slot's `andamento_slot_*` calls; `AndamentoEffect.primary_target` |
| `andamento_slots_acquire`/`_count`/`_get`/`_flags`/`_release` | `andamento_slots_get`/`_free` |
| `andamento_slot_plan_get`/`_release`, `_slot_plan_resolution` | `andamento_slot_plan` returns `AndamentoSlotContent`; `andamento_slot_plan_free` |
| `andamento_slot_resolution_acquire`/`_get`/`_release` | `AndamentoSlot.saved`, `AndamentoSlotContent.saved` |
| `andamento_overlay_acquire`/`_info`/`_note`/`_release` | `andamento_overlay_get`/`_free` |
| `andamento_set_sidebar_arrangement`, `andamento_sidebar_arrangement_acquire`, `andamento_arrangement_info`/`_panel`/`_tab`/`_floating_first`/`_note_count`/`_note`/`_release` | `andamento_sidebar_arrangement_get`/`_commit`/`_free`; restore and reset return the arrangement |
| nested `AndamentoDocPanel.children` | `AndamentoDocPanel.parent` |
| reading the dashboard record's KDL | `andamento_dashboard_get` |
| `andamento_overlay_export` (Overlay Sync proposal), `andamento_dashboard_overlay_acquire`/`_info`/`_note`/`_release`, `.view` pins on refs | gone; template drift is in the sidebar arrangement's UNRESOLVED notes |
| record versions, unknown-node preservation, u64 Workspace IDs, `v2;` loop keys | gone |
