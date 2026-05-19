# Tab Rail Render Config Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add independent ordering, rail structure, and sizing configuration to the scratch tab rail renderer.

**Architecture:** The controller owns the shared tab model and runtime rail config, then sends both in `ControllerViewModel`. The rail renderer treats ordering, structure, and sizing as separate axes: ordering chooses card order, sizing chooses cell heights, and structure chooses how cards are grouped into visual runs/boxes.

**Tech Stack:** Rust WASM Zellij plugins, `serde`, `ansi_term`, native unit tests plus WASM release build.

---

## Chunk 1: Renderer Config Foundation

### Task 1: Shared Config Types

**Files:**
- Modify: `/Users/robert/dev/zellij-scratch/crates/andamento-shared/src/lib.rs`
- Modify: `/Users/robert/dev/zellij-scratch/crates/andamento-controller/src/state.rs`
- Modify: `/Users/robert/dev/zellij-scratch/crates/andamento-rail/src/render.rs`

- [ ] **Step 1: Write failing shared/render tests**
- [ ] **Step 2: Run focused tests and verify failures**
- [ ] **Step 3: Add `RailConfig`, `RailStructure`, and sizing policy**
- [ ] **Step 4: Include config in `ControllerViewModel`**
- [ ] **Step 5: Update renderer entry points to accept config**
- [ ] **Step 6: Run focused tests**

### Task 2: Structure Modes

**Files:**
- Modify: `/Users/robert/dev/zellij-scratch/crates/andamento-rail/src/render.rs`

- [ ] **Step 1: Add tests for `JoinedCells`, `SplitAroundActive`, and `BoxPerTab`**
- [ ] **Step 2: Run tests and verify failures**
- [ ] **Step 3: Implement grouping/rendering by structure**
- [ ] **Step 4: Run focused tests**

### Task 3: Color Ownership

**Files:**
- Modify: `/Users/robert/dev/zellij-scratch/crates/andamento-rail/src/render.rs`

- [ ] **Step 1: Add themed tests for separator/title color ownership**
- [ ] **Step 2: Run tests and verify failures**
- [ ] **Step 3: Make separator rows belong to the cell below**
- [ ] **Step 4: Make title text use the same color as its border**
- [ ] **Step 5: Run focused tests**

### Task 4: Build

**Files:**
- No source changes expected.

- [ ] **Step 1: Run native tests for changed crates**
- [ ] **Step 2: Build WASM release artifacts**
