# The sidebar arrangement

Which sidebar sections are docked where is Dashboard state: another frontend
showing the same Dashboard needs it (Wheelhouse ADR 0012). Andamento stores it
as one document in the `dashboard` record and reconciles it with what the
template and the local sections declare, replacing the reconciliation
Wheelhouse did in C against region hints (`uishell_sidebar_dock_layout`,
Wheelhouse `docs/section-placement.md`, #191, #207, #221). It uses the same
document shape and commit protocol as a workspace's arrangement ([Slots and
arrangements](slots-and-arrangements.md)).

The Rust interface is `Sidebar::sidebar_arrangement`,
`set_sidebar_arrangement`, `restore_sidebar_section`,
`reset_sidebar_arrangement` and `sidebar_arrangement_generation`, over the
pure functions in `andamento_core::sidebar_arrangement`. The C interface is
the ABI 3 block "the Dashboard's sidebar arrangement" in `andamento.h`.

## What is stored, and what isn't

| Logical: in the `dashboard` record | Presentation: the host's |
|---|---|
| the dock's panel tree: splits, their axes and relative weights, tab panels holding section keys, each panel's selected tab | the sidebar's width and every pixel size |
| floating panels, each a panel tree | where a floating panel is and how big |
| the sections closed on purpose | whether a section is collapsed |

A tab holds a **section key**:

| Key | Section |
|---|---|
| a region's name | a template region |
| `.section:<id>` | a local section (`.section` entity `<id>`), in the place of the region whose placement has a `layout="section"` loop |
| `.unplaced` | the workspace fallback. Andamento always declares it; a host without workspace creation on its header hides it while it has no rows |
| `u:<id>` | a View of the host's own (`<id>` is 1 to 64 of `[a-z0-9_-]`). Andamento stores it and never places or flags it |

## Why a parallel call

The sidebar arrangement reuses the workspace arrangement's document types
(`ArrangementDoc`, `Panel`; `AndamentoPanel`, `AndamentoTab`) and the
acquired-arrangement getters (`andamento_arrangement_info`, `_panel`, `_tab`,
`_release`), but has calls of its own (`andamento_set_sidebar_arrangement`,
`andamento_sidebar_arrangement_acquire`) rather than a target parameter on
`set_arrangement`:

- A workspace call names a workspace ID; the Dashboard has none, and adding a
  target to the existing call would change its signature.
- The sidebar document has floating panels and a closed set, which a
  workspace's doesn't.
- The rules differ. A workspace places a new slot in the first tab panel; the
  sidebar places a section by its region's hints. A workspace commit with a
  slot tabbed twice is invalid; a sidebar commit repairs it, because the
  first commit adopts a layout the host saved before Andamento stored one,
  and those can hold duplicates (#207). A slot the commit leaves out is
  placed; a section it leaves out is closed.

## Committing

The host owns the live copy: docking previews, drop geometry and minimum
sizes need pixels. At the end of a gesture it commits the whole document with
`set_sidebar_arrangement(doc, expected_generation)`:

1. **Generation.** A commit at any generation but the stored one is **stale**
   and changes nothing (`ANDAMENTO_ARRANGEMENT_STALE`, no error).
2. **Validation.** The shape rules of an arrangement document across the dock
   and the floating panels (panel IDs nonempty and unique across both,
   weights finite and positive, splits nonempty, a selected tab one of its
   panel's), except that a key may have two tabs. Each tab names a declared
   section, a local section, a host View, or a key the stored document
   already holds (one that no longer resolves). Otherwise it is **invalid**
   and changes nothing.
3. **Duplicates.** Later tabs of a key are removed, keeping the first in
   preorder, the dock before the floating panels. A panel the removal
   empties goes; a panel that was already empty stays.
4. **Closing and restoring.** Every declared section the document leaves out
   is **closed**: the host closes a section by committing without it. A
   closed section the document tabs is **restored**. A key that no longer
   resolves and that the commit leaves out is forgotten, not closed.
5. **Storage.** The host owns the document from now on. Its generation
   changes unless nothing did.

The first commit is how a host adopts the layout it saved before: the
sections it lacks are closed, as Wheelhouse's legacy adoption did, and only
sections declared after that are new.

## Reconciling

Andamento reconciles the stored document whenever what is declared may have
changed: on configure (a new template or template version), when a local
section or group is set or removed (refs don't matter), and when a dashboard
record is imported. Reconciling never invalidates the sidebar's snapshot;
each change it makes moves the generation.

`reconcile(declared, stored)` is a pure function returning the reconciled
arrangement and a **report**: sections placed, sections restored, keys whose
duplicate tabs were removed, and keys that no longer resolve.

### Declarations

`Declared::new(regions, local_sections)` turns the template's regions (name,
`default-host`, `order`, `pinned`, and whether it hosts local sections) and
the local sections (ID, `.position`, whether it holds the default group) into
the sections, in **default order**:

- Each region is a section. The first region whose placement has a
  `layout="section"` loop is, while any local section exists, replaced by the
  local sections, by `.position` then ID, each with that region's hints.
  Without local sections it is a section itself (Wheelhouse's container
  rule).
- `.unplaced` follows the regions.
- An explicit `order` sorts lower first. An omitted one is the section's
  declaration index (so an unhinted section at index 2 sorts before `order=10`
  but after `order=1`: hint all regions or none). An unhinted workspace
  fallback (`.unplaced`, or the local section holding the default group)
  sorts after everything. Ties keep declaration order.
- Pinned regions sort before unpinned ones. This rule is new: Wheelhouse
  ignored `pinned`, which the terminal renderer reads as "always on top".
- `default-host="floating"` places a section in a floating panel. Any other
  host, or none, docks it (Wheelhouse's fallback for absent, unknown or
  rejected hosts).

### Placing

A declared section with neither a tab nor a closed record is **new**, and is
placed in a new tab panel of its host, holding and selecting it:

- In the dock, the root becomes a column split if it isn't a split (an empty
  dock gets one; a tab panel with tabs is lifted into it). The new panel goes
  among the root's children before the whole subtree holding the next section
  in default order that is docked, else last. Saved nested splits keep their
  structure, selection and weights; a new section never goes inside one.
- In the floating panels, likewise before the panel holding the next floating
  section, else last.
- A placed section arrives with weight 1 (the host sizes sections). Andamento
  marks it **placed** until the host's next commit.

Placed sections keep their place when hints change: changing a region's
`default-host` or `order` moves nothing until the user restores or resets.

### Template key drift

Saved keys embed the template's region names, so a template version that
renames or drops a region leaves keys that no longer resolve (Wheelhouse ADR
0013). They are **flagged, not dropped**: a tab keeps its place with its
`gone` flag set, a closed key stays closed, and both are reported
`UNRESOLVED`. If the key resolves again it is where it was, or still closed.
The host decides what to show for a flagged tab and drops it by committing
without it.

This replaces the #221 rule that an authoritative declaration without a
region removed its saved position and closed record. An empty declaration
now flags every section instead of clearing them.

### Restore and reset

`restore_sidebar_section(key, expected)` reopens a declared section (closed,
or never placed) by its hints, with an **equal share** of the dock: the new
panel's weight is `1/(n+1)`, and the root's other `n` panels are scaled to
share the rest, keeping their relative weights. Restoring a section that has a
tab changes nothing; an undeclared key is invalid. This is the Sections menu's
Restore.

`reset_sidebar_arrangement(expected)` is Reset To Default Panel Layout: it
drops the user's arrangement, the closed sections and every flagged key, and
places every declared section by its hints. The arrangement is no longer
owned.

## Reading it

`sidebar_arrangement()` (`andamento_sidebar_arrangement_acquire`) returns the
document, its generation, whether the host owns it, the closed set, the
placed and unresolved keys, and the last reconciliation's report. In C the
panels are a preorder array, the dock first: `andamento_arrangement_floating_first`
is the first floating panel's index (equal to the panel count for a
workspace's arrangement). A tab's `placed` and `gone` flags are as for a
workspace (`gone`: its key no longer resolves). Notes
(`andamento_arrangement_note`) list every CLOSED section and UNRESOLVED key,
and what the last reconciliation or call did: PLACED, RESTORED and DUPLICATE.
The Sections menu lists the CLOSED notes.

Poll `andamento_sidebar_arrangement_generation` to learn when to read it
again; it is cheap and builds no snapshot.

The unresolved keys are also the `Section` entries of the Dashboard's
template drift report (`dashboard_overlay`, see [Workspace
Overlay](workspace-overlay.md#the-dashboard-over-its-template)), beside its
display, collapse, order, variable and pin keys.

## The record

The `dashboard` record (version 4 and later) holds the arrangement once anything is
stored:

```kdl
andamento-record "dashboard" version=4 {
    sidebar generation=3 owned=true {
        dock {
            split "1" axis="column" weight=1.0 {
                tabs "2" weight=0.25 selected="tree" {
                    tab "tree"
                    tab "u:notes"
                }
                tabs "3" weight=0.75 selected=".section:pins" {
                    tab ".section:pins"
                }
            }
        }
        floating {
            tabs "4" weight=1.0 selected="git" {
                tab "git"
            }
        }
        closed "attention"
        placed "git"
    }
}
```

Version 3 dashboard records import with no sidebar arrangement, which
reconciling then places by the template's hints; in them, a `sidebar` node is
an unknown node and is kept as such. A stored document with duplicate tabs
imports, and reconciling repairs it. An imported arrangement keeps its
generation if reconciling changed nothing and it is newer than the stored
one; otherwise a changed arrangement moves past both, so a commit the host
prepared before the import is stale.

## Measured

`crates/andamento-ffi/examples/snapshot-profile.rs` (release build,
Wheelhouse's `daily-driver.kdl` and sidebar fixture, a shared Linux host): 20
commits of the seven-panel arrangement Andamento placed, dragging one weight,
each checking the rendered snapshot.

| Entities | Commit median / max ms | Snapshot current |
| ---: | ---: | ---: |
| 100 | 0.005 / 0.011 | 20 of 20 |
| 300 | 0.006 / 0.012 | 20 of 20 |
| 1000 | 0.006 / 0.012 | 20 of 20 |

## Not ported

These Wheelhouse rules need pixels or Wheelhouse's own config, so they stay
with the host:

- the docking checker's validity (a section can't be docked inside a child
  Workspace level, `section_hint_pending` after a safety restore);
- KDL titles refreshing View labels (the host reads labels from the
  snapshot);
- corrupt numeric weights in saved config (a document's weights are always
  finite and positive).

## Tests

`sidebar_arrangement/tests.rs` ports the scenarios of Wheelhouse's
`uishell_section_placement_diagnostics`: hints regardless of declaration
order, `.unplaced` last, reorder and idempotence, unrelated empty panels,
duplicates in the dock and floating panels, add before the next hinted
neighbour, close, closed restore with an equal share, removal, reset, a saved
nested split, legacy adoption, an empty declaration, host and order variants,
a changed host hint, closing everything, lifting a merged leaf, and unusual
keys; plus template drift, local sections and pinned regions.
`tests/sidebar_arrangement.rs` runs them through the sidebar and its record
(restart, migration, a repaired import), and `smoke.c`
(`check_sidebar_arrangement`) through the C ABI.
