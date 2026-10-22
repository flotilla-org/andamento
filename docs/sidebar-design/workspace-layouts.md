# Workspace Layouts

A subject's workspace has one set of Slots and can show them in several ways:
a convoy's agents as tabs beside an overview of its vessels, or only the agents
still running. A **Workspace Layout** is one named arrangement of those Slots.
This document describes how Andamento defines layouts as templates in the
sidebar configuration, chooses one for a workspace, and keeps the user's edits
to each. It builds on [Slots and arrangements](slots-and-arrangements.md), the
[Workspace Overlay](workspace-overlay.md) and the
[Suggested Layout](suggested-layouts.md) facts. The design was settled for
wheelhouse#416 (mission wheelhouse#401); the reasons are in Wheelhouse's
`docs/design/subject-workspaces.md`.

Status: design. Nothing here is implemented yet.

## In short

- **Flotilla publishes what exists**: Slots, each with a **role**, and hints.
  **Andamento holds the layouts** as templates keyed by subject kind. Changing
  a layout needs no Flotilla release.
- A workspace has **one set of Slots**. A layout only arranges them, so
  switching layouts moves the existing Views and live terminals carry over.
- A layout may leave Slots out. They are **unplaced**: still in the workspace
  and still resolving, but not shown.
- Slot edits are kept once per workspace; arrangement edits are kept per
  layout.

## Where layouts live

Layouts are children of the `workspace kind="…"` declaration in the sidebar
configuration (`template_config.rs`):

```kdl
workspace kind="vessel" {
  slot "files" role="files" kind="file" path="{git.root}"

  layout "agent-and-files" {
    split axis="row" {
      tabs id="agents" weight=3 { slots role="agent"; }
      tabs id="files" weight=2 { slots role="files"; }
    }
  }
}
```

A `workspace` without children declares an openable subject kind with no
template, as today. Layout names and panel IDs are 1 to 64 of `[a-z0-9_-]`,
starting with a letter or digit, and are unique within their `workspace`
(panel IDs within their layout). Across configuration layers, a kind is
openable if any layer declares it, as today. Its slots and layouts come whole
from the highest-ranked layer whose declaration has children, as a placement
comes from the highest-ranked layer that names it.

## Roles

A Slot's **role** says what it is for, so a template can arrange it without
knowing who published it. It is the Suggested Layout field
`layout.slot.<key>.role` (text, open vocabulary). A template slot declares its
own. The starting vocabulary:

| Role | Meaning |
|---|---|
| `agent` | An agent's view; for now its crew's terminal. |
| `shell` | A terminal for the user in the subject's environment. |
| `files` | The subject's files. |
| `tui` | A scoped Flotilla TUI. |
| `overview` | A summary of the subject's members, such as a convoy's vessels. |
| `review`, `checks`, `artifact` | A change request's review, its checks, an artifact. |

Unknown roles are carried through and can be matched like any other value.

Until Flotilla publishes roles, a Slot without one gets a default:

- The `primary` slot's role follows the subject kind: `agent` for a vessel or
  convoy, `tui` for a project or role.
- Otherwise it is empty, and only matchers on facet, presentation or entity
  kind, or `rest`, can place it.

## Template slots

A template can declare Slots of its own with `slot`, for views built from the
subject's facts that no provider needs to publish:

```kdl
slot "files" role="files" kind="file" path="{git.root}"
slot "shell" role="shell" kind="command" cwd="{git.root}"
```

- Their keys are `t:<name>`, a third namespace beside provider keys (no `:`)
  and the user's (`u:<id>`).
- They are part of the **baseline**: the overlay treats them like provider
  Slots, so the user can remove, override or reattach them.
- Their fields are the local recipe fields of a Suggested Layout slot (`kind`,
  `argv.<n>`, `command`, `cwd`, `path`, `url`, `launcher`, `endpoint`) plus
  `role` and `presentation`. A value may name a subject fact in braces,
  `{git.root}`, which is replaced by the fact's text.
- A slot whose fact is missing is **Unavailable**, not absent: the workspace
  still opens and the Slot fills in when the fact arrives.
- A template slot with the same name in several layouts is one Slot.

## Panels and matchers

A layout is one panel tree, the shape of an [arrangement
document](slots-and-arrangements.md#arrangement-documents):

- `split axis="row"|"column" { … }`: children in order.
- `tabs { … }`: a tab panel. It holds the Slots its matchers accept.
- Both take an optional `id` and `weight` (positive integer, default 1). A
  panel without an `id` gets one from its position; give panels the user is
  likely to keep an `id`, because the overlay keys edits by it.
- `tabs` takes `selected="first"|"last"` (default `first`).

A **matcher** is a `slots` node inside `tabs`:

```kdl
slots role="agent"
slots role="agent" entity-kind="vessel" { match "flotilla.role" exists=true; }
slots rest=true
```

- Its attributes name Slot fields: `role`, `facet`, `presentation`,
  `entity-kind` (the kind of the Slot's facet entity) and `key`. All given
  attributes must match.
- Its `match` children use the visibility rules' syntax and read the facts of
  the Slot's facet entity, or the subject's for a Slot with none.
- `rest=true` accepts every Slot no earlier matcher took. A layout without one
  leaves those Slots unplaced.
- A `tabs` panel may hold several matchers; their Slots are tabbed in matcher
  order, and within one matcher in the Slots' order (the provider's order,
  which is a convoy's plan order).

A `hide` block at the top of a layout holds matchers whose Slots are unplaced
in that layout:

```kdl
layout "live-only" {
  hide { slots role="agent" { match "flotilla.convoy.phase" value="landed"; }; }
  split axis="row" {
    tabs id="agents" weight=3 { slots role="agent"; }
    tabs id="overview" weight=2 { slots role="overview"; }
  }
}
```

**Evaluation is first match.** Each Slot is offered to `hide`, then to the
matchers in template order (depth first), and goes to the first that accepts
it. There is no `not`; exclusions come first, as in the visibility rules.

A layout must contain at least one `tabs` panel. A matcher-less `tabs` panel
is allowed: it starts empty and holds what the user puts there.

## Choosing a layout

A `layout` may have a `when` condition over the subject's facts, in the
visibility rules' syntax:

```kdl
layout "review" { when { match "layout.hint" value="review"; }; … }
layout "agents-and-overview" { … }
```

- The workspace's **default layout** is the first whose `when` matches; a
  layout with no `when` always matches.
- The user's choice is kept per workspace in its record, so it survives
  restarts and follows the workspace to another device. A workspace whose
  layout the user never chose follows the default as the subject's hints
  change.
- A chosen layout that a configuration change removes falls back to the
  default; the record keeps the name, so restoring the layout restores the
  choice.
- With no template for the subject kind, or no layout that matches, the
  arrangement comes from the Suggested Layout's arrangement hint, and without
  one it is today's single tab panel.

## The unplaced set

An arrangement document gains an **unplaced** list of Slot keys: Slots in the
workspace that the layout does not show. They keep resolving like any other
Slot; only their Views are not built.

- A template layout's unplaced Slots are the ones `hide` took and the ones no
  matcher took.
- A host places one by committing an arrangement that gives it a tab; that is
  an arrangement edit of the current layout. It unplaces one by committing an
  arrangement that lists it as unplaced.
- **Reconciliation changes for template layouts only.** A Slot without a tab
  is unplaced, rather than appended to the first tab panel. An arrangement
  from a hint, or the single-panel default, keeps today's rule.
- Reading the arrangement reports the unplaced list beside the tabs.

## Edits

The [Workspace Overlay](workspace-overlay.md) keeps two kinds of edit, and
layouts split them:

- **Slot edits** (add, override, remove, reattach, rebind policy, name and
  mood) belong to the workspace. They apply in every layout.
- **Arrangement edits** (soft overrides of weights and selected tabs, and
  ownership after a structural change) belong to one layout. Rearranging
  "agents-and-overview" leaves "live-only" as its template makes it.

A layout the user hasn't edited is re-evaluated whenever the Slot set or the
facts its matchers read change.

A layout the user has edited structurally is owned by the overlay, as an
arrangement is today. Its panels keep their IDs and **their matchers**:

- A new Slot goes to the first surviving panel whose matcher accepts it, as if
  the template were evaluated over the user's panels. If the user deleted or
  merged that panel away, the Slot is unplaced; it is never put somewhere
  arbitrary.
- A Slot the user moved stays where the user put it, matched or not.
- A panel the user added has no matchers and holds only what the user puts
  there.
- A Slot the baseline drops is reported gone, as today.
- A configuration change to the template is flagged, not applied, like a
  changed hint (`AndamentoWorkspaceArrangement.flags`). Resetting the layout
  drops its arrangement edits.

## Host interface

The calls extend [the workspace calls](host-interface.md#workspaces); names
are provisional and the ABI version changes:

- **List** a workspace's layouts: name, whether it is the default, whether it
  is current.
- **Choose** a layout for a workspace, or clear the choice to follow the
  default.
- **Read** the arrangement as today, now with the unplaced list; it is the
  current layout's.
- **Commit** an arrangement as today; it is recorded against the current
  layout.
- **Reset** the current layout's arrangement edits.

Choosing a layout bumps the content revision and the arrangement generation,
so a commit prepared against the old layout is stale.

Until Wheelhouse's tool strip exists (wheelhouse#122), it switches layouts
from a Layout submenu in the workspace's menus and from palette commands, shows
no submenu for a subject with one layout, and lists unplaced Slots as "Show …"
in the tab strip's `+` menu.

## Starting layouts

These go in Wheelhouse's `data/sidebar/daily-driver.kdl`:

- **Vessel**: the agents as tabs on the left, the vessel's files on the right.
  A shell is a template slot, unplaced by default.
- **Convoy**: a tab per vessel's agent on the left in plan order, the overview
  on the right; and "live-only", which hides ended agents.
- **Project**: the scoped TUI as the main panel, with files and a shell in the
  local checkout beside it; governors as tabs on the left once the agent view
  exists.

The convoy overview needs the shared table and markdown components; until they
exist its Slot shows its placeholder.

## Open points

- **Which fact says an agent has ended**, for "live-only": Flotilla's to
  publish (wheelhouse#414).
- **Roles from Flotilla**: `layout.slot.<key>.role` is added to the Suggested
  Layout facts here; Flotilla publishes it with flotilla#3012's rework.
- **A display toggle instead of a layout** for variants like "live-only", if
  switching layouts proves heavy.
