# Directory Grouping Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional directory-grouped tab projection to the Zellij tabs sidebar, using exact pane cwd metadata as the first end-to-end metadata/grouping slice.

**Architecture:** Keep metadata collection, grouping, ordering, and rendering separate. `tabs-controller` owns Zellij state ingestion, metadata storage, primary cwd selection, and view-model projection; `tabs-rail` renders the projected rows and keeps tab rows clickable. `tabs-shared` carries the wire types so rail renderers do not need controller internals.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `zellij-tile`, existing workspace crates `tabs-shared`, `tabs-controller`, and `tabs-rail`.

---

## File Structure

- Modify: `crates/tabs-shared/src/lib.rs`
  - Add metadata value/entry types.
  - Add grouping config.
  - Add projection row wire types.
  - Extend `ControllerViewModel`.

- Create: `crates/tabs-controller/src/metadata.rs`
  - Generic metadata store.
  - Metadata patch application.
  - Generic `precedence -> count -> ordinal` selector.
  - Unit tests for patch and selection behavior.

- Modify: `crates/tabs-controller/src/state.rs`
  - Track pane records with tab id, focus/selectability, kind, and ordinal.
  - Store cwd metadata as `zellij.pane.cwd` entries from source `zellij`.
  - Build a grouped/ungrouped projection for the view model.
  - Unit tests for tab primary cwd and grouped projection.

- Modify: `crates/tabs-controller/src/main.rs`
  - Parse `rail_grouping`.
  - Subscribe to `CwdChanged`.
  - Query initial cwd for terminal panes after `PaneUpdate`.
  - Push updated view models after cwd changes.

- Modify: `crates/tabs-rail/src/render.rs`
  - Convert `ControllerViewModel.rows` into renderable rows.
  - Render group headers as non-clickable rows.
  - Render grouped child tabs as indented tab cards.
  - Keep ungrouped tabs as normal tab cards.
  - Add tests for header rendering, click behavior, and active-tab visibility.

- Modify: `crates/tabs-rail/src/main.rs`
  - No major behavior change expected; keep using `rendered.hit_regions` and `visible_cards`.

- Optional modify: `README.md`
  - Document `rail_grouping="none|directory"` after behavior is working.

## Chunk 1: Shared Wire Types

### Task 1: Add Metadata And Projection Types

**Files:**
- Modify: `crates/tabs-shared/src/lib.rs`

- [ ] **Step 1: Add failing tests for config and projection JSON**

Add tests near the existing `rail_config_*` tests:

```rust
#[test]
fn rail_config_defaults_to_directory_grouping_off() {
    let config = RailConfig::default();

    assert_eq!(config.grouping, RailGroupingMode::None);
}

#[test]
fn controller_view_model_with_group_rows_round_trips_json() {
    let model = ControllerViewModel {
        sort_mode: SortMode::Position,
        config: RailConfig {
            structure: RailStructure::JoinedCells,
            sizing: RailSizingPreset::Compact,
            grouping: RailGroupingMode::Directory,
        },
        tabs: vec![TabCard {
            tab_id: 1,
            position: 0,
            name: "server".to_owned(),
            active: true,
            pinned: false,
            status: None,
        }],
        rows: vec![
            RailRow::GroupHeader {
                group_id: "cwd:/Users/robert/dev/zellij".to_owned(),
                label: "zellij".to_owned(),
                full_label: "/Users/robert/dev/zellij".to_owned(),
                tab_count: 1,
            },
            RailRow::Tab {
                tab: TabCard {
                    tab_id: 1,
                    position: 0,
                    name: "server".to_owned(),
                    active: true,
                    pinned: false,
                    status: None,
                },
                indent: 2,
            },
        ],
    };

    let encoded = serde_json::to_string(&model).unwrap();
    let decoded: ControllerViewModel = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, model);
}
```

- [ ] **Step 2: Run shared crate tests and confirm failure**

Run:

```bash
cargo test -p tabs-shared
```

Expected: fails because `RailGroupingMode`, `RailRow`, `RailConfig.grouping`, and `ControllerViewModel.rows` do not exist.

- [ ] **Step 3: Add the minimal shared types**

Add these types in `crates/tabs-shared/src/lib.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum MetadataValue {
    Text(String),
    Bool(bool),
    Integer(i64),
    StringList(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataEntry {
    pub value: MetadataValue,
    pub updated_at: u64,
    pub ttl_ms: Option<u64>,
    pub precedence: i64,
    pub ordinal: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RailGroupingMode {
    None,
    Directory,
}

impl Default for RailGroupingMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RailRow {
    GroupHeader {
        group_id: String,
        label: String,
        full_label: String,
        tab_count: usize,
    },
    Tab {
        tab: TabCard,
        indent: usize,
    },
}
```

Replace the derived `Default` for `RailConfig` with an explicit implementation if needed:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailConfig {
    #[serde(default)]
    pub structure: RailStructure,
    #[serde(default)]
    pub sizing: RailSizingPreset,
    #[serde(default)]
    pub grouping: RailGroupingMode,
}

impl Default for RailConfig {
    fn default() -> Self {
        Self {
            structure: RailStructure::default(),
            sizing: RailSizingPreset::default(),
            grouping: RailGroupingMode::default(),
        }
    }
}
```

Extend `ControllerViewModel`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerViewModel {
    pub sort_mode: SortMode,
    pub config: RailConfig,
    pub tabs: Vec<TabCard>,
    #[serde(default)]
    pub rows: Vec<RailRow>,
}
```

- [ ] **Step 4: Update existing tests that construct `ControllerViewModel`**

Add `rows: vec![]` to existing literals in:

- `crates/tabs-shared/src/lib.rs`
- `crates/tabs-rail/src/render.rs`

- [ ] **Step 5: Run shared crate tests**

Run:

```bash
cargo test -p tabs-shared
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tabs-shared/src/lib.rs crates/tabs-rail/src/render.rs
git commit -m "feat: add rail grouping view model types"
```

## Chunk 2: Metadata Store And Generic Selection

### Task 2: Implement Metadata Store

**Files:**
- Create: `crates/tabs-controller/src/metadata.rs`
- Modify: `crates/tabs-controller/src/main.rs`

- [ ] **Step 1: Add module declaration**

In `crates/tabs-controller/src/main.rs`, add:

```rust
mod metadata;
```

- [ ] **Step 2: Write metadata tests**

Create `crates/tabs-controller/src/metadata.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tabs_shared::{MetadataEntry, MetadataValue};

    fn entry(value: &str, precedence: i64, ordinal: i64, updated_at: u64) -> MetadataEntry {
        MetadataEntry {
            value: MetadataValue::Text(value.to_owned()),
            updated_at,
            ttl_ms: None,
            precedence,
            ordinal,
        }
    }

    #[test]
    fn unset_removes_only_that_source_key() {
        let mut store = MetadataStore::default();
        store.set(EntityId::Pane("terminal:1".to_owned()), "zellij.pane.cwd", "zellij", entry("/a", 0, 0, 1));
        store.set(EntityId::Pane("terminal:1".to_owned()), "zellij.pane.cwd", "shell", entry("/b", 0, 0, 2));

        store.unset(&EntityId::Pane("terminal:1".to_owned()), "zellij.pane.cwd", "zellij");

        let entries = store.entries_for(&EntityId::Pane("terminal:1".to_owned()), "zellij.pane.cwd", 3);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "shell");
    }

    #[test]
    fn selector_prefers_precedence_before_count() {
        let entries = vec![
            CandidateEntry::new("a", entry("/same", 0, 0, 1)),
            CandidateEntry::new("b", entry("/same", 0, 1, 1)),
            CandidateEntry::new("c", entry("/focused", 10, 2, 1)),
        ];

        assert_eq!(select_primary_value(&entries), Some(MetadataValue::Text("/focused".to_owned())));
    }

    #[test]
    fn selector_uses_count_inside_precedence_bucket() {
        let entries = vec![
            CandidateEntry::new("a", entry("/one", 0, 0, 1)),
            CandidateEntry::new("b", entry("/two", 0, 1, 1)),
            CandidateEntry::new("c", entry("/two", 0, 2, 1)),
        ];

        assert_eq!(select_primary_value(&entries), Some(MetadataValue::Text("/two".to_owned())));
    }

    #[test]
    fn selector_uses_ordinal_after_count() {
        let entries = vec![
            CandidateEntry::new("a", entry("/later", 0, 10, 1)),
            CandidateEntry::new("b", entry("/earlier", 0, 2, 1)),
        ];

        assert_eq!(select_primary_value(&entries), Some(MetadataValue::Text("/earlier".to_owned())));
    }

    #[test]
    fn expired_entries_are_not_returned() {
        let mut store = MetadataStore::default();
        let mut value = entry("/a", 0, 0, 10);
        value.ttl_ms = Some(5);
        store.set(EntityId::Pane("terminal:1".to_owned()), "zellij.pane.cwd", "zellij", value);

        assert!(store.entries_for(&EntityId::Pane("terminal:1".to_owned()), "zellij.pane.cwd", 16).is_empty());
    }
}
```

- [ ] **Step 3: Run controller tests and confirm failure**

Run:

```bash
cargo test -p tabs-controller
```

Expected: fails because metadata types are not implemented.

- [ ] **Step 4: Implement metadata store**

Implement:

```rust
use std::collections::{BTreeMap, HashMap};

use tabs_shared::{MetadataEntry, MetadataValue};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntityId {
    Pane(String),
    Tab(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEntry {
    pub source_id: String,
    pub entry: MetadataEntry,
}

impl CandidateEntry {
    pub fn new(source_id: impl Into<String>, entry: MetadataEntry) -> Self {
        Self { source_id: source_id.into(), entry }
    }
}

#[derive(Debug, Default)]
pub struct MetadataStore {
    entries: HashMap<EntityId, HashMap<String, BTreeMap<String, MetadataEntry>>>,
}
```

Required methods:

- `set(entity, key, source_id, entry)`
- `unset(entity, key, source_id)`
- `entries_for(entity, key, now) -> Vec<CandidateEntry>`
- `select_primary_value(entries: &[CandidateEntry]) -> Option<MetadataValue>`

Selection rules:

1. filter to the highest `entry.precedence`.
2. count values in that bucket.
3. choose the highest count.
4. choose the lowest `entry.ordinal`.
5. deterministic fallback by value debug string or source id.

Expiry rule:

```rust
entry.ttl_ms
    .map(|ttl| now <= entry.updated_at.saturating_add(ttl))
    .unwrap_or(true)
```

- [ ] **Step 5: Run controller tests**

Run:

```bash
cargo test -p tabs-controller
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tabs-controller/src/main.rs crates/tabs-controller/src/metadata.rs
git commit -m "feat: add generic metadata selection"
```

## Chunk 3: Controller Pane Cwd Collection And Projection

### Task 3: Track Pane Cwd Metadata

**Files:**
- Modify: `crates/tabs-controller/src/state.rs`

- [ ] **Step 1: Add failing tests for primary cwd selection**

Add tests in `state.rs`:

```rust
#[test]
fn focused_pane_cwd_beats_more_common_cwd_for_tab_grouping() {
    let mut state = ControllerState::default();
    state.set_rail_config(RailConfig {
        grouping: RailGroupingMode::Directory,
        ..RailConfig::default()
    });
    state.update_tabs(vec![ControllerTab { tab_id: 1, position: 0, name: "work".into(), active: true }]);
    state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
    state.set_test_pane(PaneTarget::Terminal(11), 1, true, false, 1);
    state.set_test_pane(PaneTarget::Terminal(12), 1, true, true, 2);
    state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/common".into());
    state.set_pane_cwd(PaneTarget::Terminal(11), "/repo/common".into());
    state.set_pane_cwd(PaneTarget::Terminal(12), "/repo/focused".into());

    let model = state.view_model();

    assert!(matches!(
        &model.rows[0],
        RailRow::GroupHeader { full_label, .. } if full_label == "/repo/focused"
    ));
}

#[test]
fn count_breaks_ties_within_same_precedence() {
    let mut state = ControllerState::default();
    state.set_rail_config(RailConfig {
        grouping: RailGroupingMode::Directory,
        ..RailConfig::default()
    });
    state.update_tabs(vec![ControllerTab { tab_id: 1, position: 0, name: "work".into(), active: true }]);
    state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
    state.set_test_pane(PaneTarget::Terminal(11), 1, true, false, 1);
    state.set_test_pane(PaneTarget::Terminal(12), 1, true, false, 2);
    state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());
    state.set_pane_cwd(PaneTarget::Terminal(11), "/repo/b".into());
    state.set_pane_cwd(PaneTarget::Terminal(12), "/repo/b".into());

    let model = state.view_model();

    assert!(matches!(
        &model.rows[0],
        RailRow::GroupHeader { full_label, .. } if full_label == "/repo/b"
    ));
}
```

- [ ] **Step 2: Add failing tests for projection rows**

Add:

```rust
#[test]
fn directory_grouping_compacts_tabs_and_leaves_missing_cwd_ungrouped() {
    let mut state = ControllerState::default();
    state.set_rail_config(RailConfig {
        grouping: RailGroupingMode::Directory,
        ..RailConfig::default()
    });
    state.update_tabs(vec![
        ControllerTab { tab_id: 1, position: 0, name: "one".into(), active: false },
        ControllerTab { tab_id: 2, position: 1, name: "two".into(), active: true },
        ControllerTab { tab_id: 3, position: 2, name: "three".into(), active: false },
    ]);
    state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
    state.set_test_pane(PaneTarget::Terminal(30), 3, true, false, 0);
    state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());
    state.set_pane_cwd(PaneTarget::Terminal(30), "/repo/a".into());

    let model = state.view_model();

    assert_eq!(model.rows.len(), 4);
    assert!(matches!(&model.rows[0], RailRow::GroupHeader { tab_count: 2, .. }));
    assert!(matches!(&model.rows[1], RailRow::Tab { tab, indent: 2 } if tab.tab_id == 1));
    assert!(matches!(&model.rows[2], RailRow::Tab { tab, indent: 2 } if tab.tab_id == 3));
    assert!(matches!(&model.rows[3], RailRow::Tab { tab, indent: 0 } if tab.tab_id == 2));
}

#[test]
fn grouping_none_returns_flat_rows() {
    let mut state = ControllerState::default();
    state.update_tabs(vec![ControllerTab {
        tab_id: 1,
        position: 0,
        name: "one".into(),
        active: true,
    }]);

    let model = state.view_model();

    assert_eq!(model.rows.len(), 1);
    assert!(matches!(&model.rows[0], RailRow::Tab { tab, indent: 0 } if tab.tab_id == 1));
}
```

- [ ] **Step 3: Run controller tests and confirm failure**

Run:

```bash
cargo test -p tabs-controller
```

Expected: fails because pane records, cwd metadata, and projection rows are not implemented.

- [ ] **Step 4: Add pane records and cwd metadata**

In `state.rs`:

- Import `metadata::{EntityId, MetadataStore, select_primary_value}` and shared types.
- Add constants:

```rust
const SOURCE_ZELLIJ: &str = "zellij";
const KEY_PANE_CWD: &str = "zellij.pane.cwd";
const FOCUSED_CWD_PRECEDENCE: i64 = 100;
const NORMAL_CWD_PRECEDENCE: i64 = 0;
```

- Add a private pane record:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct ControllerPane {
    pane_id: PaneTarget,
    tab_id: u64,
    is_selectable: bool,
    is_focused: bool,
    ordinal: i64,
    cwd: Option<String>,
}
```

- Extend `ControllerState`:

```rust
panes: HashMap<PaneTarget, ControllerPane>,
metadata: MetadataStore,
```

- In `update_panes_from_manifest`, record panes with `enumerate()` as `ordinal`.
- Ignore plugin and non-selectable panes when emitting cwd candidates.
- Add:

```rust
pub fn set_pane_cwd(&mut self, pane_id: PaneTarget, cwd: String) {
    if let Some(pane) = self.panes.get_mut(&pane_id) {
        pane.cwd = Some(cwd);
    }
    self.refresh_pane_cwd_metadata(pane_id);
}
```

- Add `refresh_pane_cwd_metadata(pane_id)` that writes or unsets `KEY_PANE_CWD` on `EntityId::Pane(format_pane_id(pane_id))`.
- Assign `updated_at` from `receive_counter` after incrementing it.
- Set `precedence` from pane focus.
- Set `ordinal` from pane order.

For tests, add `set_test_pane` behind `#[cfg(test)]`.

- [ ] **Step 5: Build projection rows**

Refactor `view_model()`:

1. Build and sort `Vec<TabCard>` exactly as today.
2. Build `rows` from those cards:
   - if `self.rail_config.grouping == RailGroupingMode::None`, return one `RailRow::Tab { indent: 0 }` per card.
   - if `Directory`, compute each tab primary cwd from pane metadata.
   - groups compact at first occurrence.
   - tabs with no cwd become ungrouped `RailRow::Tab { indent: 0 }`.
   - grouped child tabs use `indent: 2`.

Suggested helpers:

```rust
fn tab_primary_cwd(&self, tab_id: u64) -> Option<String>
fn rows_for_tabs(&self, tabs: &[TabCard]) -> Vec<RailRow>
fn directory_group_rows(&self, tabs: &[TabCard]) -> Vec<RailRow>
fn group_label_for_cwd(&self, cwd: &str, all_group_cwds: &[String]) -> String
```

For milestone 1, `group_label_for_cwd` can return basename, with parent fallback if duplicate basenames exist.

- [ ] **Step 6: Run controller tests**

Run:

```bash
cargo test -p tabs-controller
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/tabs-controller/src/state.rs
git commit -m "feat: project tabs into directory groups"
```

## Chunk 4: Zellij Event Wiring

### Task 4: Feed Initial And Live Cwd Into Controller State

**Files:**
- Modify: `crates/tabs-controller/src/main.rs`

- [ ] **Step 1: Add parser test for grouping config**

Add:

```rust
#[test]
fn parses_directory_grouping_from_plugin_configuration() {
    let mut configuration = BTreeMap::new();
    configuration.insert("rail_grouping".to_owned(), "directory".to_owned());

    let config = parse_rail_config(&configuration);

    assert_eq!(config.grouping, RailGroupingMode::Directory);
}
```

- [ ] **Step 2: Run controller tests and confirm failure**

Run:

```bash
cargo test -p tabs-controller
```

Expected: fails because `parse_rail_config` ignores `rail_grouping`.

- [ ] **Step 3: Parse grouping config**

Import `RailGroupingMode` and add:

```rust
fn parse_rail_grouping(value: &str) -> Option<RailGroupingMode> {
    match value {
        "none" | "off" | "false" => Some(RailGroupingMode::None),
        "directory" | "cwd" | "pane-cwd" | "pane_cwd" => Some(RailGroupingMode::Directory),
        _ => None,
    }
}
```

Update `parse_rail_config` to set `grouping`.

- [ ] **Step 4: Subscribe to cwd changes**

In `PluginState::load`, add:

```rust
EventType::CwdChanged,
```

- [ ] **Step 5: Handle live `CwdChanged`**

In `PluginState::update`:

```rust
Event::CwdChanged(pane_id, cwd, _) => {
    if let PaneId::Terminal(id) = pane_id {
        self.state.set_pane_cwd(PaneTarget::Terminal(id), cwd.display().to_string());
        self.push_view_model_to_rails();
    }
}
```

Use the actual `PaneId` variant names from `zellij_tile::prelude`.

- [ ] **Step 6: Query initial cwd after pane updates**

After `self.state.update_panes_from_manifest(pane_manifest)`, query terminal pane cwd for live terminal panes.

Add a controller-state method:

```rust
pub fn terminal_panes_needing_or_refreshing_cwd(&self) -> Vec<u32>
```

For the first version, this can return all known terminal pane ids. The plugin can call `get_pane_cwd(PaneId::Terminal(id))` and then `set_pane_cwd`.

Pseudo-code:

```rust
for terminal_id in self.state.terminal_panes_for_cwd_refresh() {
    if let Ok(cwd) = get_pane_cwd(PaneId::Terminal(terminal_id)) {
        self.state.set_pane_cwd(PaneTarget::Terminal(terminal_id), cwd.display().to_string());
    }
}
```

- [ ] **Step 7: Run controller tests**

Run:

```bash
cargo test -p tabs-controller
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add crates/tabs-controller/src/main.rs crates/tabs-controller/src/state.rs
git commit -m "feat: feed pane cwd into tab grouping"
```

## Chunk 5: Rail Group Rendering

### Task 5: Render Group Headers And Indented Tabs

**Files:**
- Modify: `crates/tabs-rail/src/render.rs`

- [ ] **Step 1: Add grouped rendering tests**

Add helper:

```rust
fn grouped_model() -> ControllerViewModel {
    let tab_one = TabCard {
        tab_id: 1,
        position: 0,
        name: "server".to_owned(),
        active: false,
        pinned: false,
        status: None,
    };
    let tab_two = TabCard {
        tab_id: 2,
        position: 1,
        name: "tests".to_owned(),
        active: true,
        pinned: false,
        status: None,
    };
    ControllerViewModel {
        sort_mode: SortMode::Position,
        config: RailConfig {
            grouping: RailGroupingMode::Directory,
            sizing: RailSizingPreset::Compact,
            ..RailConfig::default()
        },
        tabs: vec![tab_one.clone(), tab_two.clone()],
        rows: vec![
            RailRow::GroupHeader {
                group_id: "cwd:/Users/robert/dev/zellij".to_owned(),
                label: "zellij".to_owned(),
                full_label: "/Users/robert/dev/zellij".to_owned(),
                tab_count: 2,
            },
            RailRow::Tab { tab: tab_one, indent: 2 },
            RailRow::Tab { tab: tab_two, indent: 2 },
        ],
    }
}
```

Tests:

```rust
#[test]
fn grouped_rendering_draws_non_clickable_group_header() {
    let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

    assert!(rendered.lines[0].contains("zellij"));
    assert_eq!(hit_at(&rendered.hit_regions, 0, 2), None);
}

#[test]
fn grouped_child_tabs_remain_clickable_and_indented() {
    let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

    assert!(
        rendered.lines.iter().any(|line| line.starts_with("  ┌ tests")
            || line.starts_with("  ├ tests"))
    );
    assert_eq!(
        hit_at(&rendered.hit_regions, 3, 4).map(|hit| hit.action),
        Some(HitAction::SwitchTab)
    );
}

#[test]
fn grouped_rendering_preserves_active_visible_card_metadata() {
    let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

    assert!(rendered.visible_cards.iter().any(|card| card.tab_id == 2));
}
```

- [ ] **Step 2: Run rail tests and confirm failure**

Run:

```bash
cargo test -p tabs-rail
```

Expected: fails because renderer ignores `model.rows`.

- [ ] **Step 3: Add render row conversion**

Import `RailRow` and `RailGroupingMode`.

Add an internal enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum RenderRow {
    GroupHeader {
        label: String,
        full_label: String,
        tab_count: usize,
    },
    Card {
        card: RenderCard,
        indent: usize,
    },
}
```

Add:

```rust
fn rows_to_render(model: Option<&ControllerViewModel>, tabs: &[LocalTab]) -> Option<Vec<RenderRow>>
```

Behavior:

- Return `None` when no controller model exists.
- Return `None` when `model.rows` is empty, so existing card rendering remains fallback-compatible.
- Convert `RailRow::GroupHeader` to `RenderRow::GroupHeader`.
- Convert `RailRow::Tab` to `RenderRow::Card` using `render_card_from_model`.

- [ ] **Step 4: Add grouped projection render path**

In `render_lines_with_theme_and_cell_size`, before `render_cards`, branch:

```rust
if let Some(rows_to_render) = rows_to_render(model, tabs) {
    render_projection_rows(...);
} else {
    render_cards(...);
}
```

Implement `render_projection_rows`:

- group header height: 1.
- tab row height: `cell_height(card, config.sizing, false).max(2)`.
- visible range: start with active tab row, then expand before/after until `available_rows` is filled.
- render group header with `group_header_line`.
- render tabs with an indented wrapper around `render_standalone_box`.

Implement `render_indented_box` by rendering to temporary lines with width `cols - indent`, then prefixing spaces. Shift generated hit regions and `VisibleIconRect.x` by `indent`.

- [ ] **Step 5: Add group header formatting**

Add:

```rust
fn group_header_line(label: &str, tab_count: usize, width: usize, theme: Option<RenderTheme>) -> String
```

Rules:

- Label starts compact.
- If there is room, append ` (N)`.
- No hit region.
- Use body text styling.
- Always pad/truncate to `width`.

- [ ] **Step 6: Run rail tests**

Run:

```bash
cargo test -p tabs-rail
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/tabs-rail/src/render.rs
git commit -m "feat: render grouped tab rows"
```

## Chunk 6: Documentation And Verification

### Task 6: Document And Verify The Milestone

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document grouping config**

Add a short section:

````markdown
## Directory Grouping

The controller can group tabs by the exact current working directory of their panes:

```kdl
plugin location="tabs-controller" {
    rail_grouping "directory"
}
```

Use `rail_grouping "none"` or omit the setting for the flat tab rail.
````

Adjust the KDL syntax to match the existing layout/config style in this repo.

- [ ] **Step 2: Run unit tests**

Run:

```bash
cargo test -p tabs-shared -p tabs-controller -p tabs-rail
```

Expected: all pass.

- [ ] **Step 3: Build WASM plugins**

Run:

```bash
cargo build --workspace --target wasm32-wasip1 --release
```

Expected: builds `tabs-controller.wasm` and `tabs-rail.wasm` successfully under `target/wasm32-wasip1/release/`.

- [ ] **Step 4: Manual smoke test in Zellij**

Run with the existing launcher or layout:

```bash
/Users/robert/dev/zellij-scratch/run-vertical-tabs.sh
```

Manual checks:

- With `rail_grouping` omitted or set to `none`, the rail remains flat.
- With `rail_grouping="directory"`, tabs with panes in the same exact cwd appear under one header.
- A tab with no terminal cwd remains an ungrouped tab row.
- Clicking grouped child rows switches tabs.
- Group headers are not clickable.
- Active tab remains visible when grouped.
- Existing status icon/status text behavior still works.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document directory grouping"
```

## Out Of Scope For This Milestone

- Repository root detection.
- Common ancestor grouping.
- External arbitrary metadata protocol.
- User-facing resolver config.
- Group collapse/expand.
- Group drag/reorder.
- Pinned groups.
- Pinned tabs overriding groups.
- Resizable sidebar width.
- Pane text summaries.
- Image placement previews.

## Risks And Notes

- `PaneUpdate` does not include cwd. The controller must use `get_pane_cwd` for initial state and `CwdChanged` for live updates.
- `get_pane_cwd` can fail for exited or inaccessible terminal processes. Treat failures as missing cwd and render the tab ungrouped if no other cwd exists.
- Focus changes can affect cwd `precedence`. Refresh cwd metadata after pane focus/order updates, even if the cwd string did not change.
- Existing `SortMode` should remain meaningful. Build grouping over the already-sorted tab cards for now; later ordering policies can replace this cleanly.
- Keep `rows` as the rendering projection and `tabs` as the flat tab data. Do not make grouped order the stored tab order.
