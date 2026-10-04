# Structured detail extension for ABI 2

This is the Andamento contract for Wheelhouse #177. `andamento_abi_version()`
continues to return **2**. Existing C structs, exports, flat text formatting and
workspace actions retain their contracts. The authoritative C layouts and pointer
lifetimes are in `crates/andamento-ffi/include/andamento.h`. Dynamic consumers can
probe the new exports with `dlsym`; static consumers pin a revision as before.
No `ANDAMENTO_REV` workflow change is included here.

## Template declarations

Detail templates remain `<kind>/detail`, slot `detail`. Fields accept optional
`section`, `role`, `label`, and `structured-only` properties in both KDL and JSON:

```kdl
template "issue/detail" slot="detail" node-kind="entity" {
  field "title" section="header" role="title" key="display.label"
  field "state" section="header" role="state" key="status.state"
  field "ci" section="facts" role="fact" label="CI" key="ci.state"
  field "related" structured-only=true section="related" role="relation" key="flotilla.subject.works_on"
}
```

| Role | C tag | Meaning |
| --- | --- | --- |
| `identity` | 0 | Header identity |
| `title` | 1 | Header title |
| `state` | 2 | State badge |
| `fact` | 3 | Labeled fact |
| `relation` | 4 | Typed related entities |

Sections are frontend-neutral strings; bundled declarations use `header`, `facts`
and `related`. An absent section or label is an empty string. Fields without a
role remain available to flat clients and are omitted from structured cards.
`structured-only=true` adds a declaration without adding a flat field or reserving
its columns. Existing `prefix` and `suffix` affect flat output only. Structured
text resolves directly from the source, including configured transformations,
without recovering labels or identities from rendered text. Inheritance,
fragments, overrides and effective KDL dumps preserve these properties.

Bundled detail templates cover `change_request`, `issue`, `convoy`, `role`,
`project` and `worktree`; other kinds use the generic detail fallback. Bundled
relation fields consume `related.entities`, `flotilla.subject.produces`,
`flotilla.subject.works_on`, `flotilla.subject_of`, and
`flotilla.role.current_attempt`. Producers supply `MetadataValue::EntityRefs`;
text and string lists do not implicitly become relationships. Other typed
relationship keys can be declared by configuration. Existing legacy text keys,
such as `flotilla.project`, are not inferred as links.

## Snapshot lookup and ownership

All output buffers are borrowed from an acquired immutable `AndamentoSnapshot`;
release it only after finishing all reads. Indices are snapshot-local. Reacquire
and find by exact kind/id after draining facts, time, configuration, topology or
completion changes. Old snapshots remain readable. Action dispatch uses the
existing snapshot generation/client validation, including for these new actions.

| Export | Result |
| --- | --- |
| `andamento_snapshot_detail_count` | Number of catalog cards |
| `andamento_snapshot_detail_find` | Exact kind/id lookup; `ANDAMENTO_NONE` when unavailable |
| `andamento_snapshot_detail` | Card identity, label, field count, action IDs, clock, error, optional workspace ID |
| `andamento_snapshot_detail_field` | Name, section, role, label, unadorned text, presence, source and observation metadata, relation count |
| `andamento_snapshot_detail_relation` | Exact target identity, display label and detail index, filtered by navigation path |
| `andamento_snapshot_detail_action` | Semantic intent, label, stable target identity and dispatch action ID |

Lookup is over the complete resolved catalog, including entities absent from all
placements or hidden by presentation filters. A configuration resolution failure
is reported in the card's `error`; no field contents are guessed.

`has_value=0` denotes a missing fact; `has_value=1` with zero-length text denotes
known empty. Empty literals and empty typed lists are also known values. Missing
fields remain declared so consumers can choose their presentation. `source_key`
identifies the chosen metadata source, or the first declared metadata key when
all sources are missing; literal/synthetic values have no source key.

`has_observation` distinguishes an observation from a synthetic value.
`observed_at_ms` is the winning fact's controller receipt/refresh time, in the
same **host monotonic millisecond domain** as the card's `now_ms`, not a Unix
or producer event timestamp. `source_id` identifies that winning producer.
Retained observations whose producer contribution is no longer available have
an empty source ID. `has_ttl` distinguishes no expiry from a zero TTL; deadlines
use saturating addition. A fact is stale strictly after `observed_at_ms + ttl_ms`.
Ordinary expired facts become missing; retained workspace facts can remain stale.
Frontends own relative-age formatting and stale colors.

Relations come directly from typed entity references. Self references are omitted.
Iterate `relation_count`, passing the exact kind/id navigation path to each
relation lookup; return 0 means skip that index. Path filtering does not renumber
indices. The display text is the target's current label (its ID if unavailable).
A target with no placement still has a detail index; a target absent from the
catalog has `detail=ANDAMENTO_NONE`. Never parse relation field text for targets.

Action index 0 is the primary control; index 1 is copy URL when available.
Primary intent is `open-url`, `focus-workspace`, `materialize-workspace`, or
`inspect`; the copy intent is `copy-url`. The primary label uses the same
`action.primary.label` fact as workspace controls, with a semantic default.
`action` is dispatched through `andamento_dispatch`, targeting the stable entity
and producing the existing effects/completions. Primary activation retains the
existing precedence: a subject URL opens in the browser before workspace focus.

Preview identity is separate from all field roles: `has_workspace` and
`workspace_id` identify a live workspace even for a card without a placement.
For a card anchored to a specific visible alias, the existing node workspace
identity remains available. Both peek and engaged cards may draw the same live
preview; #89 owns attachment policy, #166 owns detaching, and Wheelhouse owns
layout, colors, gestures and the visibility of actions in engaged cards. The
Zellij reserved hover panel is unchanged.

## Validation

`fixtures/typed-detail.jsonl` supplies all six kinds, known-empty summary facts,
missing kind-specific facts, typed self/hidden/unavailable relations, and TTLs.
Core tests enumerate every role across missing and known-empty values and check
KDL/JSON agreement, effective KDL round trips, separate labels/text, empty
literals, invalid roles and preservation of flat output. Native Rust tests cover
path omission, exact targets, observation provenance, expiry boundaries, action
identity and immutable snapshot lifetimes. The independent C fixture links the
real library and checks the actual C layouts, hidden navigation and live preview
identity, along with the existing ABI 2 scenarios.
