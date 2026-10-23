# The tool strip

A subject's workspace has a **tool strip**: a thin column at the edge of the
workspace with the subject, its layouts, its views, the workspaces next to it
and the items it links to. The host draws it; Andamento says what goes in the
two sections that come from the catalog rather than from the workspace
itself. This document describes those sections and how a host reads them.
The design was settled for wheelhouse#122; the strip as a whole is described
in Wheelhouse's `docs/design/subject-workspaces.md`.

Status: implemented in the core (`tool_strip.rs`, `state.rs`) and the C
interface (ABI 10).

## In short

- A kind's `workspace` declaration takes a `strip` block with two sections,
  `related` and `linked`. Each is a list of `for` loops, as in a placement.
- The loops run with the subject bound by its kind's name, so they can follow
  the subject's own references forwards (`in=… of="convoy"`) or match the
  entities that point back at it (`match … of="convoy"`).
- The host reads a subject's strip as a list of entities, each with its
  section. Everything it shows about one (label, state, hover card, actions,
  whether it has a live workspace) comes from that entity's detail.

## Declaring a strip

```kdl
workspace kind="convoy" {
  strip {
    related {
      for "project" kind="project" in="flotilla.project" of="convoy";
      for "vessel" kind="vessel" { match "flotilla.convoy" of="convoy"; order "display.label"; };
    };
    linked {
      for "change_request" kind="change_request" { match "flotilla.subject_of" of="convoy"; order "flotilla.subject.number" natural=true; };
      for "issue" kind="issue" { match "flotilla.subject_of" of="convoy"; };
    };
  }
}
```

- **related** lists the workspaces next to the subject's: by convention its
  ancestors first, then its children. Siblings are one hop away, through the
  parent, and aren't listed.
- **linked** lists what the subject links to: change requests, issues,
  artifacts.
- A loop is a [placement loop](placement-model.md) that only selects and
  orders: `kind=`, `match`, `in=`/`of=`, `order` and `visibility`. It nests no
  loops and takes no `field`, `apply-template`, `layout`, `default-collapsed`
  or `tier`, since a strip would ignore them.
- The subject is bound by its kind's name (`convoy` above), so a loop can't
  use that name for itself.
- A section lists its loops' matches in loop order, each loop in its own
  order. An entity two loops match, in one section or both, is listed once,
  where it first appears.

A strip is part of the kind's template, like its slots and layouts: the
highest-ranked layer whose declaration has children supplies all three, so a
layer that declares only a strip replaces the lower layers' slots and layouts
too.

## What a strip lists

The loops match by the sidebar's rules: a visibility policy applies, ended
subjects are left out while Show finished is off (unless retained for an open
workspace), a join on a provider's entity stays within that provider, and the
subject itself is never listed.

The strip follows the catalog. Read it again when the snapshot's revision
moves.

An ended subject whose workspace is still open keeps its strip: the facts its
loops read are among those its workspace's record keeps, so the strip is the
same after a restart.

## Host interface

- `andamento_strip_get(h, subject)` returns an `AndamentoStrip`: an array of
  `AndamentoStripEntry`, each with its section (`ANDAMENTO_STRIP_RELATED` or
  `_LINKED`) and entity. Free it with `andamento_strip_free`. A subject not in
  the catalog, or a kind without a strip, gives an empty list.
- For an entry's label, state and hover card, request its detail
  (`andamento_snapshot_detail_request`). Its primary action is the detail's:
  open its URL, focus its workspace or open one.

The other sections of the strip come from the workspace:

- **Layouts** are `andamento_layouts_get`. A layout may declare a `glyph`,
  1 to 8 characters, for where there is only room for a symbol.
- **Views** are the workspace's slots (`andamento_slots_get`), placed or not.
  Each unplaced slot's UNPLACED note names the tab panel showing it puts it
  in (`AndamentoNote.panel`): the first whose template matcher takes it,
  ignoring `hide`, else the first tab panel.

## Later

- **Repeatable views**, called view factories: a template slot that opens a
  new View each time, such as another shell.
- **A layout's shape as its icon**: a miniature of its panels in their
  proportions, which needs each layout's panel tree, not only the current
  one's.
