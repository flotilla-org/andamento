# Slots and arrangements

A workspace shows several Views, and Andamento owns what they are and how they
are arranged (Wheelhouse ADR 0012). This document describes a workspace's
**Slots**, how each slot's content is planned, and the **arrangement
document** a host commits whole at the end of a docking gesture. It
generalises [managed primary content](managed-primary-content.md), which is now
the `primary` slot, and consumes the [Suggested Layout](suggested-layouts.md)
facts. The vocabulary is Wheelhouse's `CONTEXT.md`; the overlay rules are
Wheelhouse ADR 0013.

The Rust interface is `Sidebar::slots`, `set_slot`, `remove_slot`,
`reattach_slot`, `plan_slot`, `set_arrangement`, `arrangement` and
`content_revision`, with the token calls on `Sidebar::managed`. The C
interface is the ABI 3 block "Slots and arrangement documents" in
`andamento.h`.

## Slots

A **Slot** is a View's place in a workspace, identified by a slot key. It has a
**View Spec** (the content, and optionally a preferred presentation), a
**rebind policy**, a **detached** flag, and the progress of resolving its
content. Slots belong to registered workspaces.

A workspace's slots come from two places:

- **The baseline**: the slots of its subject's Suggested Layout. Andamento
  keeps a cached copy of the last valid layout, with its version and
  arrangement hint, in the workspace record. A malformed layout, or one whose
  facts expired, keeps the cached baseline: expiry is silence, not removal. A
  new valid layout replaces it. An entity that publishes only
  `workspace.primary.*` has a one-slot baseline, `primary`.
- **The host**: the user's own slots, with keys in their own namespace,
  `u:<id>` (`<id>` is 1 to 64 of `[a-z0-9_-]`). Provider keys never contain
  `:`, so the two never collide. The host chooses the ID; Andamento never
  generates one.

Setting a baseline slot's key **overrides** it: the slot shows the override's
spec and policy and is **detached**, so it no longer follows the provider.
The override records the baseline spec it was made against. Setting the
baseline's own spec and policy, or reattaching, drops the override. A
detached slot the baseline later drops is kept as the user's own (reported
with `in_baseline` false) until the host removes it. A baseline slot can't be
removed by the host: the provider removes it.

Slots are listed, and placed by default, in this order: the baseline's, then
detached slots the baseline dropped, then the user's in the order they were
added.

## Resolving a slot

Every slot follows managed primary content's protocol. The host plans a slot
with the identity of the resolution it applied (empty for none); Andamento
returns Unavailable, Held, Current, Updating or Failed, and, while Updating, a
token and the resolution to apply. The host prepares the new runtime instance
without touching the current one, validates the token immediately before
committing, commits, and completes. A Failed slot isn't retried until its
resolution changes or the host retries it. Repeated plans return the same
token until the resolution or binding changes.

A resolution is a recipe (command, file, URL or Jackstay launcher) and,
optionally, the target naming the backing instance. Its **identity**
(`Resolved::id`, `AndamentoSlotContent.resolution`) is opaque text: the host
records it once applied and passes it back to plan, which reports Current when
it matches. The `primary` slot's ABI 2 calls compute it from the target,
command and cwd they already pass, so the two kinds of call share one binding
and its tokens.

Where a slot's resolution comes from:

| Slot | Resolution |
|---|---|
| A baseline slot that follows its provider | The subject's layout: `layout.slot.<key>.*`, or `workspace.primary.*` for `primary` |
| A local recipe (user or override) | The recipe itself; the frontend resolves it |
| A provider facet (user or override) | The facet entity's own layout slot named by the facet; `primary` reads its `workspace.primary.*` facts |

The last row is an interim rule: where a facet's resolution lives is an open
question of the Suggested Layout schema.

### Rebind policy

When a slot's resolution changes, its policy decides what happens to the
runtime instance being replaced. The plan reports the policy; the host decides
how to show the change.

- `replace` (the default): the old instance goes when the new one commits.
- `keep-previous`: once the new instance commits, the plan reports the old
  resolution's identity as `previous` until the host closes that instance
  and calls `release_previous`.
- `ask`: the host asks the user before committing. Declining is completing
  the update unsuccessfully: the slot is Failed and stays on its current
  content until the resolution changes again or the host retries.

## Arrangement documents

A workspace's **arrangement** is one typed document: a tree of panels with
stable IDs. A split panel lays its children out along an axis (`row`: left to
right; `column`: top to bottom) with relative weights; a leaf is a tab panel
holding slot keys as tabs and its Selected View. This is the shape of
Wheelhouse's `RD_Arrangement`, with an explicit axis on each split rather than
alternating ones, and the Suggested Layout's arrangement hint converts to it.

Wheelhouse owns the live copy: docking previews, drop geometry and minimum
sizes need pixels. When a gesture ends it commits the whole document with
`set_arrangement(workspace, doc, expected_generation)`:

1. **Generation.** If `expected_generation` isn't the stored generation (0
   before anything is stored), the commit is **stale** and changes nothing;
   the host reads the arrangement again. The C call returns
   `ANDAMENTO_ARRANGEMENT_STALE` with no error.
2. **Validation.** Panel IDs are nonempty and unique, weights finite and
   positive, splits nonempty, each slot has at most one tab, and a selected
   tab is one of its panel's. Each tab names a current slot, or a slot the
   stored document already tabbed that has since gone; a host can't give a
   tab to a slot it never added. An invalid document changes nothing.
3. **Reconciliation.** Each slot with no tab is appended to the first tab
   panel in preorder, and selected there if that panel selects nothing. With
   no panels, a tab panel is made for them. A tab whose slot has gone is kept
   and reported gone; nothing is dropped silently.
4. **Storage.** The document becomes the stored one and the host owns it.
   Its generation changes unless the stored document is unchanged.

Reading the arrangement returns the document, its generation, whether the
host owns it, and two tab flags: **placed** (Andamento placed it since the
host last committed) and **gone** (its slot has gone).

Andamento changes the document itself only when the slot set changes from the
provider's side:

- Until the host first commits, the document is the baseline's arrangement
  hint (or empty), reconciled. A new hint replaces it.
- Once the host owns it, a baseline change only reconciles it: new baseline
  slots are placed, and dropped ones are reported gone.

Either changes the generation, so a commit the host prepared against the old
one is stale. A slot the host adds with `set_slot` does not change the
arrangement: the host gives it a tab in its next commit, or that commit
places it.

## Revisions per concern

Before this change one counter, `Sidebar::revision`, invalidated the
snapshot, the shared revision evaluation and the detail cache on every
mutation. Wheelhouse commits an arrangement at every gesture end, and
evaluation costs 8 to 90 ms depending on entity count, so arrangements must
not share it.

Slots, their resolutions and arrangements now have their own revision,
`content_revision` (`andamento_workspace_content_revision`). Slot edits,
arrangement commits and baseline changes bump it and never call
`invalidate()`, so a rendered snapshot stays current
(`andamento_snapshot_is_current`) and its actions stay dispatchable. Fact
changes still invalidate the sidebar as before, and bump the content revision
too when they change a slot resolution. Records change with both: export a
record when its generation changes.

Measured with `crates/andamento-ffi/examples/snapshot-profile.rs` (release
build, Wheelhouse's `daily-driver.kdl` and sidebar fixture, a shared Linux
host): 20 arrangement commits of a two-panel, eight-slot document per size,
each checking the rendered snapshot.

| Entities | Nodes | Commit median / max ms | Snapshot current | Evaluations | Plain acquisition after a fact change, median ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 119 | 0.004 / 0.013 | 20 of 20 | 0 | 8.2 |
| 300 | 319 | 0.004 / 0.014 | 20 of 20 | 0 | 23.6 |
| 1000 | 1019 | 0.004 / 0.016 | 20 of 20 | 0 | 87.7 |

The last column is what each commit would have cost had it invalidated the
sidebar: the snapshot rebuild after a one-fact update in the same run.
`tests/slots.rs` (`arrangement_commits_and_slot_edits_leave_the_snapshot_current`)
and `smoke.c` (`check_slots`) assert the snapshot stays current.

## Records

The `workspace/<id>` record (version 3, unchanged in version 4) holds the slots and the arrangement:

```kdl
andamento-record "workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c" version=4 {
    subject "convoy" "c" provider="sub-1"
    baseline version="1" {
        slot "primary" {
            facet "primary" "convoy" "c" provider="sub-1"
        }
        slot "reviewer" rebind="keep-previous" presentation="terminal" {
            facet "terminal" "vessel" "reviewer" provider="sub-1"
        }
        hint {
            tabs "agents" weight=1.0 selected="reviewer" {
                tab "primary"
                tab "reviewer"
            }
        }
    }
    override "reviewer" {
        argv "htop" "-d" cwd="/srv"
        against presentation="terminal" {
            facet "terminal" "vessel" "reviewer" provider="sub-1"
        }
    }
    slot "u:1" presentation="web" {
        url "https://example.com"
    }
    arrangement generation=4 owned=true {
        split "main" axis="row" weight=1.0 {
            tabs "1" weight=0.6 selected="reviewer" {
                tab "primary"
                tab "reviewer"
            }
            tabs "2" weight=0.4 {
                tab "u:1"
            }
        }
        placed "u:1"
    }
}
```

A View Spec's content is one child: `facet "<facet>" "<kind>" "<id>"
provider=".."`, `shell "<command>"` or `argv "<arg>" ...` (each with an
optional `cwd`), `file "<path>"`, `url "<url>"` or `jackstay "<launcher>"
"<endpoint>"`. A baseline is `primary-only=true` instead of a `version` when
only the primary facts define it. Version 2 records import with no slots and
no arrangement; in them, nodes named like the new ones are unknown nodes and
kept as such.

## Limits and next steps

- **Overlay rules not yet built** (ADR 0013): tombstones for removed baseline
  slots, flagging a key the provider reuses for different content, soft
  overrides (split weights and the selected tab that don't take ownership),
  ownership only on a *structural* edit, and flagging a provider change to an
  owned arrangement. Today any host commit takes ownership.
- **Target Resolutions** (`session`, `daemon_name`, `attach_token`) are not
  stored; Wheelhouse moves them in its step 7b.
- **The Dashboard's sidebar arrangement** uses the same document shape, with
  floating panels, a closed set and hint-based placement: see [The sidebar
  arrangement](sidebar-arrangement.md).
- Closed workspaces keep the slots and arrangement they had; baselines follow
  only open workspaces' subjects.
