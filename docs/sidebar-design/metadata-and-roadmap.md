# Sidebar Metadata And Roadmap

This note captures the design decisions from the sidebar exploration and lays out a sensible order for building the next version of the Zellij sidebar plugin. The immediate goal is to get a useful end-to-end slice working without committing to all of the future machinery.

## Context

The cmux sidebar is useful as a reference because it combines navigation, status, metadata, progress, logs, branch/cwd display, PR state, ports, unread notifications, drag/reorder, pinning, and progressive disclosure. The important lesson is not to copy every feature, but to separate the underlying state model from the rendered sidebar projection.

Zellij plugins can already observe enough structured state to build a first version:

- `TabUpdate` gives tab names, tab ids, active state, pane counts, viewport/display dimensions, sync state, floating pane visibility, swap layout state, and bell state.
- `PaneUpdate` gives panes grouped by tab position, including ids, titles, terminal/plugin kind, focus/fullscreen/floating/suppressed state, geometry, exit/held state, command-pane launch command, plugin URL, selectable state, and pane-group index.
- Targeted APIs can query focused pane, pane info, tab info, pane PID, current command, and current working directory.
- `CwdChanged` and `CommandChanged` can keep cwd/command metadata fresh.
- `PaneRenderReport` and `GetPaneScrollback` can expose text contents when the plugin has `ReadPaneContents`, but this should be optional for later overview/summarization work.

## Decisions

### Metadata Is The Common Data Plane

All pane and tab facts should flow through one metadata mechanism. Some keys are well-known because the plugin knows how to produce, resolve, aggregate, group, or render them, but they should not live in a separate "core" data plane.

Examples:

```text
zellij.pane.title
zellij.pane.cwd
zellij.pane.command.argv
zellij.pane.command.is_foreground
zellij.pane.exit_status
zellij.tab.name
zellij.tab.has_bell
git.branch
progress.tests
summary.local_llm
```

The first milestone should only produce Zellij-derived well-known metadata. External arbitrary metadata can be added later using the same patch mechanism.

### Metadata Updates Are Collaborative Patches

Metadata updates should be patch-based. One update does not replace the whole metadata object. It replaces or removes only the keys that the producer owns in that update.

Internal store shape:

```text
store[entity_id][metadata_key][source_id] = MetadataEntry
```

Where `entity_id` can initially be a pane id or tab id. `MetadataEntry` should contain:

```text
value
updated_at
ttl?
precedence?
ordinal?
```

Patch semantics:

```text
MetadataPatch {
  entity_id
  source_id
  set: Map<metadata_key, value_with_optional_ttl>
  unset: Set<metadata_key>
}
```

Rules:

- `set` replaces `store[entity][key][source]`.
- `unset` removes `store[entity][key][source]`.
- omitted keys are unchanged.
- updates are assumed to arrive in order per source.
- no tombstones are needed for the first model.
- no confidence field for now.
- `updated_at` should be assigned by the plugin on receipt, not trusted from producers.
- expired entries are ignored by resolution and may be garbage-collected opportunistically.
- `precedence` is producer-supplied ordering metadata that beats aggregation count.
- `ordinal` is producer-supplied stable ordering metadata used after aggregation count.

The collector should encode domain-specific knowledge into `precedence` and `ordinal`. For example, a Zellij cwd collector can give the focused pane's cwd higher `precedence` and encode pane layout order as `ordinal`. The aggregation layer should not need to know what cwd means or why focus matters.

### Resolution Is Separate From Storage

The raw store should keep per-source assertions. Rendering and grouping should use a resolved view.

The eventual resolver should support per-key configuration. Good defaults:

- default: last write wins across sources.
- source priority: useful for well-known keys where one source is authoritative.
- union/dedupe: useful for list-like values.
- aggregate: useful for values such as progress pairs.

For the first milestone, use simple defaults:

- Zellij is the only source for built-in keys.
- resolved value is the latest non-expired entry.
- no user-facing resolver config yet.

### Aggregation Is A First-Class Concept

Some metadata can be rolled up from panes to tabs or groups. Progress is the clearest example: if panes report compatible `(current, total)` values, an overview can sum them into group progress.

Aggregation should be defined by metadata key/type, not hard-coded into every renderer. The first milestone only needs simple counts and grouping:

- tabs per directory group.
- panes per directory group.
- active/running/exited counts.
- current tab/pane markers.

Generic value selection should operate on metadata entries rather than key-specific logic. A useful default selector is:

1. choose entries with the highest `precedence`.
2. choose the most frequent value among those entries.
3. break remaining ties with the lowest `ordinal`.
4. fall back to a stable deterministic order if needed.

### Progressive Disclosure Drives Rendering

The sidebar should show the most useful information for the available width and height. The same state should support multiple projections:

- compact sidebar rows.
- ungrouped tab rail.
- grouped sidebar sections.
- selected tab/pane detail.
- future expanded overview grid.

This argues for a normalized state cache first, then render policies over it. Avoid baking grouping or display decisions directly into the update ingestion path.

Grouping should be a configurable projection over the same tab/pane state, not a structural change to the stored model. The ungrouped tab list should remain a supported mode. Directory grouping is the first useful grouping criterion, but the design should allow later criteria such as repository root, git branch, command family, status/severity, custom metadata keys, or user-defined profiles.

Grouping, ordering, and rendering should remain separate concerns:

```text
grouping: tab -> optional group key
ordering: projection item -> sort key
rendering: projection -> rows
```

The grouped list is not the stored tab order. It is one computed projection. This leaves room for later manual ordering, recency/activity ordering, urgency ordering, and pinning without replacing the metadata model.

### Config Should Have Strong Defaults

Zellij plugin configuration is limited to key/value pairs, so rich configuration should eventually live in a real file.

Preferred long-term config path:

- request `FullHdAccess`.
- call `change_host_folder("/")`.
- read/write a concrete path through `/host`, such as `/host/Users/robert/.config/zellij-sidebar/config.kdl`.
- use `/cache` only as fallback or transient state.

Permissions are all-or-nothing per request vector in the current fork, so the plugin should request a coherent baseline permission set and explain it inside the plugin before triggering the Zellij prompt.

### Future Visual Overview

Expanded/full-screen mode should eventually show a compact representation of each tab/pane. That can include:

- resolved metadata.
- viewport excerpts.
- optional local summary text.
- image placement thumbnails.

For image support, the ideal future primitive is not raw image bytes. It is a read/render handle: the plugin can inspect pane image placement metadata and ask Zellij to render a new mini placement that references the same Zellij-owned asset. This likely wants a separate permission or at least careful treatment under `ReadPaneContents`.

## First Milestone: Group Tabs By Directory

Grouping tabs by directory is a good first slice because it exercises the important mechanics end to end without requiring arbitrary external metadata, rich config, content summaries, image placements, or complex aggregation.

The milestone should prove:

- ingest `TabUpdate` and `PaneUpdate`.
- query or subscribe to cwd changes for terminal panes.
- normalize Zellij-derived facts into metadata entries.
- resolve metadata for rendering.
- derive a tab-level directory identity from pane cwd metadata.
- group tabs by that directory identity.
- leave tabs without a directory ungrouped rather than placing them in an `Other` group.
- keep grouping, ordering, and rendering as separate steps.
- render grouped sections in the sidebar.
- keep the existing tab navigation behavior usable.

Suggested default grouping rule:

1. For each tab, collect cwd values from selectable terminal panes.
2. Emit each cwd as `zellij.pane.cwd` with generic selection metadata:
   - focused pane cwd gets higher `precedence`.
   - selectable terminal panes get normal `precedence`.
   - pane order is encoded as `ordinal`.
3. Select the tab's primary cwd with the generic precedence/count/ordinal selector.
4. Group by the exact selected cwd string.
5. Tabs with no cwd remain ungrouped and render as ordinary tab rows.

Each tab belongs to at most one group in the first version.

Default ordering for the first version:

- group order follows the first tab occurrence that created the group.
- tabs inside a group retain normal tab order.
- ungrouped tabs retain normal tab-row rendering.
- ordering is intentionally simple and should be implemented as a policy over projection items, not by mutating the underlying tab order.

Default rendering for the first version:

- group headers are non-selectable rows.
- group headers show a compact cwd label.
- group headers may show a tab count when space allows.
- child tabs reuse the existing tab row renderer.
- child tabs are indented under their group.
- active tab highlight stays on the tab row, not the group header.
- mouse/click behavior applies only to tab rows.
- scrolling treats group headers and tab rows as one projected row list.
- single-tab groups still render as groups when grouping is enabled; a later `min_group_size` config can change this if it feels noisy.

Path labels should start compact: use the directory basename and add parent context only when duplicate basenames would otherwise collide. Full path rendering can be added later as a config/display mode.

This can be refined later into repository-root detection, but the first version should not require git discovery.

## Roadmap

### 1. Normalize Current State Into Metadata

Create an internal state model that ingests Zellij events and produces metadata patches from a `zellij` source.

Scope:

- tab metadata from `TabUpdate`.
- pane metadata from `PaneUpdate`.
- cwd and command metadata from targeted queries or `CwdChanged`/`CommandChanged`.
- `precedence` and `ordinal` metadata emitted by collectors where useful.
- in-order patch application.
- latest-value resolution.

This is the foundation for everything else.

### 2. Directory Grouping For Tabs

Build derived tab grouping on top of resolved metadata.

Scope:

- derive a directory key per tab.
- select each tab's primary cwd using precedence, count, and ordinal.
- group tabs by directory key.
- retain an ungrouped rendering mode.
- render tabs without a grouping value as ungrouped rows, not under an `Other` group.
- keep grouping and ordering as independent functions.
- display group headers.
- keep active tab visibility and click-to-switch behavior.
- keep an ungrouped fallback path.

This is the first user-visible milestone.

### 3. Basic Group Configuration

Add minimal configuration for grouping behavior.

Scope:

- grouping mode: `none` or `directory`.
- grouping enabled/disabled.
- exact cwd vs common ancestor mode.
- optional max group label width.
- optional hidden `Other` group behavior.

This can initially remain in plugin key/value config or a hard-coded profile. Do not introduce full external config until the need is concrete. The config shape should leave room for future grouping criteria without changing the renderer contract.

### 4. Resizable Sidebar Width

Add a way to change sidebar width as grouped and detailed rendering becomes denser.

Scope:

- drag resize affordance on the sidebar boundary.
- minimum and maximum width.
- persisted width preference.
- renderer adapts labels/counts/details to available width.

This is not required to prove directory grouping, but it is likely to become important as nested/grouped rows and richer metadata are added.

### 5. Metadata Resolver Configuration

Add richer resolution only after there is more than one source for at least one key.

Scope:

- per-key strategy defaults.
- source priority.
- last-write-wins fallback.
- TTL expiry behavior.
- debug view showing raw per-source values versus resolved values.

This should wait until external metadata or inferred metadata exists.

### 6. External Metadata Patch Input

Add a pipe/protocol for arbitrary producers.

Scope:

- receive metadata patch messages.
- validate keys and values.
- support `set`, `unset`, and optional `ttl`.
- preserve source ids.
- expose values through the same resolver.

This unlocks shell integrations, build/test progress, PR state, ports, and workflow-specific annotations.

### 7. Aggregation And Profiles

Add key-aware aggregation and default profiles.

Scope:

- progress pair aggregation.
- list union/dedupe.
- severity/status rollups.
- profiles for compact navigation, workflow/status-heavy, and overview-heavy usage.

### 8. Ordering, Pinning, And Manual Arrangement

Add richer ordering as a policy over projection items.

Scope:

- group ordering from aggregated metadata, such as most recent `needs_attention`.
- recent activity ordering.
- pinned groups.
- pinned tabs.
- individual pinned tabs that render above grouped sections even if their original group is lower.
- drag of groups or tabs that switches the affected scope into manual ordering mode.

The projection should support common item types such as:

```text
ProjectionItem =
  PinnedTab(tab_id)
  PinnedGroup(group_id)
  Group(group_id, children: Vec<TabId>)
  Tab(tab_id)
```

A future layered ordering policy can be:

1. pinned items.
2. manual order if present.
3. metadata-derived urgency/activity order.
4. grouping/default order.
5. stable tab order fallback.

### 9. Expanded Overview Mode

Build a larger projection over the same state model.

Scope:

- tab/pane tiles.
- cwd/command/status summary.
- optional viewport excerpt.
- optional local summarization pipeline.
- future image placement previews.

This should not require a new data model. It should consume the same metadata store, resolver, and derived grouping/indexes.

## Open Questions

- Should the first directory grouping use exact cwd only, or common ancestor by default?
- Should tabs be allowed to appear in multiple groups if panes span multiple directories, or should each tab have one primary group?
- Should group order follow first tab occurrence, alphabetical directory order, or most recently active group?
- Should cwd metadata be pane-only with tab grouping derived from panes, or should derived tab metadata be materialized back into the metadata store?
- What is the minimal grouping config shape that supports `none` and `directory` now without blocking future criteria?
- Should resizable sidebar width be part of the first grouping milestone or the next polish milestone?
- When external metadata arrives, should values be typed JSON-like data, strings only, or a small tagged enum?

## Recommended Next Step

Implement milestone 1 as a vertical slice: Zellij event ingestion to metadata patches, directory grouping, and grouped sidebar rendering. Keep the resolver and grouping config deliberately small, but structure the code so later external metadata and richer resolution can slot in without replacing the state model.
