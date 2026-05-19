# Metadata Patch Semantics Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add generic metadata patch application with `MetadataTarget` and `MetadataValueUpdate` so pane, tab, and group facts can share one collaborative update path.

**Architecture:** Shared types define the wire/storage-adjacent patch shape. The controller metadata store applies patches by source and key, assigns `updated_at` from the controller clock, and preserves omitted keys. This is internal foundation only; arbitrary external pipe input remains a follow-up.

**Tech Stack:** Rust workspace, Serde JSON shared types, native unit tests with `--target aarch64-apple-darwin`, WASM release build with `wasm32-wasip1`.

---

## Chunk 1: Patch Types And Store Application

### Task 1: Shared Patch Types

**Files:**
- Modify: `crates/andamento-shared/src/lib.rs`

- [x] **Step 1: Write failing shared serialization test**

Add a test that round-trips:

```rust
MetadataPatch {
    target: MetadataTarget::Group(group_path),
    source_id: "flotilla".to_owned(),
    set: BTreeMap::from([(
        "summary.local_llm".to_owned(),
        MetadataValueUpdate {
            value: MetadataValue::Text("running tests".to_owned()),
            ttl_ms: Some(30_000),
            precedence: Some(10),
            ordinal: Some(2),
        },
    )]),
    unset: vec!["old.summary".to_owned()],
}
```

- [x] **Step 2: Run focused test and verify failure**

Run: `cargo test -p andamento-shared metadata_patch_round_trips_json --target aarch64-apple-darwin`

Expected: compile failure because `MetadataPatch` and `MetadataValueUpdate` do not exist.

- [x] **Step 3: Implement shared types**

Add:

```rust
pub struct MetadataValueUpdate {
    pub value: MetadataValue,
    pub ttl_ms: Option<u64>,
    pub precedence: Option<i64>,
    pub ordinal: Option<i64>,
}

pub struct MetadataPatch {
    pub target: MetadataTarget,
    pub source_id: String,
    #[serde(default)]
    pub set: BTreeMap<String, MetadataValueUpdate>,
    #[serde(default)]
    pub unset: Vec<String>,
}
```

Use `BTreeMap` for deterministic serialization and test output.

- [x] **Step 4: Verify focused test passes**

Run: `cargo test -p andamento-shared metadata_patch_round_trips_json --target aarch64-apple-darwin`

Expected: pass.

### Task 2: MetadataStore Patch Application

**Files:**
- Modify: `crates/andamento-controller/src/metadata.rs`

- [x] **Step 1: Write failing store tests**

Add tests proving:

- `apply_patch` sets entries for only the patch source.
- omitted keys remain unchanged.
- `unset` removes only the patch source's key.
- `updated_at` is assigned from the `now` argument, not accepted from the patch.
- missing precedence/ordinal default to `0`.

- [x] **Step 2: Run focused tests and verify failure**

Run: `cargo test -p andamento-controller metadata_patch --target aarch64-apple-darwin`

Expected: compile failure because `MetadataStore::apply_patch` does not exist.

- [x] **Step 3: Implement `MetadataStore::apply_patch`**

Add:

```rust
pub fn apply_patch(&mut self, patch: MetadataPatch, now: u64) {
    for key in patch.unset {
        self.unset(&patch.target, &key, &patch.source_id);
    }
    for (key, update) in patch.set {
        self.set(
            patch.target.clone(),
            key,
            patch.source_id.clone(),
            MetadataEntry {
                value: update.value,
                updated_at: now,
                ttl_ms: update.ttl_ms,
                precedence: update.precedence.unwrap_or_default(),
                ordinal: update.ordinal.unwrap_or_default(),
            },
        );
    }
}
```

- [x] **Step 4: Verify focused tests pass**

Run: `cargo test -p andamento-controller metadata_patch --target aarch64-apple-darwin`

Expected: pass.

### Task 3: Full Verification And Commit

**Files:**
- Modify: `docs/sidebar-design/metadata-and-roadmap.md`
- Modify: `docs/superpowers/plans/2026-05-17-metadata-patch-semantics.md`
- Modify: `crates/andamento-shared/src/lib.rs`
- Modify: `crates/andamento-controller/src/metadata.rs`

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
git add docs/sidebar-design/metadata-and-roadmap.md docs/superpowers/plans/2026-05-17-metadata-patch-semantics.md crates/andamento-shared/src/lib.rs crates/andamento-controller/src/metadata.rs
git commit -m "feat: add metadata patch semantics"
```
