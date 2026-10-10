# The Workspace Overlay

A provider suggests a workspace's Views and their arrangement (a
[Suggested Layout](suggested-layouts.md)), and the user edits it. The
**Workspace Overlay** holds the user's edits apart from the provider's
baseline, so the provider's changes keep arriving, the user's edits survive
them, and the edits can be proposed back later. This document describes how
Andamento implements Wheelhouse ADR 0013 ("A Workspace Overlay is an addressed
edit set against a cached baseline") on top of [Slots and
arrangements](slots-and-arrangements.md). The vocabulary is Wheelhouse's
`CONTEXT.md`.

The Rust interface is `Sidebar::overlay`, `export_overlay`, `set_slot`,
`remove_slot`, `reattach_slot`, `set_workspace_name`, `set_workspace_mood`,
`set_arrangement`, `resolve_arrangement`, `release_slot_previous` and
`dashboard_overlay`. The C interface is the ABI 3 blocks "Workspace Overlay"
and "the Dashboard over its template" in `andamento.h`, plus
`andamento_slots_flags` and `andamento_arrangement_flags`.

## The edit set

The overlay is an **addressed edit set**: each edit is keyed by a slot key or
a panel ID. It is stored with the cached copy of the Suggested Layout it was
made against, and that copy's version. It is a normalised set, not a history:
an edit the baseline comes to equal drops out, and history comes from version
control of the saved records.

| Edit | Key | What it records |
|---|---|---|
| Add a slot | `u:<id>` | The slot's View Spec and rebind policy. User keys have their own namespace; provider keys never contain `:`. |
| Override a slot's content | provider slot key | The user's View Spec, and the baseline content it was made against. The slot is **detached**: it no longer follows the provider. |
| Remove a slot (tombstone) | provider slot key | The baseline content it was removed against. |
| Change the rebind policy | provider slot key | The policy. It doesn't detach the slot. |
| Rename, change mood | the workspace | The user's name or mood over the provider's. |
| Split weight, selected tab | panel ID | A **soft override**. |
| The whole arrangement | the workspace | Once a **structural** edit has taken ownership. |

`EditSet` is the Rust shape. In the `workspace/<id>` record (version 5 and later) the
edits are an `overlay` node, and an owned arrangement is the `arrangement`
node with `owned=true`:

```kdl
andamento-record "workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c" version=5 {
    subject "vessel" "v" provider="sub-1"
    baseline version="2" {
        slot "a" { shell "top"; }
        slot "b" rebind="keep-previous" { shell "make"; }
        slot "c" rebind="ask" { shell "tail -f log"; }
        hint { split "main" axis="row" { tabs "left" selected="a" { tab "a"; tab "b"; }; tabs "right" selected="c" { tab "c"; }; }; }
    }
    overlay {
        name "Build"
        mood "calm"
        edit "a" rebind="ask"
        edit "b" {
            override { shell "make check"; }
            against { shell "make"; }
        }
        edit "c" {
            tombstone
            against { shell "tail -f log"; }
        }
        slot "u:1" { shell "lazygit"; }
        panel "left" weight=3.0 selected="b"
    }
    arrangement generation=7 owned=false { split "main" axis="row" { ... }; }
    departed "d" rebind="keep-previous"
}
```

A workspace with no Suggested Layout uses the same model with an empty
baseline.

## Slots and flags

A slot's flags (`SlotInfo::flags`, `andamento_slots_flags`) say how it
relates to the baseline:

- `DETACHED`: its content is overridden. Reattaching drops the override. This
  is a flag beside the slot's resolution state, not another state.
- `CHANGED`: the provider's content for its key is not what the user's
  override or tombstone was made against. An override still wins. A
  tombstone no longer hides the slot, so a key the provider reuses for
  different content is shown rather than hidden. Setting the slot again,
  removing it again, or reattaching it records the edit against the new
  content and settles the flag.
- `REMOVED`: the provider removed this overridden slot. It is kept as the
  user's own, with its policy frozen, until the host removes it.
- `REBIND`: the user changed its rebind policy.

`remove_slot` on a provider slot tombstones it: the slot is hidden and its
live instance forgotten, and the overlay lists it as `tombstoned`
(`ANDAMENTO_OVERLAY_TOMBSTONED`). `reattach_slot` drops a tombstone as it
drops an override.

## When the baseline changes

Nothing is dropped silently:

| Change | Result |
|---|---|
| New baseline slot | It appears, placed by the default rule: by the hint while the user doesn't own the arrangement, else at the end of the first tab panel, marked `placed`. |
| Untouched slot removed | It goes. A `departed` entry (`ANDAMENTO_OVERLAY_DEPARTED`) gives its rebind policy, which its live instance follows: close it (`replace`), keep it reachable (`keep-previous`) or ask (`ask`). The host releases it with `release_slot_previous` (`andamento_slot_release_previous`). |
| Overridden slot removed | It is flagged `REMOVED` and kept as the user's own slot. |
| Untouched slot's content changes | It follows the change: its next plan is Updating. |
| Overridden slot's content changes | The override wins; the slot is flagged `CHANGED`. |

A tombstoned slot the baseline drops loses its tombstone, which has nothing
left to hide. An override or policy the new baseline equals drops out of the
edit set: the provider has accepted it.

## The arrangement: soft and structural edits

`set_arrangement` stays a whole-document commit. Andamento tells soft edits
from structural ones by diffing the committed document against the stored
one, the document the host read at that generation:

- The **shape** of a document is its panel tree with weights and selections
  cleared: panel IDs, split axes, and each tab panel's tabs in order.
- While the user doesn't own the arrangement, a commit with the same shape
  is **soft**. Each panel's weight and selection is compared with the
  provider's document (the hint, less tombstoned slots, every slot placed),
  and each that differs becomes a soft override keyed by panel ID; one that
  matches drops out. The arrangement stays unowned.
- Any other commit is **structural** (a split, move, close, reorder or added
  tab). The overlay owns the whole document from then on, and its soft
  overrides fold into it.

An unowned arrangement is derived again whenever the baseline or the
visible slot set changes: the provider's hint, less tombstoned slots, every
slot placed, then soft overrides reapplied where their panel (and selected
tab) still exists. So a divider resize or a tab switch leaves the
provider's structure flowing. A soft override whose panel has gone is kept
and reported `unresolved` (`ANDAMENTO_ARRANGEMENT_FLAG_UNRESOLVED`, and an
UNRESOLVED note naming the panel).

An owned arrangement only has new slots placed. A provider change to its
hint is neither applied nor ignored: the arrangement is flagged
`provider_changed` (`ANDAMENTO_ARRANGEMENT_FLAG_PROVIDER_CHANGED`, stored as
`provider-changed=true`) until the host calls `resolve_arrangement`
(`andamento_arrangement_resolve`) with `Keep`, keeping the user's
arrangement, or `Follow`, dropping ownership and soft overrides so the
provider's arrangement applies again. Both take an expected generation and
return STALE like a commit.

## The proposal

`export_overlay` (`andamento_overlay_export`) writes the edit set in the
**Overlay Sync** proposal shape, the edits and the baseline version they
apply to. It is read-only, and no sync protocol reads it yet:

```kdl
overlay-proposal workspace="01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c" baseline="2" {
    subject "vessel" "v" provider="sub-1"
    name "Build"
    edit "b" {
        override { shell "make check"; }
        against { shell "make"; }
    }
    panel "left" weight=3.0
}
```

`baseline` is the layout's `layout.version`; a baseline of only the primary
facts is `primary-only=true`, and no baseline has neither. An owned
arrangement is an `arrangement { ... }` child. Departed slots and flags are
not part of it: they are the receiver's view, not the user's edits.
`records::decode_proposal` reads it back. Once a provider accepts edits,
publishing them in its baseline, they drop out of the next proposal.

## The Dashboard over its template

The Dashboard relates to its template as a workspace relates to its
Suggested Layout. Its record's keys are per-key overrides of what the
template declares: display variables name declared variables; collapse,
sibling order and placement-variable keys embed the template's region and
loop names; and the [sidebar arrangement](sidebar-arrangement.md) names
sections. The `dashboard` record (version 5) records the template version it
was made against, `template version="<digest>"`: a 64-bit FNV-1a digest of
the configuration text.

`dashboard_overlay()` (`andamento_dashboard_overlay_acquire`) returns the
configured template version, the one the last imported record named, and
every key that no longer resolves: `Display`, `Collapse`, `Order`,
`Variable`, and `Section`, which is the sidebar arrangement's own
unresolved set, not a second computation. Unresolved keys are kept in the
record and resolve again if the template brings their names back.

A loop name resolves if any placement or template declares that binding;
this does not check that the whole path of loops still nests.

## Pins to Views

A local ref can pin a View: its `.view` fact is
`"<workspace-id>/<slot-key>"`, checked when the ref is set. When the View
goes away (its workspace is forgotten, or its slot removed) the pin is kept
and listed as `Pin` with the ref's ID. Andamento never removes it; the host
offers removal with `remove_local` (`andamento_local_remove`). It resolves
again if the slot comes back. A closed workspace keeps its record and slots,
so its pins still resolve.

This is the minimal version: the snapshot doesn't present the pinned View
yet, and activating a pin is the host's.

## Tests

`tests/overlay.rs` replays Suggested Layout versions against an edited
workspace:

- each row of the baseline-change table;
- a reused provider key: a tombstone kept across an unchanged version, the
  key reused for other content (shown, flagged), and an override of a key
  the provider dropped and then reused;
- a divider resize and tab switch that leave the provider's restructuring
  flowing, and a soft override whose panel goes;
- a structural edit taking ownership, then a flagged provider change,
  kept and then followed;
- the proposal export, accepted edits dropping out, and a restart through
  the record;
- template key drift in the dashboard record;
- a pin to a View that goes away.

`records.rs` covers the version 5 record, the migration of version 3 and 4
overrides into the overlay, and the proposal round trip. `smoke.c`
(`check_overlay`) drives the C calls.

## Limits

- The user's name and mood are stored and exported but not yet shown in
  the snapshot; the baseline's still come from display facts.
- A pin is flagged, not presented: the snapshot has no row for a pinned View.
- Flags are not acknowledged separately: the host settles one by editing
  again, reattaching, releasing or resolving.
