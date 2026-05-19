# Metadata Inspection View Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first metadata inspection mode to the actual tab rail plugin.

**Architecture:** Add a `RailViewMode` axis to `RailConfig`, parse it from `rail_view`, expose it in the config editor, and make the rail renderer project the current group/tab model into expanded generic `key: value` lines. This first slice uses metadata already present in the view model; raw-source drill-in and external template matching are follow-ups.

**Tech Stack:** Rust workspace, Serde shared config types, native unit tests with `--target aarch64-apple-darwin`, WASM release build with `wasm32-wasip1`.

---

## Chunk 1: Config Axis And Renderer Projection

### Task 1: Add `RailViewMode`

**Files:**
- Modify: `crates/andamento-shared/src/lib.rs`
- Modify: `crates/andamento-controller/src/main.rs`
- Modify: `crates/andamento-config/src/main.rs`

- [x] **Step 1: Write failing tests**

Add tests for default normal view, parsing `rail_view "metadata"`, and config editor view options.

- [x] **Step 2: Verify failure**

Run focused tests for `andamento-shared`, `andamento-controller`, and `andamento-config`.

Expected: fail because `RailViewMode` and `RailConfig.view` do not exist.

- [x] **Step 3: Implement config axis**

Add `RailViewMode::{Normal, Metadata}`, add `view` to `RailConfig`, parse `rail_view`, and render config editor toggles.

- [x] **Step 4: Verify focused tests pass**

Run the focused tests again.

Expected: pass.

### Task 2: Render Metadata Projection

**Files:**
- Modify: `crates/andamento-rail/src/render.rs`

- [x] **Step 1: Write failing renderer test**

Add a test proving metadata view renders group and tab key/value lines instead of normal boxes.

- [x] **Step 2: Verify failure**

Run: `cargo test -p andamento-rail metadata_view_renders_group_and_tab_key_values --target aarch64-apple-darwin`

Expected: fail because metadata view still uses normal rendering.

- [x] **Step 3: Implement metadata projection**

Render group path segments, group labels/counts, tab ids/names/position/active/pin state, grouping metadata, and status metadata as generic `key: value` lines.

- [x] **Step 4: Verify focused test passes**

Run: `cargo test -p andamento-rail metadata_view_renders_group_and_tab_key_values --target aarch64-apple-darwin`

Expected: pass.

### Task 3: Full Verification And Commit

**Files:**
- Modify: `docs/sidebar-design/metadata-and-roadmap.md`
- Modify: `docs/superpowers/plans/2026-05-17-metadata-inspection-view.md`
- Modify: `crates/andamento-shared/src/lib.rs`
- Modify: `crates/andamento-controller/src/main.rs`
- Modify: `crates/andamento-controller/src/state.rs`
- Modify: `crates/andamento-config/src/main.rs`
- Modify: `crates/andamento-rail/src/render.rs`

- [x] **Step 1: Format**

Run: `cargo fmt`

- [x] **Step 2: Run native tests**

Run: `cargo test -p andamento-shared -p andamento-controller -p andamento-rail -p andamento-config --target aarch64-apple-darwin`

Expected: pass.

- [x] **Step 3: Run WASM build**

Run: `cargo build --workspace --target wasm32-wasip1 --release`

Expected: pass.

- [x] **Step 4: Commit**

Run:

```bash
git add docs/sidebar-design/metadata-and-roadmap.md docs/superpowers/plans/2026-05-17-metadata-inspection-view.md crates/andamento-shared/src/lib.rs crates/andamento-controller/src/main.rs crates/andamento-controller/src/state.rs crates/andamento-config/src/main.rs crates/andamento-rail/src/render.rs
git commit -m "feat: add metadata inspection rail view"
```
