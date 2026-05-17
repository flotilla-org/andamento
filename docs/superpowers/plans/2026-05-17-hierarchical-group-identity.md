# Hierarchical Group Identity Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace cwd string group identity with structured `GroupPath` while preserving current directory grouping behavior.

**Architecture:** Add shared `GroupPath`/`GroupSegment` types in `tabs-shared`, then thread them through `TabGroupingInfo` and `RailRow::GroupHeader`. The controller continues resolving exact cwd from pane metadata, but now maps that cwd to a one-segment path keyed by `zellij.pane.cwd`. The renderer keeps using labels for display, while the config editor shows the structured path for debugging.

**Tech Stack:** Rust workspace, Serde JSON view models, Zellij WASM plugins, native unit tests via `--target aarch64-apple-darwin`, release artifacts via `wasm32-wasip1`.

---

### Task 1: Shared Group Path Types

**Files:**
- Modify: `crates/tabs-shared/src/lib.rs`

- [x] **Step 1: Write failing shared serialization tests**

Add tests proving `TabGroupingInfo` and `RailRow::GroupHeader` carry a structured `GroupPath`.

- [x] **Step 2: Verify tests fail**

Run: `cargo test -p tabs-shared --target aarch64-apple-darwin`

Expected: compile failure because `GroupPath`/`GroupSegment` and the new fields do not exist.

- [x] **Step 3: Implement shared types**

Add `GroupSegment { key: String, value: MetadataValue }` and `GroupPath(Vec<GroupSegment>)`, then add `path: GroupPath` to `TabGroupingInfo` and `RailRow::GroupHeader`.

- [x] **Step 4: Verify shared tests pass**

Run: `cargo test -p tabs-shared --target aarch64-apple-darwin`

Expected: pass.

### Task 2: Controller Path Identity

**Files:**
- Modify: `crates/tabs-controller/src/state.rs`

- [x] **Step 1: Write failing controller test**

Add a test proving directory grouping emits `GroupPath([{ key: "zellij.pane.cwd", value: Text(cwd) }])` and still groups tabs by exact cwd.

- [x] **Step 2: Verify test fails**

Run: `cargo test -p tabs-controller --target aarch64-apple-darwin`

Expected: compile or assertion failure before the controller populates `path`.

- [x] **Step 3: Implement controller path generation**

Create the cwd group path when building `TabGroupingInfo`; use it as the group identity key while preserving existing labels and user-visible row ordering.

- [x] **Step 4: Verify controller tests pass**

Run: `cargo test -p tabs-controller --target aarch64-apple-darwin`

Expected: pass.

### Task 3: Config Debug Rendering

**Files:**
- Modify: `crates/tabs-rail-config/src/main.rs`
- Modify: `crates/tabs-rail/src/render.rs` if test fixtures need the new field

- [x] **Step 1: Write failing config renderer test**

Add a test that expects cwd metadata debug output to include both the display label and structured group path.

- [x] **Step 2: Verify test fails**

Run: `cargo test -p tabs-rail-config --target aarch64-apple-darwin`

Expected: failure until the renderer includes path debug text.

- [x] **Step 3: Implement debug path formatting**

Render `label` and a compact `key=value` path string for each tab with grouping metadata.

- [x] **Step 4: Verify config tests pass**

Run: `cargo test -p tabs-rail-config --target aarch64-apple-darwin`

Expected: pass.

### Task 4: Full Verification And Commit

**Files:**
- Potentially modify fixtures in `crates/tabs-rail/src/render.rs`
- Commit all touched files

- [x] **Step 1: Format**

Run: `cargo fmt`

- [x] **Step 2: Run full native test set**

Run: `cargo test -p tabs-shared -p tabs-controller -p tabs-rail -p tabs-rail-config --target aarch64-apple-darwin`

Expected: pass.

- [x] **Step 3: Run WASM release build**

Run: `cargo build --workspace --target wasm32-wasip1 --release`

Expected: pass.

- [x] **Step 4: Commit**

Run:

```bash
git add .
git commit -m "feat: add structured group paths"
```
