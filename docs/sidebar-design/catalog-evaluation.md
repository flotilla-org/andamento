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
three snapshots at each catalog size. With Wheelhouse's daily-driver configuration
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

## Next steps

All derived catalog structures belong to Andamento. Hosts should deliver facts
and topology and consume immutable snapshots.

This change does not retain catalogs between snapshots or change the C ABI.
Next, distinguish unchanged heartbeat/tick operations from fact, topology and
presentation changes. Preserve action invalidation when an opening recipe
changes even if the visible label does not. Shared change reporting should let
hosts apply a batch and acquire once, without guessing whether facts matter.

Measure those changes before implementing per-entity retained caches or a
fine-grained dependency graph. Full-catalog passes remain in diagnostics, and
some hierarchy operations still scan entities; this patch does not claim that
all snapshot work is linear for every hierarchy.
