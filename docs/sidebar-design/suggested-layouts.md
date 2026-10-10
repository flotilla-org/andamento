# Suggested Layouts

A provider can suggest more than one view for a workspace: an in-crew review
wants the agents' terminals, the pull request and the review notes side by
side. This document defines the flat facts an entity publishes to describe
that **Suggested Layout**, and how Andamento reads them. It extends
[managed primary content](managed-primary-content.md), whose facts remain valid
as the single-slot special case.

The vocabulary follows Wheelhouse's `CONTEXT.md` and its decisions on
[view specs and slots](https://github.com/flotilla-org/wheelhouse/issues/297)
and [the Workspace Overlay](https://github.com/flotilla-org/wheelhouse/issues/298)
(Wheelhouse ADR 0013). Flotilla publishes these facts in
[flotilla#3012](https://github.com/flotilla-org/flotilla/issues/3012).

`andamento_core::suggested_layout::parse` reads an entity's resolved facts into
a `SuggestedLayout`. Nothing consumes it yet: managed content, placement and
snapshots behave exactly as before.

## Shape

A Suggested Layout has three parts:

- a **baseline version**, which a Workspace Overlay records its edits against;
- **slots**, each a View's place in the workspace, with a stable **slot key**,
  a **View Spec** (content plus optional preferred presentation), a rebind
  policy and the content's current **resolution**;
- an optional **arrangement hint**: panels with stable IDs, splits with an axis,
  child order and relative weights, and tab panels holding slot keys with a
  selected tab.

Everything is a flat fact on the entity that suggests the layout, usually the
workspace subject (a convoy, role or project).

## Facts

Slot keys and panel IDs are 1 to 64 characters of `[a-z0-9_-]`, starting with a
letter or digit. They are embedded in fact keys, so they never contain `.`.
User-added slots (`u:3` in the overlay) contain `:` and so can never collide
with a provider's key. `primary` is reserved for the
[primary slot](#the-primary-slot).

### Layout

| Fact | Type | Meaning |
| --- | --- | --- |
| `layout.version` | text | Opaque baseline version, compared only for equality. Required when any `layout.*` fact is present. |
| `layout.slots` | string list | Slot keys in the provider's order, which is also the default placement order. Required. |
| `layout.root` | text | ID of the arrangement's root panel. Absent: no arrangement hint. |

### Slots

Each fact is `layout.slot.<key>.<field>`.

| Field | Type | Meaning |
| --- | --- | --- |
| `entity` | entity refs (exactly one) | Provider facet: the entity whose aspect the slot shows. |
| `facet` | text | Provider facet: which aspect, e.g. `terminal`, `review`, `checks`, `artifact`. |
| `presentation` | text | Optional preferred presentation, e.g. `terminal`, `web`, `markdown`. Open vocabulary. |
| `rebind` | text | `replace` (default), `keep-previous` or `ask`: what happens to the live instance when the resolution changes. |
| `state` | text | `ready` or `held`. Required for a provider facet; optional (default `ready`) for a local recipe. |
| `target` | text | Identity of the backing instance. Required for a ready provider facet; optional for a local recipe. |
| `kind` | text | Recipe kind: `command`, `file`, `url` or `jackstay`. |
| `argv.<n>` | text | `command`: argument `n`, contiguous from `0` ([andamento#134](https://github.com/flotilla-org/andamento/issues/134)). |
| `command` | text | `command`: shell command line, used only when no `argv.*` is present. |
| `cwd` | text | `command`: optional working directory. |
| `path` | text | `file`: path to open. |
| `url` | text | `url`: URL to open, including a Luchs page. |
| `launcher`, `endpoint` | text | `jackstay`: Jackstay launcher and stream endpoint. |

A slot with `entity` and `facet` is a **provider facet**. Its spec is the pair;
the recipe fields (`kind` and its companions) together with `state` and
`target` are its **resolution**, published by the provider, and generalise
`workspace.primary.*` and `action.primary.recipe`. A slot with neither is a
**local recipe**: the recipe fields are its spec, and the frontend resolves
it. One without the other is an error.

Unknown slot fields and unknown `layout.*` facts are ignored, so producers can
add fields ahead of readers. Unknown presentation values are carried through;
a frontend that cannot render one shows a placeholder naming the content, and
never drops the slot.

### Panels

Each fact is `layout.panel.<id>.<field>`. A panel is either a split or a tab
panel; splits and tab panels share one ID space.

| Field | Type | Meaning |
| --- | --- | --- |
| `axis` | text | Split: `row` (children left to right) or `column` (top to bottom). |
| `children` | string list | Split: child panel IDs in order. Not empty. |
| `slots` | string list | Tab panel: slot keys as tabs, in order. Not empty. |
| `selected` | text | Tab panel: the selected slot key. Default: the first. |
| `weight` | integer | Size relative to siblings in the parent split. Positive; default `1`; ignored on the root. |

The panels reachable from `layout.root` form a tree. Every panel with facts
must be reachable, appear once, and have either `axis` and `children` or
`slots`, never both. A slot appears in at most one tab panel, and a panel may
hold `primary`. A slot no panel holds is still part of the layout; frontends
place it by the overlay's default rule, as they would a new baseline slot.

## The primary slot

An entity publishing `workspace.primary.state` has a slot with the key
`primary`, read exactly as managed primary content reads it today:

| Today's fact | Slot |
| --- | --- |
| (the entity itself) | Provider facet: the entity, facet `primary` |
| `workspace.primary.state` | `state` |
| `workspace.primary.target` | `target` |
| `action.primary.recipe` | `command` (shell text), kind `command` |
| `git.root` | `cwd` |

With no `layout.*` facts, these alone are a one-slot Suggested Layout with
version `PrimaryOnly` and no arrangement hint. Alongside a published layout:

- `layout.slots` may list `primary` to fix its position; unlisted, it leads
  the slot order. Panels may hold it like any other slot.
- `layout.slot.primary.*` facts are an error: the primary slot has one source.
- Listing `primary` without `workspace.primary.state` is an error, because it
  means the publication is torn.

Producers keep publishing the primary facts unchanged. Wheelhouse's managed
content path keeps reading them until it moves to slots.

## Spec, resolution and the baseline version

The **spec** of a slot is what a Workspace Overlay compares against: its key,
its content (facet entity and name, or local recipe), its presentation and its
rebind policy. The arrangement hint is part of the baseline too. The
**resolution** of a provider facet (state, target and the recipe to run now)
is not.

The provider changes `layout.version` whenever the slot list, any slot's spec
or the arrangement changes. It need not change it for a resolution change: a
reviewer vessel restarting under the same slot is an Updating rebind of an
untouched slot, not a new baseline. The `PrimaryOnly` version never changes,
because the primary slot's spec is fixed.

## Validity, TTL and coherence

The rules match managed primary content:

- **Publish a layout coherently.** One source publishes all of an entity's
  `layout.*` facts and the primary facts in one patch, with the same TTL, and
  reasserts them together. Remove a slot's or panel's facts in the same patch
  that removes it from `layout.slots` or the arrangement.
- **A malformed spec is an error for the whole layout:** a missing version or
  slot list, a wrong value type, an invalid ID, facts for an unlisted slot, a
  bad recipe on a local slot or a malformed arrangement. A consumer treats an
  invalid layout as Unavailable: it keeps the applied workspace content and does
  not delete anything. Partial expiry from inconsistent TTLs usually shows up
  as one of these errors, which is why TTLs must match.
- **An incomplete resolution makes only that slot Unavailable.** A provider
  facet with a missing or unknown `state`, or a `ready` state without a
  `target` or a valid recipe, keeps its slot and spec. The rest of the layout
  stays usable.
- **`held` explicitly suspends replacement** for that slot; recipe fields are
  not needed.
- **Expiry is silence, not removal.** When every fact has expired the entity
  publishes no layout, and its workspace keeps what it shows, as it does today
  when the primary facts expire.

Each slot is expected to follow managed content's token protocol (plan, prepare,
validate, commit, acknowledge) with its own Unavailable, Held, Current,
Updating and Failed state. That is the next step, not part of this schema.

## Fixture: an in-crew review

[`fixtures/suggested-layout-review.jsonl`](../../fixtures/suggested-layout-review.jsonl)
is a synthetic replay of convoy `flotilla/review-147@fleet`. Every fact has a
30 s TTL and is reasserted in full at each step. At 0 ms it publishes:

```text
workspace.primary.state       = "ready"                  # the coder's terminal
workspace.primary.target      = "vessel:flotilla/review-147/coder@lab"
action.primary.recipe         = "'flotilla' attach --host 'lab' 'review-147/coder'"
git.root                      = "/srv/lab/andamento/review-147"

layout.version                = "1"
layout.slots                  = [primary, reviewer, pr, notes]

layout.slot.reviewer.entity   = vessel:flotilla/review-147/reviewer@lab
layout.slot.reviewer.facet    = "terminal"
layout.slot.reviewer.presentation = "terminal"
layout.slot.reviewer.rebind   = "keep-previous"
layout.slot.reviewer.state    = "ready"
layout.slot.reviewer.target   = "vessel:flotilla/review-147/reviewer@lab#gen-1"
layout.slot.reviewer.kind     = "command"
layout.slot.reviewer.argv.0..4 = flotilla attach --host lab review-147/reviewer
layout.slot.reviewer.cwd      = "/srv/lab/andamento/review-147"

layout.slot.pr.entity         = change_request:github/flotilla-org/andamento!147
layout.slot.pr.facet          = "review"
layout.slot.pr.presentation   = "web"
layout.slot.pr.state          = "ready"
layout.slot.pr.target         = "change_request:github/flotilla-org/andamento!147"
layout.slot.pr.kind           = "url"
layout.slot.pr.url            = "https://github.com/flotilla-org/andamento/pull/147"

layout.slot.notes.kind        = "file"               # a local recipe
layout.slot.notes.path        = "/srv/lab/andamento/review-147/.flotilla/review/notes.md"
layout.slot.notes.presentation = "markdown"

layout.root                   = "main"
layout.panel.main.axis        = "row"
layout.panel.main.children    = [agents, review]
layout.panel.agents.weight    = 3
layout.panel.agents.slots     = [primary, reviewer]
layout.panel.agents.selected  = "reviewer"
layout.panel.review.weight    = 2
layout.panel.review.axis      = "column"
layout.panel.review.children  = [pr-view, notes-view]
layout.panel.pr-view.weight   = 2
layout.panel.pr-view.slots    = [pr]
layout.panel.notes-view.slots = [notes]
```

That is, with `*` marking each panel's selected tab:

```text
┌──────────────────────────┬───────────────────┐
│ [primary] [reviewer*]    │ [pr*]             │
│                          │                   │
│                          ├───────────────────┤
│                          │ [notes*]          │
└──────────────────────────┴───────────────────┘
        weight 3                 weight 2
```

At 1000 ms the reviewer is `held` and its recipe and target are unset; the
version stays `1`. At 2000 ms version `2` adds a `checks` slot (the same change
request, facet `checks`, a URL) as a second tab in `pr-view`, and the reviewer
resolves to `#gen-2`. The facts expire together at 32 000 ms.

`crates/andamento-core/tests/suggested_layout.rs` replays it through `Sidebar`
and parses each step. It also replays `scripted-roll.jsonl` to show a standing
role's primary facts parse as a one-slot layout through ready, held and a new
generation.

## Open questions

The Wheelhouse decisions leave these open. The schema does not settle them.

- **Facet vocabulary and resolution location.** Facet names (`terminal`,
  `review`, `checks`, `artifact`) are an open vocabulary for now. Resolution
  facts live on the slot of the suggesting entity. Whether a facet should
  instead resolve from facts on the referenced entity, shared by every layout
  that shows it, is undecided.
- **Explicit provider.** The decision writes a facet as `{provider, kind, id}`.
  The provider is implied by the publishing source; there is no `provider`
  field until cross-provider facets exist.
- **Recipe kinds.** Flotilla's `attach` and `view` recipe kinds from
  andamento#134 are not slot kinds yet; an attach is published as a `command`.
  A Luchs page is a `url`. Whether either needs its own kind is open, as is
  carrying the `action.primary.direct.*` Cleat endpoint per slot.
- **Labels.** A View Spec holds only what defines the content, so there is no
  slot label. Tab titles come from the facet entity's `display.label` or the
  recipe. Whether a provider may suggest a title is open.
- **Default placement and no hint.** Without `layout.root`, or for a slot no
  panel holds, frontends need a default rule. The overlay decision gives
  examples (the provider's hint, or the end of the primary panel) but does not
  fix one.
- **Split addressing.** Splits share the panel ID space so that weight
  overrides have a key. Whether the overlay addresses splits as panels is for
  the overlay implementation.
- **Workspace name and mood.** The overlay can rename a workspace or change its
  mood, but the baseline name and mood stay in existing display facts; this
  schema adds none.
- **Several layouts per entity.** One entity suggests at most one layout. A
  second, such as a convoy's review and triage views, would need a layout name
  in the key.
