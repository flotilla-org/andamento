# Connector scenarios

These are full Flotilla query-row snapshots, not presentation patches.
`tools/scenario` calls the actual `flotilla_manifest::projection::project_catalog`
and `Catalog::diff_patches`, pinned to the revision in its Cargo.toml. It is a
separate build workspace so Flotilla dependencies never enter the core, FFI or
replay executable. Update the pin and regenerate deliberately when projection
changes; review the catalog snapshots with the resulting fixture changes.

Each array entry has a monotonic `offset_ms` and optional `convoys`,
`independents`, `standing_roles`, `project_repositories`, and `awareness` query
rows. Omitted row lists are empty, not retained from the preceding step.
`refresh: true` emits the real catalog's full TTL reassertion instead of a diff.
As in the connector, a non-null awareness input selects awareness projection.

From the repository root, on a native target:

```sh
host=$(rustc -vV | sed -n 's/^host: //p')
cargo run --locked --manifest-path tools/scenario/Cargo.toml --target "$host" -- \
  fixtures/scenarios/rail-scene.json > fixtures/rail-scene.jsonl
cargo run --locked --manifest-path tools/scenario/Cargo.toml --target "$host" -- \
  fixtures/scenarios/scripted-roll.json > fixtures/scripted-roll.jsonl
```

`rail-scene` keeps the original projects, convoy/vessel workloads, unattached
triage work and issues. It uses today's real awareness projection; labels,
actions and status facts intentionally follow that code. `rail-scene-legacy.jsonl`
is the frozen output of the removed Python script, retained to prove replay
compatibility with the old scene, alongside `flotilla-connector-patches.jsonl`.
Both old captures and the generated scene have catalog snapshots in core tests.

`scripted-roll` starts an attached coder role, holds it with a failed attempt at
1000 ms, and admits generation two at 2000 ms. Core tests assert the stable role
placement under its project, changed primary target, and subsequent TTL expiry.
This is synthetic, not a real fleet recording. Flotilla #2281 owns the live tap;
Andamento #105 must add a real roll capture when one is available.
