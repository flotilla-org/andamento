# Sidebar Metadata And Roadmap

This note captures the design decisions from the sidebar exploration and lays out a sensible order for building Andamento, the next version of the Zellij sidebar plugin/system. The immediate goal is to get a useful end-to-end slice working without committing to all of the future machinery. The prototype currently lives in `zellij-scratch`, but the intended published home is `flotilla-org/andamento`.

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

### Tabs Can Have Explicit Scope

Pane-derived grouping answers "what does this tab currently contain?" That is useful, but it is not the same as "what is this tab specifically about?"

Metadata should allow a tab to declare its placement explicitly:

```text
tab.scope
tab.materialized_from
```

`tab.scope` must be a typed `MetadataValue::GroupPath`. Text values are just metadata and do not affect grouping. This keeps placement semantics strict and avoids guessing how to parse strings. It lets the sidebar distinguish:

- a tab that happens to have panes in `/repo`.
- the main overview tab for project `zellij`.
- a worktree tab under the same project.
- a convoy or task tab associated with flotilla.
- a tab that was materialized from a latent node.

Grouping should eventually prefer explicit subject/scope metadata when present and fall back to pane-derived metadata such as cwd.

### Resolution Is Separate From Storage

The raw store should keep per-source assertions. Rendering and grouping should use a resolved view.

Metadata facts can also imply semantic identities. A render target should not eagerly copy every repo/PR/build fact onto every group or tab. Instead, the controller should resolve a target once by walking identities implied by its resolved facts, cache that per-target view, and let templates read from the resulting map. For example:

```text
tab -> git.repo=rjwittams/katzensteg
git.repo=rjwittams/katzensteg -> vcs.pr=#45
vcs.pr=#45 -> ci.status=failing
```

The bad lazy model is one graph search per template key. The intended model is one bounded traversal per target/generation, producing a resolved map plus provenance/source candidates. Union-find may still help with strict equivalence classes, but implication and nearby semantic facts need graph traversal with distance/provenance.

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

Status: started in the rail plugin. Normal rendering now uses local render nodes for groups and tabs, and groups can be collapsed/expanded by clicking the group header. Collapse and vertical scroll state live in one controller-owned session snapshot that is broadcast to every rail. The old collapse delivery reused per-client view-model pushes addressed only to renderers already known to the controller, making registration order a delivery dependency, while scroll state remained entirely local. The dedicated rail UI broadcast is untargeted, new rail instances request the current snapshot on startup, and totally ordered revisions prevent delayed or concurrent controller broadcasts from leaving instances on different snapshots.

### Spindly Hierarchies Should Conflate Compatible Levels

Deep grouping rules can produce noisy trees when each parent has exactly one child. For example, `project -> repo -> branch` is useful when a project contains multiple repositories or a repository contains multiple branches, but it is wasteful when a project has one repo and that repo has one visible branch.

The renderer should be able to conflate compatible single-child group chains without losing the underlying identities. Each original group prefix remains addressable for metadata, templates, ordering, toggles, and external facts; the visible node is only a combined presentation of the chain.

This is related to the "header niche" idea for dense child content. Both features are about using the visible header row as a denser projection of the underlying tree without changing the tree itself. A conflated group header may have more semantic context available, and a group header may also have spare horizontal space where a prefix of direct child tabs can be rendered as strip segments. These should be implemented as layout projections over the same recursive node tree, not as special grouping modes.

Compatibility should start conservative:

- only conflate adjacent groups, never tabs or latent nodes.
- conflate only when the parent has exactly one group child and no direct tab/latent siblings.
- preserve all original path prefixes internally.
- let templates decide how to render the combined label from ancestor metadata.
- avoid conflating across a group boundary that has an explicit local setting, pin, manual order, attention marker, or body content.
- keep child absorption separate from conflation: a header can absorb some direct tabs into a niche without pretending those tabs are part of the group label.

This is likely to become load-bearing as grouping gets richer, because one flexible grouping rule needs to look reasonable across very different workspace shapes.

Status: started in the rail renderer. A single-child group chain can now be visibly conflated into one header after resolved metadata has been merged into the render tree. The visible node keeps the deepest group path for normal group actions and stores the conflated prefixes for inspection. The visible title joins conflated labels with the rail border character so it reads as one combined header. The current conservative blockers are direct tabs, collapsed groups, and explicit `var.child-layout` values that change across a boundary. These blockers are intentionally provisional; as inherited variables, body/content slots, priority areas, and header-niche absorption take shape, the compatibility rule should be revisited rather than treated as final.

### Group Prefixes Are First-Class Controller Targets

Hierarchical grouping should not treat only the deepest group as real. Every prefix of a `GroupPath` is a semantic group target:

```text
[(andamento.project, zellij)]
[(andamento.project, zellij), (git.repo, zellij-org/zellij)]
[(andamento.project, zellij), (git.repo, zellij-org/zellij), (git.branch, feat/kitty-image-plumbing)]
```

The controller should resolve metadata and templates for each prefix and send those resolved states to the rail. The rail should remain mostly dumb: it builds the local recursive render tree, merges resolved metadata/templates by path, applies local layout decisions plus the shared collapse/scroll snapshot, and draws.

This keeps grouping and template matching controller-centric while preserving room for client-local variants. If templates later need active/inactive, collapsed/expanded, or width-specific fields, the controller can send multiple resolved template states for the same node rather than moving all matching logic into every rail instance.

For now:

- grouping rules project tab metadata into a full `GroupPath`.
- each prefix of that path becomes an addressable `MetadataTarget::Group`.
- group seed metadata is the key/value pairs in the prefix.
- external metadata attached to those group identities participates in normal identity resolution.
- group-header templates are resolved for each prefix from the controller's resolved metadata.
- tab rows carry their exact parent group path so a group can contain both direct tabs and child groups without relying on the rail's previous "last group header" state.
- direct tab children render before child group nodes within the same parent, while preserving relative tab order and relative group order.
- resolved template fields carry optional metadata key/value provenance. While rendering a hierarchy, a descendant elides any field whose exact metadata key/value was already visibly rendered by an ancestor group, so shared facts such as `git.repo` do not repeat under every branch.
- collapse toggles and other rail-local affordances stay outside external templates.

This avoids locking the system into the current two-level `group -> tab` display and makes project/repo/branch hierarchies behave like one regular mechanism.

### Metadata Inspection Mode Should Arrive Early

The rail should have a metadata inspection projection before the final template system is ready. This is not just a config-panel debug screen; it is a mode of the actual sidebar so the user can inspect the data plane in the same hierarchy and spatial context that normal rendering uses.

Status: partially implemented. The metadata rail view now consumes the same local render-node projection as normal rendering, so local tab overrides and grouped hierarchy are shared. It also shows controller-resolved template names and fields when present, falling back to built-in template diagnostics otherwise. The projection is grouped into sections for metadata, sources, identities, templates, grouping, and status so the same information is easier to scan. It still renders unframed `key: value` lines rather than full dynamic tiles, and it does not yet expose click-to-expand per-key detail.

Early behavior:

- every node expands to the biggest size it needs, regardless of its selected region form.
- every present resolved metadata key renders as a generic `key: value` line.
- groups, tabs, and later panes/latent nodes all use the same generic metadata display.
- clicking a value can reveal raw resolution detail for that key: source entries, ttl, updated time, precedence, ordinal, and later the reason a value won.
- the projection can later show which template matched and why.

This mode is also the authoring loop for future match templates:

```text
edit external template/config file -> reload -> inspect metadata and match result -> adjust
```

The plugin should preview, reload, and report parse/match errors, but editing should stay in the user's normal editor rather than implementing a terminal text editor.

### Config UI Should Be A Testable Inspector Surface

The config plugin is the right place for a richer settings and inspection surface, but it should not grow a second hand-written metadata inspector before we know how we want to render it. The immediate direction is:

- keep the config plugin split into a native-testable library plus tiny Wasm entrypoint.
- use the current Settings/Templates/Stats pages as the ratatui feasibility target.
- add an `Inspect` page only after the rendering approach is chosen, so the first inspector implementation can use the same widget/layout path that later metadata controls will use.
- keep the rail's existing metadata view as the source of truth for in-context hierarchy inspection until that config inspector exists.

Status: started. `andamento-config` now has the same lib/bin shape as the rail, so native unit tests run against the plugin implementation without the `register_plugin!` entrypoint collision. The config renderer now uses a ratatui-backed frame: pages render into a `Buffer`, the frame converts that buffer back to Zellij text, and Andamento still records its own `ConfigAction` hit regions for mouse interaction. This builds for `wasm32-wasip1` with `ratatui` default features disabled and only `std` enabled. The evidence so far supports using ratatui in the config plugin, but not moving the rail to ratatui yet: the rail remains custom enough that its layout, image placement, hover behavior, and recursive dense projections need tighter control.

Small fixed enum controls should render as inline segmented radio rows rather than one option per row. For example, `structure`, `child-layout`, and metadata visibility fit better as one labeled row each, with every segment directly clickable. Drop-downs should be reserved for large or dynamic option sets because they hide the available choices and require extra open/close state.

The config plugin also has a first `Inspect` page. When opened from the rail with a scope such as `tab:<id>`, the rail now sends a `ConfigInspectRequest` to the controller instead of directly launching a config plugin by URL. The controller prefers an already-registered config editor on the same client and sends it `andamento-config-inspect`; if none exists, it launches a focused floating config editor with the same scope. The config editor updates its scope on that message, switches to Inspect, and resolves the selected tab from the controller view model. The current page shows selected scope, model counts, metadata-control state, tab state, grouping, and active pane. This is intentionally the beginning of the richer item-focus surface rather than a separate metadata renderer.

### Template Matching Is A Projection Policy

Template rendering should be driven by match rules over render nodes and resolved metadata, not by one-off code at each hierarchy level.

General shape:

1. match on render node type, such as group, tab, pane, or latent.
2. optionally match metadata predicates: exact value first, prefix matching early, regex only if needed later.
3. choose the most specific matching template by default.
4. later allow interactive cycling among multiple matching templates for a tile.
5. render a priority list of fields, each with its own coalescing, compaction, and truncation behavior.

Tile density is a region-form policy. The selected form establishes the default shape, ordered `promote` rules move qualifying nodes to another form, and the renderer performs automatic squash-down within that form based on available space. Templates do not carry a separate sizing hint.

Field compaction should eventually happen before priority dropping. A string field can expose progressively shorter representations, such as full repo slug, basename, user-configured alias, and finally ellipsis truncation. Some compact labels can come from transforms, such as `rjwittams/katzensteg` -> `katzensteg`; others should be supplied by watchers or config when a user-specific abbreviation such as `ks` is meaningful enough to distinguish the node.

Status: started internally. The renderer now has generic ordered template fields with required, optional, and priority classes. Render nodes also carry an initial metadata map populated from the current compatibility view model. Group headers, tab titles, and tab status text are the first field callers, and these field builders now read their display values from render metadata. This is still hard-coded Rust, not external template config, and should be extended to nested groups before adding user-authored templates.

### Header Rows Need An Inline Layout Model

Header rows and tab strips need a smaller layout primitive than the future recursive body/content tree. These rows are not rectangular pages like the config plugin, and they are not just strings: they contain symbols, labels, counters, template fields, separators, focus/inspect controls, future image-backed buttons, and exact mouse hit regions.

The rail should therefore keep a custom inline layout model rather than move this part to ratatui. Ratatui is useful for config pages and inspector forms, but the rail needs tighter control over:

- visible-width measurement of styled text.
- priority fitting and truncation before full field elision.
- exact Zellij tabbar segment glyphs and colours.
- hit payloads tied to the placed item, not the original string.
- child strip runs that can later fit into a group header niche.
- optional future item kinds such as image placeholders, action buttons, focus toggles, and priority/pinned chips.

Status: started in `andamento-rail`. `InlineRun` and `InlineItem` now model the row as placed items with required/optional/priority classes, min widths, truncation, wrapping, styled text measurement, and local hit payloads. Group header template fields are packed through this inline model, so low-priority fields truncate with `...` before disappearing entirely. Child tab strips also use inline placement before rendering their Zellij-tabbar-like segments, preserving template-resolved tab labels and click regions.

This model is intentionally a row-level projection. It does not replace `body`, `content`, or `children`, and it should not become a second tree model. A recursive node can later project part of itself into an inline run, for example:

- collapse toggle + group title + count.
- group title plus absorbed child content in a spare header niche.
- dense focus/action controls when an inherited "show controls" variable is active.
- priority/pinned tab chips duplicated into a top-level attention area.

Header niche absorption has a first conservative implementation. A group with effective `var.child-layout=strip` computes spare header width after its own title/template fields. If direct tabs at that group level fit, the renderer absorbs a prefix of those tabs as one adjacent Zellij-tabbar-like segment run and leaves overflow below. Once any same-level tab has been absorbed, sibling child groups are not absorbed into that same header.

If no direct tabs are absorbed at that level, the renderer may absorb one child-group path instead: the first child group label is rendered as a header fragment, separated with the rail border character, and the child group can recursively donate its own first direct tab run into the remaining niche. Collapse state only controls the remaining body rows below the header; it does not hide content that has already been projected into an ancestor header niche. Absorbed tab runs are right-aligned within the available niche and keep switch-tab mouse hits. Absorbed group fragments are display-only for now, because toggling a projected group label while its overflow remains below is confusing. If there is only one child group and the rail is too narrow to absorb that group completely, the renderer falls back to normal grouped rendering for that child group instead of producing a partial header/continuation split.

This remains a projection over the recursive tree. Absorption does not mutate grouping identity, ordering, collapse state, or inherited settings. Content that does not fit remains in the normal child rendering below the header. If a child group is partially absorbed, its overflow children render below without repeating the child group's own header, avoiding a second toggle for a group that is already represented in the parent header. A group header only shows a collapse glyph and registers a collapse hit when the current projection leaves body rows that the toggle can reveal or hide.

### Bodies, Content, And Slots Need A Recursive Layout Model

Headers are not enough. Groups, tabs, panes, and latent nodes need a recursive slot/layout model. The important distinction is:

- `body` is the whole main area under a node's header.
- `content` is the node's own template-produced surface inside that body: details, actions, previews, summaries, controls, and local status.
- `children` is the recursive projection of descendant tabs, groups, panes, and latent nodes.

The default body layout can be thought of as `column(content?, children?)`. The model should keep those parts separate so a template or policy can later compose, reorder, hide, or replace the child projection without pretending child nodes are just body text.

Body layout and content slots should be able to render richer material when space allows:

- one or more metadata text lines.
- status/progress rows.
- image previews or icons.
- dense pane/tab summaries.
- action affordances.
- expanded debugging/details in metadata view.

The template result should be a small recursive layout tree rather than a single string. The first implementation can support a narrow subset, but the schema should be able to describe at least:

- rows and columns.
- text nodes with optional metadata/value sources.
- image nodes backed by the same image-placement machinery as pane/image chrome.
- references to other slots, including `children`.
- conditional fragments driven by metadata predicates and inherited UI properties.
- actions on text or images, such as focus, materialize, browser/editor launch, new tab, or floating pane.

Layout policy should stay separate from matching. A template can produce content such as text lines, image slots, counters, child references, or command buttons; the rail decides how much of the resulting body tree fits in the current projection.

Open layout policies:

- vertical body under a header, usually content followed by children.
- one-line body folded into the header.
- horizontal tab strip for child tabs.
- responsive wrap/masonry for child nodes in expanded or wider modes.
- hidden body with only header affordances in navigation mode.

Template fragments should be able to test both node metadata and inherited UI properties. For example, a group body can appear only when `git.repo` exists and `show-repo-actions` is enabled. This lets bottom-bar toggles, global header/footer controls, and per-node settings drive local template output without hard-coding global modes into every render path.

This slot model should also handle image asset chrome/buttons. The bottom control row and per-node affordances should eventually be able to use carefully-crafted transparent image assets for gear/chevrons/toggles/status buttons. Text fallback stays useful, but image assets make independent scaling, hover animation, and more polished chrome possible.

Image chrome should be optional and controlled by a visible rail toggle plus config. It depends on the same Zellij image placement machinery as pane images, and assets need to be designed with transparency and terminal cell scaling in mind.

### Node Variables Inherit Down The Tree

Many future controls are naturally scoped to a subtree:

- tab/card style.
- show/hide images.
- body form: `compact` or `detail`.
- child layout mode: cards, horizontal strip, or masonry.
- metadata/debug visibility.
- focus/action button visibility.
- priority/pinned duplication policy.
- factory/latent-node display policy.

The model supports declared per-node variables that inherit from the nearest explicitly-set ancestor. A group header can expose small right-aligned controls for local overrides, while defaults flow from the root or profile. Template `set` operations establish node-local values; applicable configuration-layer `set` operations win at the same node. Template fields read effective values from the `var.*` namespace.

This keeps configuration ergonomic: a user can say "this repo group shows images" or "this project uses horizontal child tabs" without setting the same value on every child. It also gives templates a stable way to ask for local display policy without hard-coding global modes.

Status: enacted. The bundled document declares `child-layout` with allowed values `cards` and `strip`, defaulting to `cards`. Effective variables and provenance travel in the controller view model and are exposed to rendering metadata as `var.child-layout`. A group using `strip` first offers spare header width to the header-niche projection, then renders remaining direct tab children as a Zellij-tabbar-like segment run before rendering child groups normally. Strip segments use the exact Zellij powerline separator glyph with active/inactive tab foreground/background colours, and `rail_segment_between_color "#RRGGBB"` can override the in-between separator colour to match the terminal background. The Inspect page shows the winning setter, ancestor, layer/file, and overridden history, and can set or clear a root/group override.

The segment renderer is deliberately pure and bounded: callers provide the available width and get rendered text plus hit geometry back. A group header can now pass a smaller "niche" width and absorb a prefix of direct child tabs into an available gap without the segment code assuming it owns the whole row. Segment style is also separate from segment data so future runs can be selected from metadata, choose explicit adjacency/gap rules, or be replaced by image-backed segment assets without changing the child-node projection model.

Strip child layouts should not assume that every affordance is always visible. For example, focus/config/action buttons may be hidden by default when tabs are rendered as header segments, because there may not be enough space to show them without destroying the density benefit. A separate inherited variable should put a subtree into an action/focus mode where those controls are visible or given priority. This avoids overloading child layout itself with "show buttons" semantics.

The layout should eventually be adaptive within a single group. Some direct tabs can be absorbed into a header niche, overflow into a child strip, or expand as cards when they have important status, icons, body content, or actions to show. This means `child-layout` should be treated as a policy preference and starting point, not as a rigid one-renderer-per-subtree command.

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
   Status: multi-segment paths now expand into nested render groups, common prefixes are shared, and nested group/tab indentation is derived from group depth. Existing one-segment directory grouping remains the compatibility baseline. The next controller slice is to emit and resolve metadata/templates for every group prefix, so intermediate groups are first-class model targets rather than renderer-created placeholders.

5. **Keep layout policies separate from tree shape.**
   Joined cells, boxes, collapsed groups, horizontal sub-tab bars, and expanded overview modes should be projections over the recursive tree. Do not encode "children of groups are tab rows" into the data model.

6. **Add template matching only after metadata and recursion are stable.**
   Start with hard-coded template definitions over node type plus metadata predicates. A template should produce ordered fields; region forms own density. Only after this is proven should the config file expose user-authored templates and reload diagnostics.
   Status: started. The renderer has a template resolver keyed by node kind, template slot, and metadata. Built-in group header, tab title, and tab status templates route through it while preserving the current visible output. Built-ins describe fields with field class, coalescing value sources, optional prefix/suffix wrappers, and simple conditions. The metadata view shows the matched template, source chain, and effective fields. The former template sizing hint has been removed; region form and promotion declarations now own density.

7. **Add external templates last.**
   External config should target stable concepts: node type, metadata predicates, field lists, and truncation/coalescing rules. It should not expose temporary compatibility structs or assumptions about exactly two hierarchy levels.
   Status: started. The shared crate owns the KDL template config schema and validator covering template names, node kind, slot, field specs, value sources, conditions, node variables, and region policies. Parsed config becomes a layered catalog that resolves templates and renders field specs into ordered text fields. A KDL authoring format uses `template` nodes for `slot` and `node-kind`, `field` order for render order, numeric `priority` for width pressure, `variable`/`set` for inherited policy, and `region`/`promote` for density. The controller loads the config from `template_config_path`, resolves it against node metadata, and sends `ResolvedTemplateSlots` plus diagnostics in the view model. Rails consume resolved fields and variables, apply local layout/truncation, and keep client-local affordances such as collapse toggles outside templates. The config plugin has templates and Inspect pages for resolution evidence; reload-on-change is still future work.

The key dependency is: metadata-backed fields first, recursive nodes second, configurable templates last. That avoids the pointless loop of generic metadata being projected into `TabStatusSummary` and then mapped back into generic template fields.

### Latent Tabs Are Materializable Nodes

A latent tab is a sidebar node for work that is not currently a Zellij tab. On activation, it can be materialized by sending a Zellij action or plugin message that creates the real tab/panes.

This should wait until group identity and render nodes are stable, but it is a natural fit for flotilla. Flotilla can expose desired work items as latent nodes, while running agents appear as materialized tabs/panes under the same group path. Watchers can also provide latent tabs directly; for example, a git watcher can discover sibling worktrees in the same repository and offer them as materializable nodes before any Zellij tab exists for them.

Latent nodes should have:

- stable group path identity.
- display label/template data.
- metadata such as status, priority, progress, and owner.
- an activation/materialization recipe.
- an optional relation to a materialized tab once created.
- provider/source id, so duplicates from multiple watchers can be resolved.
- display caps, filtering, and inline search when a provider exposes many possible nodes.

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

### Native Plugins And External Processes Are A Major Fork Direction

The current wasm plugin shape is useful, portable, and sandboxed, but it makes profiling hard and forces a lot of host/plugin serialization. There is a native-plugin spike in `~/dev/zellij.native-plugins`; if that direction works, Andamento is a strong candidate for native execution because it has a controller, multiple rails, rich state, and increasingly expensive rendering/projection work.

Native plugins would help with:

- real profiling and flamegraphs.
- avoiding pointless serialization between tightly-coupled local plugin panes.
- sharing one controller/runtime across multiple visible panes.
- richer image asset handling.
- lower-latency rendering and input feedback.

Longer term, external plugin processes could recover some sandbox properties while keeping native performance. Platform-specific isolation such as chroot, seccomp, or equivalent mechanisms can be explored later. The important design constraint now is to keep controller, renderer, watcher, and transport boundaries clean enough that Andamento can run in wasm, native-plugin, or external-process shapes without rewriting the core model.

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

Status: partially implemented. The config editor can now switch between ungrouped and directory grouping. The controller can also load external grouping rules from `grouping_config_path`; rules project resolved metadata into structured group paths, while `rail_grouping "none"` remains the flat rendering switch.

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

### 4a. Configurable Hierarchical Grouping

Treat grouping as a projection from resolved metadata to an optional `GroupPath`, not as cwd-specific controller logic.

Scope:

- load grouping rules from KDL/JSON through `grouping_config_path`.
- try rules by priority, preserving file order as the tie-break.
- let each rule define ordered metadata levels.
- allow optional levels, especially higher-level project/workspace labels.
- capture an entity only when every non-optional level derives; otherwise let
  later rules or the catch-all try it.
- when an optional level is missing, keep the rule captured and place the entity
  at its deepest derived level, beside any deeper sibling groups.
- keep different rules independent; they do not mingle into one hierarchy.
- keep segment identity as `key=value`, with an optional display label such as `repo.name`.
- retain the built-in directory fallback when no external grouping rule applies.

Example:

```kdl
grouping "proj-repo-branch" {
  priority 100
  level key="andamento.project" optional=true
  level key="git.repo" label-key="repo.name"
  level key="git.branch" optional=true
}
```

Status: started. The shared crate parses grouping rules, the controller evaluates
them against resolved tab metadata, identity-enriched cwd facts can now produce
repo/branch hierarchies, and group segments can carry display labels without
changing their identity. Capture requires every non-optional level to derive;
missing optional levels collapse placement to the deepest derived level.

### 5. Group Metadata Targets

Allow metadata patches to target semantic groups in addition to panes and tabs.

Scope:

- extend metadata target/entity id with `Group(GroupPath)`.
- support group-targeted metadata in the store.
- expose group-targeted values through resolved group views.
- define merge behavior between direct group metadata and rollups from child panes/tabs.
- add debug rendering for direct group values versus rolled-up values.

Status: started. The shared model now has `MetadataTarget::{Pane, Tab, Group}` and `MetadataPatch`/`MetadataValueUpdate`, and the controller metadata store can apply collaborative source-scoped patches internally. External patch input now feeds the same store and resolved metadata can surface tab/group values. Direct group metadata resolves for groups created by either cwd fallback or explicit typed `tab.scope`. Rollup/direct merge behavior and richer raw-source interaction are still future work.

### 6. Explicit Tab Scope

Allow tabs to state exactly where they belong in the group hierarchy.

Scope:

- add well-known key `tab.scope`.
- represent scope as typed `MetadataValue::GroupPath`.
- prefer explicit typed tab scope for grouping when present.
- fall back to pane-derived cwd grouping.
- render scope in debug views.

This unlocks project overview tabs, worktree tabs, convoy tabs, and better grouping for tabs whose panes are not enough to infer intent.

Status: started. The shared metadata value schema now includes `MetadataValue::GroupPath(Vec<MetadataPathSegmentValue>)`, using scalar path segment values so the type does not recursively contain arbitrary metadata values. The controller recognizes only typed `tab.scope` values as explicit grouping identities, and those take precedence over configured grouping rules and cwd fallback. Text `tab.scope` and `tab.subject` do not affect grouping. Resolved metadata view shows these keys because it renders generic resolved tab/group values.

### 7. Render Nodes, Collapse, And Templates

Move from a flat row projection to first-class render nodes.

Scope:

- recursive group, tab, and future latent node types.
- group children represented as `Vec<RenderNode>`, not `Vec<Tab>`.
- metadata maps on every render node as the template input.
- optional conflation of compatible single-child group chains.
- collapsed/expanded state.
- group-level borders and status/progress rollups.
- label templates using resolved metadata.
- string substitution rules for compact labels.
- body slots for richer group/tab/pane content.
- inherited per-node settings/toggles.
- optional sub-tab-bar, horizontal strip, or masonry projection for lower levels.
- optional image asset chrome/buttons and hover/active animations.
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

Status: started. Producers can send `ExternalMessage::MetadataPatch` through `tabs-apply-metadata-patch`; the controller applies set/unset updates with source ids, precedence, ordinal, and ttl through the shared metadata store, and resolved entries are exposed through the view model. Controller bootstrap snapshots now carry live metadata patches so newly attached clients can recover current external metadata.

Watcher discovery should also be pipe-first. A simple daemon can run in a pane, periodically call `zellij pipe --name andamento-observed-identities`, read the JSON list of observed identities from stdout, enrich the identities it understands, and publish facts back through `tabs-apply-metadata-patch`. For example, [scripts/andamento-git-watcher.py](../../scripts/andamento-git-watcher.py) looks for `zellij.pane.cwd` identities, discovers repository root/branch/remote with local git commands, then patches facts onto `MetadataTarget::Identity(zellij.pane.cwd=<cwd>)`. A later flotilla connector can use the same protocol but maintain richer state and scheduling.

The same script now has an opt-in factory spike. With `--factory-repo-manager`, it dedupes by tab name, calls `zellij action new-tab --layout <repo-manager-layout> --cwd <git-root>`, reads the tab id lines printed by the CLI, and patches each created tab with durable tab metadata:

```text
tab.kind = repo-manager
tab.scope = GroupPath([{ key = git.repo, value = owner/name, label = name }])
factory.id = repo-manager:owner/name
```

This deliberately pushes on the scripting surface rather than controller-owned tab creation. It proves that an external daemon can both materialize a tab from a repeated KDL layout and then target the returned stable tab id with generic metadata. The current example layout embeds the controller and watcher in the initial `andamento` tab, with a short-lived helper command that scopes the control tab itself under an `andamento` path. That helper resolves its own `ZELLIJ_PANE_ID` through `zellij action list-panes --json --all` so it patches the tab containing the helper pane, not whichever tab is currently focused after factory-created tabs appear. The layout also sets `controller_plugin_url ""` on the rail alias so rails broadcast to the embedded controller instead of launching another controller instance by URL. The repeated KDL in [layouts/repo-manager-tab.kdl](../../layouts/repo-manager-tab.kdl) is expected for now because Zellij does not provide a slot-style layout primitive that lets an external tab layout say "use the session tab chrome here".

### 11. Aggregation And Profiles

Add key-aware aggregation and default profiles.

Scope:

- progress pair aggregation.
- list union/dedupe.
- severity/status rollups.
- profiles for dense navigation, workflow/status-heavy, and overview-heavy usage.

### 12. Ordering, Pinning, And Manual Arrangement

Add richer ordering as a policy over projection items.

Scope:

- group ordering from aggregated metadata, such as most recent `needs_attention`.
- recent activity ordering.
- pinned groups.
- pinned tabs.
- individual pinned tabs that render above grouped sections even if their original group is lower.
- priority areas that can show important tabs independently of their normal grouped location.
- an inherited policy for whether a tab shown in a higher-priority area is still repeated in lower-priority projections.
- drag of groups or tabs that switches the affected scope into manual ordering mode.

The projection should support common item types such as:

```text
ProjectionItem =
  PriorityTab(tab_id)
  PinnedTab(tab_id)
  PinnedGroup(group_path)
  Group(group_path, children: Vec<TabId>)
  Tab(tab_id)
```

The same underlying tab may appear in more than one projection area. For example, a tab can be shown in a priority or pinned area while also remaining in its normal group, or it can be suppressed from lower-priority areas to avoid duplication. This should be configurable as an inherited display policy, because dense navigation, status-heavy, and overview-heavy profiles will want different answers.

A future layered ordering policy can be:

1. pinned items.
2. priority/status projection areas.
3. manual order if present.
4. metadata-derived urgency/activity order.
5. grouping/default order.
6. stable tab order fallback.

### 13. Latent Tabs And Materialization

Represent work items that are not currently real Zellij tabs.

Scope:

- latent render nodes.
- materialization recipes, eventually loaded from the same external configuration surface as templates/grouping rules.
- activation behavior that creates a tab/panes or asks another plugin to do so.
- relation from latent node to materialized tab.
- flotilla integration for desired work items and convoys.
- watcher-provided latent nodes, such as sibling git worktrees.
- caps, filtering, and inline search for large latent sets.

This should wait until group paths, group metadata targets, and render nodes are stable enough that latent nodes do not become a parallel model.

### 13a. Rename And Publish As Andamento

The prototype should be renamed comprehensively once the current feature work is committed.

Scope:

- rename workspace/package/crate names from `vertical-tabs` and `tabs-*` to `andamento-*`.
- rename wasm artifacts and Zellij plugin aliases to `andamento-controller`, `andamento-rail`, and `andamento-config`.
- rename pipe names to the `andamento-*` namespace.
- update scripts, layouts, README, and docs.
- remove old compatibility names rather than maintaining aliases.
- prepare the repository for eventual publication at `flotilla-org/andamento`.

This should be its own mechanical slice after the current work is stable, because mixing rename churn with behavior changes will make review and rollback unnecessarily hard.

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
- How should group-targeted metadata and rolled-up child metadata resolve when they set the same key?
- Should group order follow first visible tab occurrence, explicit group metadata, recency/activity, manual order, or a layered policy from the start?
- What should the first label-template syntax look like, and how much formatting should it support?
- Where should collapsed state live: transient controller state, plugin config, or the future `/host` config file?
- What is the minimum materialization recipe shape for latent tabs without coupling too tightly to flotilla?
- When external metadata arrives, should values be typed JSON-like data, strings only, or a small tagged enum?
- What exact rules make two single-child group levels compatible for visual conflation?
- What is the first useful header-niche policy: absorb only direct tabs, absorb direct tabs plus compatible child groups, or allow priority/pinned duplicates from elsewhere in the tree?
- What is the first body/content/children slot schema that can support rows, columns, text, status, buttons, images, actions, and child references without becoming a full UI framework?
- Which node settings should be inherited first: image visibility, child layout mode, or compact/detailed body mode?
- How should image chrome assets be packaged, cached, scaled, and toggled?
- What is the minimum native-plugin API needed for Andamento to avoid the worst wasm serialization costs while preserving the same state boundaries?

## Recommended Next Step

Stabilize the current feature work, then do the comprehensive rename to Andamento as its own slice.

The stabilization checkpoint should:

- commit the current controller/rail/config feature set.
- keep the Zellij fork exit-path fix as a separate commit in the Zellij repository.
- preserve the current scratch layout as the runnable demo.
- record that `flotilla-org/andamento` is the intended publication target.

After that, the rename slice should hard-cut old names rather than adding compatibility aliases:

- workspace/package/crate names to `andamento-*`.
- wasm artifacts and plugin aliases to `andamento-controller`, `andamento-rail`, and `andamento-config`.
- pipe names to the `andamento-*` namespace.
- launcher scripts, layouts, README, and docs.

The next feature slice after rename should probably be render/layout work, not more metadata plumbing: spindly hierarchy conflation, body slots, inherited node settings, and richer child layouts are all on the direct path to making the existing metadata useful on screen.
