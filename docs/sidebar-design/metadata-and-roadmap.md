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
store[metadata_target][metadata_key][source_id] = MetadataEntry
```

Where `metadata_target` can initially be a pane id or tab id. It should later also support semantic group targets. `MetadataEntry` should contain:

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
  metadata_target
  source_id
  set: Map<metadata_key, value_with_optional_ttl>
  unset: Set<metadata_key>
}
```

Rules:

- `set` replaces `store[target][key][source]`.
- `unset` removes `store[target][key][source]`.
- omitted keys are unchanged.
- updates are assumed to arrive in order per source.
- no tombstones are needed for the first model.
- no confidence field for now.
- `updated_at` should be assigned by the plugin on receipt, not trusted from producers.
- expired entries are ignored by resolution and may be garbage-collected opportunistically.
- `precedence` is producer-supplied ordering metadata that beats aggregation count.
- `ordinal` is producer-supplied stable ordering metadata used after aggregation count.

The collector should encode domain-specific knowledge into `precedence` and `ordinal`. For example, a Zellij cwd collector can give the focused pane's cwd higher `precedence` and encode pane layout order as `ordinal`. The aggregation layer should not need to know what cwd means or why focus matters.

### Metadata Targets Can Be Semantic Groups

Metadata should not be limited to concrete Zellij objects. Panes and tabs are important targets, but the sidebar will become much more useful if external tools can attach metadata directly to semantic groups.

Target shape:

```text
MetadataTarget =
  Pane(PaneTarget)
  Tab(TabId)
  Group(GroupPath)
```

This lets a producer describe a project, worktree, convoy, or latent workflow item even when there is no single pane or tab that owns the fact.

Examples:

```text
Group([zellij.pane.cwd = "/Users/robert/dev/zellij"])
Group([project.name = "zellij"])
Group([project.name = "zellij", zellij.pane.cwd = "/Users/robert/dev/zellij-worktree-a"])
Group([project.name = "zellij", flotilla.convoy.id = "abc123"])
```

Group-targeted metadata should merge with metadata rolled up from descendant panes/tabs. For example, a build tool can set `progress.tests` on a convoy group, while panes under that convoy can still contribute status, commands, and attention markers. The renderer should consume the resolved group view without caring whether a value was written directly to the group or aggregated upward.

### Group Identity Is A Metadata Path

Group identity should be structured, not a display string. A group path is an ordered list of metadata-key/value segments:

```text
GroupPath = Vec<GroupSegment>

GroupSegment {
  key: MetadataKey
  value: MetadataValue
}
```

The key should be the same metadata-key concept used elsewhere, serialized as a string at pipe/config boundaries. The value should use the existing `MetadataValue` tagged value type rather than being forced to a string.

Examples:

```text
[
  { key: "zellij.pane.cwd", value: Text("/Users/robert/dev/zellij") }
]

[
  { key: "project.name", value: Text("zellij") },
  { key: "zellij.pane.cwd", value: Text("/Users/robert/dev/zellij-worktree-a") },
  { key: "flotilla.convoy.id", value: Text("abc123") }
]
```

This gives the sidebar:

- stable identity for nested groups.
- prefix matching for rollups and hierarchy.
- external metadata targets that can address broad or narrow scopes.
- multiple grouping dimensions without parsing ad hoc label strings.
- a path from exact cwd grouping to project/worktree/convoy hierarchies.

Display labels must stay separate from identity. A segment value might be a full path, while the displayed label might be `zellij`, `worktree-a`, or a template-rendered string. This avoids baking presentation choices into the data model.

### Tabs Can Have Explicit Subject Or Scope

Pane-derived grouping answers "what does this tab currently contain?" That is useful, but it is not the same as "what is this tab specifically about?"

Later metadata should allow a tab to declare a subject or scope explicitly. Possible well-known keys:

```text
tab.subject
tab.scope
tab.materialized_from
```

The value can be a `GroupPath` or a compact representation of one. This lets the sidebar distinguish:

- a tab that happens to have panes in `/repo`.
- the main overview tab for project `zellij`.
- a worktree tab under the same project.
- a convoy or task tab associated with flotilla.
- a tab that was materialized from a latent node.

Grouping should eventually prefer explicit subject/scope metadata when present and fall back to pane-derived metadata such as cwd.

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
- metadata inspection mode in the actual rail plugin.
- selected group/tab/pane detail.
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

### Render Nodes Should Become First-Class

Directory grouping currently produces a flat row list: group headers plus tab rows. That is enough for the first milestone, but the next rendering model should treat groups, tabs, and latent items as first-class render nodes.

Possible shape:

```text
RenderNode =
  Group {
    path: GroupPath
    label
    metadata
    children
    collapsed
    activation
  }
  Tab {
    tab_id
    subject?
    metadata
  }
  Latent {
    path: GroupPath
    label
    metadata
    materialization_recipe
  }
```

This model supports:

- group-level borders and status rollups.
- collapsed/expanded groups.
- label templates and string substitution.
- pinning at group or tab level.
- manual ordering over groups and tabs.
- horizontal sub-tab bars for lower hierarchy levels.
- expanded overview mode over the same data.

The renderer can still project these nodes into simple rows for the sidebar. The important change is that row layout becomes one projection of a richer tree, not the primary model.

Status: started in the rail plugin. Normal rendering now uses local render nodes for groups and tabs, and groups can be collapsed/expanded locally in the rail by clicking the group header. Collapse state is default-expanded and client-local for now; persisted/shared collapse state can be added once sidebar UI state has a durable home.

### Metadata Inspection Mode Should Arrive Early

The rail should have a metadata inspection projection before the final template system is ready. This is not just a config-panel debug screen; it is a mode of the actual sidebar so the user can inspect the data plane in the same hierarchy and spatial context that normal rendering uses.

Status: partially implemented. The metadata rail view now consumes the same local render-node projection as normal rendering, so local tab overrides and grouped hierarchy are shared. It still renders unframed `key: value` lines rather than full dynamic tiles, and it does not yet expose raw per-source resolution detail.

Early behavior:

- every node expands to the biggest size it needs, ignoring compact card sizing.
- every present resolved metadata key renders as a generic `key: value` line.
- groups, tabs, and later panes/latent nodes all use the same generic metadata display.
- clicking a value can reveal raw resolution detail for that key: source entries, ttl, updated time, precedence, ordinal, and later the reason a value won.
- the projection can later show which template matched and why.

This mode is also the authoring loop for future match templates:

```text
edit external template/config file -> reload -> inspect metadata and match result -> adjust
```

The plugin should preview, reload, and report parse/match errors, but editing should stay in the user's normal editor rather than implementing a terminal text editor.

### Template Matching Is A Projection Policy

Template rendering should be driven by match rules over render nodes and resolved metadata, not by one-off code at each hierarchy level.

General shape:

1. match on render node type, such as group, tab, pane, or latent.
2. optionally match metadata predicates: exact value first, prefix matching early, regex only if needed later.
3. choose the most specific matching template by default.
4. later allow interactive cycling among multiple matching templates for a tile.
5. render a priority list of fields, each with its own coalescing and truncation behavior.

Tile size should start as automatic squash-down based on available space. Templates can later add sizing hints such as minimum useful size, preferred size, compact variant, and expanded variant.

Status: started internally. The renderer now has generic ordered template fields with required, optional, and priority classes. Render nodes also carry an initial metadata map populated from the current compatibility view model. Group headers, tab titles, and tab status text are the first field callers, and these field builders now read their display values from render metadata. This is still hard-coded Rust, not external template config, and should be extended to nested groups before adding user-authored templates.

### Sensible Order For Templates And Deep Hierarchy

Do not let templating grow around the current two-level `group -> tab` shape. The current rail implementation is useful as a compatibility step, but arbitrary-depth grouping needs the renderer to become recursive and metadata-driven before templates become user-authored configuration.

The order should be:

1. **Make render-node metadata the template input.**
   Every render node should expose a generic metadata map. The initial adapter can populate that map from the existing `TabCard`, `TabGroupingInfo`, and `TabStatusSummary` fields, but templates should read only metadata keys such as `zellij.tab.name`, `rail.tab.pinned`, `status.title`, `status.detail`, `status.priority`, `group.label`, and `group.tab_count`. `TabStatusSummary` should remain only a temporary compatibility input from the current view model, not an intermediate template model.

2. **Move current hard-coded field builders to metadata lookups.**
   Keep the existing visual output, but make group headers, tab titles, and status text call helpers that select `TemplateField`s from node metadata. This removes the typed-struct dependency without taking on external config yet.

3. **Make render nodes recursive before adding more hierarchy features.**
   Replace the local `RenderGroup { children: Vec<RenderTab> }` shape with a node that can contain `Vec<RenderNode>`. The renderer should walk children generically with a depth/indent context. A tab is then just a leaf node, not the only possible child of a group.
   Status: renderer-internal node children are now recursive and the render, active-tab, and metadata-inspection walkers handle nested groups. The projection builder still emits only the existing one-level cwd groups; multi-segment `GroupPath` expansion is the next step.

4. **Build arbitrary-depth groups from `GroupPath`.**
   Convert a multi-segment `GroupPath` into nested group nodes. The first implementation can still use one-segment cwd groups, but the projection builder should not assume there is only one group level. Tabs without a path still render as top-level leaves.
   Status: multi-segment paths now expand into nested render groups, common prefixes are shared, and nested group/tab indentation is derived from group depth. Existing one-segment directory grouping remains the compatibility baseline.

5. **Keep layout policies separate from tree shape.**
   Joined cells, boxes, collapsed groups, horizontal sub-tab bars, and expanded overview modes should be projections over the recursive tree. Do not encode "children of groups are tab rows" into the data model.

6. **Add template matching only after metadata and recursion are stable.**
   Start with hard-coded template definitions over node type plus metadata predicates. A template should produce ordered fields and sizing hints. Only after this is proven should the config file expose user-authored templates and reload diagnostics.
   Status: started. The renderer now has an internal template resolver keyed by node kind, template slot, and metadata predicates. Built-in group header, tab title, and tab status templates route through it, with the highest-specificity metadata match winning while preserving the current visible output. Predicate support currently covers existence, exact text, and text-prefix matches. Built-ins now describe fields with hard-coded field specs: field class, coalescing value sources, optional prefix/suffix wrappers, and simple conditions. Templates also carry a sizing hint; all current built-ins use `Auto`, leaving existing layout policy unchanged. Metadata view shows the matched built-in template name, sizing hint, specificity score, predicate explanation, and all matching candidates per rendered group/tab/status slot, giving the authoring loop initial diagnostics. This is still not external config, but the runtime path is no longer arbitrary Rust builder functions.

7. **Add external templates last.**
   External config should target stable concepts: node type, metadata predicates, field lists, truncation/coalescing rules, and sizing hints. It should not expose temporary compatibility structs or assumptions about exactly two hierarchy levels.
   Status: started. The rail crate now has a JSON template config schema and validator covering template names, node kind, slot, predicates, field specs, value sources, conditions, and sizing hints. Parsed config can be normalized into a catalog that resolves the highest-specificity template, reports matching candidates against metadata, and renders field specs into classified text fields. A filesystem loader can read a JSON config file into that catalog. The render path can now accept a resolved catalog and use it to override built-in group-header, tab-title, and tab-status fields before falling back to built-ins. The live rail plugin can load that catalog from `template_config_path` plugin configuration and pass it into rendering; reload-on-change and config-editor support are still future work.

The key dependency is: metadata-backed fields first, recursive nodes second, configurable templates last. That avoids the pointless loop of generic metadata being projected into `TabStatusSummary` and then mapped back into generic template fields.

### Latent Tabs Are Materializable Nodes

A latent tab is a sidebar node for work that is not currently a Zellij tab. On activation, it can be materialized by sending a Zellij action or plugin message that creates the real tab/panes.

This should wait until group identity and render nodes are stable, but it is a natural fit for flotilla. Flotilla can expose desired work items as latent nodes, while running agents appear as materialized tabs/panes under the same group path.

Latent nodes should have:

- stable group path identity.
- display label/template data.
- metadata such as status, priority, progress, and owner.
- an activation/materialization recipe.
- an optional relation to a materialized tab once created.

The sidebar should not need a separate rendering path for latent tabs. They should be another render node with different activation behavior.

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

Status: implemented as the first tracer bullet. The current code collects `zellij.pane.cwd`, resolves a primary cwd per tab with precedence/count/ordinal selection, renders directory groups, supports grouped/ungrouped mode, and shows selected cwd metadata in the config editor. The design below remains the reference for the milestone and the compatibility baseline for future changes.

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

Status: partially implemented for cwd and pane status. The next slices should continue moving Zellij-derived facts into the same metadata machinery rather than adding separate side channels.

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

Status: implemented as the first user-visible milestone.

### 3. Basic Group Configuration

Add minimal configuration for grouping behavior.

Scope:

- grouping mode: `none` or `directory`.
- exact cwd vs common ancestor mode.
- optional max group label width.
- optional hidden `Other` group behavior.

Status: partially implemented. The config editor can now switch between ungrouped and directory grouping. Richer grouping criteria can now build on structured `GroupPath` identity.

### 4. Hierarchical Group Identity

Replace the current stringly cwd group identity with a structured group path while preserving existing directory grouping behavior.

Scope:

- introduce `GroupPath` and `GroupSegment`.
- use `MetadataKey` plus `MetadataValue` for group segments.
- keep display labels separate from identity.
- map exact cwd grouping to a one-segment path using `zellij.pane.cwd`.
- serialize group paths through the shared view model.
- update debug/config rendering to show the group path and display label.
- keep the existing `rail_grouping "directory"` user-facing behavior unchanged.

Status: implemented. Directory grouping now uses structured `GroupPath` identity while keeping labels separate from identity and preserving the existing `rail_grouping "directory"` behavior.

### 5. Group Metadata Targets

Allow metadata patches to target semantic groups in addition to panes and tabs.

Scope:

- extend metadata target/entity id with `Group(GroupPath)`.
- support group-targeted metadata in the store.
- expose group-targeted values through resolved group views.
- define merge behavior between direct group metadata and rollups from child panes/tabs.
- add debug rendering for direct group values versus rolled-up values.

Status: started. The shared model now has `MetadataTarget::{Pane, Tab, Group}` and `MetadataPatch`/`MetadataValueUpdate`, and the controller metadata store can apply collaborative source-scoped patches internally. External patch input now feeds the same store and resolved metadata can surface tab/group values. Rollup/direct merge behavior and raw-source diagnostics are still future work.

### 6. Explicit Tab Subject/Scope

Allow tabs to state what they are semantically about.

Scope:

- add well-known keys such as `tab.subject` and/or `tab.scope`.
- let values reference a group path.
- prefer explicit tab subject/scope for grouping when present.
- fall back to pane-derived cwd grouping.
- render subject/scope in debug views.

This unlocks project overview tabs, worktree tabs, convoy tabs, and better grouping for tabs whose panes are not enough to infer intent.

Status: started. The controller now recognizes direct tab metadata keys `tab.scope` and `tab.subject` as explicit grouping identities, with `tab.scope` taking precedence over `tab.subject` and both taking precedence over pane-derived cwd. The first supported value shape is text, projected as a single-segment `GroupPath`; richer group-path-valued metadata remains future work. Resolved metadata view shows these keys because it now renders generic resolved tab/group values.

### 7. Render Nodes, Collapse, And Templates

Move from a flat row projection to first-class render nodes.

Scope:

- recursive group, tab, and future latent node types.
- group children represented as `Vec<RenderNode>`, not `Vec<Tab>`.
- metadata maps on every render node as the template input.
- collapsed/expanded state.
- group-level borders and status/progress rollups.
- label templates using resolved metadata.
- string substitution rules for compact labels.
- optional sub-tab-bar projection for lower levels.
- metadata inspection projection that renders all resolved metadata generically.
- template match diagnostics for authoring.

This is where most rendering/layout improvements should land. It should consume the group path and metadata model rather than inventing a renderer-only hierarchy. The next implementation slices should remove the current local two-level assumptions before adding more visual behavior.

### 8. Resizable Sidebar Width

Add a way to change sidebar width as grouped and detailed rendering becomes denser.

Current layout note: native Zellij mouse resizing works for the rail when the pane has a flexible size, including `size="23%" borderless=true`. A fixed pane such as `size=28` is treated as a fixed layout constraint and does not resize by mouse. The borderless rail can still use Zellij's native edge hit testing, so the sidebar does not need plugin-side drag handling for the first version.

Scope:

- drag resize affordance on the sidebar boundary.
- minimum and maximum width.
- persisted width preference.
- renderer adapts labels/counts/details to available width.

This is not required to prove directory grouping, but it is likely to become important as nested/grouped rows and richer metadata are added.

### 9. Metadata Resolver Configuration

Add richer resolution only after there is more than one source for at least one key.

Scope:

- per-key strategy defaults.
- source priority.
- last-write-wins fallback.
- TTL expiry behavior.
- debug view showing raw per-source values versus resolved values.

This should wait until external metadata or inferred metadata exists.

### 10. External Metadata Patch Input

Add a pipe/protocol for arbitrary producers.

Scope:

- receive metadata patch messages.
- validate keys and values.
- support `set`, `unset`, and optional `ttl`.
- preserve source ids.
- expose values through the same resolver.

This unlocks shell integrations, build/test progress, PR state, ports, and workflow-specific annotations.

Status: started. Producers can send `ExternalMessage::MetadataPatch` through `tabs-apply-metadata-patch`; the controller applies set/unset updates with source ids, precedence, ordinal, and ttl through the shared metadata store, and resolved entries are exposed through the view model.

### 11. Aggregation And Profiles

Add key-aware aggregation and default profiles.

Scope:

- progress pair aggregation.
- list union/dedupe.
- severity/status rollups.
- profiles for compact navigation, workflow/status-heavy, and overview-heavy usage.

### 12. Ordering, Pinning, And Manual Arrangement

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
  PinnedGroup(group_path)
  Group(group_path, children: Vec<TabId>)
  Tab(tab_id)
```

A future layered ordering policy can be:

1. pinned items.
2. manual order if present.
3. metadata-derived urgency/activity order.
4. grouping/default order.
5. stable tab order fallback.

### 13. Latent Tabs And Materialization

Represent work items that are not currently real Zellij tabs.

Scope:

- latent render nodes.
- materialization recipes.
- activation behavior that creates a tab/panes or asks another plugin to do so.
- relation from latent node to materialized tab.
- flotilla integration for desired work items and convoys.

This should wait until group paths, group metadata targets, and render nodes are stable enough that latent nodes do not become a parallel model.

### 14. Expanded Overview Mode

Build a larger projection over the same state model.

Scope:

- tab/pane tiles.
- cwd/command/status summary.
- optional viewport excerpt.
- optional local summarization pipeline.
- future image placement previews.

This should not require a new data model. It should consume the same metadata store, resolver, and derived grouping/indexes.

## Open Questions

- Should group path values support all `MetadataValue` variants immediately, or only text values until there is a real non-text group segment?
- Should explicit tab subject/scope be stored as normal metadata values, or should it become a typed top-level tab relation in the shared model?
- How should group-targeted metadata and rolled-up child metadata resolve when they set the same key?
- Should group order follow first visible tab occurrence, explicit group metadata, recency/activity, manual order, or a layered policy from the start?
- What should the first label-template syntax look like, and how much formatting should it support?
- Where should collapsed state live: transient controller state, plugin config, or the future `/host` config file?
- What is the minimum materialization recipe shape for latent tabs without coupling too tightly to flotilla?
- When external metadata arrives, should values be typed JSON-like data, strings only, or a small tagged enum?

## Recommended Next Step

Extend metadata inspection from derived display facts to resolved metadata entries.

The next slice should:

- add a resolved metadata view shape to the controller view model for group/tab targets.
- populate it from the current metadata store for cwd-derived groups and tabs.
- show those resolved entries in metadata rail view instead of only deriving display lines from existing tab/group fields.
- keep raw source-entry drill-in as the follow-up after resolved entries are visible.
- keep external pipe input for arbitrary metadata separate until the inspection projection can show what arrived.

Status: landed. `ControllerViewModel` now carries resolved metadata entries keyed by group/tab targets. The controller populates cwd-derived tab and group entries from the current metadata store, and the rail renderer merges those entries into render-node metadata so metadata view can show generic resolved keys alongside compatibility fields.

After this lands, the next best slice is explicit tab subject/scope. Rendering polish such as collapse, group borders, and templates will be cleaner once group metadata and tab subjects have stable semantic identities.
