# Browser sidebar prototype: notes

PROTOTYPE, throwaway. This is for "The sidebar in the browser" (wheelhouse#428, map wheelhouse#421).

## Running it

```sh
cd prototypes/web-sidebar/web && pnpm install && pnpm dev   # http://localhost:5178
```

`pnpm dev` builds the wasm shim with wasm-pack first.

## The question

Does SolidJS's fine-grained reactivity fit Andamento's core running as wasm, and what does the wasm surface need?

## What it is

- **`core-wasm/`** is a wasm-bindgen shim over `andamento-core`.
  - It wraps `Sidebar::new`, `apply`, `Tick` and `Dispatch`.
  - It returns either the whole semantic snapshot (`snapshot_json`) or keyed row deltas (`delta_json`).
  - Its deltas are computed by diffing projected rows. That diff stands in for the core's own generation-tagged deltas (wheelhouse#441).
- **`web/`** is Vite plus SolidJS.
  - It replays `fixtures/rail-scene.jsonl` with Wheelhouse's `data/sidebar/daily-driver.kdl`.
  - Clicks dispatch `ActivatePlacement`, `TogglePlacement` and `ToggleDisplayVariable` back into the core.
  - A stress panel adds N synthetic vessels and flips their status at a set rate.
  - A metrics panel times each stage and counts DOM mutations.

## Findings (22 October 2026, Chrome via Playwright, M-series Mac, dev build)

**1. The core runs in the browser unchanged.**
- `andamento-core` compiles to `wasm32-unknown-unknown` with no code changes; its only dependencies are `kdl`, `serde`, `foldhash` and `unicode-width`.
- `Instant` is used only in profiling and in the replay recorder.
- Applying one patch costs about 0.1 ms.
- The module is 1.5 MB raw, 556 KB gzipped (release, `opt-level=s`, LTO), and not slimmed yet. `kdl` and `miette` are the likely bulk.

**2. Fine-grained reactivity fits keyed deltas well.**
- With keyed row deltas, one status flip touches 1 to 3 DOM nodes.
- Store plus DOM update costs:

  | | median | p95 |
  |---|---|---|
  | 200 vessels (163 rows) | 0.4 ms | 0.6 ms |
  | 1000 vessels (734 rows) | 1.4 ms | 1.8 ms |

- The Solid code is small: one store keyed by row id, a `For` over child ids, and each row reads only its own fields.

**3. Whole snapshots don't scale as a surface.** The core's serialized snapshot is large because it includes facts, detail and effective KDL text:

| | snapshot size | total per update, median |
|---|---|---|
| 200 vessels | 1.6 MB | 7.6 ms |
| 1000 vessels | 7.3 MB | 33 ms, over a frame |

`reconcile` keeps the DOM changes small, but serializing, parsing and diffing the whole tree every time is the cost. Deltas are required, as the core-boundary decision already said.

**4. Computing deltas by diffing doesn't scale either.**
- The shim re-projects and compares every row: 2.6 ms at 163 rows and 11.6 ms at 734, per update.
- The core should emit deltas from what actually changed (wheelhouse#441), not have them diffed afterwards.
- Order changes (rows shown or hidden by visibility rules) currently resend the whole order: about 280 KB at 734 rows. They need positional insert, remove and move operations.

**5. Typed fields are missing from the semantic model.**
- Chip meaning travels as prefixed strings in field values (`chip-status:active`, `chip-medium:…`, `chip-attention:true`), which native's C parses (`uishell_sidebar.c:3045`).
- The web had to parse them too, twice in this prototype: once in the shim and once in snapshot mode.
- The model should carry typed roles (status, attention, icon, short and medium labels) instead.

**6. Intent results should come through the stream.**
- `Dispatch` returns a whole snapshot in its Response, so each click costs a full serialization.
- Results should arrive through the same delta stream as fact changes, which is decision 3 of the core-boundary ticket.

**7. Not tried:** a binnacle table in the page, real collapsed-row hiding rules versus the frontend's own, and a production build size.

## Verdict

**Adopt SolidJS for the web frontend** (Robert, 22 October 2026). The remaining costs are on the core's side (findings 3 to 6), not the framework's.
