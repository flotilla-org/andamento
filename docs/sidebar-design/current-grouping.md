# Grouping As Built

A factual description of what the grouping configuration actually does today.
No proposals. Written because the shipped behaviour had drifted from anything
written down, and successive changes were being designed against guesses.

Sources: `crates/andamento-shared/src/grouping_config.rs` (types + bundled
defaults), `crates/andamento-controller/src/state.rs` (rule selection and path
derivation), `templates/flotilla-default.kdl` (the shipped KDL rules).

## Three rules, tried in priority order

`GroupingConfigCatalog::from_rules` sorts by descending priority:

| rule | priority | defined in |
|---|---|---|
| `flotilla.default` | −1000 | bundled + `templates/flotilla-default.kdl` |
| `andamento.git` | −2000 | bundled + `templates/andamento-git.kdl` |
| `zellij.directory` | −3000 | bundled |

`with_bundled_defaults` appends a bundled rule only when no rule of the same
name is already present, so a KDL rule shadows the bundled one of that name.
The two definitions of `flotilla.default` currently agree, so this does not
bite — but they are two places that must be kept in sync.

**Selection is per entity** (`state.rs:1699`). If `active_grouping_template` is
set, only that named rule is considered. Otherwise the first rule in priority
order whose `filter` matches *and* which yields a non-empty path wins.

`flotilla.default`'s filter is `key="entity.kind" exists=true`, and
`entity_facts` injects `entity.kind` for every entity unconditionally
(`state.rs:2808`). So **`flotilla.default` matches every entity**, and the git
and directory rules never apply to entity-derived nodes. They still apply to
tab/pane-derived nodes, which do not go through this path.

## `flotilla.default` levels

All seven are `optional=true`.

| # | key | label-key | collapse-single-member | show-empty |
|---|---|---|---|---|
| 1 | `flotilla.project` | `flotilla.project.name` | | yes |
| 2 | `vcs.repo` | `vcs.repo.name` | | |
| 3 | `flotilla.convoy` | `flotilla.convoy.name` | **yes** | |
| 4 | `flotilla.vessel` | `flotilla.vessel.name` | | |
| 5 | `flotilla.session` | `display.label` | | |
| 6 | `flotilla.issue` | `display.label` | | yes |
| 7 | `flotilla.checkout` | `display.label` | | yes |

### How a path is derived

`derive_grouping_path` (`state.rs:2767`) walks the levels in order and appends a
segment for each level whose key the entity carries; optional levels the entity
lacks are skipped. Since all seven are optional, the path is exactly:

> whichever of those seven metadata keys the entity happens to carry, in that
> fixed order.

There is no concept of a level "belonging to" the entity that produced it. A
convoy entity carrying `flotilla.convoy` gets that key as its own last segment.

`level_index` is the index of the **last** level that contributed a segment.
`collapse_single_member` and `show_empty` for that entity are read from that
level — so those flags come from the deepest level the entity matched, not from
the entity's kind.

## Presence — what each kind renders as

Presence is a separate list keyed on `entity.kind`. A kind with no mapping is
`Hidden` (`state.rs:1727`).

| kind | class | notes |
|---|---|---|
| `project` | tab | |
| `convoy` | tab | |
| `vessel` | tab | |
| `session` | **tab** | a row per session |
| `issue` | inline | `form="compact"`, `visible-when="show-issues"` |
| `repo` | inline | |
| `checkout` | inline | |
| anything else | hidden | |

## Consequence: every kind is doubled

Levels and presence are independent lists over the same entities. Any kind that
has both a level keyed on its own identity metadata *and* a non-hidden presence
appears twice in the render tree — once as the group node its own path segment
creates, once as the tab/inline node its presence creates.

In the current config **all seven kinds are in that position**: project (L1 +
tab), repo (L2 + inline), convoy (L3 + tab), vessel (L4 + tab), session (L5 +
tab), issue (L6 + inline), checkout (L7 + inline).

This is what produces `▼ worker ──── ○ worker ─○─` in strip mode and a group
header followed by an identically-named card in cards mode. It is not an edge
case; it is universal under this configuration.

## The other two rules

`andamento.git` (priority −2000): levels `andamento.project` (optional),
`vcs.repo` (**required**, label `repo.name`), `git.branch` (optional). No
filter, no presence mappings.

`zellij.directory` (priority −3000): a single required level `zellij.pane.cwd`
labelled by `zellij.pane.cwd.label`. No filter, no presence mappings.

Neither declares presence, so entities routed to them would be `Hidden` — but
as noted above, entities never reach them.
