# Inline Layout System Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a small Andamento-owned inline layout system that can replace special-case group header and compact tab-strip packing before implementing header niche absorption.

**Architecture:** Keep ratatui for rectangular config/plugin pages, but keep rail layout policy custom. Introduce a pure inline model in `andamento-rail` first, because the first consumers need rail-only state such as `HitAction`, `RenderTheme`, and metadata field provenance. The inline layer should produce styled text, visible-width accounting, source provenance, and exact hit regions without knowing about Zellij tabs, groups, or recursive render nodes.

**Tech Stack:** Rust 2021, existing `andamento-rail` renderer, `andamento-shared::segment_bar` for current Zellij tab-ribbon glyph conventions, `unicode-width`, `ansi_term`, existing native unit tests, and `wasm32-wasip1` release build verification.

---

## File Structure

- Create: `crates/andamento-rail/src/inline_layout.rs`
  - Own the pure inline model and packer.
  - Define inline items, fitting policies, placed items, source provenance, and local hit payloads.
  - Keep this file free of Zellij plugin APIs and recursive rail node concepts.

- Modify: `crates/andamento-rail/src/lib.rs`
  - Add `mod inline_layout;` if the crate root is the module owner.

- Modify: `crates/andamento-rail/src/render.rs`
  - Use `inline_layout` for group header field packing first.
  - Use `inline_layout` for compact tab strip packing second.
  - Keep existing `HitRegion`, `VisibleCard`, render-node projection, and image/status code in place.

- Modify: `crates/andamento-shared/src/segment_bar.rs`
  - Only if needed to expose shared segment constants or style vocabulary.
  - Do not move rail-specific layout policy into shared code.

- Modify: `docs/sidebar-design/metadata-and-roadmap.md`
  - Record the inline layout vocabulary and current status after implementation.

## Current Constraints

- Existing uncommitted changes may include:
  - `layouts/andamento.kdl`
  - `layouts/andamento-native.kdl`
  - `crates/andamento-rail/src/render.rs`
  - `docs/sidebar-design/metadata-and-roadmap.md`
- Do not revert those changes.
- The inline slice should not implement header niche absorption yet.
- The visible output should stay stable except where tests explicitly cover an intentional visual change.
- Do not move the rail renderer wholesale to ratatui.
- Do not add a config surface for inline layout yet.

## Chunk 1: Pure Inline Model

### Task 1: Create Inline Item Types And Basic Packing

**Files:**
- Create: `crates/andamento-rail/src/inline_layout.rs`
- Modify: `crates/andamento-rail/src/lib.rs`

- [ ] **Step 1: Write failing tests for full-fit inline text**

Add tests in `inline_layout.rs`:

```rust
#[test]
fn inline_run_keeps_full_text_when_it_fits() {
    let run = InlineRun::new(vec![
        InlineItem::text("toggle", "▼").required(),
        InlineItem::text("label", "project-a").required(),
        InlineItem::text("count", "(2)").optional(),
    ]);

    let placed = run.layout(20);

    assert_eq!(placed.text, "▼ project-a (2)");
    assert_eq!(placed.visible_width, 15);
    assert_eq!(placed.items.len(), 3);
    assert_eq!(placed.items[0].id, "toggle");
    assert_eq!(placed.items[0].cols, 0..1);
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib inline_run_keeps_full_text_when_it_fits
```

Expected: fails because `inline_layout` does not exist.

- [ ] **Step 3: Implement minimal inline structs**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRun {
    items: Vec<InlineItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineItem {
    pub id: String,
    pub text: String,
    pub class: InlineClass,
    pub priority: i64,
    pub min_width: usize,
    pub compact_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineClass {
    Required,
    Optional,
    Priority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedInlineRun {
    pub text: String,
    pub visible_width: usize,
    pub items: Vec<PlacedInlineItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedInlineItem {
    pub id: String,
    pub cols: std::ops::Range<usize>,
}
```

Implement a simple `layout(width)` that joins included items with a single space, except when an item text starts with `:`.

- [ ] **Step 4: Run the test and confirm pass**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib inline_run_keeps_full_text_when_it_fits
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/andamento-rail/src/inline_layout.rs crates/andamento-rail/src/lib.rs
git commit -m "feat: add rail inline layout model"
```

### Task 2: Add Priority Drop And Truncation

**Files:**
- Modify: `crates/andamento-rail/src/inline_layout.rs`

- [ ] **Step 1: Write failing tests for optional dropping**

```rust
#[test]
fn inline_run_drops_optional_items_before_priority_items() {
    let run = InlineRun::new(vec![
        InlineItem::text("toggle", "▼").required(),
        InlineItem::text("label", "project-a").required(),
        InlineItem::text("count", "(2)").optional(),
        InlineItem::text("active", ": agent-1").priority(80),
    ]);

    let placed = run.layout(18);

    assert_eq!(placed.text, "▼ project-a: agent-1");
    assert!(!placed.items.iter().any(|item| item.id == "count"));
    assert!(placed.items.iter().any(|item| item.id == "active"));
}
```

- [ ] **Step 2: Write failing tests for low-priority truncation**

```rust
#[test]
fn inline_run_truncates_lowest_priority_item_before_dropping_it() {
    let run = InlineRun::new(vec![
        InlineItem::text("toggle", "▼").required(),
        InlineItem::text("repo", "flotilla-org/flotilla").priority(100),
        InlineItem::text("branch", "feat/very-long-branch-name").priority(60),
    ]);

    let placed = run.layout(28);

    assert!(placed.text.starts_with("▼ flotilla-org/flotilla "));
    assert!(placed.text.ends_with("…"));
    assert!(placed.items.iter().any(|item| item.id == "branch"));
    assert!(placed.visible_width <= 28);
}
```

- [ ] **Step 3: Run tests and confirm failure**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib inline_run_
```

Expected: new tests fail.

- [ ] **Step 4: Implement fitting policy**

Implement:

- full render if it fits.
- compact text substitution if provided and full text does not fit.
- truncate lowest-priority included item down to its `min_width`.
- drop optional items before priority items.
- drop priority items from lowest numeric priority upward.
- never drop required items; truncate required items only as the final fallback.

Use existing width helpers from `render.rs` as the reference behavior:

- `unicode_width::UnicodeWidthStr`
- `truncate_to_width`

If moving `truncate_to_width` would cause churn, duplicate a tiny local helper in `inline_layout.rs` for this slice and consolidate later.

- [ ] **Step 5: Run inline tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib inline_run_
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/andamento-rail/src/inline_layout.rs
git commit -m "feat: fit inline items by priority"
```

## Chunk 2: Hit Regions And Styling

### Task 3: Preserve Inline Hit Payloads

**Files:**
- Modify: `crates/andamento-rail/src/inline_layout.rs`

- [ ] **Step 1: Write failing tests for hit payload preservation**

```rust
#[test]
fn inline_run_keeps_hit_payloads_for_visible_items() {
    let run = InlineRun::new(vec![
        InlineItem::text("toggle", "▼")
            .required()
            .hit(InlineHit::GroupToggle),
        InlineItem::text("inspect", "◐")
            .required()
            .hit(InlineHit::InspectNode),
    ]);

    let placed = run.layout(8);

    assert_eq!(placed.items[0].hit, Some(InlineHit::GroupToggle));
    assert_eq!(placed.items[0].cols, 0..1);
    assert_eq!(placed.items[1].hit, Some(InlineHit::InspectNode));
}
```

- [ ] **Step 2: Implement `InlineHit`**

Add a small local enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineHit {
    GroupToggle,
    InspectNode,
    SwitchTab { index: usize },
}
```

Do not expose `HitAction` from `render.rs` to the inline module. Convert `InlineHit` to `HitRegion` at call sites.

- [ ] **Step 3: Run inline tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib inline_run_keeps_hit_payloads_for_visible_items
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/andamento-rail/src/inline_layout.rs
git commit -m "feat: carry inline hit payloads"
```

### Task 4: Add Styled Text Runs Without Changing Layout Decisions

**Files:**
- Modify: `crates/andamento-rail/src/inline_layout.rs`
- Modify: `crates/andamento-rail/src/render.rs`

- [ ] **Step 1: Write failing test for ANSI-styled visible width**

```rust
#[test]
fn inline_run_counts_visible_width_without_ansi() {
    let styled = "\u{1b}[1;38;5;2mproject\u{1b}[0m";
    let run = InlineRun::new(vec![InlineItem::styled_text("label", styled, "project").required()]);

    let placed = run.layout(7);

    assert_eq!(placed.visible_width, 7);
    assert_eq!(visible_width_without_ansi(&placed.text), 7);
}
```

- [ ] **Step 2: Implement styled text support**

Add enough shape for styled items:

```rust
pub struct InlineItem {
    pub text: String,
    pub measure_text: String,
    ...
}
```

Use `measure_text` for width and `text` for output.

- [ ] **Step 3: Run rail tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib inline_run_counts_visible_width_without_ansi
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/andamento-rail/src/inline_layout.rs crates/andamento-rail/src/render.rs
git commit -m "feat: support styled inline items"
```

## Chunk 3: Port Group Headers

### Task 5: Render Group Header Fields Through Inline Layout

**Files:**
- Modify: `crates/andamento-rail/src/render.rs`
- Modify: `crates/andamento-rail/src/inline_layout.rs`

- [ ] **Step 1: Add regression tests for current group header output**

Use existing tests as anchors and add one explicit test if needed:

```rust
#[test]
fn group_header_inline_layout_preserves_toggle_label_count_and_filler() {
    let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

    assert!(rendered.lines[0].starts_with("▼ zellij (2)"));
    assert!(rendered.lines[0].contains("──"));
    assert_eq!(hit_at(&rendered.hit_regions, 0, 0).map(|hit| hit.action), Some(HitAction::ToggleGroup));
}
```

- [ ] **Step 2: Run test before changes**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib group_header_inline_layout_preserves_toggle_label_count_and_filler
```

Expected: pass before refactor.

- [ ] **Step 3: Replace `render_template_fields_with_suppression` header call path**

In `group_header_line`:

- convert `TemplateField` values to `InlineItem`s.
- keep visible source provenance on the inline item.
- let inline layout choose fitted items.
- keep right-side filler `─` behavior.
- keep `style_group_header_text`.
- return `visible_sources` from placed inline items.

Do not remove old template field fitting helpers yet; tab status/title still use them.

- [ ] **Step 4: Run rail tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib group_header
cargo test --target aarch64-apple-darwin -p andamento-rail --lib descendant_group_header_elides_metadata_field_rendered_by_ancestor
```

Expected: pass.

- [ ] **Step 5: Run all rail tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/andamento-rail/src/render.rs crates/andamento-rail/src/inline_layout.rs
git commit -m "refactor: render group headers with inline layout"
```

## Chunk 4: Port Compact Tab Strips

### Task 6: Express Compact Tab Segments As Inline Items

**Files:**
- Modify: `crates/andamento-rail/src/render.rs`
- Modify: `crates/andamento-rail/src/inline_layout.rs`

- [ ] **Step 1: Add regression tests for compact tab strip output and hits**

Existing tests to preserve:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib group_child_layout_metadata_can_render_direct_tabs_as_compact_strip
cargo test --target aarch64-apple-darwin -p andamento-rail --lib themed_compact_strip_uses_exact_separator_with_active_inactive_and_between_colors
cargo test --target aarch64-apple-darwin -p andamento-rail --lib rail_config_overrides_compact_segment_between_color
```

Expected: pass before refactor.

- [ ] **Step 2: Add an inline segment item type**

Extend `InlineItem` with a semantic kind:

```rust
pub enum InlineItemKind {
    Text,
    Symbol,
    Segment,
    Spacer,
    ImagePlaceholder,
}
```

For this task, implement only `Segment` rendering through the existing `render_compact_segment` behavior in `render.rs`. The inline module should decide placement and wrapping; render.rs can still style the placed segment.

- [ ] **Step 3: Replace `render_compact_segment_run` placement logic**

Keep `render_compact_segment` or an equivalent styling helper, but remove duplicated wrapping/width decisions from compact tab strips:

- build `InlineItem::segment` per tab.
- pack/wrap with the inline layout system.
- convert placed segment hits into `HitRegion { action: HitAction::SwitchTab }`.
- keep exact `` glyph behavior.
- keep themed active/inactive/between colors.

- [ ] **Step 4: Run compact strip tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib compact
cargo test --target aarch64-apple-darwin -p andamento-rail --lib segment
```

Expected: pass.

- [ ] **Step 5: Run all rail tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/andamento-rail/src/render.rs crates/andamento-rail/src/inline_layout.rs
git commit -m "refactor: render compact tab strips with inline layout"
```

## Chunk 5: Documentation And Verification

### Task 7: Document Inline Layout Vocabulary

**Files:**
- Modify: `docs/sidebar-design/metadata-and-roadmap.md`

- [ ] **Step 1: Add roadmap status under render/layout sections**

Document:

- inline items are the unit for header-row layout.
- `body`, `content`, and `children` remain separate from inline layout.
- header niche absorption is not implemented yet.
- ratatui remains config-focused; rail uses custom inline/block projection.
- future inline kinds include focus buttons, action buttons, collapse alternatives, image placeholders, and priority area chips.

- [ ] **Step 2: Run markdown whitespace check**

Run:

```bash
git diff --check -- docs/sidebar-design/metadata-and-roadmap.md
```

Expected: no output and exit 0.

- [ ] **Step 3: Commit docs**

```bash
git add docs/sidebar-design/metadata-and-roadmap.md
git commit -m "docs: describe rail inline layout vocabulary"
```

### Task 8: Full Verification

**Files:**
- No code changes unless verification finds a bug.

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-rail --lib
```

Expected: all rail tests pass.

- [ ] **Step 2: Run workspace verification**

Run:

```bash
cargo test --target aarch64-apple-darwin -p andamento-shared --lib
cargo test --target aarch64-apple-darwin -p andamento-controller --lib
cargo test --target aarch64-apple-darwin -p andamento-config --lib
cargo test --target aarch64-apple-darwin -p andamento-rail --lib
cargo build --workspace --target wasm32-wasip1 --release -j1
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 3: Manual smoke test in Zellij**

Run the standard layout:

```bash
ANDAMENTO_ROOT=/Users/robert/dev/andamento \
cargo run --manifest-path /Users/robert/dev/zellij/Cargo.toml --profile dev-opt -- \
  --layout /Users/robert/dev/andamento/layouts/andamento.kdl
```

Check:

- group headers still show collapse glyph, label, count, and filler.
- conflated labels still use `─`.
- inspecting group glyph still works.
- compact child layout still renders `` tab segments.
- compact segment clicks still switch tabs.

- [ ] **Step 4: Final commit if smoke test required fixes**

Only commit if Step 3 required changes:

```bash
git add <changed-files>
git commit -m "fix: preserve rail inline layout behavior"
```

## Out Of Scope For This Plan

- Header niche absorption.
- New config toggles for action/focus mode.
- Image-backed buttons or kitty placements.
- Moving the rail to ratatui.
- External template schema changes.
- Priority/pinned duplicate/suppress policy.
- Body/content recursive layout.

## Success Criteria

- Group headers and compact tab strips both use the inline layout substrate.
- Existing visual output is preserved where not explicitly changed.
- Hit regions remain exact for group toggle, inspect glyph, and compact tab segments.
- Inline layout is pure enough to unit test without Zellij state.
- The rail has a clear next step toward header niche absorption without adding another special-case renderer.
