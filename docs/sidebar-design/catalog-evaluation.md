# Catalog evaluation cost

Wheelhouse profiling on 2026-09-21 found almost all main-thread samples in
sidebar snapshot construction. Each group ordinal lookup rebuilt and sorted the
entire catalog, including resolution of every entity's metadata. A configuration
with one path per entity made this work grow approximately quadratically.
Observed workspaces also rebuilt the catalog for each entity grouping lookup.

`ControllerState::view_model` now owns one `CatalogEvaluation`. The grouping,
latent entity, metadata, template and surface passes share its resolved entities.
Entity identity and group ordinal lookups use indexes, preserving the first
ordinal in catalog order when entities share a path. Inline and compact filters
borrow entities instead of cloning their metadata. The evaluation has snapshot
scope, so changes to facts, time, configuration or host topology are picked up
by the next build without a persistent invalidation mechanism.

## Measurement

The `andamento-core` example `snapshot-bench` accepts a host KDL configuration:

```sh
cargo run -p andamento-core --example snapshot-bench -- /path/to/sidebar.kdl
```

The probe applies synthetic vessel facts, warms the snapshot path, then measures
three changed snapshots at each catalog size. It also reports 1,000 unchanged
tick/snapshot queries separately. With Wheelhouse's daily-driver configuration
and an unoptimised build on macOS:

| Entities | Before (ms) | Shared evaluation (ms) |
| --- | ---: | ---: |
| 25 | 20.948 | 3.324 |
| 50 | 66.070 | 6.395 |
| 100 | 236.383 | 13.530 |
| 200 | 923.033 | 30.279 |

These measurements isolate catalog scaling. They are not an end-to-end latency
measurement or a representative benchmark of every template/placement pattern.
The fixture has vessel facts without project/convoy parent entities; it exercises
legacy grouping but does not populate the configured native project tree.

The regression test counts full catalog evaluations, rather than asserting a
machine-dependent time limit. It checks one build for both latent and observed
workspaces at two sizes. Before the change, eight latent entities caused 14
catalog builds. Existing semantic tests cover ordering, collapse, templates,
activation and separate frontend state.

Validation: `scripts/check-independent-core.py` passed core, terminal, HTML and
FFI tests, the no-JSON FFI build, and the linked C fixture. The expanded observed
workspace regression also passed. Rustfmt and diff whitespace checks passed.
Strict Clippy remains blocked by existing warnings in the core (including
metadata map iteration, boolean assertions and derivable defaults).

## Revision-based reuse

`Sidebar` now retains its immutable snapshot until an input changes its
presentation or action dependencies. Identical facts, heartbeat renewals and
unchanged topology preserve the revision. Ticks use a deadline-count index to
check for expirations without scanning facts. Renewals replace the old deadline;
removals unregister it. Expiration and removal of an entity's last contribution
invalidate the snapshot. Recipe-only changes invalidate actions even when the
visible row content is unchanged.

The additive C query `andamento_snapshot_is_current` lets hosts drain updates
and acquire at most one replacement snapshot. Existing acquisition still returns
an independently owned snapshot, and old snapshot text remains valid. The Rust
`snapshot_shared` method borrows the retained snapshot without cloning it.

In the debug scaling probe, unchanged tick plus borrowed snapshot queries took
0.05 to 0.08 microseconds per iteration at 25 through 200 entities. These are
in-process measurements, not C snapshot flattening or end-to-end rendering.
Changed inputs still perform a full evaluation. Repeated timing under concurrent
build load is variable; use the probe to compare on an otherwise idle machine.

Validation now includes 150 core unit tests, 15 sidebar integration tests,
terminal/HTML/FFI tests, the no-JSON FFI build and the linked C fixture. Tests cover
renewed deadlines, bounded deadline storage, expiry, removal of the last expired
fact, identical topology, selection changes and recipe-only stale actions.

## Remaining work

Derived structures and change tracking stay in Andamento. This implementation
retains snapshots but does not yet retain and incrementally update individual
catalog entries. Some hierarchy operations still scan entities, and diagnostics
resolve metadata separately. It does not claim all snapshot work is linear for
every hierarchy.

Producer-side batch envelopes and source lease renewal remain possible protocol
improvements. Wheelhouse can already defer snapshot acquisition until after its
existing queue drain, using core-owned revision reporting. Measure real traffic
with these changes before adding per-entity caches or a dependency graph.
