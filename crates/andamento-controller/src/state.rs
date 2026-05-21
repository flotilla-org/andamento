use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

use crate::metadata::{
    select_primary_entry, select_primary_value, CandidateEntry, EntityId, MetadataStore,
};
use andamento_shared::grouping_config::{GroupingConfigCatalog, GroupingRule};
use andamento_shared::{
    ControllerBootstrapSnapshot, ControllerViewModel, GroupPath, GroupSegment, MetadataEntry,
    MetadataIdentity, MetadataSourceEntry, MetadataValue, ObservedMetadataIdentity, PaneTarget,
    Priority, RailConfig, RailGroupingMode, RailRow, ReachableMetadataIdentity, RendererHello,
    ResolvedMetadata, ResolvedTemplateField, ResolvedTemplateSlot, ResolvedTemplateSlots,
    SetPaneStatus, SortMode, TabCard, TabGroupingInfo, TabStatusSummary, TemplateConfigDiagnostics,
};
use zellij_tile::prelude::{PaneManifest, TabInfo};

const SOURCE_ZELLIJ: &str = "zellij";
const KEY_PANE_CWD: &str = "zellij.pane.cwd";
const KEY_TAB_SCOPE: &str = "tab.scope";
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
    grouping_catalog: Option<GroupingConfigCatalog>,
    template_catalog: Option<andamento_shared::template_config::TemplateConfigCatalog>,
    template_config: TemplateConfigDiagnostics,
    receive_counter: u64,
    metadata_controls: andamento_shared::MetadataControls,
}

impl ControllerState {
    #[allow(dead_code)]
    pub fn update_tabs_from_zellij(&mut self, tabs: Vec<TabInfo>) -> bool {
        let mut next_tabs: Vec<ControllerTab> = tabs
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
        next_tabs.sort_by_key(|tab| tab.position);
        if self.tabs == next_tabs {
            return false;
        }
        self.tabs = next_tabs;

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
        true
    }

    #[cfg(test)]
    pub fn update_tabs(&mut self, tabs: Vec<ControllerTab>) {
        self.tabs = tabs;
        self.tabs.sort_by_key(|tab| tab.position);
    }

    #[allow(dead_code)]
    pub fn update_panes_from_manifest(&mut self, pane_manifest: PaneManifest) -> bool {
        let position_to_tab_id: HashMap<usize, u64> = self
            .tabs
            .iter()
            .map(|tab| (tab.position, tab.tab_id))
            .collect();
        let mut live_panes = HashSet::new();
        let previous_pane_to_tab = self.pane_to_tab.clone();
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
        let pane_topology_changed =
            self.pane_to_tab != previous_pane_to_tab || self.panes != previous_panes;
        let pane_ids: Vec<PaneTarget> = self.panes.keys().copied().collect();
        let mut metadata_changed = false;
        for pane_id in pane_ids {
            metadata_changed |= self.refresh_pane_cwd_metadata(pane_id);
        }
        pane_topology_changed || metadata_changed
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
    pub fn set_pane_cwd(&mut self, pane_id: PaneTarget, cwd: String) -> bool {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.cwd = Some(cwd);
        }
        self.refresh_pane_cwd_metadata(pane_id)
    }

    #[allow(dead_code)]
    pub fn terminal_panes_for_cwd_refresh(&self) -> Vec<u32> {
        let mut terminal_ids: Vec<u32> = self
            .panes
            .values()
            .filter(|pane| pane.is_selectable)
            .filter(|pane| pane.cwd.is_none())
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

    pub fn toggle_metadata_root(&mut self) {
        self.metadata_controls.root_enabled = !self.metadata_controls.root_enabled;
    }

    pub fn cycle_metadata_tristate(&mut self, key: andamento_shared::NodeKey) {
        self.metadata_controls.cycle(key);
    }

    pub fn set_template_catalog(
        &mut self,
        catalog: Option<andamento_shared::template_config::TemplateConfigCatalog>,
    ) {
        self.template_catalog = catalog;
    }

    pub fn set_grouping_catalog(&mut self, catalog: Option<GroupingConfigCatalog>) {
        self.grouping_catalog = catalog;
    }

    pub fn set_template_config_diagnostics(&mut self, diagnostics: TemplateConfigDiagnostics) {
        self.template_config = diagnostics;
    }

    pub fn template_config_diagnostics(&self) -> &TemplateConfigDiagnostics {
        &self.template_config
    }

    pub fn apply_metadata_patch(&mut self, patch: andamento_shared::MetadataPatch) -> bool {
        let next_receive_counter = self.receive_counter.saturating_add(1);
        let outcome = self.metadata.apply_patch(patch, next_receive_counter);
        if outcome.touched {
            self.receive_counter = next_receive_counter;
        }
        outcome.view_changed
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
            metadata_patches: self.metadata.snapshot_patches(self.receive_counter),
        }
    }

    pub fn apply_bootstrap_snapshot(&mut self, snapshot: ControllerBootstrapSnapshot) {
        self.sort_mode = snapshot.sort_mode;
        self.rail_config = snapshot.config;
        self.pinned_tabs.extend(snapshot.pinned_tabs);
        for status in snapshot.pane_statuses {
            self.set_status(status);
        }
        for patch in snapshot.metadata_patches {
            self.apply_metadata_patch(patch);
        }
    }

    #[allow(dead_code)]
    pub fn register_rail(&mut self, hello: RendererHello) -> bool {
        if self.known_rails.get(&hello.plugin_id) == Some(&hello) {
            return false;
        }
        self.known_rails.insert(hello.plugin_id, hello);
        true
    }

    #[allow(dead_code)]
    pub fn retain_rails(&mut self, live_plugin_ids: &HashSet<u32>) {
        self.known_rails
            .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
        self.known_config_editors
            .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
    }

    /// Remove a renderer by plugin id, regardless of whether it was registered
    /// as a rail or a config editor. Returns true if anything was removed.
    pub fn unregister_renderer(&mut self, plugin_id: u32) -> bool {
        let r = self.known_rails.remove(&plugin_id).is_some();
        let c = self.known_config_editors.remove(&plugin_id).is_some();
        r || c
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
    pub fn known_rail_count(&self) -> usize {
        self.known_rails.len()
    }

    #[allow(dead_code)]
    pub fn known_config_editor_count(&self) -> usize {
        self.known_config_editors.len()
    }

    pub fn config_editor_target_for_client(&self, client_id: u16) -> Option<RendererHello> {
        self.known_config_editors
            .values()
            .find(|target| target.client_id == client_id)
            .cloned()
    }

    #[allow(dead_code)]
    pub fn register_config_editor(&mut self, hello: RendererHello) -> bool {
        if self.known_config_editors.get(&hello.plugin_id) == Some(&hello) {
            return false;
        }
        self.known_config_editors.insert(hello.plugin_id, hello);
        true
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
                templates: ResolvedTemplateSlots::default(),
                active_pane: self
                    .panes
                    .values()
                    .find(|pane| pane.tab_id == tab.tab_id && pane.is_focused)
                    .map(|pane| pane.pane_id),
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

        let resolved_metadata = self.resolved_metadata_for_tabs(&tabs);
        let observed_identities = observed_metadata_identities(&resolved_metadata);
        self.resolve_tab_templates(&mut tabs, &resolved_metadata);
        let rows = self.rows_with_group_templates(self.rows_for_tabs(&tabs), &resolved_metadata);

        ControllerViewModel {
            sort_mode: self.sort_mode,
            config: self.rail_config,
            template_config: self.template_config.clone(),
            resolved_metadata,
            observed_identities,
            rows,
            tabs,
            metadata_controls: self.metadata_controls.clone(),
        }
    }

    fn rows_for_tabs(&self, tabs: &[TabCard]) -> Vec<RailRow> {
        match self.rail_config.grouping {
            RailGroupingMode::None => tabs
                .iter()
                .map(|tab| RailRow::Tab {
                    tab_id: tab.tab_id,
                    indent: 0,
                    parent_path: None,
                })
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
        let mut tab_count_by_prefix: BTreeMap<GroupPath, usize> = BTreeMap::new();
        for (path, grouped_tabs) in &path_to_tabs {
            for prefix in group_path_prefixes(path) {
                *tab_count_by_prefix.entry(prefix).or_default() += grouped_tabs.len();
            }
        }
        let mut emitted_groups = HashSet::new();
        let mut emitted_leaf_groups = HashSet::new();
        let mut rows = vec![];
        for tab in tabs {
            let Some(grouping) = tab.grouping.as_ref() else {
                rows.push(RailRow::Tab {
                    tab_id: tab.tab_id,
                    indent: 0,
                    parent_path: None,
                });
                continue;
            };
            if !emitted_leaf_groups.insert(grouping.path.clone()) {
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
            for prefix in group_path_prefixes(&grouping.path) {
                if !emitted_groups.insert(prefix.clone()) {
                    continue;
                }
                let (group_id, label, full_label) = group_header_identity_for_prefix(
                    &prefix,
                    &grouping.path,
                    &grouping.key,
                    &grouping.label,
                    &grouping.full_label,
                );
                rows.push(RailRow::GroupHeader {
                    group_id,
                    path: prefix.clone(),
                    label,
                    full_label,
                    tab_count: tab_count_by_prefix
                        .get(&prefix)
                        .copied()
                        .unwrap_or(grouped_tabs.len()),
                    templates: ResolvedTemplateSlots::default(),
                });
            }
            rows.extend(grouped_tabs.into_iter().map(|tab| RailRow::Tab {
                tab_id: tab.tab_id,
                indent: grouping.path.0.len() * 2,
                parent_path: Some(grouping.path.clone()),
            }));
        }
        rows
    }

    fn rows_with_group_templates(
        &self,
        rows: Vec<RailRow>,
        resolved_metadata: &[ResolvedMetadata],
    ) -> Vec<RailRow> {
        rows.into_iter()
            .map(|row| match row {
                RailRow::GroupHeader {
                    group_id,
                    path,
                    label,
                    full_label,
                    tab_count,
                    mut templates,
                } => {
                    let metadata = group_template_metadata(
                        &path,
                        &label,
                        &full_label,
                        tab_count,
                        resolved_metadata,
                    );
                    templates.group_header = self.resolve_template_slot(
                        andamento_shared::template_config::TemplateConfigSlot::GroupHeader,
                        andamento_shared::template_config::TemplateConfigNodeKind::Group,
                        &metadata,
                    );
                    RailRow::GroupHeader {
                        group_id,
                        path,
                        label,
                        full_label,
                        tab_count,
                        templates,
                    }
                }
                RailRow::Tab {
                    tab_id,
                    indent,
                    parent_path,
                } => RailRow::Tab {
                    tab_id,
                    indent,
                    parent_path,
                },
            })
            .collect()
    }

    fn resolve_tab_templates(&self, tabs: &mut [TabCard], resolved_metadata: &[ResolvedMetadata]) {
        for tab in tabs {
            let metadata = tab_template_metadata(tab, resolved_metadata);
            tab.templates.tab_title = self.resolve_template_slot(
                andamento_shared::template_config::TemplateConfigSlot::TabTitle,
                andamento_shared::template_config::TemplateConfigNodeKind::Tab,
                &metadata,
            );
            if tab.status.is_some() {
                tab.templates.tab_status = self.resolve_template_slot(
                    andamento_shared::template_config::TemplateConfigSlot::TabStatus,
                    andamento_shared::template_config::TemplateConfigNodeKind::Tab,
                    &metadata,
                );
            }
        }
    }

    fn resolve_template_slot(
        &self,
        slot: andamento_shared::template_config::TemplateConfigSlot,
        node_kind: andamento_shared::template_config::TemplateConfigNodeKind,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> Option<ResolvedTemplateSlot> {
        let catalog = self.template_catalog.as_ref()?;
        let context = andamento_shared::template_config::TemplateConfigMatchContext {
            slot,
            node_kind,
            metadata,
            collapsed: false,
            active_tab_name: None,
        };
        let resolved = catalog.resolve(context)?;
        let fields = resolved
            .template
            .render_fields(context)
            .into_iter()
            .map(|field| ResolvedTemplateField {
                text: field.value,
                priority: field.priority.unwrap_or(match field.class {
                    andamento_shared::template_config::TemplateConfigFieldClass::Optional => 0,
                    andamento_shared::template_config::TemplateConfigFieldClass::Required
                    | andamento_shared::template_config::TemplateConfigFieldClass::Priority => 100,
                }),
                source: field.source,
            })
            .collect::<Vec<_>>();
        (!fields.is_empty()).then_some(ResolvedTemplateSlot {
            template_name: resolved.template.name.clone(),
            fields,
        })
    }

    fn tab_grouping_infos(&self) -> HashMap<u64, TabGroupingInfo> {
        let tab_scopes: HashMap<u64, TabGroupingInfo> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                self.tab_explicit_scope_grouping(tab.tab_id)
                    .map(|grouping| (tab.tab_id, grouping))
            })
            .collect();
        let tab_configured_groupings: HashMap<u64, TabGroupingInfo> = self
            .grouping_catalog
            .as_ref()
            .filter(|catalog| !catalog.is_empty())
            .map(|catalog| {
                self.tabs
                    .iter()
                    .filter(|tab| !tab_scopes.contains_key(&tab.tab_id))
                    .filter_map(|tab| {
                        self.tab_rule_grouping(tab.tab_id, catalog)
                            .map(|grouping| (tab.tab_id, grouping))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tab_cwds: HashMap<u64, String> = self
            .tabs
            .iter()
            .filter(|tab| !tab_scopes.contains_key(&tab.tab_id))
            .filter(|tab| !tab_configured_groupings.contains_key(&tab.tab_id))
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

        let mut groupings: HashMap<u64, TabGroupingInfo> = tab_cwds
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
            .collect();
        groupings.extend(tab_configured_groupings);
        groupings.extend(tab_scopes);
        groupings
    }

    fn tab_rule_grouping(
        &self,
        tab_id: u64,
        catalog: &GroupingConfigCatalog,
    ) -> Option<TabGroupingInfo> {
        let metadata = self.tab_resolved_metadata_values(tab_id);
        catalog
            .rules
            .iter()
            .find_map(|rule| self.tab_grouping_for_rule(rule, &metadata))
    }

    fn tab_grouping_for_rule(
        &self,
        rule: &GroupingRule,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> Option<TabGroupingInfo> {
        let mut segments = vec![];
        let mut labels = vec![];
        for level in &rule.levels {
            let Some(value) = metadata.get(&level.key).cloned() else {
                if level.optional {
                    continue;
                }
                if segments.is_empty() {
                    return None;
                }
                break;
            };
            let label = level
                .label_key
                .as_ref()
                .and_then(|key| metadata.get(key))
                .map(metadata_value_display)
                .unwrap_or_else(|| metadata_value_display(&value));
            segments.push(GroupSegment {
                key: level.key.clone(),
                value,
                label: level.label_key.as_ref().map(|_| label.clone()),
            });
            labels.push(label);
        }
        if segments.is_empty() {
            return None;
        }
        let key = format!(
            "{}:{}",
            rule.name,
            segments
                .iter()
                .map(|segment| format!(
                    "{}={}",
                    segment.key,
                    metadata_value_display(&segment.value)
                ))
                .collect::<Vec<_>>()
                .join("/")
        );
        let label = labels.last().cloned().unwrap_or_else(|| key.clone());
        Some(TabGroupingInfo {
            key,
            path: GroupPath(segments),
            label,
            full_label: labels.join(" / "),
        })
    }

    fn tab_resolved_metadata_values(&self, tab_id: u64) -> BTreeMap<String, MetadataValue> {
        let target = EntityId::Tab(tab_id);
        let seed_values = self.tab_seed_metadata_entries(tab_id);
        let (values, _, _) = self.resolve_target_metadata(&target, seed_values);
        values
            .into_iter()
            .map(|(key, entry)| (key, entry.value))
            .collect()
    }

    fn tab_explicit_scope_grouping(&self, tab_id: u64) -> Option<TabGroupingInfo> {
        let path = match self
            .metadata
            .resolved_entries_for(&EntityId::Tab(tab_id), self.receive_counter)
            .remove(KEY_TAB_SCOPE)?
            .value
        {
            MetadataValue::GroupPath(segments) => metadata_path_segments_to_group_path(segments),
            _ => return None,
        };
        tab_grouping_info_for_explicit_scope(path)
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
        let mut sources_by_target: BTreeMap<EntityId, BTreeMap<String, Vec<MetadataSourceEntry>>> =
            BTreeMap::new();
        let mut identities_by_target: BTreeMap<EntityId, Vec<ReachableMetadataIdentity>> =
            BTreeMap::new();
        let mut group_paths = BTreeSet::new();
        for tab in tabs {
            let tab_target = EntityId::Tab(tab.tab_id);
            let tab_seed_values = self.tab_seed_metadata_entries(tab.tab_id);
            let (tab_values, tab_sources, tab_identities) =
                self.resolve_target_metadata(&tab_target, tab_seed_values);
            by_target
                .entry(tab_target.clone())
                .or_default()
                .extend(tab_values);
            sources_by_target
                .entry(tab_target.clone())
                .or_default()
                .extend(tab_sources);
            identities_by_target
                .entry(tab_target.clone())
                .or_default()
                .extend(tab_identities);
            if let Some(grouping) = tab.grouping.as_ref() {
                group_paths.extend(group_path_prefixes(&grouping.path));
            }
        }
        for path in group_paths {
            let group_target = EntityId::Group(path.clone());
            let group_seed_values = self.group_path_seed_metadata_entries(&path);
            let (group_values, group_sources, group_identities) =
                self.resolve_target_metadata(&group_target, group_seed_values);
            by_target
                .entry(group_target.clone())
                .or_default()
                .extend(group_values);
            sources_by_target
                .entry(group_target.clone())
                .or_default()
                .extend(group_sources);
            identities_by_target
                .entry(group_target)
                .or_default()
                .extend(group_identities);
        }
        by_target
            .into_iter()
            .map(|(target, values)| ResolvedMetadata {
                source_entries: sources_by_target.remove(&target).unwrap_or_default(),
                reachable_identities: identities_by_target.remove(&target).unwrap_or_default(),
                target,
                values,
            })
            .collect()
    }

    fn resolve_target_metadata(
        &self,
        target: &EntityId,
        seed_values: BTreeMap<String, MetadataEntry>,
    ) -> (
        BTreeMap<String, MetadataEntry>,
        BTreeMap<String, Vec<MetadataSourceEntry>>,
        Vec<ReachableMetadataIdentity>,
    ) {
        let mut values = BTreeMap::new();
        let mut source_entries = BTreeMap::<String, Vec<MetadataSourceEntry>>::new();
        let mut reachable_identities = vec![];
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::from([(target.clone(), 0usize)]);

        while let Some((current, distance)) = queue.pop_front() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if let EntityId::Identity(identity) = &current {
                reachable_identities.push(ReachableMetadataIdentity {
                    identity: identity.clone(),
                    distance,
                });
            }
            let current_values = self
                .metadata
                .resolved_entries_for(&current, self.receive_counter);
            let current_values = if &current == target {
                seed_values
                    .clone()
                    .into_iter()
                    .chain(current_values)
                    .collect::<BTreeMap<_, _>>()
            } else {
                current_values
            };
            for (key, entry) in current_values {
                let identity = EntityId::Identity(MetadataIdentity {
                    key: key.clone(),
                    value: entry.value.clone(),
                });
                values.entry(key).or_insert(entry);
                if !visited.contains(&identity) {
                    queue.push_back((identity, distance + 1));
                }
            }
            for (key, entries) in self
                .metadata
                .source_entries_for(&current, self.receive_counter)
            {
                source_entries.entry(key).or_default().extend(entries);
            }
        }

        (values, source_entries, reachable_identities)
    }

    fn tab_seed_metadata_entries(&self, tab_id: u64) -> BTreeMap<String, MetadataEntry> {
        self.tab_primary_metadata_entry(tab_id, KEY_PANE_CWD)
            .map(|entry| BTreeMap::from([(KEY_PANE_CWD.to_owned(), entry)]))
            .unwrap_or_default()
    }

    fn group_path_seed_metadata_entries(
        &self,
        path: &GroupPath,
    ) -> BTreeMap<String, MetadataEntry> {
        path.0
            .iter()
            .map(|segment| {
                (
                    segment.key.clone(),
                    self.seed_metadata_entry(segment.value.clone()),
                )
            })
            .collect()
    }

    fn seed_metadata_entry(&self, value: MetadataValue) -> MetadataEntry {
        MetadataEntry {
            value,
            updated_at: self.receive_counter,
            ttl_ms: None,
            precedence: 0,
            ordinal: 0,
        }
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

    fn refresh_pane_cwd_metadata(&mut self, pane_id: PaneTarget) -> bool {
        let entity_id = EntityId::Pane(pane_id);
        let Some(pane) = self.panes.get(&pane_id) else {
            let changed = self
                .metadata
                .source_entry(
                    &entity_id,
                    KEY_PANE_CWD,
                    SOURCE_ZELLIJ,
                    self.receive_counter,
                )
                .is_some();
            self.metadata.unset(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ);
            return changed;
        };
        let should_emit = matches!(pane_id, PaneTarget::Terminal(_)) && pane.is_selectable;
        let Some(cwd) = pane.cwd.clone().filter(|_| should_emit) else {
            let changed = self
                .metadata
                .source_entry(
                    &entity_id,
                    KEY_PANE_CWD,
                    SOURCE_ZELLIJ,
                    self.receive_counter,
                )
                .is_some();
            self.metadata.unset(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ);
            return changed;
        };
        let value = MetadataValue::Text(cwd);
        let precedence = if pane.is_focused {
            FOCUSED_CWD_PRECEDENCE
        } else {
            NORMAL_CWD_PRECEDENCE
        };
        if self
            .metadata
            .source_entry(
                &entity_id,
                KEY_PANE_CWD,
                SOURCE_ZELLIJ,
                self.receive_counter,
            )
            .is_some_and(|entry| {
                entry.value == value
                    && entry.ttl_ms.is_none()
                    && entry.precedence == precedence
                    && entry.ordinal == pane.ordinal
            })
        {
            return false;
        }
        self.receive_counter = self.receive_counter.saturating_add(1);
        self.metadata.set(
            entity_id,
            KEY_PANE_CWD,
            SOURCE_ZELLIJ,
            MetadataEntry {
                value,
                updated_at: self.receive_counter,
                ttl_ms: None,
                precedence,
                ordinal: pane.ordinal,
            },
        );
        true
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
        label: None,
    }])
}

fn metadata_path_segments_to_group_path(
    segments: Vec<andamento_shared::MetadataPathSegmentValue>,
) -> GroupPath {
    GroupPath(
        segments
            .into_iter()
            .map(|segment| GroupSegment {
                key: segment.key,
                value: segment.value.into(),
                label: segment.label,
            })
            .collect(),
    )
}

fn tab_grouping_info_for_explicit_scope(path: GroupPath) -> Option<TabGroupingInfo> {
    if path.0.is_empty() {
        return None;
    }
    let labels = path.0.iter().map(group_segment_label).collect::<Vec<_>>();
    let key = format!(
        "tab.scope:{}",
        path.0
            .iter()
            .map(|segment| format!("{}={}", segment.key, metadata_value_display(&segment.value)))
            .collect::<Vec<_>>()
            .join("/")
    );
    Some(TabGroupingInfo {
        key,
        path,
        label: labels.last().cloned().unwrap_or_default(),
        full_label: labels.join(" / "),
    })
}

fn group_path_prefixes(path: &GroupPath) -> Vec<GroupPath> {
    (1..=path.0.len())
        .map(|depth| GroupPath(path.0[..depth].to_vec()))
        .collect()
}

fn group_header_identity_for_prefix(
    prefix: &GroupPath,
    leaf_path: &GroupPath,
    leaf_key: &str,
    leaf_label: &str,
    leaf_full_label: &str,
) -> (String, String, String) {
    if prefix == leaf_path {
        return (
            leaf_key.to_owned(),
            leaf_label.to_owned(),
            leaf_full_label.to_owned(),
        );
    }
    let labels = prefix.0.iter().map(group_segment_label).collect::<Vec<_>>();
    let key = format!(
        "group:{}",
        prefix
            .0
            .iter()
            .map(|segment| format!("{}={}", segment.key, metadata_value_display(&segment.value)))
            .collect::<Vec<_>>()
            .join("/")
    );
    let label = labels.last().cloned().unwrap_or_else(|| key.clone());
    let full_label = labels.join(" / ");
    (key, label, full_label)
}

fn group_segment_label(segment: &GroupSegment) -> String {
    segment
        .label
        .clone()
        .unwrap_or_else(|| metadata_value_display(&segment.value))
}

fn group_template_metadata(
    path: &GroupPath,
    label: &str,
    full_label: &str,
    tab_count: usize,
    resolved_metadata: &[ResolvedMetadata],
) -> BTreeMap<String, MetadataValue> {
    let mut metadata = resolved_metadata_values(resolved_metadata, &EntityId::Group(path.clone()));
    metadata.insert(
        "group.label".to_owned(),
        MetadataValue::Text(label.to_owned()),
    );
    metadata.insert(
        "group.full_label".to_owned(),
        MetadataValue::Text(full_label.to_owned()),
    );
    metadata.insert(
        "group.tab_count".to_owned(),
        MetadataValue::Integer(tab_count as i64),
    );
    for segment in &path.0 {
        metadata.insert(segment.key.clone(), segment.value.clone());
    }
    metadata
}

fn tab_template_metadata(
    tab: &TabCard,
    resolved_metadata: &[ResolvedMetadata],
) -> BTreeMap<String, MetadataValue> {
    let mut metadata = resolved_metadata_values(resolved_metadata, &EntityId::Tab(tab.tab_id));
    metadata.insert(
        "zellij.tab.id".to_owned(),
        MetadataValue::Integer(tab.tab_id as i64),
    );
    metadata.insert(
        "zellij.tab.position".to_owned(),
        MetadataValue::Integer(tab.position as i64),
    );
    metadata.insert(
        "zellij.tab.name".to_owned(),
        MetadataValue::Text(tab.name.clone()),
    );
    metadata.insert(
        "zellij.tab.active".to_owned(),
        MetadataValue::Bool(tab.active),
    );
    metadata.insert(
        "rail.tab.pinned".to_owned(),
        MetadataValue::Bool(tab.pinned),
    );
    if let Some(status) = tab.status.as_ref() {
        metadata.insert(
            "status.priority".to_owned(),
            MetadataValue::Text(format!("{:?}", status.priority).to_ascii_lowercase()),
        );
        metadata.insert(
            "status.title".to_owned(),
            MetadataValue::Text(status.title.clone()),
        );
        if let Some(detail) = status.detail.as_ref() {
            metadata.insert(
                "status.detail".to_owned(),
                MetadataValue::Text(detail.clone()),
            );
        }
        metadata.insert(
            "status.source_pane".to_owned(),
            MetadataValue::Text(match status.source_pane {
                PaneTarget::Terminal(id) => format!("terminal:{id}"),
                PaneTarget::Plugin(id) => format!("plugin:{id}"),
            }),
        );
    }
    metadata
}

fn resolved_metadata_values(
    resolved_metadata: &[ResolvedMetadata],
    target: &EntityId,
) -> BTreeMap<String, MetadataValue> {
    resolved_metadata
        .iter()
        .find(|metadata| &metadata.target == target)
        .map(|metadata| {
            metadata
                .values
                .iter()
                .map(|(key, entry)| (key.clone(), entry.value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn metadata_value_display(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Text(value) => value.clone(),
        MetadataValue::Bool(value) => value.to_string(),
        MetadataValue::Integer(value) => value.to_string(),
        MetadataValue::StringList(values) => values.join(", "),
        MetadataValue::GroupPath(segments) => segments
            .iter()
            .map(|segment| {
                segment
                    .label
                    .clone()
                    .unwrap_or_else(|| segment.value.display())
            })
            .collect::<Vec<_>>()
            .join(" / "),
    }
}

fn observed_metadata_identities(
    resolved_metadata: &[ResolvedMetadata],
) -> Vec<ObservedMetadataIdentity> {
    let mut by_identity: BTreeMap<MetadataIdentity, (BTreeSet<EntityId>, usize)> = BTreeMap::new();
    for metadata in resolved_metadata {
        for reachable in &metadata.reachable_identities {
            let (targets, nearest_distance) = by_identity
                .entry(reachable.identity.clone())
                .or_insert_with(|| (BTreeSet::new(), reachable.distance));
            targets.insert(metadata.target.clone());
            *nearest_distance = (*nearest_distance).min(reachable.distance);
        }
    }
    by_identity
        .into_iter()
        .map(
            |(identity, (targets, nearest_distance))| ObservedMetadataIdentity {
                identity,
                target_count: targets.len(),
                nearest_distance,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use andamento_shared::{
        GroupPath, GroupSegment, RailGroupingMode, RailRow, RailSizingPreset, RailStructure,
        StatusIcon,
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
    fn unchanged_tab_update_is_not_a_controller_change() {
        let mut state = ControllerState::default();
        let tabs = vec![tab_info(1, 0, "work", true), tab_info(2, 1, "logs", false)];

        assert!(state.update_tabs_from_zellij(tabs.clone()));
        assert!(!state.update_tabs_from_zellij(tabs));
    }

    #[test]
    fn tab_update_change_is_a_controller_change() {
        let mut state = ControllerState::default();
        state.update_tabs_from_zellij(vec![
            tab_info(1, 0, "work", true),
            tab_info(2, 1, "logs", false),
        ]);

        assert!(state.update_tabs_from_zellij(vec![
            tab_info(1, 0, "work", false),
            tab_info(2, 1, "logs", true),
        ]));
    }

    #[test]
    fn setting_same_pane_cwd_is_not_a_metadata_change() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "work".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);

        assert!(state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into()));
        let receive_counter = state.receive_counter;

        assert!(!state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into()));
        assert_eq!(state.receive_counter, receive_counter);
    }

    #[test]
    fn duplicate_metadata_patch_is_not_a_model_change() {
        let mut state = ControllerState::default();
        let patch = andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(1),
            source_id: "watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("zellij-org/zellij".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        };

        assert!(state.apply_metadata_patch(patch.clone()));
        let receive_counter = state.receive_counter;
        assert!(!state.apply_metadata_patch(patch));
        assert_eq!(state.receive_counter, receive_counter);
    }

    #[test]
    fn duplicate_ttl_metadata_patch_refreshes_without_model_change() {
        let mut state = ControllerState::default();
        let patch = andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(1),
            source_id: "watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("zellij-org/zellij".to_owned()),
                    ttl_ms: Some(10_000),
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        };

        assert!(state.apply_metadata_patch(patch.clone()));
        let receive_counter = state.receive_counter;
        assert!(!state.apply_metadata_patch(patch));
        assert!(state.receive_counter > receive_counter);
    }

    #[test]
    fn unchanged_pane_manifest_is_not_a_controller_change() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "work".into(),
            active: true,
        }]);
        let manifest = PaneManifest {
            panes: HashMap::from([(
                0,
                vec![zellij_tile::prelude::PaneInfo {
                    id: 10,
                    is_plugin: false,
                    is_selectable: true,
                    ..Default::default()
                }],
            )]),
        };

        assert!(state.update_panes_from_manifest(manifest.clone()));
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());
        let receive_counter = state.receive_counter;

        assert!(!state.update_panes_from_manifest(manifest));
        assert_eq!(state.receive_counter, receive_counter);
    }

    #[test]
    fn cwd_refresh_only_requests_selectable_terminals_with_unknown_cwd() {
        let mut state = ControllerState::default();
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_test_pane(PaneTarget::Terminal(11), 1, true, false, 1);
        state.set_test_pane(PaneTarget::Terminal(12), 1, false, false, 2);
        state.set_test_pane(PaneTarget::Plugin(20), 1, true, false, 3);
        state.set_pane_cwd(PaneTarget::Terminal(11), "/repo/known".into());

        assert_eq!(state.terminal_panes_for_cwd_refresh(), vec![10]);
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
        assert!(matches!(
            &model.rows[1],
            RailRow::Tab {
                tab_id: 1,
                indent: 2,
                ..
            }
        ));
        assert!(matches!(
            &model.rows[2],
            RailRow::Tab {
                tab_id: 3,
                indent: 2,
                ..
            }
        ));
        assert!(matches!(
            &model.rows[3],
            RailRow::Tab {
                tab_id: 2,
                indent: 0,
                ..
            }
        ));
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
        assert!(matches!(
            &model.rows[0],
            RailRow::Tab {
                tab_id: 1,
                indent: 0,
                ..
            }
        ));
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
            label: None,
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
    fn configured_grouping_rule_uses_resolved_identity_metadata_before_directory_fallback() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::from_config(
                andamento_shared::grouping_config::ExternalGroupingConfig {
                    version: 1,
                    rules: vec![
                        andamento_shared::grouping_config::GroupingRule {
                            name: "proj-repo-branch".to_owned(),
                            priority: 100,
                            levels: vec![
                                andamento_shared::grouping_config::GroupingLevel {
                                    key: "andamento.project".to_owned(),
                                    optional: true,
                                    label_key: None,
                                },
                                andamento_shared::grouping_config::GroupingLevel {
                                    key: "git.repo".to_owned(),
                                    optional: false,
                                    label_key: Some("repo.name".to_owned()),
                                },
                                andamento_shared::grouping_config::GroupingLevel {
                                    key: "git.branch".to_owned(),
                                    optional: false,
                                    label_key: None,
                                },
                            ],
                        },
                        andamento_shared::grouping_config::GroupingRule {
                            name: "directory".to_owned(),
                            priority: 10,
                            levels: vec![andamento_shared::grouping_config::GroupingLevel {
                                key: KEY_PANE_CWD.to_owned(),
                                optional: false,
                                label_key: None,
                            }],
                        },
                    ],
                },
            ),
        ));
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/Users/robert/dev/zellij".into());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(andamento_shared::MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
            }),
            source_id: "git-watcher".to_owned(),
            set: BTreeMap::from([
                (
                    "git.repo".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("zellij-org/zellij".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "repo.name".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("zellij".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "git.branch".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("feat/kitty-image-plumbing".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        });

        let model = state.view_model();
        let grouping = model.tabs[0].grouping.as_ref().expect("tab grouping");

        assert_eq!(
            grouping.key,
            "proj-repo-branch:git.repo=zellij-org/zellij/git.branch=feat/kitty-image-plumbing"
        );
        assert_eq!(grouping.label, "feat/kitty-image-plumbing");
        assert_eq!(grouping.full_label, "zellij / feat/kitty-image-plumbing");
        assert_eq!(grouping.path.0[0].label.as_deref(), Some("zellij"));
        assert_eq!(
            grouping.path,
            GroupPath(vec![
                GroupSegment {
                    key: "git.repo".to_owned(),
                    value: MetadataValue::Text("zellij-org/zellij".to_owned()),
                    label: Some("zellij".to_owned()),
                },
                GroupSegment {
                    key: "git.branch".to_owned(),
                    value: MetadataValue::Text("feat/kitty-image-plumbing".to_owned()),
                    label: None,
                },
            ])
        );
    }

    #[test]
    fn configured_grouping_rules_fall_back_to_builtin_directory_grouping() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::from_config(
                andamento_shared::grouping_config::ExternalGroupingConfig {
                    version: 1,
                    rules: vec![andamento_shared::grouping_config::GroupingRule {
                        name: "repo".to_owned(),
                        priority: 100,
                        levels: vec![andamento_shared::grouping_config::GroupingLevel {
                            key: "git.repo".to_owned(),
                            optional: false,
                            label_key: None,
                        }],
                    }],
                },
            ),
        ));
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/Users/robert/dev/zellij".into());

        let model = state.view_model();
        let grouping = model.tabs[0].grouping.as_ref().expect("tab grouping");

        assert_eq!(grouping.key, "cwd:/Users/robert/dev/zellij");
        assert_eq!(grouping.label, "zellij");
        assert_eq!(grouping.full_label, "/Users/robert/dev/zellij");
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
    fn explicit_tab_scope_group_path_overrides_cwd_grouping_identity() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "overview".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/zellij".into());
        let scope = vec![andamento_shared::MetadataPathSegmentValue {
            key: "git.repo".to_owned(),
            value: andamento_shared::MetadataPathValue::Text("zellij-org/zellij".to_owned()),
            label: Some("zellij".to_owned()),
        }];
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                KEY_TAB_SCOPE.to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::GroupPath(scope),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let grouping = model.tabs[0].grouping.as_ref().expect("tab grouping");

        assert_eq!(grouping.label, "zellij");
        assert_eq!(
            grouping.path,
            GroupPath(vec![GroupSegment {
                key: "git.repo".to_owned(),
                value: MetadataValue::Text("zellij-org/zellij".to_owned()),
                label: Some("zellij".to_owned()),
            }])
        );
        assert!(matches!(
            &model.rows[0],
            RailRow::GroupHeader { path, .. } if path == &grouping.path
        ));
    }

    #[test]
    fn text_tab_scope_metadata_does_not_override_grouping() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "overview".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/zellij".into());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                KEY_TAB_SCOPE.to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("project:zellij".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let grouping = model.tabs[0].grouping.as_ref().expect("tab grouping");

        assert_eq!(grouping.key, "cwd:/repo/zellij");
        assert_eq!(grouping.path, cwd_group_path("/repo/zellij"));
    }

    #[test]
    fn explicit_group_metadata_resolves_for_scope_group_without_cwd() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "overview".into(),
            active: true,
        }]);
        let group_path = GroupPath(vec![GroupSegment {
            key: "git.repo".to_owned(),
            value: MetadataValue::Text("zellij-org/zellij".to_owned()),
            label: Some("zellij".to_owned()),
        }]);
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                KEY_TAB_SCOPE.to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::GroupPath(vec![
                        andamento_shared::MetadataPathSegmentValue {
                            key: "git.repo".to_owned(),
                            value: andamento_shared::MetadataPathValue::Text(
                                "zellij-org/zellij".to_owned(),
                            ),
                            label: Some("zellij".to_owned()),
                        },
                    ]),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Group(group_path.clone()),
            source_id: "flotilla".to_owned(),
            set: BTreeMap::from([(
                "group.summary".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("build running".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let group_metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == EntityId::Group(group_path.clone()))
            .expect("group metadata");

        assert_eq!(
            group_metadata
                .values
                .get("group.summary")
                .map(|entry| &entry.value),
            Some(&MetadataValue::Text("build running".to_owned()))
        );
    }

    #[test]
    fn resolved_tab_metadata_follows_transitive_identity_facts() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(1),
            source_id: "dir-watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(andamento_shared::MetadataIdentity {
                key: "git.repo".to_owned(),
                value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
            }),
            source_id: "gh".to_owned(),
            set: BTreeMap::from([(
                "vcs.pr".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("#45".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(andamento_shared::MetadataIdentity {
                key: "vcs.pr".to_owned(),
                value: MetadataValue::Text("#45".to_owned()),
            }),
            source_id: "ci".to_owned(),
            set: BTreeMap::from([(
                "ci.status".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("failing".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let tab_metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == EntityId::Tab(1))
            .expect("tab metadata");

        assert_eq!(
            tab_metadata
                .values
                .get("git.repo")
                .map(|entry| &entry.value),
            Some(&MetadataValue::Text("rjwittams/katzensteg".to_owned()))
        );
        assert_eq!(
            tab_metadata.values.get("vcs.pr").map(|entry| &entry.value),
            Some(&MetadataValue::Text("#45".to_owned()))
        );
        assert_eq!(
            tab_metadata
                .values
                .get("ci.status")
                .map(|entry| &entry.value),
            Some(&MetadataValue::Text("failing".to_owned()))
        );
        assert_eq!(
            tab_metadata.reachable_identities,
            vec![
                ReachableMetadataIdentity {
                    identity: MetadataIdentity {
                        key: "git.repo".to_owned(),
                        value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                    },
                    distance: 1,
                },
                ReachableMetadataIdentity {
                    identity: MetadataIdentity {
                        key: "vcs.pr".to_owned(),
                        value: MetadataValue::Text("#45".to_owned()),
                    },
                    distance: 2,
                },
                ReachableMetadataIdentity {
                    identity: MetadataIdentity {
                        key: "ci.status".to_owned(),
                        value: MetadataValue::Text("failing".to_owned()),
                    },
                    distance: 3,
                },
            ]
        );
    }

    #[test]
    fn resolved_tab_metadata_follows_selected_cwd_identity_facts() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(1), 1, true, true, 0);
        state.set_pane_cwd(
            PaneTarget::Terminal(1),
            "/Users/robert/dev/katzensteg".to_owned(),
        );
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text("/Users/robert/dev/katzensteg".to_owned()),
            }),
            source_id: "dir-watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let tab_metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == EntityId::Tab(1))
            .expect("tab metadata");

        assert_eq!(
            tab_metadata
                .values
                .get("git.repo")
                .map(|entry| &entry.value),
            Some(&MetadataValue::Text("rjwittams/katzensteg".to_owned()))
        );
        assert!(tab_metadata
            .reachable_identities
            .iter()
            .any(|reachable| reachable.identity
                == MetadataIdentity {
                    key: KEY_PANE_CWD.to_owned(),
                    value: MetadataValue::Text("/Users/robert/dev/katzensteg".to_owned()),
                }
                && reachable.distance == 1));
    }

    #[test]
    fn resolved_group_metadata_follows_group_path_identity_facts() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(1), 1, true, true, 0);
        let cwd = "/Users/robert/dev/katzensteg".to_owned();
        state.set_pane_cwd(PaneTarget::Terminal(1), cwd.clone());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text(cwd.clone()),
            }),
            source_id: "dir-watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let group_path = cwd_group_path(&cwd);
        let group_metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == EntityId::Group(group_path.clone()))
            .expect("group metadata");

        assert_eq!(
            group_metadata
                .values
                .get("git.repo")
                .map(|entry| &entry.value),
            Some(&MetadataValue::Text("rjwittams/katzensteg".to_owned()))
        );
        assert!(group_metadata
            .reachable_identities
            .iter()
            .any(|reachable| reachable.identity
                == MetadataIdentity {
                    key: KEY_PANE_CWD.to_owned(),
                    value: MetadataValue::Text(cwd.clone()),
                }
                && reachable.distance == 1));
    }

    #[test]
    fn view_model_resolves_group_header_template_fields_from_group_metadata() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"
                    template "git.group-header" slot="group-header" node-kind="group" {
                      when exists="git.repo"
                      field key="git.repo" priority=100
                      field key="git.branch" priority=60
                    }
                    "#,
                )
                .expect("valid template config"),
            ),
        ));
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(1), 1, true, true, 0);
        let cwd = "/Users/robert/dev/katzensteg".to_owned();
        state.set_pane_cwd(PaneTarget::Terminal(1), cwd.clone());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text(cwd),
            }),
            source_id: "git-watcher".to_owned(),
            set: BTreeMap::from([
                (
                    "git.repo".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "git.branch".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("main".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        });

        let model = state.view_model();
        let group_slot = model
            .rows
            .iter()
            .find_map(|row| match row {
                RailRow::GroupHeader { templates, .. } => templates.group_header.as_ref(),
                RailRow::Tab { .. } => None,
            })
            .expect("group header template");

        assert_eq!(group_slot.template_name, "git.group-header");
        assert_eq!(
            group_slot
                .fields
                .iter()
                .map(|field| (field.text.as_str(), field.priority))
                .collect::<Vec<_>>(),
            vec![("rjwittams/katzensteg", 100), ("main", 60)]
        );
    }

    #[test]
    fn view_model_resolves_metadata_and_templates_for_each_group_path_prefix() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::from_config(
                andamento_shared::grouping_config::ExternalGroupingConfig {
                    version: 1,
                    rules: vec![andamento_shared::grouping_config::GroupingRule {
                        name: "project-repo-branch".to_owned(),
                        priority: 100,
                        levels: vec![
                            andamento_shared::grouping_config::GroupingLevel {
                                key: "andamento.project".to_owned(),
                                optional: false,
                                label_key: None,
                            },
                            andamento_shared::grouping_config::GroupingLevel {
                                key: "git.repo".to_owned(),
                                optional: false,
                                label_key: Some("repo.name".to_owned()),
                            },
                            andamento_shared::grouping_config::GroupingLevel {
                                key: "git.branch".to_owned(),
                                optional: false,
                                label_key: None,
                            },
                        ],
                    }],
                },
            ),
        ));
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"
                    template "project.group-header" slot="group-header" node-kind="group" {
                      when exists="andamento.project"
                      field key="andamento.project" priority=100
                    }
                    template "repo.group-header" slot="group-header" node-kind="group" {
                      when exists="git.repo"
                      when exists="andamento.project"
                      field key="git.repo" priority=100
                    }
                    template "branch.group-header" slot="group-header" node-kind="group" {
                      when exists="git.repo"
                      when exists="andamento.project"
                      when exists="git.branch"
                      field key="git.branch" priority=100
                    }
                    "#,
                )
                .expect("valid template config"),
            ),
        ));
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(1), 1, true, true, 0);
        let cwd = "/Users/robert/dev/zellij".to_owned();
        state.set_pane_cwd(PaneTarget::Terminal(1), cwd.clone());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text(cwd),
            }),
            source_id: "git-watcher".to_owned(),
            set: BTreeMap::from([
                (
                    "andamento.project".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("zellij".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "git.repo".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("zellij-org/zellij".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "repo.name".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("zellij".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "git.branch".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("feat/kitty-image-plumbing".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        });

        let model = state.view_model();
        let group_headers = model
            .rows
            .iter()
            .filter_map(|row| match row {
                RailRow::GroupHeader {
                    path, templates, ..
                } => Some((path, templates.group_header.as_ref())),
                RailRow::Tab { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(group_headers.len(), 3);
        assert_eq!(
            group_headers
                .iter()
                .map(|(path, _)| path.0.len())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            group_headers
                .iter()
                .map(|(_, slot)| slot
                    .expect("resolved group template")
                    .template_name
                    .as_str())
                .collect::<Vec<_>>(),
            vec![
                "project.group-header",
                "repo.group-header",
                "branch.group-header"
            ]
        );
        for (path, _) in group_headers {
            assert!(model
                .resolved_metadata
                .iter()
                .any(|metadata| metadata.target == EntityId::Group(path.clone())));
        }
    }

    #[test]
    fn grouped_rows_anchor_tabs_to_their_exact_group_path() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::from_config(
                andamento_shared::grouping_config::ExternalGroupingConfig {
                    version: 1,
                    rules: vec![andamento_shared::grouping_config::GroupingRule {
                        name: "repo-branch".to_owned(),
                        priority: 100,
                        levels: vec![
                            andamento_shared::grouping_config::GroupingLevel {
                                key: "git.repo".to_owned(),
                                optional: false,
                                label_key: None,
                            },
                            andamento_shared::grouping_config::GroupingLevel {
                                key: "git.branch".to_owned(),
                                optional: true,
                                label_key: None,
                            },
                        ],
                    }],
                },
            ),
        ));
        state.update_tabs(vec![
            ControllerTab {
                tab_id: 1,
                position: 0,
                name: "repo-overview".into(),
                active: true,
            },
            ControllerTab {
                tab_id: 2,
                position: 1,
                name: "branch-agent".into(),
                active: false,
            },
        ]);
        for (pane_id, tab_id, cwd, branch) in [
            (
                PaneTarget::Terminal(10),
                1,
                "/Users/robert/dev/zellij",
                None,
            ),
            (
                PaneTarget::Terminal(20),
                2,
                "/Users/robert/dev/zellij-feature",
                Some("feat/kitty-image-plumbing"),
            ),
        ] {
            state.set_test_pane(pane_id, tab_id, true, tab_id == 1, 0);
            state.set_pane_cwd(pane_id, cwd.to_owned());
            let mut set = BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("zellij-org/zellij".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]);
            if let Some(branch) = branch {
                set.insert(
                    "git.branch".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text(branch.to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                );
            }
            state.apply_metadata_patch(andamento_shared::MetadataPatch {
                target: EntityId::Identity(MetadataIdentity {
                    key: KEY_PANE_CWD.to_owned(),
                    value: MetadataValue::Text(cwd.to_owned()),
                }),
                source_id: "git-watcher".to_owned(),
                set,
                unset: vec![],
            });
        }

        let model = state.view_model();
        let repo_path = GroupPath(vec![GroupSegment {
            key: "git.repo".to_owned(),
            value: MetadataValue::Text("zellij-org/zellij".to_owned()),
            label: None,
        }]);
        let branch_path = GroupPath(vec![
            repo_path.0[0].clone(),
            GroupSegment {
                key: "git.branch".to_owned(),
                value: MetadataValue::Text("feat/kitty-image-plumbing".to_owned()),
                label: None,
            },
        ]);

        assert!(model
            .rows
            .iter()
            .any(|row| matches!(row, RailRow::GroupHeader { path, .. } if path == &repo_path)));
        assert!(model
            .rows
            .iter()
            .any(|row| matches!(row, RailRow::GroupHeader { path, .. } if path == &branch_path)));
        assert!(model.rows.iter().any(|row| {
            matches!(
                row,
                RailRow::Tab {
                    tab_id: 1,
                    parent_path: Some(parent_path),
                    ..
                } if parent_path == &repo_path
            )
        }));
        assert!(model.rows.iter().any(|row| {
            matches!(
                row,
                RailRow::Tab {
                    tab_id: 2,
                    parent_path: Some(parent_path),
                    ..
                } if parent_path == &branch_path
            )
        }));
    }

    #[test]
    fn view_model_exposes_observed_metadata_identity_index() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig {
            grouping: RailGroupingMode::Directory,
            ..RailConfig::default()
        });
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(1), 1, true, true, 0);
        let cwd = "/Users/robert/dev/katzensteg".to_owned();
        state.set_pane_cwd(PaneTarget::Terminal(1), cwd.clone());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Identity(MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text(cwd.clone()),
            }),
            source_id: "dir-watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();

        assert!(model.observed_identities.iter().any(|observed| {
            observed.identity
                == MetadataIdentity {
                    key: KEY_PANE_CWD.to_owned(),
                    value: MetadataValue::Text(cwd.clone()),
                }
                && observed.target_count == 2
                && observed.nearest_distance == 1
        }));
        assert!(model.observed_identities.iter().any(|observed| {
            observed.identity
                == MetadataIdentity {
                    key: "git.repo".to_owned(),
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                }
                && observed.target_count == 2
                && observed.nearest_distance == 2
        }));
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
    fn bootstrap_snapshot_carries_metadata_patches() {
        let mut source = ControllerState::default();
        source.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(7),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                KEY_TAB_SCOPE.to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text("project:zellij".to_owned()),
                    ttl_ms: None,
                    precedence: Some(4),
                    ordinal: Some(2),
                },
            )]),
            unset: vec![],
        });

        let snapshot = source.bootstrap_snapshot();
        let mut target = ControllerState::default();
        target.apply_bootstrap_snapshot(snapshot);
        target.update_tabs(vec![ControllerTab {
            tab_id: 7,
            position: 0,
            name: "overview".into(),
            active: true,
        }]);

        let model = target.view_model();
        let metadata = model
            .resolved_metadata
            .iter()
            .find(|metadata| metadata.target == EntityId::Tab(7))
            .expect("tab metadata");

        assert_eq!(
            metadata.values.get(KEY_TAB_SCOPE).map(|entry| (
                &entry.value,
                entry.precedence,
                entry.ordinal
            )),
            Some((&MetadataValue::Text("project:zellij".to_owned()), 4, 2))
        );
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

        assert_eq!(state.known_rail_count(), 1);
        assert_eq!(state.known_config_editor_count(), 0);
        assert_eq!(state.rail_plugin_ids(), vec![8]);
    }

    #[test]
    fn config_editor_target_prefers_same_client() {
        let mut state = ControllerState::default();
        state.register_config_editor(RendererHello {
            plugin_id: 30,
            client_id: 1,
        });
        state.register_config_editor(RendererHello {
            plugin_id: 31,
            client_id: 2,
        });

        assert_eq!(
            state.config_editor_target_for_client(2),
            Some(RendererHello {
                plugin_id: 31,
                client_id: 2
            })
        );
    }
}
