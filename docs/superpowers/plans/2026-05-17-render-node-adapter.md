# Render Node Adapter Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Normalize tab rail rendering through local render nodes so grouped and ungrouped views share one projection model before richer hierarchy work.

**Architecture:** Keep the shared `RailRow` view model unchanged. Add an internal `RenderNode` adapter in `crates/andamento-rail/src/render.rs` that converts fallback tabs, flat controller rows, and grouped controller rows into a single local node list consumed by normal rendering. Preserve current visual behavior while making the next collapse/template/group-border slices land on one hierarchy.

**Tech Stack:** Rust, existing `andamento-rail` renderer tests, `cargo test -p andamento-rail --target aarch64-apple-darwin`.

---

## Chunk 1: Local Render Node Adapter

### Task 1: Characterize Current Structure Behavior

**Files:**
- Modify/test: `crates/andamento-rail/src/render.rs`

- [x] **Step 1: Write failing tests**

Add focused tests showing grouped rendering does not collapse all `rail_structure` settings into the same standalone child boxes. For `JoinedCells`, grouped child tabs should render as an indented joined run under the group header. For `BoxPerTab`, grouped child tabs should continue to render as complete boxes.

- [x] **Step 2: Run focused tests to verify red**

Run: `cargo test -p andamento-rail --target aarch64-apple-darwin grouped_child -- --nocapture`

Expected: the new joined-cells grouped test fails against the current projection renderer.

- [x] **Step 3: Implement local node normalization**

Add a private `RenderNode` enum and a helper that produces nodes from `ControllerViewModel` rows when present, from `model.tabs` otherwise, and from local tabs as a controller-unavailable fallback. Replace the normal-mode `cards_to_render`/`rows_to_render` branch with rendering from these nodes.

- [x] **Step 4: Render tab runs through existing structure renderers**

When a node list has only top-level tabs, pass the cards directly to `render_cards`. When it has groups, render group headers and render each contiguous child tab run through a helper that applies `RailStructure` within the group's indented width.

- [x] **Step 5: Run focused tests to verify green**

Run: `cargo test -p andamento-rail --target aarch64-apple-darwin grouped_child flat_controller_rows -- --nocapture`

Expected: all focused renderer tests pass.

- [x] **Step 6: Run broader verification**

Run: `cargo test -p andamento-rail --target aarch64-apple-darwin`

Expected: all `andamento-rail` tests pass.
