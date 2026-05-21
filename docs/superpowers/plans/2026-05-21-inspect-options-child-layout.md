# Inspect Options Child Layout Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make compact child-tab rendering tryable by adding direct Inspect Options controls for metadata visibility and group child layout.

**Architecture:** Keep Inspect selection separate from options. The config plugin renders a single `Options` section with segmented rows. Metadata visibility writes the existing `MetadataControls`; child layout writes group metadata under a stable inspector source so the existing rail renderer can consume `rail.child_layout`.

**Tech Stack:** Rust, Zellij plugin pipe messages, Andamento shared JSON messages, existing config frame/hit-region renderer.

---

## Chunk 1: Explicit Option Messages

### Task 1: Replace metadata cycling with explicit metadata visibility setting

**Files:**
- Modify: `crates/andamento-shared/src/lib.rs`
- Modify: `crates/andamento-controller/src/main.rs`
- Modify: `crates/andamento-controller/src/state.rs`
- Modify: `crates/andamento-config/src/lib.rs`

- [x] Add `MetadataVisibilitySetRequest { client_id, node_key, state: Option<MetadataTriState> }`.
- [x] Add `MSG_SET_METADATA_VISIBILITY`.
- [x] Add controller parsing/handling that sets or removes the sparse per-node value.
- [x] Replace config `CycleInspectedMetadata` action with `SetInspectedMetadata(Option<MetadataTriState>)`.
- [x] Update Inspect rendering to show `Options` with a `Metadata` segmented row.

### Task 2: Add child layout option for group inspect targets

**Files:**
- Modify: `crates/andamento-shared/src/lib.rs`
- Modify: `crates/andamento-controller/src/main.rs`
- Modify: `crates/andamento-config/src/lib.rs`

- [x] Add `ChildLayoutSetRequest { client_id, node_key, layout: Option<ChildLayoutSetting> }`.
- [x] Add `ChildLayoutSetting::{Cards, CompactStrip}`.
- [x] Add `MSG_SET_CHILD_LAYOUT`.
- [x] Controller converts the request into a metadata patch on the group target:
  - `None` unsets `rail.child_layout`.
  - `Cards` sets `rail.child_layout=vertical`, matching the existing normal child renderer.
  - `CompactStrip` sets `rail.child_layout=compact-strip`.
- [x] Config renders active `Child layout` segmented row only for group targets.

## Chunk 2: Verification

- [x] Run focused red tests before implementation.
- [x] Run `cargo test --target aarch64-apple-darwin -p andamento-shared --lib`.
- [x] Run `cargo test --target aarch64-apple-darwin -p andamento-controller --lib`.
- [x] Run `cargo test --target aarch64-apple-darwin -p andamento-config --lib`.
- [x] Run `cargo test --target aarch64-apple-darwin -p andamento-rail --lib`.
- [x] Run `cargo build --workspace --target wasm32-wasip1 --release -j1`.
- [ ] Commit the implementation.
