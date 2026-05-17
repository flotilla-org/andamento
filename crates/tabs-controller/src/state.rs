use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use crate::metadata::{
    select_primary_entry, select_primary_value, CandidateEntry, EntityId, MetadataStore,
};
use tabs_shared::{
    ControllerBootstrapSnapshot, ControllerViewModel, GroupPath, GroupSegment, MetadataEntry,
    MetadataValue, PaneTarget, Priority, RailConfig, RailGroupingMode, RailRow, RendererHello,
    ResolvedMetadata, SetPaneStatus, SortMode, TabCard, TabGroupingInfo, TabStatusSummary,
};
use zellij_tile::prelude::{PaneManifest, TabInfo};

const SOURCE_ZELLIJ: &str = "zellij";
const KEY_PANE_CWD: &str = "zellij.pane.cwd";
const FOCUSED_CWD_PRECEDENCE: i64 = 100;
const NORMAL_CWD_PRECEDENCE: i64 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerTab {
    pub tab_id: u64,
    pub position: usize,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredPaneStatus {
    status: SetPaneStatus,
    received_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControllerPane {
    pane_id: PaneTarget,
    tab_id: u64,
    is_selectable: bool,
    is_focused: bool,
    ordinal: i64,
    cwd: Option<String>,
}

#[derive(Debug, Default)]
pub struct ControllerState {
    tabs: Vec<ControllerTab>,
    pane_to_tab: HashMap<PaneTarget, u64>,
    panes: HashMap<PaneTarget, ControllerPane>,
    metadata: MetadataStore,
    pane_statuses: HashMap<PaneTarget, StoredPaneStatus>,
    pinned_tabs: HashSet<u64>,
    known_rails: BTreeMap<u32, RendererHello>,
    known_config_editors: BTreeMap<u32, RendererHello>,
    sort_mode: SortMode,
    rail_config: RailConfig,
    receive_counter: u64,
}

impl ControllerState {
    #[allow(dead_code)]
    pub fn update_tabs_from_zellij(&mut self, tabs: Vec<TabInfo>) {
        self.tabs = tabs
            .into_iter()
            .map(|tab| ControllerTab {
                tab_id: tab.tab_id as u64,
                position: tab.position,
                name: if tab.name.is_empty() {
                    format!("Tab {}", tab.position + 1)
                } else {
                    tab.name
                },
                active: tab.active,
            })
            .collect();
        self.tabs.sort_by_key(|tab| tab.position);

        let live_tab_ids: HashSet<u64> = self.tabs.iter().map(|tab| tab.tab_id).collect();
        self.pinned_tabs
            .retain(|tab_id| live_tab_ids.contains(tab_id));
        self.pane_to_tab
            .retain(|_, tab_id| live_tab_ids.contains(tab_id));
        self.panes
            .retain(|_, pane| live_tab_ids.contains(&pane.tab_id));
        self.pane_statuses.retain(|pane_id, _| {
            self.pane_to_tab
                .get(pane_id)
                .map(|tab_id| live_tab_ids.contains(tab_id))
                .unwrap_or(true)
        });
    }

    #[cfg(test)]
    pub fn update_tabs(&mut self, tabs: Vec<ControllerTab>) {
        self.tabs = tabs;
        self.tabs.sort_by_key(|tab| tab.position);
    }

    #[allow(dead_code)]
    pub fn update_panes_from_manifest(&mut self, pane_manifest: PaneManifest) {
        let position_to_tab_id: HashMap<usize, u64> = self
            .tabs
            .iter()
            .map(|tab| (tab.position, tab.tab_id))
            .collect();
        let mut live_panes = HashSet::new();
        self.pane_to_tab.clear();
        let previous_panes = self.panes.clone();
        self.panes.clear();

        for (tab_position, panes) in pane_manifest.panes {
            let Some(tab_id) = position_to_tab_id.get(&tab_position).copied() else {
                continue;
            };
            for (ordinal, pane) in panes.into_iter().enumerate() {
                let pane_target = if pane.is_plugin {
                    PaneTarget::Plugin(pane.id)
                } else {
                    PaneTarget::Terminal(pane.id)
                };
                live_panes.insert(pane_target);
                self.pane_to_tab.insert(pane_target, tab_id);
                self.panes.insert(
                    pane_target,
                    ControllerPane {
                        pane_id: pane_target,
                        tab_id,
                        is_selectable: pane.is_selectable,
                        is_focused: pane.is_focused,
                        ordinal: ordinal as i64,
                        cwd: previous_panes
                            .get(&pane_target)
                            .and_then(|previous| previous.cwd.clone()),
                    },
                );
            }
        }

        self.retain_panes(live_panes);
        let pane_ids: Vec<PaneTarget> = self.panes.keys().copied().collect();
        for pane_id in pane_ids {
            self.refresh_pane_cwd_metadata(pane_id);
        }
    }

    #[cfg(test)]
    pub fn set_pane_tab(&mut self, pane_id: PaneTarget, tab_id: u64) {
        self.pane_to_tab.insert(pane_id, tab_id);
    }

    #[cfg(test)]
    pub fn set_test_pane(
        &mut self,
        pane_id: PaneTarget,
        tab_id: u64,
        is_selectable: bool,
        is_focused: bool,
        ordinal: i64,
    ) {
        self.pane_to_tab.insert(pane_id, tab_id);
        self.panes.insert(
            pane_id,
            ControllerPane {
                pane_id,
                tab_id,
                is_selectable,
                is_focused,
                ordinal,
                cwd: None,
            },
        );
    }

    #[allow(dead_code)]
    pub fn set_pane_cwd(&mut self, pane_id: PaneTarget, cwd: String) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.cwd = Some(cwd);
        }
        self.refresh_pane_cwd_metadata(pane_id);
    }

    #[allow(dead_code)]
    pub fn terminal_panes_for_cwd_refresh(&self) -> Vec<u32> {
        let mut terminal_ids: Vec<u32> = self
            .panes
            .values()
            .filter(|pane| pane.is_selectable)
            .filter_map(|pane| match pane.pane_id {
                PaneTarget::Terminal(id) => Some(id),
                PaneTarget::Plugin(_) => None,
            })
            .collect();
        terminal_ids.sort_unstable();
        terminal_ids
    }

    pub fn set_status(&mut self, mut status: SetPaneStatus) {
        self.receive_counter = self.receive_counter.saturating_add(1);
        if status.timestamp_ms.is_none() {
            status.timestamp_ms = Some(self.receive_counter);
        }
        self.pane_statuses.insert(
            status.pane_id,
            StoredPaneStatus {
                status,
                received_at: self.receive_counter,
            },
        );
    }

    #[allow(dead_code)]
    pub fn clear_status(&mut self, pane_id: PaneTarget) {
        self.pane_statuses.remove(&pane_id);
    }

    pub fn retain_panes(&mut self, live_panes: HashSet<PaneTarget>) {
        self.pane_to_tab
            .retain(|pane_id, _| live_panes.contains(pane_id));
        self.panes.retain(|pane_id, _| live_panes.contains(pane_id));
        self.pane_statuses
            .retain(|pane_id, _| live_panes.contains(pane_id));
    }

    #[allow(dead_code)]
    pub fn toggle_pin(&mut self, tab_id: u64) {
        if self.pinned_tabs.contains(&tab_id) {
            self.pinned_tabs.remove(&tab_id);
        } else {
            self.pinned_tabs.insert(tab_id);
        }
    }

    #[allow(dead_code)]
    pub fn set_sort_mode(&mut self, sort_mode: SortMode) {
        self.sort_mode = sort_mode;
    }

    #[allow(dead_code)]
    pub fn set_rail_config(&mut self, rail_config: RailConfig) {
        self.rail_config = rail_config;
    }

    pub fn bootstrap_snapshot(&self) -> ControllerBootstrapSnapshot {
        let mut pinned_tabs: Vec<u64> = self.pinned_tabs.iter().copied().collect();
        pinned_tabs.sort_unstable();
        let mut pane_statuses: Vec<SetPaneStatus> = self
            .pane_statuses
            .values()
            .map(|stored| stored.status.clone())
            .collect();
        pane_statuses.sort_by_key(|status| status.timestamp_ms.unwrap_or(0));

        ControllerBootstrapSnapshot {
            sort_mode: self.sort_mode,
            config: self.rail_config,
            pinned_tabs,
            pane_statuses,
        }
    }

    pub fn apply_bootstrap_snapshot(&mut self, snapshot: ControllerBootstrapSnapshot) {
        self.sort_mode = snapshot.sort_mode;
        self.rail_config = snapshot.config;
        self.pinned_tabs.extend(snapshot.pinned_tabs);
        for status in snapshot.pane_statuses {
            self.set_status(status);
        }
    }

    #[allow(dead_code)]
    pub fn register_rail(&mut self, hello: RendererHello) {
        self.known_rails.insert(hello.plugin_id, hello);
    }

    #[allow(dead_code)]
    pub fn retain_rails(&mut self, live_plugin_ids: &HashSet<u32>) {
        self.known_rails
            .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
        self.known_config_editors
            .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
    }

    #[allow(dead_code)]
    pub fn rail_plugin_ids(&self) -> Vec<u32> {
        self.known_rails
            .keys()
            .chain(self.known_config_editors.keys())
            .copied()
            .collect()
    }

    #[allow(dead_code)]
    pub fn rail_plugin_targets(&self) -> Vec<RendererHello> {
        self.known_rails
            .values()
            .chain(self.known_config_editors.values())
            .cloned()
            .collect()
    }

    #[allow(dead_code)]
    pub fn register_config_editor(&mut self, hello: RendererHello) {
        self.known_config_editors.insert(hello.plugin_id, hello);
    }

    pub fn view_model(&self) -> ControllerViewModel {
        let grouping_by_tab = self.tab_grouping_infos();
        let mut tabs: Vec<TabCard> = self
            .tabs
            .iter()
            .map(|tab| TabCard {
                tab_id: tab.tab_id,
                position: tab.position,
                name: tab.name.clone(),
                active: tab.active,
                pinned: self.pinned_tabs.contains(&tab.tab_id),
                status: self.status_for_tab(tab.tab_id),
                grouping: grouping_by_tab.get(&tab.tab_id).cloned(),
            })
            .collect();

        match self.sort_mode {
            SortMode::Controller | SortMode::Position => {
                tabs.sort_by_key(|tab| tab.position);
            }
            SortMode::PinnedFirst => {
                tabs.sort_by_key(|tab| (!tab.pinned, tab.position));
            }
            SortMode::LatestStatus => {
                tabs.sort_by(|a, b| {
                    let a_key = self.status_sort_key(a.status.as_ref());
                    let b_key = self.status_sort_key(b.status.as_ref());
                    b_key.cmp(&a_key).then_with(|| a.position.cmp(&b.position))
                });
            }
        }

        ControllerViewModel {
            sort_mode: self.sort_mode,
            config: self.rail_config,
            resolved_metadata: self.resolved_metadata_for_tabs(&tabs),
            rows: self.rows_for_tabs(&tabs),
            tabs,
        }
    }

    fn rows_for_tabs(&self, tabs: &[TabCard]) -> Vec<RailRow> {
        match self.rail_config.grouping {
            RailGroupingMode::None => tabs
                .iter()
                .cloned()
                .map(|tab| RailRow::Tab { tab, indent: 0 })
                .collect(),
            RailGroupingMode::Directory => self.directory_group_rows(tabs),
        }
    }

    fn directory_group_rows(&self, tabs: &[TabCard]) -> Vec<RailRow> {
        let mut path_to_tabs: BTreeMap<GroupPath, Vec<TabCard>> = BTreeMap::new();
        let mut grouping_by_path: BTreeMap<GroupPath, TabGroupingInfo> = BTreeMap::new();
        for tab in tabs {
            if let Some(grouping) = tab.grouping.clone() {
                grouping_by_path.insert(grouping.path.clone(), grouping.clone());
                path_to_tabs
                    .entry(grouping.path.clone())
                    .or_default()
                    .push(tab.clone());
            }
        }
        let mut emitted_groups = HashSet::new();
        let mut rows = vec![];
        for tab in tabs {
            let Some(grouping) = tab.grouping.as_ref() else {
                rows.push(RailRow::Tab {
                    tab: tab.clone(),
                    indent: 0,
                });
                continue;
            };
            if !emitted_groups.insert(grouping.path.clone()) {
                continue;
            }
            let grouped_tabs = path_to_tabs
                .get(&grouping.path)
                .cloned()
                .unwrap_or_default();
            let grouping = grouping_by_path
                .get(&grouping.path)
                .cloned()
                .unwrap_or_else(|| grouping.clone());
            rows.push(RailRow::GroupHeader {
                group_id: grouping.key,
                path: grouping.path,
                label: grouping.label,
                full_label: grouping.full_label,
                tab_count: grouped_tabs.len(),
            });
            rows.extend(
                grouped_tabs
                    .into_iter()
                    .map(|tab| RailRow::Tab { tab, indent: 2 }),
            );
        }
        rows
    }

    fn tab_grouping_infos(&self) -> HashMap<u64, TabGroupingInfo> {
        let tab_cwds: HashMap<u64, String> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                self.tab_primary_cwd(tab.tab_id)
                    .map(|cwd| (tab.tab_id, cwd))
            })
            .collect();
        let all_group_cwds: Vec<String> = tab_cwds
            .values()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        tab_cwds
            .into_iter()
            .map(|(tab_id, cwd)| {
                let label = self.group_label_for_cwd(&cwd, &all_group_cwds);
                (
                    tab_id,
                    TabGroupingInfo {
                        key: format!("cwd:{cwd}"),
                        path: cwd_group_path(&cwd),
                        label,
                        full_label: cwd,
                    },
                )
            })
            .collect()
    }

    fn tab_primary_cwd(&self, tab_id: u64) -> Option<String> {
        let entries: Vec<CandidateEntry> = self
            .panes
            .values()
            .filter(|pane| pane.tab_id == tab_id)
            .filter_map(|pane| {
                self.metadata
                    .entries_for(
                        &EntityId::Pane(pane.pane_id),
                        KEY_PANE_CWD,
                        self.receive_counter,
                    )
                    .into_iter()
                    .next()
            })
            .collect();
        match select_primary_value(&entries) {
            Some(MetadataValue::Text(cwd)) => Some(cwd),
            _ => None,
        }
    }

    fn tab_primary_metadata_entry(&self, tab_id: u64, key: &str) -> Option<MetadataEntry> {
        let entries: Vec<CandidateEntry> = self
            .panes
            .values()
            .filter(|pane| pane.tab_id == tab_id)
            .flat_map(|pane| {
                self.metadata
                    .entries_for(&EntityId::Pane(pane.pane_id), key, self.receive_counter)
            })
            .collect();
        select_primary_entry(&entries).map(|candidate| candidate.entry)
    }

    fn resolved_metadata_for_tabs(&self, tabs: &[TabCard]) -> Vec<ResolvedMetadata> {
        let mut by_target: BTreeMap<EntityId, BTreeMap<String, MetadataEntry>> = BTreeMap::new();
        for tab in tabs {
            if let Some(entry) = self.tab_primary_metadata_entry(tab.tab_id, KEY_PANE_CWD) {
                by_target
                    .entry(EntityId::Tab(tab.tab_id))
                    .or_default()
                    .insert(KEY_PANE_CWD.to_owned(), entry.clone());
                if let Some(grouping) = tab.grouping.as_ref() {
                    by_target
                        .entry(EntityId::Group(grouping.path.clone()))
                        .or_default()
                        .insert(KEY_PANE_CWD.to_owned(), entry);
                }
            }
        }
        by_target
            .into_iter()
            .map(|(target, values)| ResolvedMetadata { target, values })
            .collect()
    }

    fn group_label_for_cwd(&self, cwd: &str, all_group_cwds: &[String]) -> String {
        let basename = Path::new(cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(cwd);
        let duplicate_basename_count = all_group_cwds
            .iter()
            .filter(|candidate| {
                Path::new(candidate)
                    .file_name()
                    .and_then(|name| name.to_str())
                    == Some(basename)
            })
            .count();
        if duplicate_basename_count <= 1 {
            return basename.to_owned();
        }
        Path::new(cwd)
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|parent| parent.to_str())
            .map(|parent| format!("{parent}/{basename}"))
            .unwrap_or_else(|| cwd.to_owned())
    }

    fn refresh_pane_cwd_metadata(&mut self, pane_id: PaneTarget) {
        let entity_id = EntityId::Pane(pane_id);
        let Some(pane) = self.panes.get(&pane_id) else {
            self.metadata.unset(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ);
            return;
        };
        let should_emit = matches!(pane_id, PaneTarget::Terminal(_)) && pane.is_selectable;
        let Some(cwd) = pane.cwd.clone().filter(|_| should_emit) else {
            self.metadata.unset(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ);
            return;
        };
        self.receive_counter = self.receive_counter.saturating_add(1);
        self.metadata.set(
            entity_id,
            KEY_PANE_CWD,
            SOURCE_ZELLIJ,
            MetadataEntry {
                value: MetadataValue::Text(cwd),
                updated_at: self.receive_counter,
                ttl_ms: None,
                precedence: if pane.is_focused {
                    FOCUSED_CWD_PRECEDENCE
                } else {
                    NORMAL_CWD_PRECEDENCE
                },
                ordinal: pane.ordinal,
            },
        );
    }

    fn status_for_tab(&self, tab_id: u64) -> Option<TabStatusSummary> {
        self.pane_statuses
            .iter()
            .filter(|(pane_id, _)| self.pane_to_tab.get(pane_id) == Some(&tab_id))
            .max_by_key(|(_, stored)| {
                (
                    stored.status.priority,
                    stored.status.timestamp_ms.unwrap_or(stored.received_at),
                    stored.received_at,
                )
            })
            .map(|(pane_id, stored)| TabStatusSummary {
                priority: stored.status.priority,
                title: stored.status.title.clone(),
                detail: stored.status.detail.clone(),
                icon: stored.status.icon.clone(),
                source_pane: *pane_id,
            })
    }

    fn status_sort_key(&self, status: Option<&TabStatusSummary>) -> (Priority, u64) {
        let Some(status) = status else {
            return (Priority::Idle, 0);
        };
        let timestamp = self
            .pane_statuses
            .get(&status.source_pane)
            .and_then(|stored| stored.status.timestamp_ms.or(Some(stored.received_at)))
            .unwrap_or(0);
        (status.priority, timestamp)
    }
}

fn cwd_group_path(cwd: &str) -> GroupPath {
    GroupPath(vec![GroupSegment {
        key: KEY_PANE_CWD.to_owned(),
        value: MetadataValue::Text(cwd.to_owned()),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tabs_shared::{
        GroupPath, GroupSegment, RailGroupingMode, RailRow, RailSizingPreset, RailStructure,
        RailViewMode, StatusIcon,
    };

    fn status(
        pane_id: PaneTarget,
        priority: Priority,
        title: &str,
        timestamp_ms: u64,
    ) -> SetPaneStatus {
        SetPaneStatus {
            pane_id,
            priority,
            title: title.to_owned(),
            detail: None,
            icon: None,
            timestamp_ms: Some(timestamp_ms),
        }
    }

    fn tab_info(tab_id: usize, position: usize, name: &str, active: bool) -> TabInfo {
        TabInfo {
            position,
            name: name.to_owned(),
            active,
            panes_to_hide: 0,
            is_fullscreen_active: false,
            is_sync_panes_active: false,
            are_floating_panes_visible: false,
            other_focused_clients: vec![],
            active_swap_layout_name: None,
            is_swap_layout_dirty: false,
            viewport_rows: 0,
            viewport_columns: 0,
            display_area_rows: 0,
            display_area_columns: 0,
            selectable_tiled_panes_count: 0,
            selectable_floating_panes_count: 0,
            tab_id,
            has_bell_notification: false,
            is_flashing_bell: false,
        }
    }

    #[test]
    fn focused_pane_cwd_beats_more_common_cwd_for_tab_grouping() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "work".into(),
            active: true,
        }]);
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
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "work".into(),
            active: true,
        }]);
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

    #[test]
    fn directory_grouping_compacts_tabs_and_leaves_missing_cwd_ungrouped() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![
            ControllerTab {
                tab_id: 1,
                position: 0,
                name: "one".into(),
                active: false,
            },
            ControllerTab {
                tab_id: 2,
                position: 1,
                name: "two".into(),
                active: true,
            },
            ControllerTab {
                tab_id: 3,
                position: 2,
                name: "three".into(),
                active: false,
            },
        ]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_test_pane(PaneTarget::Terminal(30), 3, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());
        state.set_pane_cwd(PaneTarget::Terminal(30), "/repo/a".into());

        let model = state.view_model();

        assert_eq!(model.rows.len(), 4);
        assert!(matches!(
            &model.rows[0],
            RailRow::GroupHeader { tab_count: 2, .. }
        ));
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

    #[test]
    fn tab_cards_include_selected_cwd_grouping_metadata_even_when_ungrouped() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "one".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());

        let model = state.view_model();
        let grouping = model.tabs[0].grouping.as_ref().unwrap();

        assert_eq!(model.config.grouping, RailGroupingMode::None);
        assert_eq!(grouping.key, "cwd:/repo/a");
        assert_eq!(grouping.label, "a");
        assert_eq!(grouping.full_label, "/repo/a");
    }

    #[test]
    fn directory_grouping_uses_cwd_group_path_identity() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "one".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());

        let model = state.view_model();
        let expected_path = GroupPath(vec![GroupSegment {
            key: KEY_PANE_CWD.to_owned(),
            value: MetadataValue::Text("/repo/a".to_owned()),
        }]);

        assert_eq!(
            model.tabs[0]
                .grouping
                .as_ref()
                .map(|grouping| &grouping.path),
            Some(&expected_path)
        );
        assert!(matches!(
            &model.rows[0],
            RailRow::GroupHeader { path, .. } if path == &expected_path
        ));
    }

    #[test]
    fn view_model_exposes_resolved_cwd_metadata_for_tab_and_group_targets() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "one".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());

        let model = state.view_model();
        let group_path = cwd_group_path("/repo/a");

        let tab_metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == EntityId::Tab(1))
            .expect("tab metadata");
        assert_eq!(
            tab_metadata
                .values
                .get(KEY_PANE_CWD)
                .map(|entry| &entry.value),
            Some(&MetadataValue::Text("/repo/a".to_owned()))
        );

        let group_metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == EntityId::Group(group_path.clone()))
            .expect("group metadata");
        assert_eq!(
            group_metadata
                .values
                .get(KEY_PANE_CWD)
                .map(|entry| &entry.value),
            Some(&MetadataValue::Text("/repo/a".to_owned()))
        );
    }

    #[test]
    fn highest_priority_status_wins_for_tab() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "main".into(),
            active: true,
        }]);
        state.set_pane_tab(PaneTarget::Terminal(10), 1);
        state.set_pane_tab(PaneTarget::Terminal(11), 1);
        state.set_status(SetPaneStatus {
            pane_id: PaneTarget::Terminal(10),
            priority: Priority::Info,
            title: "info".into(),
            detail: None,
            icon: None,
            timestamp_ms: Some(100),
        });
        state.set_status(SetPaneStatus {
            pane_id: PaneTarget::Terminal(11),
            priority: Priority::Error,
            title: "broken".into(),
            detail: None,
            icon: None,
            timestamp_ms: Some(90),
        });

        let model = state.view_model();
        let status = model.tabs[0].status.as_ref().unwrap();
        assert_eq!(status.priority, Priority::Error);
        assert_eq!(status.title, "broken");
    }

    #[test]
    fn status_icon_is_carried_to_tab_summary() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "main".into(),
            active: true,
        }]);
        state.set_pane_tab(PaneTarget::Terminal(10), 1);
        let mut status = status(PaneTarget::Terminal(10), Priority::Waiting, "waiting", 10);
        status.icon = Some(StatusIcon::Builtin("waiting".to_owned()));
        state.set_status(status);

        let model = state.view_model();

        assert_eq!(
            model.tabs[0]
                .status
                .as_ref()
                .and_then(|status| status.icon.as_ref()),
            Some(&StatusIcon::Builtin("waiting".to_owned()))
        );
    }

    #[test]
    fn bootstrap_snapshot_carries_external_state_without_zellij_tabs() {
        let mut source = ControllerState::default();
        source.set_sort_mode(SortMode::PinnedFirst);
        source.set_rail_config(RailConfig {
            structure: RailStructure::BoxPerTab,
            sizing: RailSizingPreset::Compact,
            grouping: RailGroupingMode::Directory,
            view: RailViewMode::Normal,
        });
        source.toggle_pin(7);
        source.set_status(status(
            PaneTarget::Terminal(10),
            Priority::Waiting,
            "waiting",
            10,
        ));

        let snapshot = source.bootstrap_snapshot();
        let mut target = ControllerState::default();
        target.apply_bootstrap_snapshot(snapshot);

        let target_snapshot = target.bootstrap_snapshot();
        assert_eq!(target_snapshot.sort_mode, SortMode::PinnedFirst);
        assert_eq!(target_snapshot.config.structure, RailStructure::BoxPerTab);
        assert_eq!(target_snapshot.pinned_tabs, vec![7]);
        assert_eq!(target_snapshot.pane_statuses.len(), 1);
        assert_eq!(target_snapshot.pane_statuses[0].title, "waiting");
    }

    #[test]
    fn tab_update_does_not_drop_bootstrapped_status_before_pane_manifest() {
        let mut source = ControllerState::default();
        source.set_status(status(
            PaneTarget::Terminal(10),
            Priority::Waiting,
            "waiting",
            10,
        ));

        let mut target = ControllerState::default();
        target.apply_bootstrap_snapshot(source.bootstrap_snapshot());
        target.update_tabs_from_zellij(vec![tab_info(1, 0, "main", true)]);
        target.set_pane_tab(PaneTarget::Terminal(10), 1);

        let model = target.view_model();
        assert_eq!(model.tabs[0].status.as_ref().unwrap().title, "waiting");
    }

    #[test]
    fn recency_breaks_priority_ties() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "main".into(),
            active: true,
        }]);
        state.set_pane_tab(PaneTarget::Terminal(10), 1);
        state.set_pane_tab(PaneTarget::Terminal(11), 1);
        state.set_status(status(
            PaneTarget::Terminal(10),
            Priority::Waiting,
            "old",
            10,
        ));
        state.set_status(status(
            PaneTarget::Terminal(11),
            Priority::Waiting,
            "new",
            20,
        ));

        let model = state.view_model();
        assert_eq!(model.tabs[0].status.as_ref().unwrap().title, "new");
    }

    #[test]
    fn cleanup_removes_status_for_missing_panes() {
        let mut state = ControllerState::default();
        state.set_pane_tab(PaneTarget::Terminal(10), 1);
        state.set_status(status(
            PaneTarget::Terminal(10),
            Priority::Error,
            "gone",
            10,
        ));

        state.retain_panes([PaneTarget::Terminal(99)].into_iter().collect());

        assert!(state.pane_statuses.is_empty());
    }

    #[test]
    fn cleanup_removes_missing_rail_renderers() {
        let mut state = ControllerState::default();
        state.register_rail(RendererHello {
            plugin_id: 7,
            client_id: 1,
        });
        state.register_rail(RendererHello {
            plugin_id: 8,
            client_id: 1,
        });

        state.retain_rails(&[8].into_iter().collect());

        assert_eq!(state.rail_plugin_ids(), vec![8]);
    }
}
