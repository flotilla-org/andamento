# The Workspace Overlay

A provider suggests a workspace's Views and their arrangement (a
[Suggested Layout](suggested-layouts.md)), and the user edits it. The
**Workspace Overlay** holds the user's edits apart from the provider's
baseline, so the provider's changes keep arriving, the user's edits survive
them, and the edits could later be proposed back. This document describes how
Andamento implements Wheelhouse ADR 0013 ("A Workspace Overlay is an addressed
edit set against a cached baseline") on top of [Slots and
arrangements](slots-and-arrangements.md). The vocabulary is Wheelhouse's
`CONTEXT.md`.

The Rust interface is `Sidebar::overlay`, `set_slot`, `remove_slot`,
`reattach_slot`, `set_workspace_name`, `set_workspace_mood`,
`set_arrangement`, `resolve_arrangement` and `release_slot_previous`. The C
calls are listed in [The host interface](host-interface.md#workspaces); the
flags are `AndamentoSlot.flags` and `AndamentoWorkspaceArrangement.flags`.

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

`WorkspaceSlots` is the Rust shape: the cached baseline, `edits` by slot key
(`SlotEdit`: an override or tombstone with the content it was made against,
and a rebind policy), the user's own slots, name and mood, and the departed
slots below. It is part of the `workspace/<id>` record, beside the
arrangement, which holds the soft overrides and whether it is owned.

A workspace with no Suggested Layout uses the same model with an empty
baseline.

## Slots and flags

A slot's flags (`SlotInfo::flags`, `AndamentoSlot.flags`) say how it
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
`provider_changed` (`ANDAMENTO_ARRANGEMENT_FLAG_PROVIDER_CHANGED`) until the host calls `resolve_arrangement`
(`andamento_arrangement_resolve`) with `Keep`, keeping the user's
arrangement, or `Follow`, dropping ownership and soft overrides so the
provider's arrangement applies again. Both take an expected generation and
return STALE like a commit.

## The Dashboard over its template

The Dashboard relates to its template as a workspace relates to its
Suggested Layout. Its record's keys are per-key overrides of what the
template declares: display variables name declared variables; collapse,
sibling order and placement-variable keys embed the template's region and
loop names; and the [sidebar arrangement](sidebar-arrangement.md) names
sections. A key that no longer resolves is kept, and reported among the
sidebar arrangement's UNRESOLVED notes
([Template key drift](sidebar-arrangement.md#reading-it)); it applies again
if the template brings its names back. A loop name resolves if any placement
or template declares that binding; this does not check that the whole path
of loops still nests.

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
  kept and then followed, with generations that a stale commit can't erase,
  even across an import;
- a restart through the record;
- template key drift in the dashboard record.

`smoke.c` drives the C calls.

## Limits

- The user's name and mood are stored and recorded but not yet shown in
  the snapshot; the baseline's still come from display facts.
- Nothing exports the edits for a provider yet (Overlay Sync).
- Flags are not acknowledged separately: the host settles one by editing
  again, reattaching, releasing or resolving.
