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
cargo test --locked --manifest-path tools/scenario/Cargo.toml --target "$host"
```

The generator test regenerates all scenes and compares them byte for byte.
Run it when editing scenarios or updating the pin. It stays separate from
`check-independent-core.py`, whose dependency graph must exclude Flotilla.

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


## Standing role gap and restart regressions (andamento#105)

`scripted-gap` uses the roll's initial and re-admitted states, but its held
observation has no convoy rows while retaining the `ConvoyEnsure` declaration.
The real pinned projection removes the project's `flotilla.project` join even
though the role still points at that project. `scripted-restart` publishes empty
query rows at 1000 ms and fully reasserts the same attempt at 2000 ms. This models
an empty **publication**, not transport silence: the pinned connector's socket
closure path emits no removal patches and lets facts fade by TTL. These are
scripted reproductions, not live recordings of the daily-driver incident.

Neither scenario supplies project repository or awareness rows, deliberately
exercising absence of those independent project publications. Keeping those
rows observed can preserve the parent; a declared role must also publish its
project independently of attempts. Temporary missing observations must not be
interpreted as declaration deletion.

Regenerate with the same real projection tool (native target):

```sh
cargo run --locked --manifest-path tools/scenario/Cargo.toml --target x86_64-unknown-linux-gnu -- fixtures/scenarios/scripted-gap.json > fixtures/scripted-gap.jsonl
cargo run --locked --manifest-path tools/scenario/Cargo.toml --target x86_64-unknown-linux-gnu -- fixtures/scenarios/scripted-restart.json > fixtures/scripted-restart.jsonl
cargo test --locked --manifest-path tools/scenario/Cargo.toml --target x86_64-unknown-linux-gnu
```

The core `replay` integration tests check one role on the correct project row
with a stable placement key at **every observation**. The two `declared_role_stays`
tests are ignored while the connector defect remains; run them explicitly to
see the 1000 ms failure. After fixing the upstream projection, update its pin,
regenerate the fixtures and remove the ignores when both tests pass. The passing
`declared_role_parent_retained_control_stays_on_project` removes only the
single project-withdrawal patch from the gap stream when present, proving that retaining the
parent join alone preserves placement. After the upstream fix it leaves the intact publication alone. It is a test
control, not runtime code.

With a native `andamento-replay` binary, the standalone exact-symptom check is:

```sh
python3 scripts/check-standing-role.py fixtures/scripted-gap.jsonl --binary target/independent/x86_64-unknown-linux-gnu/debug/andamento-replay
python3 scripts/check-standing-role.py fixtures/scripted-restart.jsonl --binary target/independent/x86_64-unknown-linux-gnu/debug/andamento-replay
```

Both currently exit nonzero with `role dissociated or duplicated at 1000 ms`.
`fixtures/standing-role.kdl` is shared by that checker and the core tests. Build
the binary with `cargo build -p andamento-core --bin andamento-replay --target
x86_64-unknown-linux-gnu`, or use a standalone core workspace when the sibling
Zellij SDK is unavailable. `scripts/check-independent-core.py` runs the core
integration tests without Zellij; the projection generator is a separate test
workspace so core does not gain a Flotilla dependency.
