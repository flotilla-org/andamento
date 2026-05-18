# Plugin-Initiated Pane Resize Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Andamento synchronize rail/sidebar size per client by reading a pane's live size constraint from `PaneUpdate` and asking Zellij to resize other rail panes to the same target.

**Architecture:** Extend Zellij's plugin-visible pane model with runtime size constraints, then add a plugin command that applies a target size to a tiled pane using the existing resize/layout machinery. Andamento should treat width as client-local desired state: observe the rail's constraint, debounce changes in the controller, then let all matching rail panes converge so inactive tabs are ready before they become visible.

**Tech Stack:** Rust, Zellij plugin API protobufs, Zellij tiled pane layout/resizer, Andamento wasm plugins.

---

## Chunk 1: Expose Runtime Pane Size Constraints

### Task 1: Add plugin API data shape for live pane constraints

**Files:**
- Modify: `/Users/robert/dev/zellij/zellij-utils/src/data.rs`
- Modify: `/Users/robert/dev/zellij/zellij-utils/src/plugin_api/event.proto`
- Modify/generated: `/Users/robert/dev/zellij/zellij-utils/assets/prost/api.event.rs`
- Modify: `/Users/robert/dev/zellij/zellij-utils/src/plugin_api/event.rs`
- Test: `/Users/robert/dev/zellij/zellij-utils/src/plugin_api/event.rs`

- [ ] Add a Rust data type near `PaneInfo`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PaneDimensionConstraint {
    Fixed(usize),
    Percent(f64),
}
```

- [ ] Add optional fields to `PaneInfo`:

```rust
pub pane_rows_constraint: Option<PaneDimensionConstraint>,
pub pane_columns_constraint: Option<PaneDimensionConstraint>,
```

- [ ] Add protobuf messages in `event.proto`:

```proto
message PaneDimensionConstraint {
  oneof constraint {
    uint32 fixed = 1;
    double percent = 2;
  }
}
```

- [ ] Add optional fields to `message PaneInfo` using new field numbers after the current last field:

```proto
optional PaneDimensionConstraint pane_rows_constraint = <next>;
optional PaneDimensionConstraint pane_columns_constraint = <next>;
```

- [ ] Run proto generation from `/Users/robert/dev/zellij`:

```bash
cargo xtask build --no-plugins
```

Expected: regenerated `zellij-utils/assets/prost/api.event.rs`; build may continue beyond codegen, but generated files must update cleanly.

- [ ] Implement `TryFrom<ProtobufPaneDimensionConstraint> for PaneDimensionConstraint` and reverse conversion in `zellij-utils/src/plugin_api/event.rs`.

- [ ] Extend `PaneInfo` protobuf conversion in both directions.

- [ ] Add roundtrip tests that prove `Percent(22.5)` survives conversion without truncation.

- [ ] Run:

```bash
cargo test -p zellij-utils plugin_api::event
```

Expected: new roundtrip test passes.

### Task 2: Populate pane constraints from runtime geometry

**Files:**
- Modify: `/Users/robert/dev/zellij/zellij-server/src/tab/mod.rs`
- Test: existing tab/screen/plugin API tests if a focused test exists; otherwise add a narrow unit around `pane_info_for_pane`.

- [ ] Update `pane_info_for_pane` to read `pane.current_geom().rows.constraint` and `.cols.constraint`.

- [ ] Map runtime `Constraint::Fixed(n)` to `PaneDimensionConstraint::Fixed(n)`.

- [ ] Map runtime `Constraint::Percent(p)` to `PaneDimensionConstraint::Percent(p)`.

- [ ] Add/adjust test coverage so a percent-sized pane produces percent constraints in `PaneInfo`.

- [ ] Run:

```bash
cargo test -p zellij-server pane_info
```

Expected: test passes and existing pane info behavior remains unchanged.

## Chunk 2: Add Targeted Pane Resize-To API

### Task 3: Define plugin command for target size

**Files:**
- Modify: `/Users/robert/dev/zellij/zellij-utils/src/data.rs`
- Modify: `/Users/robert/dev/zellij/zellij-utils/src/plugin_api/plugin_command.proto`
- Modify/generated: `/Users/robert/dev/zellij/zellij-utils/assets/prost/api.plugin_command.rs`
- Modify: `/Users/robert/dev/zellij/zellij-utils/src/plugin_api/plugin_command.rs`
- Modify: `/Users/robert/dev/zellij/zellij-tile/src/shim.rs`
- Test: `/Users/robert/dev/zellij/zellij-utils/src/plugin_api/plugin_command.rs`

- [ ] Add command variant:

```rust
ResizePaneWithIdTo(PaneId, PaneDimensionConstraint)
```

- [ ] Add protobuf payload:

```proto
message ResizePaneWithIdToPayload {
  optional PaneId pane_id = 1;
  optional event.PaneDimensionConstraint size = 2;
}
```

- [ ] Add a new `CommandName` and oneof payload entry.

- [ ] Regenerate protobufs with:

```bash
cargo xtask build --no-plugins
```

- [ ] Implement Rust/protobuf conversions and command tests.

- [ ] Add `zellij_tile::resize_pane_with_id_to(pane_id, size)` shim.

- [ ] Run:

```bash
cargo test -p zellij-utils plugin_api::plugin_command
```

Expected: command roundtrip passes.

### Task 4: Implement host-side resize-to behavior

**Files:**
- Modify: `/Users/robert/dev/zellij/zellij-server/src/plugins/zellij_exports.rs`
- Modify: `/Users/robert/dev/zellij/zellij-server/src/screen.rs`
- Modify: `/Users/robert/dev/zellij/zellij-server/src/tab/mod.rs`
- Modify: `/Users/robert/dev/zellij/zellij-server/src/panes/tiled_panes/mod.rs`
- Modify: `/Users/robert/dev/zellij/zellij-server/src/panes/tiled_panes/tiled_pane_grid.rs`
- Test: `/Users/robert/dev/zellij/zellij-server/src/tab/unit/tab_integration_tests.rs` or focused tiled pane tests.

- [ ] Add `ScreenInstruction::ResizePaneWithIdTo(PaneId, PaneDimensionConstraint)`.

- [ ] Wire plugin command export to screen instruction.

- [ ] Implement tiled-only support first; floating panes can return/log unsupported for this slice.

- [ ] In tiled panes, derive resize axis from the target pane's active dimension:
  - left/right rail style panes use columns/horizontal.
  - top/bottom rail style panes use rows/vertical.
  - if ambiguous, prefer the dimension whose constraint differs from 100% and whose current span has adjacent neighbors; otherwise return no-op/error.

- [ ] For `Percent(p)`, compute target cell size from display area on the inferred axis, compare to current `pane.cols()`/`pane.rows()`, then apply existing resize machinery with the corresponding direction and percent delta.

- [ ] Preserve flexible percent semantics: do not convert panes to fixed constraints when applying `Fixed(n)`; treat fixed target as "resize until the actual cells are n".

- [ ] Add tolerance to prevent resize loops:
  - cell targets within 1 cell are no-op.
  - percent targets within a small epsilon are no-op.

- [ ] Run focused tests:

```bash
cargo test -p zellij-server resize_pane
```

Expected: existing resize tests pass and new resize-to tests pass.

## Chunk 3: Use The API In Andamento

### Task 5: Capture local rail size intent

**Files:**
- Modify: `/Users/robert/dev/zellij-scratch/crates/tabs-shared/src/lib.rs`
- Modify: `/Users/robert/dev/zellij-scratch/crates/tabs-controller/src/state.rs`
- Modify: `/Users/robert/dev/zellij-scratch/crates/tabs-controller/src/main.rs`
- Modify: `/Users/robert/dev/zellij-scratch/crates/tabs-rail/src/main.rs`
- Test: shared/controller unit tests where present.

- [ ] Add shared message for rail size observation:

```rust
RailSizeObserved {
    client_id: u16,
    pane_id: ...,
    size: PaneDimensionConstraint,
    observed_at_ms: u64,
}
```

- [ ] Rail reads its own `PaneInfo` from `PaneUpdate` and picks `pane_columns_constraint` for left/right sidebar.

- [ ] Rail sends observed size to controller only when changed beyond tolerance.

- [ ] Controller stores desired rail size per client, debounced by timestamp/version.

- [ ] Controller broadcasts desired size to rails for the same client only.

### Task 6: Apply debounced desired size without waiting for visibility

**Files:**
- Modify: `/Users/robert/dev/zellij-scratch/crates/tabs-rail/src/main.rs`
- Test: rail state tests if present; otherwise add small pure helper tests.

- [ ] Rail stores latest desired size from controller.

- [ ] Rail applies desired size when:
  - it has a known pane id;
  - current size differs from desired size beyond tolerance;
  - the desired size version is newer than the last size this rail applied.

- [ ] Do not use `Visible(false)` as a hard gate. Inactive rails should be resized after the controller debounce so they are already correct when the user switches tabs.

- [ ] On `Visible(true)`, reconcile once in case this rail missed an update while unloaded, newly attached, or recovering from a failed resize.

- [ ] Avoid throwaway work:
  - controller broadcasts only after debounce;
  - rails no-op if the version was already applied;
  - rails no-op if the current constraint/cell size is already within tolerance;
  - rails do not render just to apply size.

- [ ] Use `resize_pane_with_id_to` when compiled against the fork API.

- [ ] Keep a stock-Zellij fallback behind a cargo feature or compile gate; fallback should simply skip width sync rather than trying to simulate with repeated old resize calls.

### Task 7: Verification

**Files:**
- No new files unless tests require fixtures.

- [ ] Run Zellij API tests:

```bash
cd /Users/robert/dev/zellij
cargo test -p zellij-utils plugin_api
cargo test -p zellij-server resize_pane
```

- [ ] Run Andamento tests/build:

```bash
cd /Users/robert/dev/zellij-scratch
cargo test --target aarch64-apple-darwin -p tabs-shared -p tabs-controller -p tabs-rail -p tabs-rail-config
cargo build --release -p tabs-controller -p tabs-rail -p tabs-rail-config
```

- [ ] Manual check:
  - Start the layout with rails on multiple tabs.
  - Resize the visible rail with the normal Zellij border drag.
  - Switch tabs.
  - Expected: newly visible rail is already at the same width, or converges immediately without repeated jitter.
  - Attach a second client.
  - Expected: width preference is per client, not global.

## Open Decisions Before Implementation

- [ ] Name: `PaneDimensionConstraint` vs `PaneSizeConstraint`.
- [ ] Axis inference: host-only inference, or include optional `Direction`/axis in the plugin command for deterministic rail placement.
- [ ] Percent precision: protobuf `double` is preferred for live constraints; `SplitSize::Percent(usize)` is too lossy for this path.
- [ ] Fixed targets: first slice treats fixed as a target cell count, not a request to make the pane fixed.
