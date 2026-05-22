# Header Niche Absorption Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let group headers absorb a prefix of child tabs or one child-group path into their spare inline width.

**Architecture:** Keep the behavior rail-local as a render projection over `RenderNode`. Reuse the existing group-header inline layout and compact segment rendering instead of creating a second segment renderer. The projection returns absorbed header text/hits plus the remaining children to render below, preserving the underlying tree and child order.

**Tech Stack:** Rust 2021, `andamento-rail` renderer unit tests, existing `InlineRun`, existing compact segment renderer, `unicode-width`, native `aarch64-apple-darwin` tests, wasm32 release build verification.

---

## File Structure

- Modify: `crates/andamento-rail/src/render.rs`
  - Add the niche projection data structures near the compact strip helpers.
  - Extend group-header rendering to expose a niche insertion point.
  - Reuse compact segment rendering for absorbed same-level tab runs.
  - Render leftover direct tabs/child groups below the header.

- Modify: `docs/sidebar-design/metadata-and-roadmap.md`
  - Record the conservative first-pass niche behavior after implementation.

## Chunk 1: Direct Tab Prefix Absorption

### Task 1: Absorb Direct Tabs Into The Header Niche

**Files:**
- Modify: `crates/andamento-rail/src/render.rs`

- [ ] **Step 1: Write a failing render test**

Add a test near the compact-strip tests:

```rust
#[test]
fn group_header_absorbs_direct_tab_prefix_into_header_niche() {
    let mut model = mixed_child_group_model();
    let parent_path = match &model.rows[0] {
        RailRow::GroupHeader { path, .. } => path.clone(),
        _ => panic!("first row should be parent group"),
    };
    model.resolved_metadata = vec![ResolvedMetadata {
        target: MetadataTarget::Group(parent_path),
        values: BTreeMap::from([(
            "rail.child_layout".to_owned(),
            MetadataEntry {
                value: MetadataValue::Text("compact-strip".to_owned()),
                updated_at: 1,
                ttl_ms: None,
                precedence: 0,
                ordinal: 0,
            },
        )]),
        source_entries: BTreeMap::new(),
        reachable_identities: vec![],
    }];

    let rendered = render_lines_with_theme(Some(&model), &[], 8, 72, true, Some(test_theme()));

    assert!(
        rendered.lines[0].contains("repo-overview"),
        "first direct tab should be rendered in the parent header niche: {:?}",
        rendered.lines
    );
    assert!(
        !rendered.lines[1].contains("repo-overview"),
        "absorbed direct tab should not be repeated below the header: {:?}",
        rendered.lines
    );
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib group_header_absorbs_direct_tab_prefix_into_header_niche
```

Expected: fail because direct tabs still render only below the header.

- [ ] **Step 3: Add a bounded niche result type**

Add small internal structs:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeaderNicheProjection {
    text: String,
    visible_width: usize,
    hits: Vec<CompactSegmentHitBox>,
    consumed_tabs: usize,
}
```

Add a helper that receives `direct_tabs`, `width`, `theme`, and `template_catalog`. It should try to render the direct tabs as one compact segment run with the available niche width. For the first pass, consume only the segments that actually produced hits on the first rendered row.

- [ ] **Step 4: Insert the niche projection into group headers**

Change the compact-strip branch in `render_nodes_to_buffer`:

- split `direct_tabs` and `remaining_children` before appending the group header.
- pass the direct tabs to `append_group_header`.
- make `append_group_header` insert the niche text before the trailing filler/tail.
- register absorbed tab hit boxes on the header row.
- render only unabsorbed direct tabs below in the compact strip.

Keep the behavior disabled when the effective child layout is not `CompactStrip`.

- [ ] **Step 5: Run the focused test**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib group_header_absorbs_direct_tab_prefix_into_header_niche
```

Expected: pass.

- [ ] **Step 6: Run compact/group tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib compact
cargo test --target aarch64-apple-darwin -p andamento-rail --lib group_header
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/andamento-rail/src/render.rs
git commit -m "feat: absorb direct tabs into group header niche"
```

## Chunk 2: One Child Group Path

### Task 2: Absorb One Child Group When No Direct Tabs Were Absorbed

**Files:**
- Modify: `crates/andamento-rail/src/render.rs`

- [ ] **Step 1: Write a failing render test**

Add a test with a parent group that has no direct tabs and one child group with direct tabs. Assert that the parent header contains the child group label and the first child tab segment, and that the absorbed child tab is not repeated immediately below.

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib group_header_absorbs_one_child_group_path_into_niche
```

Expected: fail because child groups render only as separate headers below.

- [ ] **Step 3: Add recursive absorption for one group path**

Extend the niche helper so:

- if no direct tabs fit at this level, consider the first child group.
- render a compact group fragment using the child group's resolved header label without its collapse glyph.
- if the child group is expanded and width remains, recursively offer the remaining width to its children.
- separate group fragments and descendant tab runs with the border character.
- return the child group with consumed descendants removed for normal rendering below.

- [ ] **Step 4: Preserve collapsed behavior**

Add or update a test proving a collapsed absorbed child group does not absorb its children.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib niche
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/andamento-rail/src/render.rs
git commit -m "feat: absorb one child group path into header niche"
```

## Chunk 3: Docs And Verification

### Task 3: Document The First-Pass Niche Semantics

**Files:**
- Modify: `docs/sidebar-design/metadata-and-roadmap.md`

- [ ] **Step 1: Update docs**

Record:

- direct tab prefixes can be absorbed as one segment run.
- when direct tabs are absorbed, sibling child groups are not absorbed at that level.
- if no direct tabs are absorbed, one child group path can be absorbed recursively.
- collapsed groups do not donate descendants.
- overflow remains in normal child rendering.

- [ ] **Step 2: Run diff check**

Run:

```bash
git diff --check -- docs/sidebar-design/metadata-and-roadmap.md
```

Expected: no output.

- [ ] **Step 3: Commit**

```bash
git add docs/sidebar-design/metadata-and-roadmap.md
git commit -m "docs: describe header niche absorption semantics"
```

### Task 4: Final Verification

- [ ] **Step 1: Run native tests and wasm build**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-shared --lib
cargo test --target aarch64-apple-darwin -p andamento-controller --lib
cargo test --target aarch64-apple-darwin -p andamento-config --lib
cargo test --target aarch64-apple-darwin -p andamento-rail --lib
cargo build --workspace --target wasm32-wasip1 --release -j1
git diff --check
```

Expected: all commands exit 0. The existing controller Cargo target warning may still appear.

- [ ] **Step 2: Manual smoke**

Run the standard layout if practical and check:

- compact child layout still looks like the Zellij tabbar.
- absorbed tabs can be clicked.
- overflow tabs still render below.
- groups remain collapsed when explicitly collapsed.

If not run, report that explicitly.
