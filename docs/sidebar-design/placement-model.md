# The Placement Model

Agreed 2026-08-08; syntax and migration added 2026-08-11; revised the same day
after review. All in grilling sessions. This records decisions and the reasons
for them. It is not an implementation plan.

Read `current-grouping.md` first for what the shipped grouping actually does.
The short version: one ordered list of seven optional levels is walked for every
entity, and each entity's path is whichever of those keys it happens to carry.
Because levels and presence classes are independent lists over the same
entities, **every kind is doubled** — it appears once as the group node its own
path segment creates, and once as the node its presence creates. That is the
layer this model replaces.

## The model

**Pull, not push.** Today every entity computes where it belongs and the tree is
what emerges. Instead, each node asks what it contains. A project node's
children are the convoys in it; a convoy's children are its vessels. Nothing
computes a global placement.

This is what dissolves the doubling: a convoy that is queried for is a node with
children, not a path segment that also happens to be an entity. There is no
second thing to suppress.

**Queries are fact predicates.** Matching on `entity.kind` is sugar over the
common case, not a privileged axis. The current
`filter key="entity.kind" exists=true` already shows the matcher is fact-shaped;
it is just that there is one matcher and one global level list.

**Placement.** A *placement* is one appearance of an entity under one node via
one query. An entity may have several. Placement identity is separate from
entity identity.

- Placement state: collapse, node toggles, layout variables. These inherit down
  the placement tree.
- Entity state: highlight and pin. Hovering an entity in one place highlights
  every occurrence — the hovered one more emphatically — so you can see where a
  thing normally lives from the attention tray.

**Unmatched entities are surfaced outside the rail.** Never in it: if unmatched
things appeared in the rail, hiding anything would be impossible. But a pull
model silently drops what no query asked for, and that failure mode has to be
visible somewhere. *Open: where, and how it tells deliberate hiding apart from
an accidentally-omitted query.*

**Mixed content is not foreclosed.** A loop may return children of several kinds,
sorted across them, with rendering chosen by fact match. Not needed now; it must
not become constructively impossible, or a later agent will decide it was banned.

## Placement identity

A **placement key** is an ordered chain of `(loop-name, entity)` pairs from the
root down. A section is the outermost link, because a section is a root loop.
The entity is needed at every level or two siblings' subtrees collide.

This is deliberately the same *shape* as `GroupPath` — an ordered path of pairs,
with loop names where fact keys were and entities where fact values were. The
collapse snapshot, its serialization, scroll state and inspect addressing keep
their shape; only the contents change.

Two consequences, accepted rather than discovered. Renaming a loop resets
everything collapsed beneath it, which is right: a renamed loop is a different
arrangement. And keys are long — but `GroupPath` already is, so this is not new.

**`NodeKey` needs a fifth variant.** It is currently
`Root | Group(GroupPath) | Tab(u64) | Entity(EntityRef)`. An earlier draft of
this document claimed placement-scoped layout variables were "free on the wire"
because `NodeVariableSetRequest.name` is a free-form string. That was wrong: the
*name* is free, but the *target* is a `NodeKey`, which cannot address a
placement, so several appearances of one entity cannot carry distinct state
through that interface today.

Metadata still targets entities and groups. Only UI state targets placements.

*Open: pin.* This document treats pin as entity state, but `ControllerState`
holds `pinned_tabs: HashSet<u64>` and `toggle_pin(tab_id: u64)` — pin is keyed
on a zellij tab id. Those are materially different contracts and one has to give.

## Sections

**A section is a root template.** Whether it contains loops is its own business:
Tree and Attention have them, Controls does not. Nothing needs to be optional,
because a template with no loop is already legal.

This removes `SurfaceRegionSource` — the four hardcoded variants that are the
only reason header/attention/tree/controls behave differently in code — and
`attention_key`, which is a degenerate query special-cased because there was no
query mechanism.

Section titles are *already* templated (`root-template` resolves to a template
whose only field is a literal), so nothing there is hardcoded today. Section
styling — colour, border — does not exist yet and would be new fields.

`pinned` stays: attention and controls must not scroll away, and that is a real
property a plain node does not have. Dragging sections or resizing them is much
later.

Labels are optional and often unwanted. A separator is what you get when a
section renders no line of its own.

**Controls needs a third content type.** Its contents are variable toggles, the
gear, and the scroll affordances — not catalog entities. Those are hardcoded
today in `render_footer_with_variables` (all of it except the root inspect
toggle). They become **control widgets**: template content alongside `field` and
`loop`. Synthesising "control entities" to fit them into the query model would
contaminate the entity model and is rejected.

**Overview is dropped.** It does nothing today, and root/fleet-level things can
live in the main tree when they are real. Keeping it would have forced a fourth
content type — aggregates — since the root has no entity to read counts from
(`awareness_node_entity` returns `None` for `Fleet`, so flotilla publishes no
fleet entity). If it returns, it is probably summary information and pinned
things.

So the content vocabulary is three: `field`, `loop`, `control`. Each section
requirement so far has added one, which is DSL creep in progress; dropping
Overview is what stops it at three rather than deferring the problem.

## Hover geometry

**The hover card gets a fixed multi-row area.** It is reserved whether or not
anything is hovered, consistent with columns being reserved by declaration.

The single-line detail surface in the current renderer
(`render_lines_with_detail_surface` steals exactly one row) was an incidental
agent decision, not a constraint. This matters because the whole argument for
rigid rows — that abbreviation is affordable because detail lives in the card —
depends on the card having somewhere to live, and a zellij plugin cannot draw
outside its pane. A floating overlay is not available.

Making the area collapsible or resizable is a later question, not a blocker.

## Presentation

A template body contains fields and **loops**. A loop carries a name, the query
that fills it, and a layout; its block says how to render each result — either
inline, or by handing off to another template. See *Syntax* below.

An earlier draft had a separate `slot` construct that a loop fed. It earned
nothing: the layout can sit on the loop, and the only other job — being
addressable, for layout variables and as the alignment scope — can use the
loop's own name. It collapsed.

Any "separate X from Y" bottoms out somewhere, and it bottoms out here: the
loop's layout is a presentation decision written at the query site. That is
accepted.

**The template-slot vocabulary becomes presentation types only.** The current —
`group-header | tab-title | tab-status | compact | detail` — is two axes wearing
one name. The first three are structural roles from the push model. In a pull
model there is no group-vs-tab distinction to encode, so:

- `group-header` and `tab-title` merge into the **collapsible line**
- `detail` becomes the **card**, normally shown in hover, inline where there is
  room
- `tab-status` is a field of the card, not a peer slot

The template picks the *type* of presentation, not the exact rendering.

`is_rail_local` is not a structural distinction. It is an ownership rule —
"resolution of this needs state only the rail has" — approximated by slot name,
and the approximation already leaks, since `compact` and `detail` depend on
width too. Notionally the split should be: the controller resolves *which*
template applies, the rail decides *how it fits*.

**Inspect and hover are different surfaces.** Inspect is the property panel — it
already exists, shows everything across tabs in a floating pane, and is mainly
there to understand what andamento is doing. Hover is the templated summary
card: what you wanted to see that could not fit in a pill or a row. A per-item
inspect glyph costs a column per item, which is why niche pills never got one
(`project_direct_tab_header_niche` sets `inspect_target: None` on every hit).
Inspect is therefore likely a mode rather than an affordance rendered per item.

Note the rail currently has no keyboard at all — it subscribes to `Mouse` but
not `Key`.

## Layout discipline

The rail's job is to be a **stable surface to pick things from**, not to show
detail. Detail is the card's job. That is what makes the discipline below
affordable.

**Layout is resolved once per loop instance, not per row.** The loop instance is
the alignment scope. The existing
`render_template_fields_inline_with_suppression` already decides widths and
which optional fields survive — it just decides per row, which is why two
siblings disagree and a PR number on one convoy shifts its neighbours.

Per *instance*, not per definition: only things next to each other need to
match. Different projects may legitimately use different templates — an ML
experiment's convoys might carry a sparkline or a score — so forcing every
instance of a loop into one grid buys nothing and lets one outlier tax
everyone.

**Columns are reserved by declaration, not by content.** This generalises the
toggle-column reservation already in the tree
(`empty_group_reserves_the_toggle_column_without_becoming_collapsible`).

**Abbreviation is producer-supplied on a fixed ladder.** The connector supplies
short forms — short is roughly an acronym, `grouping-live-session` →`gls` —
because the producer knows what shortens meaningfully and the renderer can only
ever produce `grou…sion`. There is precedent: `keys.rs` already calls
`vcs.repo.name` and `flotilla.convoy.name` "display-only companions" to
canonical identity facts. A middle-eliding fallback stays for producers that
supply nothing, as the degraded path. Something smarter could generate these
later.

The canonical keys: `display.label` remains the full form, joined by
`display.label.medium` and `display.label.short`. Suffix companions match how
`flotilla.convoy` / `flotilla.convoy.name` already work, so this needs no new
convention. A missing tier falls back to the next longer one available, and to
eliding only when nothing is supplied.

This is a **contract with flotilla**, not a local decision, and it is the one
part of the layout work that cannot be settled by iterating in the harness.

*Open: collisions.* Two siblings both shortening to `gls` are indistinguishable.
Treating it as producer data quality is the simple answer; promoting the whole
loop to a longer tier is the fit-negotiation banned below wearing a different
hat.

*Open: what happens when the declared tier still does not fit, and whether
heterogeneous templates within one loop share a grid.*

**Tier is declared, not negotiated.** Trying full, then medium, then short until
things fit is denser, but it means materialising one more vessel flips a whole
pill row to acronyms and closing it flips them back — a jolt caused by something
elsewhere on the row rather than by what you touched. Declared tiers will
sometimes show acronyms with obvious room to spare. That is the intended
trade, and it is written down here because it will otherwise look like a bug
worth "fixing" back to negotiation.

## Inline overflow, and collapse

These are two different rules. An earlier draft ran them together and read as a
contradiction.

**Inline overflow** applies to a loop whose layout puts its items on the
parent's line — the niche. Either every item fits there, or they *all* drop to a
row of their own:

```
▾ flotilla ─────────────────── tui  governor        all fit

▾ flotilla ─────────────────────────────────        they don't, so none stay
  tui  governor  reviewer  windows-port
```

What this rules out is today's behaviour, which packs as many as fit onto the
parent's line and wraps or silently drops the leftovers — the source of the
stray `#62` row and the orphaned `governor` row in harness output.

**All-or-nothing is per loop, not per node.** A project's issues and its convoys
each decide independently, so a long convoy name cannot push the issue chips
down.

Children take the remainder of the parent's line, with an inherited node
property to suppress that.

**Collapse is not this.** Collapsed means the children are not shown. There is
no partial list, no count, no packing. Collapsed is collapsed.

**The niche is not a mechanism.** It is a loop rendered inline on the parent's
line, and inherits everything above: per-instance alignment, reserved columns,
the abbreviation ladder, no silent dropping.

## Where layout properties live

Loop-scoped variables with node-scoped defaults — `convoy.tier`, `issue.tier`,
named by the loop, falling back to a node-level default. A project may want its
issues short and its convoys full; one setting per node cannot say that.

This is **not** free on the wire, contrary to an earlier draft.
`NodeVariableSetRequest.name` is a free-form string, but its target is a
`NodeKey`, which has no placement variant — so distinct state per appearance
cannot be expressed through that interface. See *Placement identity*.

The config UI needs work regardless: it does not render variables generically.
`child-layout` is hardcoded as a bespoke `SetChildLayout` action with a
hand-written control gated on `Root | Group`, while the KDL *declaration* is
already generic (name, default, allowed values). So the UI must become
declaration-driven whether or not names are dotted.

## Syntax

Shape only — spelling will move. The point is which constructs exist.

```kdl
template "project/line" {
  field "label" { value source="metadata-text" key="display.label" }

  for "convoy" kind="convoy" layout="lines" {
    match "flotilla.project" of="project"
    apply-template
  }

  for "issue" kind="issue" layout="inline" {
    match "flotilla.project" of="project"
    field "label" { value source="metadata-text" key="display.label.short" }
  }
}
```

**Loop names are singular and are the binding.** `for "convoy"` binds `convoy`
to each item for the duration of its block; an enclosing `for "project"` is what
`of="project"` refers to. The same name addresses the loop's layout variables
(`convoy.tier`) and identifies it in the placement key.

**Loops, not placement rules.** An earlier draft had standalone `placement`
declarations joined to templates by slot name. Writing the loop inside the
template is better: you can read a node top to bottom and see what it contains,
instead of hunting for the rules that point at it. This is roughly XSLT, minus
the match rules — `for-each` and `apply-templates`, nothing else.

**Block content is inline fields or `apply-template`.** Bare `apply-template` is
polymorphic dispatch: it resolves the item's own template by convention, which
is what makes a heterogeneous result renderable. `apply-template="convoy/line"`
names one explicitly, for when you know what you are getting.

Where the override lives matters. Today the escape hatch is
`presentation.template` — a *fact*, so the producer decides how its data is
presented. Moving the override to the call site puts that decision on the
surface, where it belongs. This is the `SEGMENT_*` mistake in miniature and
should not be repeated.

**Predicates are KDL nodes, not an expression string.** `match "key" of="loop"`
joins against an already-bound loop; `match "key" equals=<literal>` matches a
constant. Several `match` nodes conjoin.

This is deliberate. A string grammar grows — someone adds `!=`, then `or`, then
a function, and it becomes a dynamic language nobody designed. Node-shaped
predicates can only grow by inventing a node type, which is a visible act
somebody has to argue for.

**Every predicate must be servable by an index.** Equality only; left side a
fact on the thing being matched, right side a constant or a fact of an
already-bound loop. This is what keeps nested loops from being quadratic: if the
controller indexes entities by (key, value) once per model build, a `match` is a
hash lookup rather than a scan. The moment a predicate cannot be indexed, every
nested loop becomes a full scan of the catalog.

So the rule against operators is a performance property, not a style
preference — which is the point. A later agent wanting `!=` has to argue against
the index, not against taste.

The accepted cost: no negation, no `not exists`. "Attention things that aren't
done" is not expressible and must arrive as a fact. flotilla already computes
exactly this (`badge.attention` → `status.attention`), so today it costs
nothing — but some future filter will need a connector change rather than a
config change, and that will be irritating at the time.

**References are by loop name.** `of="project"` refers to the enclosing loop
called `project`, not to a positional `^`. Names reach a grandparent without
counting carets.

**Bindings are lexical within one template, and nest.** Shadowing is a config
error caught at parse — a loop whose name is already bound is rejected. Cheap to
enforce, and shadowing in a config language is never worth the confusion.

**`apply-template` resets the environment.** The applied template sees only its
own current entity, not the caller's bindings. That is what makes it reusable: a
`convoy/line` that depended on a `project` being in scope would not work in the
attention section. Inline block content keeps the enclosing bindings; handing
off drops them.

The accepted constraint: a template cannot say "render differently because I am
inside the attention section". That has to arrive as a loop-level layout
property, or as an explicitly named template at the call site. If it ever really
needs solving, the answer is passing explicit variables to templates — at which
point they are functions, and this is a language. That is a step to take
deliberately, not to drift into.

**Cycles terminate on the placement key: an entity may not appear twice in its
own ancestor chain.** That falls out of the key already defined and breaks
genuine data cycles at exactly the point they would repeat, rather than at an
arbitrary depth. A depth limit sits on top purely as a backstop against
pathological data.

**Mixed content is a union in the query.** Not solved now. The shape is one
query returning several kinds, rendered by bare `apply-template`, sorted once —
not several loops feeding one region, which would need a merge rule for no
clear gain.

## Migration

**Two pipelines, one active at a time. Never two identity schemes in one
render.** `GroupPath`-as-identity is used by the collapse snapshot, metadata
targeting and pins; the placement model replaces it. Supporting both inside a
single render means carrying two identity schemes, which is expensive and never
gets cleaned up afterwards.

So the new path is built alongside, selected by config, and the live rail keeps
the old pipeline until the new one renders the real fixture properly. Then cut
over and delete. There is precedent — the README records the Andamento rename as
a deliberate hard cut with no compatibility aliases.

**Harness snapshots run in CI from the first slice**, not added later. The new
pipeline is unused until cutover, so nothing else stops it drifting.

**This is a rapid loop, not a web of deliveries.** The native harness renders the
real pipeline in milliseconds without zellij:

```sh
cargo run -p andamento-controller -- fixture.jsonl config.kdl
```

So the work is one enabling slice — placement evaluation plus template loops,
wired to harness output — after which most of the remaining design is KDL
iteration rather than Rust. Layouts, tiers, what fits in a collapsed line, what
a section looks like: all config, all a one-second turnaround, all snapshot-able.

Only the things that genuinely need code should become tickets: the abbreviation
ladder, column reservation, collapse-as-one-line, the declaration-driven config
UI. The visual design belongs in the loop, not in a dependency graph.

## Parked

- **Ordinals and sort defaults.** The metadata store carries an `ordinal`, and
  flotilla stamps `ARCHIPELAGO_ORDINAL = -100` deliberately. Whether a
  producer's ordinal is a default the loop inherits or merely another sortable
  fact is unresolved.
- **Per-client vs per-rail resolution.** Zellij instantiates a rail per tab. For
  the embedded case the controller should compute once per client and send to
  every rail. Agreed notionally, tracked separately.
- **Absolute vs relative grid across depths.** Whether the column grid is the
  same screen columns regardless of depth, or relative to the loop's indent.
  Render both and look rather than argue.

## Scope

This replaces the grouping and region layer. It is not a rewrite, and it is not
a refactor — one load-bearing layer is swapped.

Survives untouched: the metadata store and resolver, patches and precedence,
template config with inheritance/fragments/effective-KDL dump, hit regions, the
native harness and frame snapshots.

Survives in *shape* but needs identity migration: node variables and their
provenance, the collapse/scroll snapshot, inspect addressing, and any persisted
UI state — all of them key off `NodeKey` or `GroupPath`, and placements are
neither. An earlier draft said "survives untouched" of these, which was too
strong.

Replaced: `derive_grouping_path`, presence classes, `GroupPath` as identity,
`SurfaceRegionSource`, `attention_key`, three of the five template slots, and
the hardcoded controls in `render_footer_with_variables`.

First slice: the placement/query model with the existing renderer on top.
Placement is where the doubling lives, and the doubling is what makes every
rendering decision downstream look bad.

That slice needs an **explicit adapter seam**. The existing renderer wants
`GroupPath`, group-versus-tab roles, and current `NodeKey` values; the placement
model produces none of those. The adapter is throwaway and should be named as
such, or it will quietly become the interface.

## Open

Deliberately unresolved. Recorded so they are decided rather than invented.

- **Pin**: entity state (this document) or zellij tab id (the implementation).
- **Unmatched entities**: named destination, and how it distinguishes deliberate
  hiding from an accidentally-omitted query.
- **Abbreviation collisions**: two siblings that shorten identically.
- **Tier overflow**: what happens when the declared tier still does not fit.
- **Heterogeneous templates in one loop**: whether they share a grid.

Column widths, alignment rules, what a section looks like and how dense a
collapsed line should be are *not* on this list. Those belong in the harness
loop, and writing answers into a document pre-commits to something better found
by looking.

## A standing warning

Much of what exists today was invented by prior agents rather than asked for —
the seven-level rule, `child-layout="cards"`, the four regions. The same applies
to things that look like contracts: flotilla's `SEGMENT_*` constants are a
presentation guess, not a specification of what those facts are for.

Do not treat the current state as evidence that it was wanted. Equally, do not
rewrite as if nothing is known: the list above of what survives is most of the
system.
