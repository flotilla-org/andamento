use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

use crate::metadata::{select_primary_entry, CandidateEntry, EntityId, MetadataStore};
use andamento_shared::grouping_config::{GroupingConfigCatalog, GroupingRule, PresenceClass};
use andamento_shared::{
    ControllerBootstrapSnapshot, ControllerViewModel, DisplayEntity, DisplayVariableValue,
    EffectiveNodeVariables, EffectiveVariableValue, EntityRef, GroupPath, GroupSegment,
    GroupingRuleDiagnostic, LatentMaterializationState, LatentTab, MetadataControls, MetadataEntry,
    MetadataIdentity, MetadataSourceEntry, MetadataTriState, MetadataValue, NodeKey,
    ObservedMetadataIdentity, PaneTarget, PluginPlacement, PluginRegistrationHello, Priority,
    RailConfig, RailRow, RailUiAction, RailUiRevision, RailUiState, ReachableMetadataIdentity,
    RendererHello, ResolvedMetadata, ResolvedTemplateSlot, ResolvedTemplateSlots, SetPaneStatus,
    SortMode, TabCard, TabGroupingInfo, TabStatusSummary, TemplateConfigDiagnostics,
    VariableSetterProvenance, DISPLAY_FORM_COMPACT, DISPLAY_FORM_FULL,
    NODE_VARIABLE_CONFIG_OVERRIDE_SETTER,
};
use zellij_tile::prelude::{PaneManifest, TabInfo};

const SOURCE_ZELLIJ: &str = "zellij";
const SOURCE_LATENT_MATERIALIZER: &str = "andamento-latent-materializer";
const DIRECTORY_GROUPING_RULE: &str = "zellij.directory";
const KEY_PANE_CWD: &str = "zellij.pane.cwd";
const KEY_PANE_CWD_LABEL: &str = "zellij.pane.cwd.label";
const KEY_ENTITY_KIND: &str = "entity.kind";
const KEY_ENTITY_ID: &str = "entity.id";
const KEY_ACTION_TARGET: &str = "action.primary.target";
const KEY_MATERIALIZE_RECIPE: &str = "action.primary.recipe";
const KEY_CHECKOUT_PATH: &str = "git.root";
const KEY_STATUS_STATE: &str = "status.state";
const KEY_SUMMARY_TEXT: &str = "summary.text";
const KEY_SOURCE: &str = "source";
const KEY_DISPLAY_LABEL: &str = "display.label";
const FOCUSED_CWD_PRECEDENCE: i64 = 100;
const NORMAL_CWD_PRECEDENCE: i64 = 0;
// Opener-owned identity must outrank observational discovery such as cwd grouping.
const LATENT_MATERIALIZER_PRECEDENCE: i64 = 1_000;

type TabSeedMetadata = HashMap<u64, BTreeMap<String, MetadataEntry>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerTab {
    pub tab_id: u64,
    pub position: usize,
    pub name: String,
    pub active: bool,
}

/// THROWAWAY ADAPTER. Bends a placed entity into the `DisplayEntity` the
/// current renderer expects, so the placement pipeline can render before the
/// renderer learns about placements.
///
/// Do not build on this and do not widen it. It exists only until the renderer
/// takes placements directly; treating it as the interface is how the old
/// grouping shapes would survive the cutover.
fn placed_display_entity(
    mut display: andamento_shared::DisplayEntity,
    loop_definition: &andamento_shared::template_config::PlacementLoop,
    form: &str,
) -> andamento_shared::DisplayEntity {
    display.form = form.to_owned();
    if !loop_definition.fields.is_empty() {
        display.templates.compact = Some(andamento_shared::ResolvedTemplateSlot {
            template_name: format!("placement:{}", loop_definition.binding),
            fields: vec![],
            render_ready: Some(
                andamento_shared::template_config::TemplateConfigRenderReady {
                    fields: loop_definition.fields.clone(),
                    controls: vec![],
                    chrome: Default::default(),
                },
            ),
            setters: vec![],
            effective_kdl: String::new(),
            resolve_error: None,
        });
        display.templates.detail = display.templates.compact.clone();
    }
    display
}

/// Catalog entities indexed by `(fact key, indexable text)`.
///
/// Built once per model. Every placement predicate is answered by a lookup
/// here; nothing scans the catalog.
#[derive(Debug, Default)]
struct PlacementIndex {
    by_fact: BTreeMap<(String, String), Vec<usize>>,
}

impl PlacementIndex {
    fn build(entities: &[CatalogEntity]) -> Self {
        let mut by_fact = BTreeMap::<(String, String), Vec<usize>>::new();
        for (position, entity) in entities.iter().enumerate() {
            let mut insert = |key: String, text: String| {
                let postings = by_fact.entry((key, text)).or_default();
                // Positions arrive ascending, so binary_search stays valid; the
                // guard only stops an entity appearing twice under one fact.
                if postings.last() != Some(&position) {
                    postings.push(position);
                }
            };
            // An entity's kind and id come from its ref, not from whatever the
            // producer happened to echo into its facts, so `kind=` always works.
            insert("entity.kind".to_owned(), entity.entity.kind.clone());
            insert("entity.id".to_owned(), entity.entity.id.clone());
            for (key, entry) in &entity.values {
                let Some(text) =
                    andamento_shared::template_config::placement_index_text(&entry.value)
                else {
                    continue;
                };
                insert(key.clone(), text);
            }
        }
        Self { by_fact }
    }

    fn lookup_value(&self, key: &str, value: &str) -> &[usize] {
        self.by_fact
            .get(&(key.to_owned(), value.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// What a node resolved to: the variable values in effect, and the declarations
/// that are in scope there.
#[derive(Debug, Clone, Default)]
struct ResolvedNodeVariables {
    values: BTreeMap<String, EffectiveVariableValue>,
    declarations: Vec<andamento_shared::template_config::NodeVariableDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityActivation {
    FocusTab { position: usize },
    Materialize(andamento_shared::MaterializeLatentRequest),
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
    // One host-call attempt per pane lifetime; afterwards cwd arrives only
    // via Event::CwdChanged, so a failed lookup is never retried per event.
    cwd_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingLatentMaterialization {
    request: andamento_shared::MaterializeLatentRequest,
    tab_id: Option<u64>,
}

#[derive(Debug, Clone)]
struct CatalogEntity {
    entity: EntityRef,
    values: BTreeMap<String, MetadataEntry>,
    ordinal: i64,
    path: GroupPath,
    presence: PresenceClass,
    form: String,
    visible_when: Option<String>,
    template: Option<String>,
    group_templates: BTreeMap<String, String>,
    grouping_priority: i64,
    collapse_single_member: bool,
    show_empty: bool,
}

#[derive(Debug, Default)]
struct ControllerRailUiState {
    revision: RailUiRevision,
    collapsed_groups: BTreeSet<GroupPath>,
    scroll_offset: isize,
    variables: BTreeMap<String, DisplayVariableValue>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControllerClientState {
    pub client_id: u16,
    pub inspected_node: Option<NodeKey>,
    pub metadata_controls: MetadataControls,
    pub tabs: BTreeMap<u64, ControllerClientTabState>,
    pub background_rails: BTreeMap<u32, PluginRegistration>,
    pub background_config_editors: BTreeMap<u32, PluginRegistration>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControllerClientTabState {
    pub tab_id: u64,
    pub rails: BTreeMap<u32, PluginRegistration>,
    pub config_editors: BTreeMap<u32, PluginRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRegistration {
    pub identity: RendererHello,
    pub placement: PluginPlacement,
}

#[derive(Debug, Default)]
pub struct ControllerState {
    tabs: Vec<ControllerTab>,
    pane_to_tab: HashMap<PaneTarget, u64>,
    panes: HashMap<PaneTarget, ControllerPane>,
    metadata: MetadataStore,
    pane_statuses: HashMap<PaneTarget, StoredPaneStatus>,
    pinned_tabs: HashSet<u64>,
    clients: BTreeMap<u16, ControllerClientState>,
    sort_mode: SortMode,
    rail_config: RailConfig,
    grouping_catalog: Option<GroupingConfigCatalog>,
    active_grouping_template: Option<String>,
    template_catalog: Option<andamento_shared::template_config::TemplateConfigCatalog>,
    template_config: TemplateConfigDiagnostics,
    receive_counter: u64,
    pending_materialized_tab_names: HashMap<u64, String>,
    pending_latent_materializations: BTreeMap<String, PendingLatentMaterialization>,
    rail_ui: ControllerRailUiState,
    rail_ui_writer_client_id: u16,
    node_variable_overrides: BTreeMap<NodeKey, BTreeMap<String, String>>,
}

impl ControllerState {
    #[allow(dead_code)]
    pub fn client(&self, client_id: u16) -> Option<&ControllerClientState> {
        self.clients.get(&client_id)
    }

    fn client_mut(&mut self, client_id: u16) -> &mut ControllerClientState {
        self.clients
            .entry(client_id)
            .or_insert_with(|| ControllerClientState {
                client_id,
                ..Default::default()
            })
    }

    #[allow(dead_code)]
    pub fn update_tabs_from_zellij(&mut self, tabs: Vec<TabInfo>) -> bool {
        let mut pending_materialized_tab_names =
            std::mem::take(&mut self.pending_materialized_tab_names);
        let mut next_tabs = Vec::with_capacity(tabs.len());
        for tab in tabs {
            let tab_id = tab.tab_id as u64;
            let default_name = format!("Tab {}", tab.position + 1);
            let incoming_name = if tab.name.is_empty() {
                default_name.clone()
            } else {
                tab.name
            };
            let name = match pending_materialized_tab_names.get(&tab_id).cloned() {
                Some(expected_name) if incoming_name == expected_name => {
                    pending_materialized_tab_names.remove(&tab_id);
                    incoming_name
                }
                Some(expected_name) if incoming_name == default_name => expected_name,
                Some(_) => {
                    pending_materialized_tab_names.remove(&tab_id);
                    incoming_name
                }
                None => incoming_name,
            };
            next_tabs.push(ControllerTab {
                tab_id: tab.tab_id as u64,
                position: tab.position,
                name,
                active: tab.active,
            });
        }
        next_tabs.sort_by_key(|tab| tab.position);
        let live_tab_ids: HashSet<u64> = next_tabs.iter().map(|tab| tab.tab_id).collect();
        pending_materialized_tab_names.retain(|tab_id, _| live_tab_ids.contains(tab_id));
        self.pending_materialized_tab_names = pending_materialized_tab_names;
        let tabs_changed = self.tabs != next_tabs;
        self.tabs = next_tabs;

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
        let claimed_materialization = self.claim_materializing_tabs();
        tabs_changed || claimed_materialization
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
                        cwd_requested: previous_panes
                            .get(&pane_target)
                            .is_some_and(|previous| previous.cwd_requested),
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
                cwd_requested: false,
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
            .filter(|pane| pane.cwd.is_none() && !pane.cwd_requested)
            .filter_map(|pane| match pane.pane_id {
                PaneTarget::Terminal(id) => Some(id),
                PaneTarget::Plugin(_) => None,
            })
            .collect();
        terminal_ids.sort_unstable();
        terminal_ids
    }

    pub fn mark_pane_cwd_requested(&mut self, pane_id: PaneTarget) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.cwd_requested = true;
        }
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

    pub fn set_metadata_visibility_for_client(
        &mut self,
        client_id: u16,
        key: NodeKey,
        state: Option<MetadataTriState>,
    ) {
        let controls = &mut self.client_mut(client_id).metadata_controls;
        if let Some(state) = state {
            controls.per_node.insert(key, state);
        } else {
            controls.per_node.remove(&key);
        }
    }

    pub fn set_node_variable(
        &mut self,
        node: NodeKey,
        name: String,
        value: Option<String>,
    ) -> bool {
        let previous = self
            .node_variable_overrides
            .get(&node)
            .and_then(|values| values.get(&name))
            .cloned();
        if previous == value {
            return false;
        }
        if let Some(value) = value {
            self.node_variable_overrides
                .entry(node)
                .or_default()
                .insert(name, value);
        } else if let Some(values) = self.node_variable_overrides.get_mut(&node) {
            values.remove(&name);
            if values.is_empty() {
                self.node_variable_overrides.remove(&node);
            }
        }
        true
    }

    pub fn set_rail_ui_writer_client_id(&mut self, client_id: u16) {
        self.rail_ui_writer_client_id = client_id;
    }

    pub fn apply_rail_ui_action(&mut self, action: RailUiAction) -> bool {
        let toggle_definition = if let RailUiAction::ToggleVariable { name } = &action {
            let Some(definition) = self
                .template_catalog
                .as_ref()
                .and_then(|catalog| {
                    catalog
                        .display_variables()
                        .iter()
                        .find(|item| &item.name == name)
                })
                .cloned()
            else {
                return false;
            };
            Some(definition)
        } else {
            None
        };
        self.rail_ui.revision = RailUiRevision {
            sequence: self.rail_ui.revision.sequence.saturating_add(1),
            writer_client_id: self.rail_ui_writer_client_id,
        };
        match action {
            RailUiAction::ToggleGroup { path } => {
                if !self.rail_ui.collapsed_groups.insert(path.clone()) {
                    self.rail_ui.collapsed_groups.remove(&path);
                }
            }
            RailUiAction::ToggleVariable { name } => {
                let definition = toggle_definition.expect("toggle definition resolved above");
                let current = self
                    .rail_ui
                    .variables
                    .get(&name)
                    .unwrap_or(&definition.default);
                let next = match (&definition.variable_type, current) {
                    (
                        andamento_shared::template_config::TemplateVariableType::Bool,
                        DisplayVariableValue::Bool(value),
                    ) => DisplayVariableValue::Bool(!value),
                    (
                        andamento_shared::template_config::TemplateVariableType::Enum { values },
                        DisplayVariableValue::Enum(value),
                    ) => {
                        let next = values
                            .iter()
                            .position(|candidate| candidate == value)
                            .map(|index| (index + 1) % values.len())
                            .unwrap_or(0);
                        DisplayVariableValue::Enum(
                            values.get(next).cloned().unwrap_or_else(|| value.clone()),
                        )
                    }
                    _ => definition.default.clone(),
                };
                self.rail_ui.variables.insert(name, next);
            }
            RailUiAction::ScrollBy { delta } => {
                self.rail_ui.scroll_offset = self.rail_ui.scroll_offset.saturating_add(delta);
            }
            RailUiAction::SetScrollOffset { offset } => {
                self.rail_ui.scroll_offset = offset;
            }
            RailUiAction::ResetScroll => {
                self.rail_ui.scroll_offset = 0;
            }
        }
        true
    }

    pub fn rail_ui_state(&self) -> RailUiState {
        RailUiState {
            revision: self.rail_ui.revision,
            collapsed_groups: self.rail_ui.collapsed_groups.iter().cloned().collect(),
            scroll_offset: self.rail_ui.scroll_offset,
            variables: self.rail_ui.variables.clone(),
        }
    }

    pub fn apply_rail_ui_state(&mut self, state: RailUiState) -> bool {
        if state.revision <= self.rail_ui.revision {
            return false;
        }
        self.rail_ui = ControllerRailUiState {
            revision: state.revision,
            collapsed_groups: state.collapsed_groups.into_iter().collect(),
            scroll_offset: state.scroll_offset,
            variables: state.variables,
        };
        true
    }

    pub fn set_template_catalog(
        &mut self,
        catalog: Option<andamento_shared::template_config::TemplateConfigCatalog>,
    ) {
        self.template_catalog = Some(catalog.unwrap_or_default());
        if let Some(catalog) = self.template_catalog.as_ref() {
            for variable in catalog.display_variables() {
                self.rail_ui
                    .variables
                    .entry(variable.name.clone())
                    .or_insert_with(|| variable.default.clone());
            }
        }
        self.refresh_display_variable_warnings();
    }

    pub fn set_grouping_catalog(&mut self, catalog: Option<GroupingConfigCatalog>) {
        self.grouping_catalog = Some(catalog.unwrap_or_default());
        self.refresh_display_variable_warnings();
    }

    #[allow(dead_code)]
    pub fn set_active_grouping_template(&mut self, name: Option<String>) -> bool {
        if name.as_ref().is_some_and(|name| {
            let configured = self
                .grouping_catalog
                .as_ref()
                .is_some_and(|catalog| catalog.rules.iter().any(|rule| &rule.name == name));
            let bundled = GroupingConfigCatalog::default()
                .rules
                .iter()
                .any(|rule| &rule.name == name);
            !(configured || bundled)
        }) {
            return false;
        }
        let changed = self.active_grouping_template != name;
        self.active_grouping_template = name;
        changed
    }

    pub fn set_template_config_diagnostics(&mut self, diagnostics: TemplateConfigDiagnostics) {
        self.template_config = diagnostics;
    }

    pub fn template_config_diagnostics(&self) -> &TemplateConfigDiagnostics {
        &self.template_config
    }

    pub fn refresh_display_variable_warnings(&mut self) {
        let default_grouping_catalog = GroupingConfigCatalog::default();
        let grouping_catalog = self
            .grouping_catalog
            .as_ref()
            .unwrap_or(&default_grouping_catalog);
        let declared = self
            .template_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .display_variables()
                    .iter()
                    .map(|variable| (variable.name.as_str(), &variable.variable_type))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        self.template_config.warnings = grouping_catalog
            .rules
            .iter()
            .flat_map(|rule| &rule.presence)
            .filter_map(|mapping| {
                let name = mapping.visible_when.as_deref()?;
                match declared.get(name) {
                    Some(andamento_shared::template_config::TemplateVariableType::Bool) => None,
                    Some(_) => Some(format!("visible-when references non-bool variable {name}")),
                    None => Some(format!(
                        "visible-when references undeclared variable {name}"
                    )),
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
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
            node_variable_overrides: self
                .node_variable_overrides
                .iter()
                .map(|(node, values)| andamento_shared::NodeVariableOverrides {
                    node: node.clone(),
                    values: values.clone(),
                })
                .collect(),
            metadata_patches: self.metadata.snapshot_patches(self.receive_counter),
            rail_ui_state: RailUiState {
                variables: self
                    .template_catalog
                    .as_ref()
                    .map(|catalog| {
                        catalog
                            .display_variables()
                            .iter()
                            .filter(|variable| variable.persist)
                            .filter_map(|variable| {
                                self.rail_ui
                                    .variables
                                    .get(&variable.name)
                                    .cloned()
                                    .map(|value| (variable.name.clone(), value))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                ..self.rail_ui_state()
            },
        }
    }

    pub fn apply_bootstrap_snapshot(&mut self, snapshot: ControllerBootstrapSnapshot) {
        self.sort_mode = snapshot.sort_mode;
        self.rail_config = snapshot.config;
        self.pinned_tabs.extend(snapshot.pinned_tabs);
        for overrides in snapshot.node_variable_overrides {
            self.node_variable_overrides
                .entry(overrides.node)
                .or_default()
                .extend(overrides.values);
        }
        for status in snapshot.pane_statuses {
            self.set_status(status);
        }
        for patch in snapshot.metadata_patches {
            self.apply_metadata_patch(patch);
        }
        self.apply_rail_ui_state(snapshot.rail_ui_state);
    }

    fn plugin_registration(&self, plugin_id: u32) -> Option<&PluginRegistration> {
        for client in self.clients.values() {
            if let Some(registration) = client.background_rails.get(&plugin_id) {
                return Some(registration);
            }
            if let Some(registration) = client.background_config_editors.get(&plugin_id) {
                return Some(registration);
            }
            for tab in client.tabs.values() {
                if let Some(registration) = tab.rails.get(&plugin_id) {
                    return Some(registration);
                }
                if let Some(registration) = tab.config_editors.get(&plugin_id) {
                    return Some(registration);
                }
            }
        }
        None
    }

    #[allow(dead_code)]
    pub fn register_rail(&mut self, hello: PluginRegistrationHello) -> bool {
        let registration = PluginRegistration {
            identity: hello.identity,
            placement: hello.placement,
        };
        if self.plugin_registration(registration.identity.plugin_id) == Some(&registration) {
            return false;
        }
        self.unregister_renderer(registration.identity.plugin_id);
        let client = self.client_mut(registration.identity.client_id);
        match registration.placement {
            PluginPlacement::Tab { tab_id, .. } => {
                let tab = client
                    .tabs
                    .entry(tab_id)
                    .or_insert_with(|| ControllerClientTabState {
                        tab_id,
                        ..Default::default()
                    });
                tab.rails
                    .insert(registration.identity.plugin_id, registration);
            }
            PluginPlacement::Background | PluginPlacement::Unknown => {
                client
                    .background_rails
                    .insert(registration.identity.plugin_id, registration);
            }
        }
        true
    }

    #[allow(dead_code)]
    pub fn retain_rails(&mut self, live_plugin_ids: &HashSet<u32>) {
        for client in self.clients.values_mut() {
            client
                .background_rails
                .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
            client
                .background_config_editors
                .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
            for tab in client.tabs.values_mut() {
                tab.rails
                    .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
                tab.config_editors
                    .retain(|plugin_id, _| live_plugin_ids.contains(plugin_id));
            }
            client
                .tabs
                .retain(|_, tab| !tab.rails.is_empty() || !tab.config_editors.is_empty());
        }
        self.clients.retain(|_, client| {
            !client.background_rails.is_empty()
                || !client.background_config_editors.is_empty()
                || !client.tabs.is_empty()
                || client.inspected_node.is_some()
                || client.metadata_controls != MetadataControls::default()
        });
    }

    /// Remove a renderer by plugin id, regardless of whether it was registered
    /// as a rail or a config editor. Returns true if anything was removed.
    pub fn unregister_renderer(&mut self, plugin_id: u32) -> bool {
        let mut removed = false;
        for client in self.clients.values_mut() {
            removed |= client.background_rails.remove(&plugin_id).is_some();
            removed |= client
                .background_config_editors
                .remove(&plugin_id)
                .is_some();
            for tab in client.tabs.values_mut() {
                removed |= tab.rails.remove(&plugin_id).is_some();
                removed |= tab.config_editors.remove(&plugin_id).is_some();
            }
            client
                .tabs
                .retain(|_, tab| !tab.rails.is_empty() || !tab.config_editors.is_empty());
        }
        removed
    }

    #[allow(dead_code)]
    pub fn rail_plugin_ids(&self) -> Vec<u32> {
        self.rail_plugin_targets()
            .into_iter()
            .map(|target| target.plugin_id)
            .collect()
    }

    #[allow(dead_code)]
    pub fn rail_plugin_targets(&self) -> Vec<RendererHello> {
        let mut targets = vec![];
        for client in self.clients.values() {
            targets.extend(
                client
                    .background_rails
                    .values()
                    .map(|registration| registration.identity.clone()),
            );
            targets.extend(
                client
                    .background_config_editors
                    .values()
                    .map(|registration| registration.identity.clone()),
            );
            for tab in client.tabs.values() {
                targets.extend(
                    tab.rails
                        .values()
                        .map(|registration| registration.identity.clone()),
                );
                targets.extend(
                    tab.config_editors
                        .values()
                        .map(|registration| registration.identity.clone()),
                );
            }
        }
        targets
    }

    #[allow(dead_code)]
    pub fn known_rail_count(&self) -> usize {
        self.clients
            .values()
            .map(|client| {
                client.background_rails.len()
                    + client
                        .tabs
                        .values()
                        .map(|tab| tab.rails.len())
                        .sum::<usize>()
            })
            .sum()
    }

    #[allow(dead_code)]
    pub fn known_config_editor_count(&self) -> usize {
        self.clients
            .values()
            .map(|client| {
                client.background_config_editors.len()
                    + client
                        .tabs
                        .values()
                        .map(|tab| tab.config_editors.len())
                        .sum::<usize>()
            })
            .sum()
    }

    pub fn config_editor_target_for_client_tab(
        &self,
        client_id: u16,
        tab_id: u64,
    ) -> Option<RendererHello> {
        let client = self.clients.get(&client_id)?;
        if let Some(target) = client
            .tabs
            .get(&tab_id)
            .and_then(|tab| tab.config_editors.values().next())
        {
            return Some(target.identity.clone());
        }
        None
    }

    pub fn set_inspected_node(&mut self, client_id: u16, node_key: NodeKey) -> bool {
        let client = self.client_mut(client_id);
        if client.inspected_node.as_ref() == Some(&node_key) {
            return false;
        }
        client.inspected_node = Some(node_key);
        true
    }

    #[allow(dead_code)]
    pub fn register_config_editor(&mut self, hello: PluginRegistrationHello) -> bool {
        let registration = PluginRegistration {
            identity: hello.identity,
            placement: hello.placement,
        };
        if self.plugin_registration(registration.identity.plugin_id) == Some(&registration) {
            return false;
        }
        self.unregister_renderer(registration.identity.plugin_id);
        let client = self.client_mut(registration.identity.client_id);
        match registration.placement {
            PluginPlacement::Tab { tab_id, .. } => {
                let tab = client
                    .tabs
                    .entry(tab_id)
                    .or_insert_with(|| ControllerClientTabState {
                        tab_id,
                        ..Default::default()
                    });
                tab.config_editors
                    .insert(registration.identity.plugin_id, registration);
            }
            PluginPlacement::Background | PluginPlacement::Unknown => {
                client
                    .background_config_editors
                    .insert(registration.identity.plugin_id, registration);
            }
        }
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

        let latent_tabs = self.latent_tabs();
        let resolved_metadata = self.resolved_metadata_for_tabs(&tabs);
        let observed_identities = observed_metadata_identities(&resolved_metadata);
        self.resolve_tab_templates(&mut tabs, &resolved_metadata);
        let rows = self
            .rows_with_group_templates(self.rows_for_tabs(&tabs, &latent_tabs), &resolved_metadata);
        let region_catalog_entities = self.catalog_entities();
        let (effective_variables, variable_warnings) = self.resolve_effective_variables(
            &rows,
            &tabs,
            &resolved_metadata,
            &region_catalog_entities,
        );
        let mut template_config = self.template_config.clone();
        template_config.effective_variables = effective_variables;
        for warning in variable_warnings {
            if !template_config.warnings.contains(&warning) {
                template_config.warnings.push(warning);
            }
        }
        // Only built when something asks for it. Indexing every fact of every
        // entity is pure cost on the legacy path, which is still the default.
        let placement_index = self
            .template_catalog
            .as_ref()
            .is_some_and(|catalog| {
                catalog
                    .regions()
                    .iter()
                    .any(|region| region.placement.is_some())
            })
            .then(|| PlacementIndex::build(&region_catalog_entities))
            .unwrap_or_default();
        let surface_regions = self
            .template_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .regions()
                    .iter()
                    .cloned()
                    .map(|definition| {
                        let metadata = BTreeMap::from([(
                            "presentation.template".to_owned(),
                            MetadataValue::Text(definition.root_template.clone()),
                        )]);
                        let entities = if let Some(placement) = definition
                            .placement
                            .as_deref()
                            .and_then(|name| catalog.placement(name))
                        {
                            // Placement pipeline. Only this region uses it; the
                            // rest of the render stays on legacy grouping.
                            self.evaluate_placement(
                                placement,
                                &region_catalog_entities,
                                &placement_index,
                                &definition.form,
                            )
                        } else if definition.source
                            == andamento_shared::template_config::SurfaceRegionSource::Attention
                        {
                            let attention_key = definition
                                .attention_key
                                .as_deref()
                                .unwrap_or("status.attention");
                            region_catalog_entities
                                .iter()
                                .filter(|entity| entity.presence != PresenceClass::Hidden)
                                .filter(|entity| self.entity_is_visible(entity))
                                .filter(|entity| {
                                    entity.values.get(attention_key).map(|entry| &entry.value)
                                        == Some(&MetadataValue::Bool(true))
                                })
                                .map(|entity| {
                                    let mut display = self.display_entity(entity);
                                    display.form = definition.form.clone();
                                    display
                                })
                                .collect()
                        } else {
                            vec![]
                        };
                        andamento_shared::DisplayRegion {
                            definition,
                            root: self.resolve_template_slot(
                                andamento_shared::template_config::TemplateConfigSlot::Compact,
                                andamento_shared::template_config::TemplateConfigNodeKind::Entity,
                                &metadata,
                            ),
                            entities,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        ControllerViewModel {
            sort_mode: self.sort_mode,
            config: self.rail_config,
            template_config,
            resolved_metadata,
            observed_identities,
            grouping_diagnostics: self.grouping_diagnostics(),
            rows,
            tabs,
            metadata_controls: self
                .clients
                .get(&0)
                .map(|client| client.metadata_controls.clone())
                .unwrap_or_default(),
            inspected_node: None,
            collapsed_groups: self.rail_ui.collapsed_groups.iter().cloned().collect(),
            display_variables: self
                .template_catalog
                .as_ref()
                .map(|catalog| catalog.display_variables().to_vec())
                .unwrap_or_default(),
            display_variable_values: self.rail_ui.variables.clone(),
            surface_regions,
        }
    }

    pub fn view_model_for_client(&self, client_id: u16) -> ControllerViewModel {
        let mut model = self.view_model();
        if let Some(client) = self.clients.get(&client_id) {
            model.metadata_controls = client.metadata_controls.clone();
            model.inspected_node = client.inspected_node.clone();
        }
        model
    }

    fn rows_for_tabs(&self, tabs: &[TabCard], latent_tabs: &[LatentTab]) -> Vec<RailRow> {
        self.catalog_group_rows(tabs, latent_tabs)
    }

    fn catalog_group_rows(&self, tabs: &[TabCard], latent_tabs: &[LatentTab]) -> Vec<RailRow> {
        let inline_entities = self.visible_inline_entities();
        let compact_entities = self.compact_entities();
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
        for latent in latent_tabs {
            for prefix in group_path_prefixes(&latent.path) {
                *tab_count_by_prefix.entry(prefix).or_default() += 1;
            }
        }
        for inline in &inline_entities {
            for prefix in group_path_prefixes(&inline.path) {
                *tab_count_by_prefix.entry(prefix).or_default() += 1;
            }
        }
        for entity in &compact_entities {
            for prefix in group_path_prefixes(&compact_parent_path(&entity.path)) {
                *tab_count_by_prefix.entry(prefix).or_default() += 1;
            }
        }
        let mut latent_by_path: BTreeMap<GroupPath, Vec<LatentTab>> = BTreeMap::new();
        for latent in latent_tabs {
            latent_by_path
                .entry(latent.path.clone())
                .or_default()
                .push(latent.clone());
        }
        let mut tab_order_by_path = BTreeMap::new();
        for (index, tab) in tabs.iter().enumerate() {
            if let Some(grouping) = tab.grouping.as_ref() {
                tab_order_by_path
                    .entry(grouping.path.clone())
                    .or_insert(index);
            }
        }
        let compact_parent_paths = compact_entities
            .iter()
            .map(|entity| compact_parent_path(&entity.path))
            .collect::<Vec<_>>();
        let mut ordered_paths = path_to_tabs
            .keys()
            .chain(latent_by_path.keys())
            .chain(inline_entities.iter().map(|entity| &entity.path))
            .chain(compact_parent_paths.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let catalog_ordinal_by_path = ordered_paths
            .iter()
            .cloned()
            .map(|path| {
                let ordinal = self.catalog_group_ordinal(&path);
                (path, ordinal)
            })
            .collect::<BTreeMap<_, _>>();
        ordered_paths.sort_by(|left, right| {
            match (
                catalog_ordinal_by_path.get(left).copied().flatten(),
                catalog_ordinal_by_path.get(right).copied().flatten(),
            ) {
                (Some(left_ordinal), Some(right_ordinal)) => left_ordinal
                    .cmp(&right_ordinal)
                    .then_with(|| left.cmp(right)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => tab_order_by_path
                    .get(left)
                    .cmp(&tab_order_by_path.get(right))
                    .then_with(|| left.cmp(right)),
            }
        });

        let ungrouped_tabs = tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.grouping.is_none())
            .map(|(index, tab)| (index, tab))
            .collect::<Vec<_>>();
        let mut next_ungrouped = 0usize;
        let mut rows = vec![];
        let mut emitted_groups = HashSet::new();
        for path in ordered_paths {
            if let Some(group_index) = tab_order_by_path.get(&path).copied() {
                while let Some((index, tab)) = ungrouped_tabs.get(next_ungrouped) {
                    if *index >= group_index {
                        break;
                    }
                    rows.push(RailRow::Tab {
                        tab_id: tab.tab_id,
                        indent: 0,
                        parent_path: None,
                    });
                    next_ungrouped += 1;
                }
            }
            let grouping = grouping_by_path.get(&path);
            for prefix in group_path_prefixes(&path) {
                if !emitted_groups.insert(prefix.clone()) {
                    continue;
                }
                let (group_id, label, full_label) = grouping
                    .map(|grouping| {
                        group_header_identity_for_prefix(
                            &prefix,
                            &path,
                            &grouping.key,
                            &grouping.label,
                            &grouping.full_label,
                        )
                    })
                    .unwrap_or_else(|| group_header_identity_for_path(&prefix));
                rows.push(RailRow::GroupHeader {
                    group_id,
                    path: prefix.clone(),
                    label,
                    full_label,
                    tab_count: tab_count_by_prefix.get(&prefix).copied().unwrap_or(1),
                    templates: ResolvedTemplateSlots::default(),
                });
            }
            rows.extend(
                path_to_tabs
                    .get(&path)
                    .into_iter()
                    .flatten()
                    .map(|tab| RailRow::Tab {
                        tab_id: tab.tab_id,
                        indent: path.0.len() * 2,
                        parent_path: Some(path.clone()),
                    }),
            );
            rows.extend(
                latent_by_path
                    .get(&path)
                    .into_iter()
                    .flatten()
                    .map(|latent| RailRow::Latent {
                        latent: latent.clone(),
                        indent: path.0.len() * 2,
                        parent_path: Some(path.clone()),
                    }),
            );
            rows.extend(
                compact_entities
                    .iter()
                    .filter(|entity| compact_parent_path(&entity.path) == path)
                    .map(|entity| RailRow::Entity {
                        entity: self.display_entity(entity),
                        indent: path.0.len() * 2,
                        parent_path: Some(path.clone()),
                    }),
            );
        }
        rows.extend(
            ungrouped_tabs[next_ungrouped..]
                .iter()
                .map(|(_, tab)| RailRow::Tab {
                    tab_id: tab.tab_id,
                    indent: 0,
                    parent_path: None,
                }),
        );
        rows
    }

    fn display_entity(&self, entity: &CatalogEntity) -> DisplayEntity {
        let metadata = entity_facts(&entity.entity, &entity.values);
        let mut compact_metadata = metadata.clone();
        if entity.form == DISPLAY_FORM_COMPACT {
            if let Some(template) = entity.template.as_ref() {
                compact_metadata.insert(
                    "presentation.template".to_owned(),
                    MetadataValue::Text(template.clone()),
                );
            }
        }
        let templates = ResolvedTemplateSlots {
            compact: self.resolve_template_slot(
                andamento_shared::template_config::TemplateConfigSlot::Compact,
                andamento_shared::template_config::TemplateConfigNodeKind::Entity,
                &compact_metadata,
            ),
            detail: self.resolve_template_slot(
                andamento_shared::template_config::TemplateConfigSlot::Detail,
                andamento_shared::template_config::TemplateConfigNodeKind::Entity,
                &metadata,
            ),
            ..Default::default()
        };
        DisplayEntity {
            entity: entity.entity.clone(),
            label: metadata_entry_text(&entity.values, KEY_DISPLAY_LABEL)
                .map(str::to_owned)
                .unwrap_or_else(|| entity.entity.id.clone()),
            form: entity.form.clone(),
            metadata,
            templates,
            children: vec![],
        }
    }

    fn catalog_group_ordinal(&self, path: &GroupPath) -> Option<i64> {
        self.catalog_entities()
            .into_iter()
            .find(|entity| &entity.path == path)
            .map(|entity| entity.ordinal)
    }

    fn rows_with_group_templates(
        &self,
        rows: Vec<RailRow>,
        resolved_metadata: &[ResolvedMetadata],
    ) -> Vec<RailRow> {
        let declared_templates = self.declared_group_templates();
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
                    let mut metadata = group_template_metadata(
                        &path,
                        &label,
                        &full_label,
                        tab_count,
                        resolved_metadata,
                    );
                    if let Some(template) = declared_templates.get(&path) {
                        metadata.insert(
                            "presentation.template".to_owned(),
                            MetadataValue::Text(template.clone()),
                        );
                    }
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
                RailRow::Latent {
                    latent,
                    indent,
                    parent_path,
                } => RailRow::Latent {
                    latent,
                    indent,
                    parent_path,
                },
                RailRow::Entity {
                    entity,
                    indent,
                    parent_path,
                } => RailRow::Entity {
                    entity,
                    indent,
                    parent_path,
                },
            })
            .collect()
    }

    /// Evaluate a placement's single loop into the entities it places.
    ///
    /// Predicates intersect: the smallest posting list is taken first and the
    /// rest filter it, so cost tracks the most selective fact rather than the
    /// catalog size.
    fn evaluate_placement(
        &self,
        placement: &andamento_shared::template_config::PlacementDefinition,
        entities: &[CatalogEntity],
        index: &PlacementIndex,
        form: &str,
    ) -> Vec<andamento_shared::DisplayEntity> {
        placement
            .loops
            .iter()
            .flat_map(|loop_definition| {
                self.evaluate_placement_loop(
                    loop_definition,
                    entities,
                    index,
                    form,
                    &BTreeMap::new(),
                    &[],
                    0,
                )
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_placement_loop<'a>(
        &self,
        loop_definition: &andamento_shared::template_config::PlacementLoop,
        entities: &'a [CatalogEntity],
        index: &PlacementIndex,
        form: &str,
        bindings: &BTreeMap<String, &'a CatalogEntity>,
        ancestors: &[andamento_shared::EntityRef],
        depth: usize,
    ) -> Vec<andamento_shared::DisplayEntity> {
        const MAX_PLACEMENT_DEPTH: usize = 64;
        if depth >= MAX_PLACEMENT_DEPTH {
            return vec![];
        }
        let mut postings = loop_definition
            .predicates
            .iter()
            .map(|predicate| {
                let value = predicate.value.clone().or_else(|| {
                    let bound = bindings.get(predicate.of.as_deref()?)?;
                    if predicate.key == "entity.kind" {
                        Some(bound.entity.kind.clone())
                    } else if predicate.key == "entity.id" {
                        Some(bound.entity.id.clone())
                    } else {
                        bound.values.get(&predicate.key).and_then(|entry| {
                            andamento_shared::template_config::placement_index_text(&entry.value)
                        })
                    }
                });
                value
                    .map(|value| index.lookup_value(&predicate.key, &value))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        postings.sort_by_key(|matches| matches.len());
        let Some((seed, rest)) = postings.split_first() else {
            return vec![];
        };
        seed.iter()
            .filter(|position| {
                rest.iter()
                    .all(|matches| matches.binary_search(position).is_ok())
            })
            .filter_map(|position| entities.get(*position))
            .filter(|entity| self.entity_is_visible(entity))
            .filter(|entity| !ancestors.contains(&entity.entity))
            .map(|entity| {
                let mut display =
                    placed_display_entity(self.display_entity(entity), loop_definition, form);
                let mut nested_bindings = bindings.clone();
                nested_bindings.insert(loop_definition.binding.clone(), entity);
                let mut nested_ancestors = ancestors.to_vec();
                nested_ancestors.push(entity.entity.clone());

                let mut child_loops = loop_definition.loops.as_slice();
                if let Some(requested) = loop_definition.apply_template.as_deref() {
                    let template_name = if requested.is_empty() {
                        format!("{}/line", entity.entity.kind)
                    } else {
                        requested.to_owned()
                    };
                    if let Some(catalog) = self.template_catalog.as_ref() {
                        if let Some(template) =
                            catalog.placement_template(&template_name, &display.metadata)
                        {
                            let resolved_slot = match catalog
                                .resolve_placement_template(&template_name, &display.metadata)
                            {
                                Ok(Some(resolved)) => Some(ResolvedTemplateSlot {
                                    template_name: resolved.name.clone(),
                                    fields: vec![],
                                    render_ready: Some(resolved.render_ready()),
                                    setters: resolved.setters.clone(),
                                    effective_kdl: resolved.dump_kdl(),
                                    resolve_error: None,
                                }),
                                Ok(None) => None,
                                Err(error) => Some(ResolvedTemplateSlot {
                                    template_name: "<resolve-error>".to_owned(),
                                    fields: vec![],
                                    render_ready: None,
                                    setters: vec![],
                                    effective_kdl: String::new(),
                                    resolve_error: Some(error.to_string()),
                                }),
                            };
                            if let Some(slot) = resolved_slot {
                                display.templates.compact = Some(slot.clone());
                                display.templates.detail = Some(slot);
                            }
                            child_loops = template.loops.as_slice();
                            // Applied templates intentionally start a new lexical environment.
                            nested_bindings.clear();
                            let own_binding = template_name
                                .split('/')
                                .next()
                                .unwrap_or(&template_name)
                                .to_owned();
                            nested_bindings.insert(own_binding, entity);
                        }
                    }
                }
                display.children = child_loops
                    .iter()
                    .flat_map(|nested| {
                        self.evaluate_placement_loop(
                            nested,
                            entities,
                            index,
                            form,
                            &nested_bindings,
                            &nested_ancestors,
                            depth + 1,
                        )
                    })
                    .collect();
                display
            })
            .collect()
    }

    fn resolve_effective_variables(
        &self,
        rows: &[RailRow],
        tabs: &[TabCard],
        resolved_metadata: &[ResolvedMetadata],
        catalog_entities: &[CatalogEntity],
    ) -> (Vec<EffectiveNodeVariables>, Vec<String>) {
        let Some(catalog) = self.template_catalog.as_ref() else {
            return (vec![], vec![]);
        };
        let mut variable_warnings = BTreeSet::new();
        let mut resolved_by_node = BTreeMap::<NodeKey, ResolvedNodeVariables>::new();
        let root = NodeKey::Root;
        let root_metadata = node_metadata(&root, resolved_metadata);
        let root_values = self.resolve_variables_at_node(
            catalog,
            &root,
            None,
            &root_metadata,
            None,
            &mut variable_warnings,
        );
        resolved_by_node.insert(root.clone(), root_values);

        let mut groups = rows
            .iter()
            .filter_map(|row| match row {
                RailRow::GroupHeader {
                    path, templates, ..
                } => Some((path.clone(), templates.group_header.as_ref())),
                _ => None,
            })
            .collect::<Vec<_>>();
        groups.sort_by_key(|(path, _)| path.0.len());
        for (path, template) in groups {
            let node = NodeKey::Group(path.clone());
            let parent = if path.0.len() <= 1 {
                NodeKey::Root
            } else {
                NodeKey::Group(GroupPath(path.0[..path.0.len() - 1].to_vec()))
            };
            let metadata = node_metadata(&node, resolved_metadata);
            let values = self.resolve_variables_at_node(
                catalog,
                &node,
                resolved_by_node
                    .get(&parent)
                    .map(|resolved| &resolved.values),
                &metadata,
                template,
                &mut variable_warnings,
            );
            resolved_by_node.insert(node, values);
        }

        let tab_by_id = tabs
            .iter()
            .map(|tab| (tab.tab_id, tab))
            .collect::<HashMap<_, _>>();
        for row in rows {
            let (node, parent, metadata, template) = match row {
                RailRow::Tab {
                    tab_id,
                    parent_path,
                    ..
                } => {
                    let Some(tab) = tab_by_id.get(tab_id) else {
                        continue;
                    };
                    let node = NodeKey::Tab(*tab_id);
                    let parent = parent_path
                        .clone()
                        .map(NodeKey::Group)
                        .unwrap_or(NodeKey::Root);
                    (
                        node.clone(),
                        parent,
                        node_metadata(&node, resolved_metadata),
                        tab.templates.tab_title.as_ref(),
                    )
                }
                RailRow::Latent {
                    latent,
                    parent_path,
                    ..
                } => {
                    let node = NodeKey::Entity(latent.entity.clone());
                    let parent = parent_path
                        .clone()
                        .map(NodeKey::Group)
                        .unwrap_or(NodeKey::Root);
                    (
                        node.clone(),
                        parent,
                        node_metadata(&node, resolved_metadata),
                        latent
                            .templates
                            .detail
                            .as_ref()
                            .or(latent.templates.compact.as_ref()),
                    )
                }
                RailRow::Entity {
                    entity,
                    parent_path,
                    ..
                } => {
                    let node = NodeKey::Entity(entity.entity.clone());
                    let parent = parent_path
                        .clone()
                        .map(NodeKey::Group)
                        .unwrap_or(NodeKey::Root);
                    let template = if entity.form == DISPLAY_FORM_COMPACT {
                        entity.templates.compact.as_ref()
                    } else {
                        entity.templates.detail.as_ref()
                    };
                    (node, parent, entity.metadata.clone(), template)
                }
                RailRow::GroupHeader { .. } => continue,
            };
            if resolved_by_node.contains_key(&node) {
                continue;
            }
            let values = self.resolve_variables_at_node(
                catalog,
                &node,
                resolved_by_node
                    .get(&parent)
                    .map(|resolved| &resolved.values),
                &metadata,
                template,
                &mut variable_warnings,
            );
            resolved_by_node.insert(node, values);
        }

        for entity in catalog_entities {
            let node = NodeKey::Entity(entity.entity.clone());
            if resolved_by_node.contains_key(&node) {
                continue;
            }
            let display = self.display_entity(entity);
            let parent = if entity.path.0.is_empty() {
                NodeKey::Root
            } else {
                NodeKey::Group(entity.path.clone())
            };
            let template = if display.form == DISPLAY_FORM_COMPACT {
                display.templates.compact.as_ref()
            } else {
                display.templates.detail.as_ref()
            };
            let values = self.resolve_variables_at_node(
                catalog,
                &node,
                resolved_by_node
                    .get(&parent)
                    .map(|resolved| &resolved.values),
                &display.metadata,
                template,
                &mut variable_warnings,
            );
            resolved_by_node.insert(node, values);
        }

        (
            resolved_by_node
                .into_iter()
                .map(|(node, resolved)| EffectiveNodeVariables {
                    node,
                    values: resolved.values,
                    declarations: resolved.declarations,
                })
                .collect(),
            variable_warnings.into_iter().collect(),
        )
    }

    fn resolve_variables_at_node(
        &self,
        catalog: &andamento_shared::template_config::TemplateConfigCatalog,
        node: &NodeKey,
        inherited: Option<&BTreeMap<String, EffectiveVariableValue>>,
        metadata: &BTreeMap<String, MetadataValue>,
        template: Option<&ResolvedTemplateSlot>,
        warnings: &mut BTreeSet<String>,
    ) -> ResolvedNodeVariables {
        let scope = catalog.variable_scope(metadata);
        warnings.extend(scope.warnings);
        let declarations = scope.declarations;
        let declaration_by_name = declarations
            .iter()
            .map(|resolved| (resolved.definition.name.as_str(), resolved))
            .collect::<BTreeMap<_, _>>();
        let mut values = inherited.cloned().unwrap_or_default();
        for declaration in &declarations {
            values
                .entry(declaration.definition.name.clone())
                .or_insert_with(|| EffectiveVariableValue {
                    value: declaration.definition.default.clone(),
                    provenance: VariableSetterProvenance {
                        setter: "default".to_owned(),
                        ancestor: node.clone(),
                        origin: declaration.origin.clone(),
                    },
                    overridden: vec![],
                });
        }
        if let Some(template) = template {
            for setter in &template.setters {
                let Some(declaration) = declaration_by_name.get(setter.setter.name.as_str()) else {
                    warnings.insert(format!(
                        "{} template {} sets undeclared node variable {}",
                        setter.origin.label(),
                        template.template_name,
                        setter.setter.name,
                    ));
                    continue;
                };
                if !declaration.definition.accepts(&setter.setter.value) {
                    warnings.insert(format!(
                        "{} template {} sets node variable {} to disallowed value {}",
                        setter.origin.label(),
                        template.template_name,
                        setter.setter.name,
                        setter.setter.value,
                    ));
                    continue;
                }
                apply_variable_setter(
                    &mut values,
                    &setter.setter.name,
                    &setter.setter.value,
                    VariableSetterProvenance {
                        setter: format!("template {}", template.template_name),
                        ancestor: node.clone(),
                        origin: setter.origin.clone(),
                    },
                );
            }
        }
        for setter in scope.setters {
            apply_variable_setter(
                &mut values,
                &setter.setter.name,
                &setter.setter.value,
                VariableSetterProvenance {
                    setter: "config".to_owned(),
                    ancestor: node.clone(),
                    origin: setter.origin,
                },
            );
        }
        if let Some(overrides) = self.node_variable_overrides.get(node) {
            for (name, value) in overrides {
                let Some(declaration) = declaration_by_name.get(name.as_str()) else {
                    warnings.insert(format!(
                        "andamento-inspector sets undeclared node variable {name}"
                    ));
                    continue;
                };
                if !declaration.definition.accepts(value) {
                    warnings.insert(format!(
                        "andamento-inspector sets node variable {name} to disallowed value {value}"
                    ));
                    continue;
                }
                apply_variable_setter(
                    &mut values,
                    name,
                    value,
                    VariableSetterProvenance {
                        setter: NODE_VARIABLE_CONFIG_OVERRIDE_SETTER.to_owned(),
                        ancestor: node.clone(),
                        origin: andamento_shared::template_config::TemplateConfigOrigin {
                            layer: andamento_shared::template_config::TemplateConfigLayerKind::User,
                            membership: None,
                            source: "andamento-inspector".to_owned(),
                        },
                    },
                );
            }
        }
        ResolvedNodeVariables {
            values,
            declarations: declarations
                .into_iter()
                .map(|resolved| resolved.definition)
                .collect(),
        }
    }

    fn latent_tabs(&self) -> Vec<LatentTab> {
        let entities = self.catalog_entities();
        let collapsed = collapsed_child_entities(&entities);
        let live_targets = self.materialized_action_targets(&entities);
        entities
            .iter()
            .filter(|entity| entity.presence == PresenceClass::Tab)
            .filter(|entity| !collapsed.contains(&entity.entity))
            .filter_map(|entity| {
                let display = self.display_entity(entity);
                let action_target = metadata_entry_text(&entity.values, KEY_ACTION_TARGET)
                    .map(str::to_owned)
                    .unwrap_or_else(|| entity.entity.action_target());
                if live_targets.contains(&action_target) {
                    return None;
                }
                let name = metadata_entry_text(&entity.values, KEY_DISPLAY_LABEL)
                    .map(str::to_owned)
                    .or_else(|| entity.path.0.last().map(group_segment_label))
                    .unwrap_or_else(|| entity.entity.id.clone());
                Some(LatentTab {
                    entity: entity.entity.clone(),
                    materialization: if self
                        .pending_latent_materializations
                        .contains_key(&action_target)
                    {
                        LatentMaterializationState::Opening
                    } else {
                        LatentMaterializationState::Ready
                    },
                    action_target,
                    path: entity.path.clone(),
                    name,
                    status_state: metadata_entry_text(&entity.values, KEY_STATUS_STATE)
                        .map(str::to_owned),
                    summary: metadata_entry_text(&entity.values, KEY_SUMMARY_TEXT)
                        .map(str::to_owned),
                    source: metadata_entry_text(&entity.values, KEY_SOURCE).map(str::to_owned),
                    materialize_recipe: metadata_entry_text(&entity.values, KEY_MATERIALIZE_RECIPE)
                        .map(str::to_owned),
                    checkout_path: metadata_entry_text(&entity.values, KEY_CHECKOUT_PATH)
                        .map(str::to_owned),
                    templates: display.templates,
                })
            })
            .collect()
    }

    fn catalog_entities(&self) -> Vec<CatalogEntity> {
        let default_catalog = GroupingConfigCatalog::default();
        let catalog = self.grouping_catalog.as_ref().unwrap_or(&default_catalog);
        let mut entities = self
            .metadata
            .targets()
            .filter_map(|target| {
                let EntityId::Entity(entity) = target else {
                    return None;
                };
                let values = self
                    .metadata
                    .resolved_entries_for(target, self.receive_counter);
                let facts = entity_facts(entity, &values);
                let (rule, path, level_index) =
                    if let Some(name) = self.active_grouping_template.as_deref() {
                        let rule = catalog.named(name).filter(|rule| {
                            rule.filter
                                .as_ref()
                                .is_none_or(|filter| filter.matches(&facts))
                        })?;
                        let (path, level_index) = grouping_path_for_facts(rule, &facts)?;
                        (rule, path, level_index)
                    } else {
                        catalog.rules.iter().find_map(|rule| {
                            if !rule
                                .filter
                                .as_ref()
                                .is_none_or(|filter| filter.matches(&facts))
                            {
                                return None;
                            }
                            grouping_path_for_facts(rule, &facts)
                                .map(|(path, level_index)| (rule, path, level_index))
                        })?
                    };
                let mapping = rule
                    .presence
                    .iter()
                    .find(|mapping| mapping.kind == entity.kind);
                let presence = mapping
                    .map(|mapping| mapping.class)
                    .unwrap_or(PresenceClass::Hidden);
                let form = mapping
                    .map(|mapping| mapping.form.clone())
                    .unwrap_or_else(|| DISPLAY_FORM_FULL.to_owned());
                let visible_when = mapping.and_then(|mapping| mapping.visible_when.clone());
                let template = mapping.and_then(|mapping| mapping.template.clone());
                let level = &rule.levels[level_index];
                let group_templates = rule
                    .levels
                    .iter()
                    .filter_map(|level| {
                        level
                            .template
                            .as_ref()
                            .map(|template| (level.key.clone(), template.clone()))
                    })
                    .collect();
                Some(CatalogEntity {
                    entity: entity.clone(),
                    values,
                    ordinal: self.metadata.target_ordinal(target).unwrap_or_default(),
                    path,
                    presence,
                    form,
                    visible_when,
                    template,
                    group_templates,
                    grouping_priority: rule.priority,
                    collapse_single_member: level.collapse_single_member,
                    show_empty: level.show_empty,
                })
            })
            .collect::<Vec<_>>();
        entities.sort_by(|left, right| {
            left.ordinal
                .cmp(&right.ordinal)
                .then_with(|| left.path.cmp(&right.path))
        });
        entities
    }

    fn grouping_diagnostics(&self) -> Vec<GroupingRuleDiagnostic> {
        let default_catalog = GroupingConfigCatalog::default();
        let catalog = self.grouping_catalog.as_ref().unwrap_or(&default_catalog);
        self.metadata
            .targets()
            .filter_map(|target| {
                let EntityId::Entity(entity) = target else {
                    return None;
                };
                let values = self
                    .metadata
                    .resolved_entries_for(target, self.receive_counter);
                let facts = entity_facts(entity, &values);
                let rules = self
                    .active_grouping_template
                    .as_deref()
                    .and_then(|name| catalog.named(name))
                    .into_iter()
                    .collect::<Vec<_>>();
                let rules = if rules.is_empty() && self.active_grouping_template.is_none() {
                    catalog.rules.iter().collect()
                } else {
                    rules
                };
                Some(
                    rules
                        .into_iter()
                        .filter(|rule| {
                            rule.filter
                                .as_ref()
                                .is_none_or(|filter| filter.matches(&facts))
                        })
                        .map_while(|rule| match derive_grouping_path(rule, &facts) {
                            Ok(_) => None,
                            Err(GroupingPathError::MissingNonOptionalLevel(key)) => {
                                Some(GroupingRuleDiagnostic {
                                    target: NodeKey::Entity(entity.clone()),
                                    rule: rule.name.clone(),
                                    message: format!(
                                        "not captured: `{key}` absent (non-optional level)"
                                    ),
                                })
                            }
                            Err(GroupingPathError::NoDerivedLevels) => {
                                Some(GroupingRuleDiagnostic {
                                    target: NodeKey::Entity(entity.clone()),
                                    rule: rule.name.clone(),
                                    message: "not captured: no grouping levels derived".to_owned(),
                                })
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    }

    fn declared_group_templates(&self) -> BTreeMap<GroupPath, String> {
        let entities = self.catalog_entities();
        let mut declarations: BTreeMap<GroupPath, (i64, String)> = BTreeMap::new();
        for entity in &entities {
            for depth in 1..=entity.path.0.len() {
                let prefix = GroupPath(entity.path.0[..depth].to_vec());
                let key = &entity.path.0[depth - 1].key;
                if let Some(template) = entity.group_templates.get(key) {
                    declarations
                        .entry(prefix)
                        .and_modify(|(priority, selected)| {
                            if entity.grouping_priority > *priority {
                                *priority = entity.grouping_priority;
                                *selected = template.clone();
                            }
                        })
                        .or_insert_with(|| (entity.grouping_priority, template.clone()));
                }
            }
        }

        let default_catalog = GroupingConfigCatalog::default();
        let catalog = self.grouping_catalog.as_ref().unwrap_or(&default_catalog);
        let tab_seed_metadata = self.tab_seed_metadata_entries();
        for tab in &self.tabs {
            let metadata = self.tab_resolved_metadata_values(tab.tab_id, &tab_seed_metadata);
            let selected = if let Some(name) = self.active_grouping_template.as_deref() {
                catalog.named(name).and_then(|rule| {
                    self.tab_grouping_for_rule(rule, &metadata)
                        .map(|path| (rule, path))
                })
            } else {
                catalog.rules.iter().find_map(|rule| {
                    self.tab_grouping_for_rule(rule, &metadata)
                        .map(|path| (rule, path))
                })
            };
            let Some((rule, grouping)) = selected else {
                continue;
            };
            for depth in 1..=grouping.path.0.len() {
                let prefix = GroupPath(grouping.path.0[..depth].to_vec());
                let key = &grouping.path.0[depth - 1].key;
                if let Some(template) = rule
                    .levels
                    .iter()
                    .find(|level| &level.key == key)
                    .and_then(|level| level.template.as_ref())
                {
                    declarations
                        .entry(prefix)
                        .and_modify(|(priority, selected)| {
                            if rule.priority > *priority {
                                *priority = rule.priority;
                                *selected = template.clone();
                            }
                        })
                        .or_insert_with(|| (rule.priority, template.clone()));
                }
            }
        }

        // A non-compact presence declaration owns its exact full surface and
        // takes precedence over the grouping level's default for that path.
        for entity in entities {
            if entity.form != DISPLAY_FORM_COMPACT {
                if let Some(template) = entity.template {
                    declarations.insert(entity.path, (i64::MAX, template));
                }
            }
        }
        declarations
            .into_iter()
            .map(|(path, (_, template))| (path, template))
            .collect()
    }

    fn materialized_action_targets(&self, entities: &[CatalogEntity]) -> BTreeSet<String> {
        let tab_seed_metadata = self.tab_seed_metadata_entries();
        self.tabs
            .iter()
            .filter_map(|tab| self.tab_entity_ref(tab.tab_id, &tab_seed_metadata))
            .map(|entity| {
                entities
                    .iter()
                    .find(|candidate| candidate.entity == entity)
                    .and_then(|candidate| metadata_entry_text(&candidate.values, KEY_ACTION_TARGET))
                    .map(str::to_owned)
                    .unwrap_or_else(|| entity.action_target())
            })
            .collect()
    }

    fn visible_inline_entities(&self) -> Vec<CatalogEntity> {
        let entities = self.catalog_entities();
        entities
            .iter()
            .filter(|entity| entity.presence == PresenceClass::Inline)
            .filter(|entity| entity.form != DISPLAY_FORM_COMPACT)
            .filter(|entity| self.entity_is_visible(entity))
            .filter(|entity| {
                entity.show_empty
                    || entities.iter().any(|candidate| {
                        candidate.entity != entity.entity
                            && candidate.presence != PresenceClass::Hidden
                            && candidate.path.0.starts_with(&entity.path.0)
                    })
            })
            .cloned()
            .collect()
    }

    fn compact_entities(&self) -> Vec<CatalogEntity> {
        self.catalog_entities()
            .into_iter()
            .filter(|entity| entity.presence == PresenceClass::Inline)
            .filter(|entity| entity.form == DISPLAY_FORM_COMPACT)
            .filter(|entity| self.entity_is_visible(entity))
            .collect()
    }

    fn entity_is_visible(&self, entity: &CatalogEntity) -> bool {
        entity
            .visible_when
            .as_ref()
            .is_none_or(|name| match self.rail_ui.variables.get(name) {
                Some(DisplayVariableValue::Bool(value)) => *value,
                _ => true,
            })
    }

    fn tab_entity_ref(
        &self,
        tab_id: u64,
        tab_seed_metadata: &TabSeedMetadata,
    ) -> Option<EntityRef> {
        let target = EntityId::Tab(tab_id);
        let seed_values = tab_seed_metadata.get(&tab_id).cloned().unwrap_or_default();
        let (values, _, _) = self.resolve_target_metadata(&target, seed_values);
        entity_ref_from_entries(&values)
    }

    fn presentation_path_for_entity(&self, target: &EntityRef) -> Option<GroupPath> {
        let entities = self.catalog_entities();
        let collapsed = collapsed_child_entities(&entities);
        let entity = entities.iter().find(|entity| &entity.entity == target)?;
        if !collapsed.contains(target) {
            return Some(entity.path.clone());
        }
        entities
            .iter()
            .filter(|parent| parent.presence == PresenceClass::Tab && parent.collapse_single_member)
            .filter(|parent| direct_child_path(&parent.path, &entity.path))
            .map(|parent| parent.path.clone())
            .next()
    }

    pub fn can_materialize_latent(
        &self,
        request: &andamento_shared::MaterializeLatentRequest,
    ) -> bool {
        // This exact equality check is the security boundary for controller pipe callers:
        // accepting an action target alone would let a caller substitute its own recipe or path.
        self.latent_tabs()
            .iter()
            .filter_map(LatentTab::materialize_request)
            .any(|candidate| candidate == *request)
    }

    pub fn begin_latent_materialization(
        &mut self,
        request: &andamento_shared::MaterializeLatentRequest,
    ) -> bool {
        if self.materialized_tab_position(request).is_some()
            || self
                .pending_latent_materializations
                .contains_key(&request.action_target)
            || !self.can_materialize_latent(request)
        {
            return false;
        }
        self.pending_latent_materializations.insert(
            request.action_target.clone(),
            PendingLatentMaterialization {
                request: request.clone(),
                tab_id: None,
            },
        );
        true
    }

    pub fn bind_materializing_tab(
        &mut self,
        request: &andamento_shared::MaterializeLatentRequest,
        tab_id: u64,
    ) -> bool {
        let Some(pending) = self
            .pending_latent_materializations
            .get_mut(&request.action_target)
        else {
            return false;
        };
        if pending.request != *request || pending.tab_id.is_some() {
            return false;
        }
        pending.tab_id = Some(tab_id);
        self.pending_materialized_tab_names
            .insert(tab_id, request.name.clone());
        true
    }

    pub fn abort_latent_materialization(
        &mut self,
        request: &andamento_shared::MaterializeLatentRequest,
    ) -> bool {
        let Some(pending) = self
            .pending_latent_materializations
            .get(&request.action_target)
        else {
            return false;
        };
        if pending.request != *request {
            return false;
        }
        if let Some(tab_id) = pending.tab_id {
            self.pending_materialized_tab_names.remove(&tab_id);
        }
        self.pending_latent_materializations
            .remove(&request.action_target);
        true
    }

    pub fn materialized_tab_position(
        &self,
        request: &andamento_shared::MaterializeLatentRequest,
    ) -> Option<usize> {
        self.materialized_tab_position_for_action_target(&request.action_target)
    }

    fn materialized_tab_position_for_action_target(&self, action_target: &str) -> Option<usize> {
        let tab_seed_metadata = self.tab_seed_metadata_entries();
        self.tabs.iter().find_map(|tab| {
            let target = EntityId::Tab(tab.tab_id);
            let seed_values = tab_seed_metadata
                .get(&tab.tab_id)
                .cloned()
                .unwrap_or_default();
            let (values, _, _) = self.resolve_target_metadata(&target, seed_values);
            let entity_target =
                entity_ref_from_entries(&values).map(|entity| entity.action_target());
            let resolved_action_target = metadata_entry_text(&values, KEY_ACTION_TARGET)
                .map(str::to_owned)
                .or(entity_target);
            (resolved_action_target.as_deref() == Some(action_target)).then_some(tab.position)
        })
    }

    pub fn activation_for_entity(&self, subject: &EntityRef) -> Option<EntityActivation> {
        let entities = self.catalog_entities();
        let entity = entities.iter().find(|entity| &entity.entity == subject)?;
        let action_target = metadata_entry_text(&entity.values, KEY_ACTION_TARGET)
            .map(str::to_owned)
            .unwrap_or_else(|| entity.entity.action_target());
        if let Some(position) = self.materialized_tab_position_for_action_target(&action_target) {
            return Some(EntityActivation::FocusTab { position });
        }
        self.latent_tabs()
            .into_iter()
            .find(|latent| latent.entity == *subject)
            .and_then(|latent| latent.materialize_request())
            .map(EntityActivation::Materialize)
    }

    fn claim_materializing_tabs(&mut self) -> bool {
        let live_tab_ids = self
            .tabs
            .iter()
            .map(|tab| tab.tab_id)
            .collect::<HashSet<_>>();
        let claims = self
            .pending_latent_materializations
            .iter()
            .filter_map(|(action_target, pending)| {
                let tab_id = pending.tab_id?;
                live_tab_ids.contains(&tab_id).then_some((
                    action_target.clone(),
                    tab_id,
                    pending.request.clone(),
                ))
            })
            .collect::<Vec<_>>();
        for (action_target, tab_id, request) in &claims {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.tab_id == *tab_id) {
                tab.name = request.name.clone();
            }
            self.apply_materialized_identity(*tab_id, request);
            self.pending_latent_materializations.remove(action_target);
        }
        !claims.is_empty()
    }

    fn apply_materialized_identity(
        &mut self,
        tab_id: u64,
        request: &andamento_shared::MaterializeLatentRequest,
    ) -> bool {
        let Some(entity) = self
            .catalog_entities()
            .into_iter()
            .find(|entity| {
                metadata_entry_text(&entity.values, KEY_ACTION_TARGET)
                    .map(str::to_owned)
                    .unwrap_or_else(|| entity.entity.action_target())
                    == request.action_target
            })
            .map(|entity| entity.entity)
        else {
            return false;
        };
        self.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(tab_id),
            source_id: SOURCE_LATENT_MATERIALIZER.to_owned(),
            set: BTreeMap::from([
                (
                    KEY_ENTITY_KIND.to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text(entity.kind.clone()),
                        ttl_ms: None,
                        precedence: Some(LATENT_MATERIALIZER_PRECEDENCE),
                        ordinal: None,
                    },
                ),
                (
                    KEY_ENTITY_ID.to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text(entity.id),
                        ttl_ms: None,
                        precedence: Some(LATENT_MATERIALIZER_PRECEDENCE),
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        })
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
            collapsible: true,
            active_tab_name: None,
        };
        let resolved = match catalog.resolve(context) {
            Ok(Some(resolved)) => resolved,
            Ok(None) => return None,
            Err(error) => {
                return Some(ResolvedTemplateSlot {
                    template_name: "<resolve-error>".to_owned(),
                    fields: vec![],
                    render_ready: None,
                    setters: vec![],
                    effective_kdl: format!("// error: {error}\n"),
                    resolve_error: Some(error.to_string()),
                });
            }
        };
        if resolved.is_bundled && slot.is_rail_local() {
            // These slots depend on rail-local state such as collapse and available width.
            // Leave bundled resolution to the rail; configured overrides remain resolved here.
            return None;
        }
        Some(ResolvedTemplateSlot {
            template_name: resolved.name.clone(),
            fields: vec![],
            render_ready: Some(resolved.render_ready()),
            setters: resolved.setters.clone(),
            effective_kdl: resolved.dump_kdl(),
            resolve_error: None,
        })
    }

    fn tab_grouping_infos(&self) -> HashMap<u64, TabGroupingInfo> {
        let tab_seed_metadata = self.tab_seed_metadata_entries();
        let tab_entities: HashMap<u64, TabGroupingInfo> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                self.tab_entity_grouping(tab.tab_id, &tab_seed_metadata)
                    .map(|grouping| (tab.tab_id, grouping))
            })
            .collect();
        let default_catalog = GroupingConfigCatalog::default();
        let grouping_catalog = self.grouping_catalog.as_ref().unwrap_or(&default_catalog);
        let mut groupings: HashMap<u64, TabGroupingInfo> = self
            .tabs
            .iter()
            .filter(|tab| !tab_entities.contains_key(&tab.tab_id))
            .filter_map(|tab| {
                self.tab_rule_grouping(tab.tab_id, grouping_catalog, &tab_seed_metadata)
                    .map(|grouping| (tab.tab_id, grouping))
            })
            .collect();
        let directory_tab_ids = groupings
            .iter()
            .filter_map(|(tab_id, grouping)| {
                grouping_uses_rule(grouping, DIRECTORY_GROUPING_RULE).then_some(*tab_id)
            })
            .collect::<HashSet<_>>();
        if !directory_tab_ids.is_empty() {
            let directory_seed_metadata = self.tab_seed_metadata_entries_for(&directory_tab_ids);
            for tab_id in &directory_tab_ids {
                if let Some(grouping) =
                    self.tab_rule_grouping(*tab_id, grouping_catalog, &directory_seed_metadata)
                {
                    groupings.insert(*tab_id, grouping);
                }
            }
        }
        groupings.extend(tab_entities);
        groupings
    }

    fn tab_rule_grouping(
        &self,
        tab_id: u64,
        catalog: &GroupingConfigCatalog,
        tab_seed_metadata: &TabSeedMetadata,
    ) -> Option<TabGroupingInfo> {
        let metadata = self.tab_resolved_metadata_values(tab_id, tab_seed_metadata);
        if let Some(name) = self.active_grouping_template.as_deref() {
            return catalog
                .named(name)
                .and_then(|rule| self.tab_grouping_for_rule(rule, &metadata));
        }
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
        if rule
            .filter
            .as_ref()
            .is_some_and(|filter| !filter.matches(metadata))
        {
            return None;
        }
        let (path, _) = grouping_path_for_facts(rule, metadata)?;
        let labels = path.0.iter().map(group_segment_label).collect::<Vec<_>>();
        let key = format!(
            "{}:{}",
            rule.name,
            path.0
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
            path,
            label,
            full_label: labels.join(" / "),
        })
    }

    fn tab_resolved_metadata_values(
        &self,
        tab_id: u64,
        tab_seed_metadata: &TabSeedMetadata,
    ) -> BTreeMap<String, MetadataValue> {
        let target = EntityId::Tab(tab_id);
        let seed_values = tab_seed_metadata.get(&tab_id).cloned().unwrap_or_default();
        let (values, _, _) = self.resolve_target_metadata(&target, seed_values);
        values
            .into_iter()
            .map(|(key, entry)| (key, entry.value))
            .collect()
    }

    fn tab_entity_grouping(
        &self,
        tab_id: u64,
        tab_seed_metadata: &TabSeedMetadata,
    ) -> Option<TabGroupingInfo> {
        let entity = self.tab_entity_ref(tab_id, tab_seed_metadata)?;
        let path = self.presentation_path_for_entity(&entity)?;
        tab_grouping_info_for_entity_path(path)
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
        let directory_tab_ids = tabs
            .iter()
            .filter_map(|tab| {
                tab.grouping
                    .as_ref()
                    .filter(|grouping| grouping_uses_rule(grouping, DIRECTORY_GROUPING_RULE))
                    .map(|_| tab.tab_id)
            })
            .collect::<HashSet<_>>();
        let tab_seed_metadata = self.tab_seed_metadata_entries_for(&directory_tab_ids);
        let mut by_target: BTreeMap<EntityId, BTreeMap<String, MetadataEntry>> = BTreeMap::new();
        let mut sources_by_target: BTreeMap<EntityId, BTreeMap<String, Vec<MetadataSourceEntry>>> =
            BTreeMap::new();
        let mut identities_by_target: BTreeMap<EntityId, Vec<ReachableMetadataIdentity>> =
            BTreeMap::new();
        let catalog_entities = self.catalog_entities();
        let mut group_paths = catalog_entities
            .iter()
            .flat_map(|entity| group_path_prefixes(&entity.path))
            .chain(self.metadata.targets().filter_map(|target| match target {
                EntityId::Group(path) => Some(path.clone()),
                _ => None,
            }))
            .collect::<BTreeSet<_>>();
        let root_target = EntityId::Root;
        let (root_values, root_sources, root_identities) =
            self.resolve_target_metadata(&root_target, BTreeMap::new());
        if !root_values.is_empty() || !root_sources.is_empty() || !root_identities.is_empty() {
            by_target
                .entry(root_target.clone())
                .or_default()
                .extend(root_values);
            sources_by_target
                .entry(root_target.clone())
                .or_default()
                .extend(root_sources);
            identities_by_target
                .entry(root_target)
                .or_default()
                .extend(root_identities);
        }
        for tab in tabs {
            let tab_target = EntityId::Tab(tab.tab_id);
            let tab_seed_values = tab_seed_metadata
                .get(&tab.tab_id)
                .cloned()
                .unwrap_or_default();
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
            let (mut group_values, mut group_sources, mut group_identities) =
                self.resolve_target_metadata(&group_target, group_seed_values);
            if let Some(entity) = catalog_entities.iter().find(|entity| entity.path == path) {
                let entity_target = EntityId::Entity(entity.entity.clone());
                let (entity_values, entity_sources, entity_identities) =
                    self.resolve_target_metadata(&entity_target, BTreeMap::new());
                group_values.extend(entity_values);
                for (key, entries) in entity_sources {
                    group_sources.entry(key).or_default().extend(entries);
                }
                group_identities.extend(entity_identities);
            }
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
            if let Some(entity) = entity_ref_from_entries(&current_values) {
                let entity_target = EntityId::Entity(entity);
                if !visited.contains(&entity_target) {
                    queue.push_back((entity_target, distance + 1));
                }
            }
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

    fn tab_seed_metadata_entries(&self) -> TabSeedMetadata {
        self.tab_seed_metadata_entries_for(&HashSet::new())
    }

    fn tab_seed_metadata_entries_for(&self, directory_tab_ids: &HashSet<u64>) -> TabSeedMetadata {
        let cwd_entries = self
            .tabs
            .iter()
            .filter_map(|tab| {
                self.tab_primary_metadata_entry(tab.tab_id, KEY_PANE_CWD)
                    .map(|entry| (tab.tab_id, entry))
            })
            .collect::<HashMap<_, _>>();
        let all_group_cwds = cwd_entries
            .iter()
            .filter_map(|(tab_id, entry)| {
                if !directory_tab_ids.contains(tab_id) {
                    return None;
                }
                match &entry.value {
                    MetadataValue::Text(cwd) => Some(cwd.clone()),
                    _ => None,
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        cwd_entries
            .into_iter()
            .map(|(tab_id, cwd_entry)| {
                let mut entries = BTreeMap::from([(KEY_PANE_CWD.to_owned(), cwd_entry.clone())]);
                if let MetadataValue::Text(cwd) = &cwd_entry.value {
                    entries.insert(
                        KEY_PANE_CWD_LABEL.to_owned(),
                        MetadataEntry {
                            value: MetadataValue::Text(cwd_group_label(cwd, &all_group_cwds)),
                            ..cwd_entry
                        },
                    );
                }
                (tab_id, entries)
            })
            .collect()
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

fn grouping_path_for_facts(
    rule: &GroupingRule,
    facts: &BTreeMap<String, MetadataValue>,
) -> Option<(GroupPath, usize)> {
    derive_grouping_path(rule, facts).ok()
}

fn node_metadata(
    node: &NodeKey,
    resolved_metadata: &[ResolvedMetadata],
) -> BTreeMap<String, MetadataValue> {
    let target = match node {
        NodeKey::Root => andamento_shared::ResolvedMetadataTarget::Root,
        NodeKey::Group(path) => andamento_shared::ResolvedMetadataTarget::Group(path.clone()),
        NodeKey::Tab(tab_id) => andamento_shared::ResolvedMetadataTarget::Tab(*tab_id),
        NodeKey::Entity(entity) => andamento_shared::ResolvedMetadataTarget::Entity(entity.clone()),
    };
    resolved_metadata
        .iter()
        .find(|metadata| metadata.target == target)
        .map(|metadata| {
            metadata
                .values
                .iter()
                .map(|(key, entry)| (key.clone(), entry.value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn apply_variable_setter(
    values: &mut BTreeMap<String, EffectiveVariableValue>,
    name: &str,
    value: &str,
    provenance: VariableSetterProvenance,
) {
    let overridden = values
        .remove(name)
        .map(|previous| {
            let mut history = vec![previous.provenance];
            history.extend(previous.overridden);
            history
        })
        .unwrap_or_default();
    values.insert(
        name.to_owned(),
        EffectiveVariableValue {
            value: value.to_owned(),
            provenance,
            overridden,
        },
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GroupingPathError {
    MissingNonOptionalLevel(String),
    NoDerivedLevels,
}

fn derive_grouping_path(
    rule: &GroupingRule,
    facts: &BTreeMap<String, MetadataValue>,
) -> Result<(GroupPath, usize), GroupingPathError> {
    let mut segments = vec![];
    let mut last_level = None;
    for (index, level) in rule.levels.iter().enumerate() {
        let Some(value) = facts.get(&level.key).cloned() else {
            if level.optional {
                continue;
            }
            return Err(GroupingPathError::MissingNonOptionalLevel(
                level.key.clone(),
            ));
        };
        let label = level
            .label_key
            .as_ref()
            .and_then(|key| facts.get(key))
            .map(metadata_value_display);
        segments.push(GroupSegment {
            key: level.key.clone(),
            value,
            label,
        });
        last_level = Some(index);
    }
    let last_level = last_level.ok_or(GroupingPathError::NoDerivedLevels)?;
    Ok((GroupPath(segments), last_level))
}

fn entity_facts(
    entity: &EntityRef,
    values: &BTreeMap<String, MetadataEntry>,
) -> BTreeMap<String, MetadataValue> {
    let mut facts = values
        .iter()
        .map(|(key, entry)| (key.clone(), entry.value.clone()))
        .collect::<BTreeMap<_, _>>();
    // The target is the canonical entity identity. Producers must not have to
    // duplicate it in every patch's set map for grouping filters and templates.
    facts.insert(
        KEY_ENTITY_KIND.to_owned(),
        MetadataValue::Text(entity.kind.clone()),
    );
    facts.insert(
        KEY_ENTITY_ID.to_owned(),
        MetadataValue::Text(entity.id.clone()),
    );
    facts
}

fn entity_ref_from_entries(entries: &BTreeMap<String, MetadataEntry>) -> Option<EntityRef> {
    Some(EntityRef {
        kind: metadata_entry_text(entries, KEY_ENTITY_KIND)?.to_owned(),
        id: metadata_entry_text(entries, KEY_ENTITY_ID)?.to_owned(),
    })
}

fn direct_child_path(parent: &GroupPath, child: &GroupPath) -> bool {
    child.0.len() == parent.0.len() + 1 && child.0.starts_with(&parent.0)
}

fn collapsed_child_entities(entities: &[CatalogEntity]) -> BTreeSet<EntityRef> {
    let mut collapsed = BTreeSet::new();
    for parent in entities
        .iter()
        .filter(|entity| entity.presence == PresenceClass::Tab && entity.collapse_single_member)
    {
        let children = entities
            .iter()
            .filter(|child| child.presence == PresenceClass::Tab)
            .filter(|child| direct_child_path(&parent.path, &child.path))
            .collect::<Vec<_>>();
        if let [child] = children.as_slice() {
            collapsed.insert(child.entity.clone());
        }
    }
    collapsed
}

fn tab_grouping_info_for_entity_path(path: GroupPath) -> Option<TabGroupingInfo> {
    if path.0.is_empty() {
        return None;
    }
    let labels = path.0.iter().map(group_segment_label).collect::<Vec<_>>();
    let key = format!(
        "entity:{}",
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

fn compact_parent_path(path: &GroupPath) -> GroupPath {
    GroupPath(path.0[..path.0.len().saturating_sub(1)].to_vec())
}

fn metadata_entry_text<'a>(
    values: &'a BTreeMap<String, MetadataEntry>,
    key: &str,
) -> Option<&'a str> {
    match values.get(key).map(|entry| &entry.value) {
        Some(MetadataValue::Text(value)) => Some(value),
        _ => None,
    }
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

fn group_header_identity_for_path(path: &GroupPath) -> (String, String, String) {
    let labels = path.0.iter().map(group_segment_label).collect::<Vec<_>>();
    let key = format!(
        "group:{}",
        path.0
            .iter()
            .map(|segment| format!("{}={}", segment.key, metadata_value_display(&segment.value)))
            .collect::<Vec<_>>()
            .join("/")
    );
    (
        key,
        labels.last().cloned().unwrap_or_default(),
        labels.join(" / "),
    )
}

fn group_segment_label(segment: &GroupSegment) -> String {
    segment
        .label
        .clone()
        .unwrap_or_else(|| metadata_value_display(&segment.value))
}

fn grouping_uses_rule(grouping: &TabGroupingInfo, rule_name: &str) -> bool {
    grouping
        .key
        .split_once(':')
        .is_some_and(|(selected_rule, _)| selected_rule == rule_name)
}

fn cwd_group_label(cwd: &str, all_group_cwds: &[String]) -> String {
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
    if let Some(segment) = path.0.last() {
        metadata.insert(
            "group.key".to_owned(),
            MetadataValue::Text(segment.key.clone()),
        );
        metadata.insert("group.value".to_owned(), segment.value.clone());
        metadata.insert(
            "group.segment.label".to_owned(),
            MetadataValue::Text(group_segment_label(segment)),
        );
    }
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
        GroupPath, GroupSegment, PluginPaneKind, PluginPlacement, PluginRegistrationHello, RailRow,
        RailStructure, StatusIcon,
    };

    type EntityId = andamento_shared::MetadataTarget;
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

    fn entity_ref(kind: &str, id: &str) -> andamento_shared::EntityRef {
        andamento_shared::EntityRef {
            kind: kind.to_owned(),
            id: id.to_owned(),
        }
    }

    #[test]
    fn node_variables_inherit_and_config_layers_override_template_setters() {
        let declaration = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            variable "child-layout" default="cards" {
              value "cards"
              value "strip"
            }
            "#,
        )
        .expect("variable declaration");
        let template = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            template "tab/title" slot="tab-title" node-kind="tab" {
              set "child-layout" "strip"
            }
            "#,
        )
        .expect("template setter");
        let project = andamento_shared::template_config::parse_template_config_kdl(
            r#"set "child-layout" "cards""#,
        )
        .expect("project setter");
        let catalog = andamento_shared::template_config::TemplateConfigCatalog::from_layers(vec![
            andamento_shared::template_config::TemplateConfigLayer::user("user.kdl", template),
            andamento_shared::template_config::TemplateConfigLayer::project(
                "flotilla-org/andamento",
                "project.kdl",
                project,
            ),
            andamento_shared::template_config::TemplateConfigLayer::bundled(
                "bundled.kdl",
                declaration,
            ),
        ]);
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(catalog.clone()));

        let plain_metadata = BTreeMap::new();
        let resolved_template = state
            .resolve_template_slot(
                andamento_shared::template_config::TemplateConfigSlot::TabTitle,
                andamento_shared::template_config::TemplateConfigNodeKind::Tab,
                &plain_metadata,
            )
            .expect("user template resolves");
        let mut warnings = BTreeSet::new();
        let parent = NodeKey::Tab(1);
        let parent_values = state.resolve_variables_at_node(
            &catalog,
            &parent,
            None,
            &plain_metadata,
            Some(&resolved_template),
            &mut warnings,
        );
        let child = NodeKey::Tab(2);
        let child_values = state.resolve_variables_at_node(
            &catalog,
            &child,
            Some(&parent_values.values),
            &plain_metadata,
            None,
            &mut warnings,
        );
        let inherited = child_values
            .values
            .get("child-layout")
            .expect("inherited child-layout");
        assert_eq!(inherited.value, "strip");
        assert_eq!(inherited.provenance.ancestor, parent);
        assert_eq!(inherited.provenance.setter, "template tab/title");

        let project_metadata = BTreeMap::from([(
            "flotilla.project".to_owned(),
            MetadataValue::Text("flotilla-org/andamento".to_owned()),
        )]);
        let project_node = NodeKey::Tab(3);
        let project_values = state.resolve_variables_at_node(
            &catalog,
            &project_node,
            None,
            &project_metadata,
            Some(&resolved_template),
            &mut warnings,
        );
        let configured = project_values
            .values
            .get("child-layout")
            .expect("configured child-layout");
        assert_eq!(configured.value, "cards");
        assert_eq!(configured.provenance.setter, "config");
        assert_eq!(
            configured.provenance.origin.membership.as_deref(),
            Some("flotilla-org/andamento")
        );
        assert_eq!(configured.overridden[0].setter, "template tab/title");
        assert!(warnings.is_empty());
    }

    #[test]
    fn cross_layer_variable_setter_warnings_reach_template_diagnostics() {
        let user = andamento_shared::template_config::parse_template_config_kdl(
            r#"set "child-layout" "compact-strip""#,
        )
        .expect("user setter");
        let bundled = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            variable "child-layout" default="cards" {
              value "cards"
              value "strip"
            }
            "#,
        )
        .expect("bundled declaration");
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_layers(vec![
                andamento_shared::template_config::TemplateConfigLayer::user("user.kdl", user),
                andamento_shared::template_config::TemplateConfigLayer::bundled(
                    "bundled.kdl",
                    bundled,
                ),
            ]),
        ));

        let model = state.view_model();

        assert!(model.template_config.warnings.iter().any(|warning| {
            warning.contains("child-layout") && warning.contains("disallowed value compact-strip")
        }));
    }

    fn apply_entity(
        state: &mut ControllerState,
        kind: &str,
        id: &str,
        ordinal: i64,
        facts: &[(&str, &str)],
    ) {
        let entity = entity_ref(kind, id);
        let mut set = BTreeMap::from([
            (
                KEY_ENTITY_KIND.to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text(kind.to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: Some(ordinal),
                },
            ),
            (
                KEY_ENTITY_ID.to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text(id.to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: Some(ordinal),
                },
            ),
        ]);
        set.extend(facts.iter().map(|(key, value)| {
            (
                (*key).to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Text((*value).to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: Some(ordinal),
                },
            )
        }));
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Entity(entity),
            source_id: "flotilla-connector".to_owned(),
            set,
            unset: vec![],
        });
    }

    fn apply_target_only_entity(
        state: &mut ControllerState,
        kind: &str,
        id: &str,
        ordinal: i64,
        facts: &[(&str, &str)],
    ) {
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Entity(entity_ref(kind, id)),
            source_id: "flotilla-connector".to_owned(),
            set: facts
                .iter()
                .map(|(key, value)| {
                    (
                        (*key).to_owned(),
                        andamento_shared::MetadataValueUpdate {
                            value: MetadataValue::Text((*value).to_owned()),
                            ttl_ms: None,
                            precedence: None,
                            ordinal: Some(ordinal),
                        },
                    )
                })
                .collect(),
            unset: vec![],
        });
    }

    fn directory_entity_state() -> ControllerState {
        let mut state = ControllerState::default();
        state.set_template_catalog(None);
        state.set_rail_config(RailConfig::default());
        state
    }

    #[test]
    fn automatic_grouping_skips_a_matching_rule_without_a_derivable_path() {
        use andamento_shared::grouping_config::{
            ExternalGroupingConfig, GroupingLevel, GroupingRule, PresenceMapping,
        };

        let mut state = directory_entity_state();
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(
            ExternalGroupingConfig {
                version: 1,
                rules: vec![
                    GroupingRule {
                        name: "missing-facts".to_owned(),
                        priority: 100,
                        filter: None,
                        presence: vec![],
                        levels: vec![GroupingLevel {
                            key: "git.repo".to_owned(),
                            optional: false,
                            label_key: None,
                            collapse_single_member: false,
                            show_empty: false,
                            template: None,
                        }],
                    },
                    GroupingRule {
                        name: "entity-target".to_owned(),
                        priority: 50,
                        filter: None,
                        presence: vec![PresenceMapping {
                            kind: "issue".to_owned(),
                            class: PresenceClass::Inline,
                            form: DISPLAY_FORM_COMPACT.to_owned(),
                            visible_when: None,
                            template: None,
                        }],
                        levels: vec![GroupingLevel {
                            key: KEY_ENTITY_ID.to_owned(),
                            optional: false,
                            label_key: None,
                            collapse_single_member: false,
                            show_empty: false,
                            template: None,
                        }],
                    },
                ],
            },
        )));
        apply_target_only_entity(
            &mut state,
            "issue",
            "github/flotilla-org/andamento#37",
            7,
            &[(KEY_DISPLAY_LABEL, "#37")],
        );

        let entities = state.catalog_entities();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].path.0[0].key, KEY_ENTITY_ID);
        assert_eq!(
            entities[0].path.0[0].value,
            MetadataValue::Text("github/flotilla-org/andamento#37".to_owned())
        );
    }

    #[test]
    fn required_partial_path_falls_through_to_a_later_rule() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "repo-branch" priority=100 {
              presence kind="convoy" class="tab"
              level key="vcs.repo"
              level key="git.branch"
            }
            grouping "repo" priority=50 {
              presence kind="convoy" class="tab"
              level key="vcs.repo"
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        apply_target_only_entity(
            &mut state,
            "convoy",
            "flotilla/partial-path@fleet",
            1,
            &[("vcs.repo", "flotilla-org/flotilla")],
        );

        let entities = state.catalog_entities();

        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].grouping_priority, 50);
        assert_eq!(
            entities[0]
                .path
                .0
                .iter()
                .map(|segment| segment.key.as_str())
                .collect::<Vec<_>>(),
            vec!["vcs.repo"]
        );
    }

    #[test]
    fn rejected_rule_explains_its_missing_non_optional_level() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "repo-branch" priority=100 {
              presence kind="convoy" class="tab"
              level key="vcs.repo"
              level key="git.branch"
            }
            grouping "repo" priority=50 {
              presence kind="convoy" class="tab"
              level key="vcs.repo"
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        let entity = entity_ref("convoy", "flotilla/inspect-path@fleet");
        apply_target_only_entity(
            &mut state,
            &entity.kind,
            &entity.id,
            1,
            &[("vcs.repo", "flotilla-org/flotilla")],
        );

        let diagnostics = state.view_model().grouping_diagnostics;

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.target == NodeKey::Entity(entity.clone())
                && diagnostic.rule == "repo-branch"
                && diagnostic.message == "not captured: `git.branch` absent (non-optional level)"
        }));
    }

    #[test]
    fn optional_missing_level_places_entity_at_the_deepest_derived_group() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "repo-branch" priority=100 {
              presence kind="convoy" class="tab"
              level key="vcs.repo"
              level key="git.branch" optional=true
            }
            grouping "repo" priority=50 {
              presence kind="convoy" class="tab"
              level key="vcs.repo"
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        apply_target_only_entity(
            &mut state,
            "convoy",
            "flotilla/optional-path@fleet",
            1,
            &[("vcs.repo", "flotilla-org/flotilla")],
        );

        let entities = state.catalog_entities();

        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].grouping_priority, 100);
        assert_eq!(
            entities[0]
                .path
                .0
                .iter()
                .map(|segment| segment.key.as_str())
                .collect::<Vec<_>>(),
            vec!["vcs.repo"]
        );
    }

    #[test]
    fn target_only_entities_keep_patch_ordinals_for_catalog_ordering() {
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "issue",
            "later",
            10,
            &[("flotilla.project", "dev"), ("flotilla.issue", "later")],
        );
        apply_target_only_entity(
            &mut state,
            "issue",
            "earlier",
            2,
            &[("flotilla.project", "dev"), ("flotilla.issue", "earlier")],
        );

        let entities = state.catalog_entities();
        assert_eq!(
            entities
                .iter()
                .map(|entity| (entity.entity.id.as_str(), entity.ordinal))
                .collect::<Vec<_>>(),
            vec![("earlier", 2), ("later", 10)]
        );
    }

    #[test]
    fn default_entity_template_collapses_a_single_vessel_into_its_convoy_tab() {
        let mut state = directory_entity_state();
        let shared_target = "flotilla:attach:dev/only@lab";
        apply_entity(
            &mut state,
            "convoy",
            "dev/only@lab",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.project.name", "dev"),
                ("flotilla.convoy", "dev/only@lab"),
                ("flotilla.convoy.name", "only"),
                (KEY_DISPLAY_LABEL, "only"),
                (KEY_ACTION_TARGET, shared_target),
                (KEY_MATERIALIZE_RECIPE, "flotilla attach dev/only"),
            ],
        );
        apply_entity(
            &mut state,
            "vessel",
            "dev/only/worker@lab",
            2,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.project.name", "dev"),
                ("flotilla.convoy", "dev/only@lab"),
                ("flotilla.convoy.name", "only"),
                ("flotilla.vessel", "dev/only/worker@lab"),
                ("flotilla.vessel.name", "worker"),
                (KEY_DISPLAY_LABEL, "worker"),
                (KEY_ACTION_TARGET, shared_target),
                (KEY_MATERIALIZE_RECIPE, "flotilla attach dev/only"),
            ],
        );

        let model = state.view_model();
        let latents = model
            .rows
            .iter()
            .filter_map(|row| match row {
                RailRow::Latent { latent, .. } => Some(latent),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(latents.len(), 1);
        assert_eq!(latents[0].action_target, shared_target);
        assert_ne!(latents[0].action_target, "dev/only@lab");
        assert_eq!(latents[0].name, "only");
        assert_eq!(latents[0].path.0.len(), 2);
        assert_eq!(latents[0].path.0.last().unwrap().key, "flotilla.convoy");
        assert_eq!(
            state.activation_for_entity(&entity_ref("convoy", "dev/only@lab")),
            latents[0]
                .materialize_request()
                .map(EntityActivation::Materialize)
        );
    }

    #[test]
    fn recipe_less_tab_entity_is_visible_but_not_openable() {
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "vessel",
            "dev/solo/worker@lab",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.project.name", "dev"),
                ("flotilla.vessel", "dev/solo/worker@lab"),
                ("flotilla.vessel.name", "worker"),
                (KEY_DISPLAY_LABEL, "worker"),
            ],
        );

        let model = state.view_model();
        let latent = model
            .rows
            .iter()
            .find_map(|row| match row {
                RailRow::Latent { latent, .. } => Some(latent),
                _ => None,
            })
            .expect("recipe-less vessel remains visible");

        assert_eq!(latent.name, "worker");
        assert!(latent.materialize_request().is_none());
    }

    #[test]
    fn issue_entities_use_the_bundled_compact_form_and_class_toggle() {
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "issue",
            "github/flotilla-org/flotilla#982",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.project.name", "dev"),
                ("flotilla.issue", "github/flotilla-org/flotilla#982"),
                (KEY_DISPLAY_LABEL, "#982 entities-only cutover"),
                ("summary.text", "Cached entity metadata survives"),
            ],
        );

        let model = state.view_model();

        assert!(model.rows.iter().any(|row| matches!(
            row,
            RailRow::Entity { entity, .. }
                if entity.label == "#982 entities-only cutover"
                    && entity.form == DISPLAY_FORM_COMPACT
                    && entity.templates.compact.is_some()
                    && entity.templates.detail.is_some()
                    && matches!(
                        entity.metadata.get("summary.text"),
                        Some(MetadataValue::Text(summary))
                            if summary == "Cached entity metadata survives"
                    )
        )));
        assert!(!model
            .rows
            .iter()
            .any(|row| matches!(row, RailRow::Latent { .. })));

        state.apply_rail_ui_action(RailUiAction::ToggleVariable {
            name: "show-issues".to_owned(),
        });

        assert!(!state
            .view_model()
            .rows
            .iter()
            .any(|row| matches!(row, RailRow::Entity { .. })));
    }

    #[test]
    fn novel_wire_kind_reaches_the_generic_compact_template() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "deployments" priority=100 {
              filter key="entity.kind"
              presence kind="deployment" class="inline" form="compact"
              level key="deployment.name" label-key="display.label"
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::default(),
        ));
        apply_entity(
            &mut state,
            "deployment",
            "prod/api",
            1,
            &[
                ("deployment.name", "api"),
                (KEY_DISPLAY_LABEL, "API deployment"),
            ],
        );

        let entity = state
            .view_model()
            .rows
            .into_iter()
            .find_map(|row| match row {
                RailRow::Entity { entity, .. } => Some(entity),
                _ => None,
            })
            .expect("novel entity renders inline");
        let compact = entity
            .templates
            .compact
            .expect("generic compact template resolves");

        assert_eq!(entity.entity.kind, "deployment");
        assert_eq!(compact.template_name, "deployment/compact");
        assert!(compact.effective_kdl.contains("flotilla/entity/compact"));
        assert!(compact.render_ready.is_some());
    }

    #[test]
    fn attention_regions_promote_tab_presence_entities_from_normalized_facts() {
        let mut state = directory_entity_state();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::default(),
        ));
        apply_entity(
            &mut state,
            "convoy",
            "flotilla/sidebar@fleet",
            1,
            &[
                ("flotilla.convoy", "flotilla/sidebar@fleet"),
                ("flotilla.convoy.name", "sidebar"),
                (KEY_DISPLAY_LABEL, "Sidebar convoy"),
            ],
        );
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Entity(entity_ref(
                "convoy",
                "flotilla/sidebar@fleet",
            )),
            source_id: "flotilla-connector".to_owned(),
            set: BTreeMap::from([(
                "status.attention".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Bool(true),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: Some(1),
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let attention = model
            .surface_regions
            .iter()
            .find(|region| {
                region.definition.source
                    == andamento_shared::template_config::SurfaceRegionSource::Attention
            })
            .expect("bundled attention region");

        assert!(attention.entities.iter().any(|entity| {
            entity.entity == entity_ref("convoy", "flotilla/sidebar@fleet")
                && entity.form == DISPLAY_FORM_FULL
        }));
        assert!(
            !model.rows.iter().any(|row| matches!(row, RailRow::Entity { entity, .. } if entity.entity.kind == "convoy")),
            "the promoted convoy normally has tab/latent presence, not inline presence"
        );
    }

    #[test]
    fn attention_promotion_highlights_inline_entities_without_removing_tree_navigation() {
        let mut state = directory_entity_state();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::default(),
        ));
        apply_entity(
            &mut state,
            "issue",
            "github/flotilla-org/flotilla#1060",
            1,
            &[
                ("flotilla.issue", "github/flotilla-org/flotilla#1060"),
                (KEY_DISPLAY_LABEL, "#1060 region stack"),
            ],
        );
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Entity(entity_ref(
                "issue",
                "github/flotilla-org/flotilla#1060",
            )),
            source_id: "flotilla-connector".to_owned(),
            set: BTreeMap::from([(
                "status.attention".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::Bool(true),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: Some(1),
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let issue = entity_ref("issue", "github/flotilla-org/flotilla#1060");
        assert!(model
            .rows
            .iter()
            .any(|row| matches!(row, RailRow::Entity { entity, .. } if entity.entity == issue)));
        assert!(model.surface_regions.iter().any(|region| {
            region.definition.source
                == andamento_shared::template_config::SurfaceRegionSource::Attention
                && region.entities.iter().any(|entity| entity.entity == issue)
        }));
    }

    #[test]
    fn novel_inline_form_uses_the_full_surface_instead_of_disappearing() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "deployments" priority=100 {
              filter key="entity.kind"
              presence kind="deployment" class="inline" form="ribbon"
              level key="deployment.name" label-key="display.label" show-empty=true
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        apply_entity(
            &mut state,
            "deployment",
            "prod/api",
            1,
            &[
                ("deployment.name", "api"),
                (KEY_DISPLAY_LABEL, "API deployment"),
            ],
        );

        let model = state.view_model();

        assert!(model.rows.iter().any(|row| matches!(
            row,
            RailRow::GroupHeader { label, .. } if label == "API deployment"
        )));
        assert!(!model
            .rows
            .iter()
            .any(|row| matches!(row, RailRow::Entity { .. })));
    }

    #[test]
    fn presence_template_overrides_the_selected_form_without_leaking_to_detail() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
                grouping "issues" priority=100 {
                  filter key="entity.kind"
                  presence kind="issue" class="inline" form="compact" template="issue/attention"
                  level key="flotilla.project" optional=true
                  level key="flotilla.issue"
                }
                "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        let templates = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            template "issue/attention" slot="compact" node-kind="entity" {
              field "label" source="literal" value="attention"
            }
            template "issue/detail" slot="detail" node-kind="entity" {
              field "label" key="display.label"
            }
            "#,
        )
        .expect("template config");
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(
                templates,
            ),
        ));
        apply_entity(
            &mut state,
            "issue",
            "github/flotilla-org/flotilla#1058",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.issue", "github/flotilla-org/flotilla#1058"),
                (KEY_DISPLAY_LABEL, "#1058 template binding"),
            ],
        );

        let entity = state
            .view_model()
            .rows
            .into_iter()
            .find_map(|row| match row {
                RailRow::Entity { entity, .. } => Some(entity),
                _ => None,
            })
            .expect("compact issue entity");

        assert_eq!(
            entity
                .templates
                .compact
                .as_ref()
                .map(|slot| slot.template_name.as_str()),
            Some("issue/attention")
        );
        assert_eq!(
            entity
                .templates
                .detail
                .as_ref()
                .map(|slot| slot.template_name.as_str()),
            Some("issue/detail")
        );
    }

    #[test]
    fn group_template_comes_from_the_rule_that_produced_the_path() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "issues" priority=200 {
              filter key="entity.kind" equals="issue"
              level key="vcs.repo" template="wrong/full"
              level key="flotilla.issue"
            }
            grouping "convoys" priority=100 {
              filter key="entity.kind" equals="convoy"
              presence kind="convoy" class="tab"
              level key="vcs.repo" template="right/full"
              level key="flotilla.convoy"
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        let templates = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            template "wrong/full" slot="group-header" node-kind="group" {
              field "label" source="literal" value="wrong"
            }
            template "right/full" slot="group-header" node-kind="group" {
              field "label" source="literal" value="right"
            }
            "#,
        )
        .expect("template config");
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(templates),
        ));
        apply_entity(
            &mut state,
            "convoy",
            "flotilla/review-regression@fleet",
            1,
            &[
                ("vcs.repo", "flotilla-org/andamento"),
                ("flotilla.convoy", "flotilla/review-regression@fleet"),
                ("flotilla.convoy.name", "review-regression"),
                (KEY_DISPLAY_LABEL, "review-regression"),
            ],
        );

        let repo_template = state
            .view_model()
            .rows
            .into_iter()
            .find_map(|row| match row {
                RailRow::GroupHeader {
                    path, templates, ..
                } if path.0.len() == 1 && path.0[0].key == "vcs.repo" => templates.group_header,
                _ => None,
            });

        assert_eq!(
            repo_template
                .as_ref()
                .map(|slot| slot.template_name.as_str()),
            Some("right/full")
        );
    }

    #[test]
    fn colliding_group_paths_choose_the_higher_priority_rule_template() {
        let mut state = directory_entity_state();
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "issues" priority=200 {
              filter key="entity.kind" equals="issue"
              presence kind="issue" class="tab"
              level key="vcs.repo" template="high/full"
            }
            grouping "convoys" priority=100 {
              filter key="entity.kind" equals="convoy"
              presence kind="convoy" class="tab"
              level key="vcs.repo" template="low/full"
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        let templates = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            template "high/full" slot="group-header" node-kind="group" {
              field "label" source="literal" value="high"
            }
            template "low/full" slot="group-header" node-kind="group" {
              field "label" source="literal" value="low"
            }
            "#,
        )
        .expect("template config");
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(templates),
        ));
        apply_entity(
            &mut state,
            "convoy",
            "flotilla/low@fleet",
            1,
            &[
                ("vcs.repo", "flotilla-org/andamento"),
                (KEY_DISPLAY_LABEL, "low"),
            ],
        );
        apply_entity(
            &mut state,
            "issue",
            "github/flotilla-org/andamento#41",
            2,
            &[
                ("vcs.repo", "flotilla-org/andamento"),
                (KEY_DISPLAY_LABEL, "high"),
            ],
        );

        let repo_template = state
            .view_model()
            .rows
            .into_iter()
            .find_map(|row| match row {
                RailRow::GroupHeader {
                    path, templates, ..
                } if path.0.len() == 1 && path.0[0].key == "vcs.repo" => templates.group_header,
                _ => None,
            });

        assert_eq!(
            repo_template
                .as_ref()
                .map(|slot| slot.template_name.as_str()),
            Some("high/full")
        );
    }

    #[test]
    fn visible_when_mismatches_are_reported_without_breaking_fallback_rendering() {
        let mut state = directory_entity_state();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::ExternalTemplateConfig {
                    placements: vec![],
                    version: 1,
                    templates: vec![],
                    fragments: vec![],
                    variables: vec![],
                    display_variables: vec![],
                    sets: vec![],
                    regions: vec![],
                },
            ),
        ));
        assert_eq!(
            state.template_config_diagnostics().warnings,
            vec!["visible-when references undeclared variable show-issues"]
        );

        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"
                    display-variable "show-issues" type="enum" default="all" label="Issues" icon="I" {
                      value "all"
                      value "none"
                    }
                    "#,
                )
                .unwrap(),
            ),
        ));
        assert_eq!(
            state.template_config_diagnostics().warnings,
            vec!["visible-when references non-bool variable show-issues"]
        );

        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"display-variable "show-issues" type="bool" default=true label="Issues" icon="I""#,
                )
                .unwrap(),
            ),
        ));
        assert!(state.template_config_diagnostics().warnings.is_empty());
    }

    #[test]
    fn bundled_rail_templates_stay_unresolved_until_the_rail_has_local_state() {
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::default(),
        ));

        let group_metadata = BTreeMap::from([
            (
                "group.key".to_owned(),
                MetadataValue::Text("vcs.repo".to_owned()),
            ),
            (
                "vcs.repo".to_owned(),
                MetadataValue::Text("flotilla-org/andamento".to_owned()),
            ),
        ]);
        assert!(state
            .resolve_template_slot(
                andamento_shared::template_config::TemplateConfigSlot::GroupHeader,
                andamento_shared::template_config::TemplateConfigNodeKind::Group,
                &group_metadata,
            )
            .is_none());

        let entity_metadata = BTreeMap::from([
            (
                "entity.kind".to_owned(),
                MetadataValue::Text("issue".to_owned()),
            ),
            (
                "display.label".to_owned(),
                MetadataValue::Text("#1057 template KDL migration".to_owned()),
            ),
        ]);
        assert!(state
            .resolve_template_slot(
                andamento_shared::template_config::TemplateConfigSlot::Compact,
                andamento_shared::template_config::TemplateConfigNodeKind::Entity,
                &entity_metadata,
            )
            .is_some());
    }

    #[test]
    fn missing_hierarchy_levels_are_skipped_without_inventing_repo_facts() {
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "convoy",
            "dev/no-repo@lab",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.project.name", "dev"),
                ("flotilla.convoy", "dev/no-repo@lab"),
                ("flotilla.convoy.name", "no repo"),
            ],
        );

        let latent = state
            .view_model()
            .rows
            .into_iter()
            .find_map(|row| match row {
                RailRow::Latent { latent, .. } => Some(latent),
                _ => None,
            })
            .expect("convoy candidate");

        assert_eq!(
            latent
                .path
                .0
                .iter()
                .map(|segment| segment.key.as_str())
                .collect::<Vec<_>>(),
            vec!["flotilla.project", "flotilla.convoy"]
        );
    }

    #[test]
    fn changing_the_active_template_regroups_existing_entity_facts() {
        use andamento_shared::grouping_config::{
            ExternalGroupingConfig, GroupingLevel, GroupingRule, PresenceMapping,
        };

        let mut state = directory_entity_state();
        state.set_grouping_catalog(Some(GroupingConfigCatalog::with_bundled_defaults(
            ExternalGroupingConfig {
                version: 1,
                rules: vec![GroupingRule {
                    name: "convoy-only".to_owned(),
                    priority: 100,
                    filter: None,
                    presence: vec![PresenceMapping {
                        kind: "convoy".to_owned(),
                        class: PresenceClass::Tab,
                        form: DISPLAY_FORM_FULL.to_owned(),
                        visible_when: None,
                        template: None,
                    }],
                    levels: vec![GroupingLevel {
                        key: "flotilla.convoy".to_owned(),
                        optional: true,
                        label_key: Some("flotilla.convoy.name".to_owned()),
                        collapse_single_member: false,
                        show_empty: false,
                        template: None,
                    }],
                }],
            },
        )));
        assert!(state.set_active_grouping_template(Some("flotilla.default".to_owned())));
        apply_entity(
            &mut state,
            "convoy",
            "dev/regroup@lab",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.project.name", "dev"),
                ("flotilla.convoy", "dev/regroup@lab"),
                ("flotilla.convoy.name", "regroup"),
            ],
        );
        let default_path = state
            .latent_tabs()
            .into_iter()
            .next()
            .expect("default candidate")
            .path;

        assert!(state.set_active_grouping_template(Some("convoy-only".to_owned())));
        let switched_path = state
            .latent_tabs()
            .into_iter()
            .next()
            .expect("regrouped candidate")
            .path;

        assert_eq!(default_path.0.len(), 2);
        assert_eq!(switched_path.0.len(), 1);
        assert_eq!(switched_path.0[0].key, "flotilla.convoy");
    }

    fn placement_loop(
        predicates: &[(&str, &str)],
    ) -> andamento_shared::template_config::PlacementDefinition {
        andamento_shared::template_config::PlacementDefinition {
            name: "section".to_owned(),
            loops: vec![andamento_shared::template_config::PlacementLoop {
                binding: "item".to_owned(),
                predicates: predicates
                    .iter()
                    .map(
                        |(key, value)| andamento_shared::template_config::PlacementPredicate {
                            key: (*key).to_owned(),
                            value: Some((*value).to_owned()),
                            of: None,
                        },
                    )
                    .collect(),
                fields: vec![],
                layout: None,
                loops: vec![],
                apply_template: None,
            }],
        }
    }

    #[test]
    fn an_entity_whose_facts_restate_its_kind_is_placed_once() {
        // Producers do restate identity in their patches — flotilla's connector
        // sets entity.kind and entity.id as facts — so the index sees the same
        // (key, value) twice for one entity: once from the EntityRef, once from
        // its facts. Without the dedup guard it gets two postings and renders
        // twice.
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "vessel",
            "dev/focus/worker@lab",
            1,
            &[
                ("entity.kind", "vessel"),
                ("entity.id", "dev/focus/worker@lab"),
                ("flotilla.vessel", "dev/focus/worker@lab"),
                ("display.label", "worker"),
            ],
        );

        let entities = state.catalog_entities();
        let index = PlacementIndex::build(&entities);
        let placed = state.evaluate_placement(
            &placement_loop(&[("entity.kind", "vessel")]),
            &entities,
            &index,
            "full",
        );

        assert_eq!(
            placed.len(),
            1,
            "restating identity in facts must not duplicate the placement: {:?}",
            placed.iter().map(|e| &e.entity).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_loop_matching_nothing_places_nothing() {
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "vessel",
            "dev/focus/worker@lab",
            1,
            &[("flotilla.vessel", "dev/focus/worker@lab")],
        );
        let entities = state.catalog_entities();
        let index = PlacementIndex::build(&entities);

        assert!(state
            .evaluate_placement(
                &placement_loop(&[("entity.kind", "convoy")]),
                &entities,
                &index,
                "full",
            )
            .is_empty());
        assert!(
            state
                .evaluate_placement(
                    &placement_loop(&[("entity.kind", "vessel"), ("status.attention", "true")]),
                    &entities,
                    &index,
                    "full",
                )
                .is_empty(),
            "predicates intersect: an unmatched one empties the result"
        );
    }

    #[test]
    fn nested_placement_builds_one_tree_and_stops_an_ancestor_cycle() {
        let config = andamento_shared::template_config::parse_template_config_kdl(r#"
version 1
region "attention" source="attention" root-template="flotilla/region/attention" form="full" placement="tree"
placement "tree" {
  for "project" kind="project" {
    apply-template
  }
}
template "project/line" {
  field "label" {
    value source="metadata-text" key="display.label"
  }
  for "convoy" kind="convoy" {
    match "flotilla.project" of="project"
    apply-template
  }
}
template "convoy/line" {
  field "label" {
    value source="metadata-text" key="display.label"
  }
  for "vessel" kind="vessel" {
    match "flotilla.convoy" of="convoy"
    apply-template
  }
  for "project" kind="project" {
    match "flotilla.project" of="convoy"
  }
}
template "vessel/line" {
  field "label" {
    value source="metadata-text" key="display.label"
  }
}
"#).expect("nested placement config");
        let mut state = directory_entity_state();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(config),
        ));
        apply_entity(
            &mut state,
            "project",
            "p",
            1,
            &[("flotilla.project", "p"), ("display.label", "Project P")],
        );
        apply_entity(
            &mut state,
            "convoy",
            "c",
            1,
            &[
                ("flotilla.project", "p"),
                ("flotilla.convoy", "c"),
                ("display.label", "Convoy C"),
            ],
        );
        apply_entity(
            &mut state,
            "vessel",
            "v",
            1,
            &[("flotilla.convoy", "c"), ("display.label", "Vessel V")],
        );

        let model = state.view_model();
        let roots = &model.surface_regions[0].entities;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].label, "Project P");
        assert_eq!(roots[0].children.len(), 1);
        assert_eq!(roots[0].children[0].label, "Convoy C");
        assert_eq!(
            roots[0].children[0].children.len(),
            1,
            "the project back-edge is cut while the vessel remains"
        );
        assert_eq!(roots[0].children[0].children[0].label, "Vessel V");
        let rendered = format!(
            "{}\n  {}\n    {}",
            roots[0].label, roots[0].children[0].label, roots[0].children[0].children[0].label
        );
        insta::assert_snapshot!(rendered, @r###"
        Project P
          Convoy C
            Vessel V
        "###);
    }

    #[test]
    fn applied_template_resolution_failure_is_visible_on_the_placed_entity() {
        let config = andamento_shared::template_config::parse_template_config_kdl(
            r#"
version 1
region "attention" source="attention" root-template="flotilla/region/attention" form="full" placement="tree"
placement "tree" {
  for "project" kind="project" {
    apply-template
  }
}
template "project/line" extends="missing/line" {
  field "label" source="metadata-text" key="display.label"
}
"#,
        )
        .expect("placement config parses before template resolution");
        let mut state = directory_entity_state();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::with_bundled_defaults(config),
        ));
        apply_entity(
            &mut state,
            "project",
            "p",
            1,
            &[("flotilla.project", "p"), ("display.label", "Project P")],
        );

        let slot = state.view_model().surface_regions[0].entities[0]
            .templates
            .compact
            .clone()
            .expect("applied template leaves visible resolution evidence");
        assert_eq!(slot.template_name, "<resolve-error>");
        assert!(slot
            .resolve_error
            .as_deref()
            .is_some_and(|error| error.contains("missing/line")));
    }

    #[test]
    fn an_inline_presence_entity_has_no_activation_and_falls_back_to_inspect() {
        // The attention region admits every non-hidden presence class, but only
        // Tab-presence entities become tabs. An issue is Inline, so it has no
        // activation at all — the caller opens the inspector instead, which is
        // what the row did before activation existed.
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "issue",
            "github/flotilla-org/andamento#61",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.issue", "github/flotilla-org/andamento#61"),
                ("display.label", "Hover does nothing on UI elements"),
                ("status.attention", "true"),
            ],
        );

        let issue = entity_ref("issue", "github/flotilla-org/andamento#61");
        assert!(
            state
                .catalog_entities()
                .iter()
                .any(|entity| entity.entity == issue),
            "the issue must be in the catalog for this test to mean anything"
        );
        assert_eq!(
            state.activation_for_entity(&issue),
            None,
            "an inline-presence entity is neither focusable nor materializable"
        );
    }

    #[test]
    fn materialization_focuses_an_existing_tab_with_the_same_action_target() {
        let mut state = directory_entity_state();
        let shared_target = "flotilla:attach:dev/focus@lab";
        apply_entity(
            &mut state,
            "convoy",
            "dev/focus@lab",
            1,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.convoy", "dev/focus@lab"),
                (KEY_ACTION_TARGET, shared_target),
                (KEY_MATERIALIZE_RECIPE, "flotilla attach dev/focus"),
            ],
        );
        apply_entity(
            &mut state,
            "vessel",
            "dev/focus/worker@lab",
            2,
            &[
                ("flotilla.project", "dev"),
                ("flotilla.vessel", "dev/focus/worker@lab"),
                (KEY_ACTION_TARGET, shared_target),
            ],
        );
        state.update_tabs(vec![ControllerTab {
            tab_id: 7,
            position: 3,
            name: "worker".to_owned(),
            active: true,
        }]);
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(7),
            source_id: "flotilla-actuator".to_owned(),
            set: BTreeMap::from([
                (
                    KEY_ENTITY_KIND.to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("vessel".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    KEY_ENTITY_ID.to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("dev/focus/worker@lab".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        });
        let request = andamento_shared::MaterializeLatentRequest {
            action_target: shared_target.to_owned(),
            path: GroupPath::default(),
            name: "focus".to_owned(),
            recipe: "unused".to_owned(),
            checkout_path: None,
        };

        assert_eq!(state.materialized_tab_position(&request), Some(3));
        assert_eq!(
            state.activation_for_entity(&entity_ref("convoy", "dev/focus@lab")),
            Some(EntityActivation::FocusTab { position: 3 })
        );
        assert!(!state.begin_latent_materialization(&request));
    }

    #[test]
    fn focused_pane_cwd_beats_more_common_cwd_for_tab_grouping() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
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
            RailRow::GroupHeader { full_label, .. } if full_label == "focused"
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
    fn cwd_refresh_requests_each_pane_at_most_once() {
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
        state.update_panes_from_manifest(manifest.clone());

        assert_eq!(state.terminal_panes_for_cwd_refresh(), vec![10]);
        state.mark_pane_cwd_requested(PaneTarget::Terminal(10));

        // A failed lookup must not be retried on the next manifest update.
        state.update_panes_from_manifest(manifest.clone());
        assert_eq!(state.terminal_panes_for_cwd_refresh(), Vec::<u32>::new());

        // CwdChanged still lands and keeps the pane out of the refresh set.
        assert!(state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into()));
        state.update_panes_from_manifest(manifest);
        assert_eq!(state.terminal_panes_for_cwd_refresh(), Vec::<u32>::new());
    }

    #[test]
    fn count_breaks_ties_within_same_precedence() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
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
            RailRow::GroupHeader { full_label, .. } if full_label == "b"
        ));
    }

    #[test]
    fn directory_fallback_rule_compacts_tabs_and_leaves_missing_cwd_ungrouped() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
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
    fn tabs_without_facts_for_any_grouping_rule_remain_flat() {
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
    fn directory_fallback_rule_adds_cwd_grouping_metadata() {
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

        assert_eq!(grouping.key, "zellij.directory:zellij.pane.cwd=/repo/a");
        assert_eq!(grouping.label, "a");
        assert_eq!(grouping.full_label, "a");
    }

    #[test]
    fn directory_fallback_rule_disambiguates_duplicate_cwd_basenames() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![
            ControllerTab {
                tab_id: 1,
                position: 0,
                name: "one".into(),
                active: true,
            },
            ControllerTab {
                tab_id: 2,
                position: 1,
                name: "two".into(),
                active: false,
            },
        ]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_test_pane(PaneTarget::Terminal(20), 2, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/workspace/a/app".into());
        state.set_pane_cwd(PaneTarget::Terminal(20), "/workspace/b/app".into());

        let model = state.view_model();

        assert_eq!(
            model.tabs[0]
                .grouping
                .as_ref()
                .map(|grouping| grouping.label.as_str()),
            Some("a/app")
        );
        assert_eq!(
            model.tabs[1]
                .grouping
                .as_ref()
                .map(|grouping| grouping.label.as_str()),
            Some("b/app")
        );
    }

    #[test]
    fn directory_fallback_ignores_matching_cwd_basename_in_a_git_group() {
        let mut state = ControllerState::default();
        state.update_tabs(vec![
            ControllerTab {
                tab_id: 1,
                position: 0,
                name: "repo".into(),
                active: true,
            },
            ControllerTab {
                tab_id: 2,
                position: 1,
                name: "shell".into(),
                active: false,
            },
        ]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_test_pane(PaneTarget::Terminal(20), 2, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/work/foo".into());
        state.set_pane_cwd(PaneTarget::Terminal(20), "/tmp/foo".into());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(1),
            source_id: "git-watcher".to_owned(),
            set: BTreeMap::from([
                (
                    "vcs.repo".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("example/foo".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    "repo.name".to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("foo".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
            ]),
            unset: vec![],
        });

        let model = state.view_model();

        assert!(model.tabs[0]
            .grouping
            .as_ref()
            .is_some_and(|grouping| grouping_uses_rule(grouping, "andamento.git")));
        assert_eq!(
            model.tabs[1]
                .grouping
                .as_ref()
                .map(|grouping| grouping.label.as_str()),
            Some("foo")
        );
    }

    #[test]
    fn directory_fallback_rule_uses_cwd_group_path_identity() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
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
            label: Some("a".to_owned()),
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
        state.set_rail_config(RailConfig::default());
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::from_config(
                andamento_shared::grouping_config::ExternalGroupingConfig {
                    version: 1,
                    rules: vec![
                        andamento_shared::grouping_config::GroupingRule {
                            name: "proj-repo-branch".to_owned(),
                            priority: 100,
                            filter: None,
                            presence: vec![],
                            levels: vec![
                                andamento_shared::grouping_config::GroupingLevel {
                                    key: "andamento.project".to_owned(),
                                    optional: true,
                                    label_key: None,
                                    collapse_single_member: false,
                                    show_empty: false,
                                    template: None,
                                },
                                andamento_shared::grouping_config::GroupingLevel {
                                    key: "git.repo".to_owned(),
                                    optional: false,
                                    label_key: Some("repo.name".to_owned()),
                                    collapse_single_member: false,
                                    show_empty: false,
                                    template: None,
                                },
                                andamento_shared::grouping_config::GroupingLevel {
                                    key: "git.branch".to_owned(),
                                    optional: false,
                                    label_key: None,
                                    collapse_single_member: false,
                                    show_empty: false,
                                    template: None,
                                },
                            ],
                        },
                        andamento_shared::grouping_config::GroupingRule {
                            name: "directory".to_owned(),
                            priority: 10,
                            filter: None,
                            presence: vec![],
                            levels: vec![andamento_shared::grouping_config::GroupingLevel {
                                key: KEY_PANE_CWD.to_owned(),
                                optional: false,
                                label_key: None,
                                collapse_single_member: false,
                                show_empty: false,
                                template: None,
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
    fn configured_grouping_rules_include_the_bundled_directory_fallback() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::with_bundled_defaults(
                andamento_shared::grouping_config::ExternalGroupingConfig {
                    version: 1,
                    rules: vec![andamento_shared::grouping_config::GroupingRule {
                        name: "repo".to_owned(),
                        priority: 100,
                        filter: None,
                        presence: vec![],
                        levels: vec![andamento_shared::grouping_config::GroupingLevel {
                            key: "git.repo".to_owned(),
                            optional: false,
                            label_key: None,
                            collapse_single_member: false,
                            show_empty: false,
                            template: None,
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

        assert_eq!(
            grouping.key,
            "zellij.directory:zellij.pane.cwd=/Users/robert/dev/zellij"
        );
        assert_eq!(grouping.label, "zellij");
        assert_eq!(grouping.full_label, "zellij");
    }

    #[test]
    fn tab_with_required_partial_path_falls_through_to_a_later_rule() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
        let grouping = andamento_shared::grouping_config::parse_grouping_config_kdl(
            r#"
            grouping "repo-branch" priority=100 {
              level key="git.repo"
              level key="git.branch"
            }
            grouping "directory" priority=50 {
              level key="zellij.pane.cwd"
            }
            "#,
        )
        .expect("grouping config");
        state.set_grouping_catalog(Some(GroupingConfigCatalog::from_config(grouping)));
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "repo".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/Users/robert/dev/zellij".into());
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: andamento_shared::MetadataTarget::Tab(1),
            source_id: "git-watcher".to_owned(),
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
        });

        let model = state.view_model();
        let grouping = model.tabs[0].grouping.as_ref().expect("tab grouping");

        assert_eq!(
            grouping.key,
            "directory:zellij.pane.cwd=/Users/robert/dev/zellij"
        );
        assert_eq!(
            grouping.path,
            GroupPath(vec![GroupSegment {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
                label: None,
            }])
        );
    }

    #[test]
    fn entity_identity_overrides_cwd_grouping_without_a_stamped_path() {
        let mut state = directory_entity_state();
        state.update_tabs(vec![ControllerTab {
            tab_id: 1,
            position: 0,
            name: "overview".into(),
            active: true,
        }]);
        state.set_test_pane(PaneTarget::Terminal(10), 1, true, false, 0);
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/zellij".into());
        apply_entity(
            &mut state,
            "repo",
            "zellij-org/zellij",
            1,
            &[
                ("vcs.repo", "zellij-org/zellij"),
                ("vcs.repo.name", "zellij"),
            ],
        );
        state.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(1),
            source_id: "test".to_owned(),
            set: BTreeMap::from([
                (
                    KEY_ENTITY_KIND.to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("repo".to_owned()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                ),
                (
                    KEY_ENTITY_ID.to_owned(),
                    andamento_shared::MetadataValueUpdate {
                        value: MetadataValue::Text("zellij-org/zellij".to_owned()),
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

        assert_eq!(grouping.label, "zellij");
        assert_eq!(grouping.path.0.len(), 1);
        assert_eq!(grouping.path.0[0].key, "vcs.repo");
    }

    #[test]
    fn arbitrary_path_like_metadata_does_not_override_grouping() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
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
                "producer.path".to_owned(),
                andamento_shared::MetadataValueUpdate {
                    value: MetadataValue::GroupPath(vec![]),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });

        let model = state.view_model();
        let grouping = model.tabs[0].grouping.as_ref().expect("tab grouping");

        assert_eq!(
            grouping.key,
            "zellij.directory:zellij.pane.cwd=/repo/zellij"
        );
        assert_eq!(
            grouping.path,
            GroupPath(vec![GroupSegment {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text("/repo/zellij".to_owned()),
                label: Some("zellij".to_owned()),
            }])
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
            .find(|metadata| metadata.target == andamento_shared::ResolvedMetadataTarget::Tab(1))
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
            .find(|metadata| metadata.target == andamento_shared::ResolvedMetadataTarget::Tab(1))
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
    fn view_model_keeps_group_header_render_payload_in_sync_with_effective_template() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"
                    template "group/full" slot="group-header" node-kind="group" {
                      field "repo" key="git.repo" priority=100
                      field "branch" key="git.branch" priority=60
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
                RailRow::Tab { .. } | RailRow::Latent { .. } | RailRow::Entity { .. } => None,
            })
            .expect("group header template");

        assert_eq!(group_slot.template_name, "group/full");
        assert_eq!(
            group_slot
                .render_ready
                .as_ref()
                .expect("render-ready template")
                .fields
                .iter()
                .map(|field| (field.name.as_str(), field.priority))
                .collect::<Vec<_>>(),
            vec![("repo", Some(100)), ("branch", Some(60))]
        );
        assert!(group_slot
            .effective_kdl
            .contains("// origin: user (<memory>)"));
        assert!(group_slot.effective_kdl.contains("template \"group/full\""));

        let first_effective_kdl = group_slot.effective_kdl.clone();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"
                    template "group/full" slot="group-header" node-kind="group" {
                      field "replacement" class="required" source="literal" value="updated"
                    }
                    "#,
                )
                .expect("updated template config"),
            ),
        ));

        let updated_model = state.view_model();
        let updated_slot = updated_model
            .rows
            .iter()
            .find_map(|row| match row {
                RailRow::GroupHeader { templates, .. } => templates.group_header.as_ref(),
                RailRow::Tab { .. } | RailRow::Latent { .. } | RailRow::Entity { .. } => None,
            })
            .expect("updated group header template");

        assert_ne!(updated_slot.effective_kdl, first_effective_kdl);
        assert!(updated_slot.effective_kdl.contains("value=\"updated\""));
        assert_eq!(
            updated_slot
                .render_ready
                .as_ref()
                .expect("updated render-ready template")
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["replacement"]
        );
    }

    #[test]
    fn grouped_rows_anchor_tabs_to_their_exact_group_path() {
        let mut state = ControllerState::default();
        state.set_rail_config(RailConfig::default());
        state.set_grouping_catalog(Some(
            andamento_shared::grouping_config::GroupingConfigCatalog::from_config(
                andamento_shared::grouping_config::ExternalGroupingConfig {
                    version: 1,
                    rules: vec![andamento_shared::grouping_config::GroupingRule {
                        name: "repo-branch".to_owned(),
                        priority: 100,
                        filter: None,
                        presence: vec![],
                        levels: vec![
                            andamento_shared::grouping_config::GroupingLevel {
                                key: "git.repo".to_owned(),
                                optional: false,
                                label_key: None,
                                collapse_single_member: false,
                                show_empty: false,
                                template: None,
                            },
                            andamento_shared::grouping_config::GroupingLevel {
                                key: "git.branch".to_owned(),
                                optional: true,
                                label_key: None,
                                collapse_single_member: false,
                                show_empty: false,
                                template: None,
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
        state.set_rail_config(RailConfig::default());
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
            segment_between_color: None,
        });
        source.toggle_pin(7);
        source.set_status(status(
            PaneTarget::Terminal(10),
            Priority::Waiting,
            "waiting",
            10,
        ));
        source.set_node_variable(
            NodeKey::Root,
            "child-layout".to_owned(),
            Some("strip".to_owned()),
        );
        source.apply_rail_ui_action(RailUiAction::ScrollBy { delta: 8 });

        let snapshot = source.bootstrap_snapshot();
        let mut target = ControllerState::default();
        target.apply_bootstrap_snapshot(snapshot);

        let target_snapshot = target.bootstrap_snapshot();
        assert_eq!(target_snapshot.sort_mode, SortMode::PinnedFirst);
        assert_eq!(target_snapshot.config.structure, RailStructure::BoxPerTab);
        assert_eq!(target_snapshot.pinned_tabs, vec![7]);
        assert_eq!(target_snapshot.pane_statuses.len(), 1);
        assert_eq!(target_snapshot.pane_statuses[0].title, "waiting");
        assert_eq!(
            target_snapshot
                .node_variable_overrides
                .iter()
                .find(|overrides| overrides.node == NodeKey::Root)
                .and_then(|overrides| overrides.values.get("child-layout"))
                .map(String::as_str),
            Some("strip")
        );
        assert_eq!(
            target_snapshot.rail_ui_state,
            RailUiState {
                revision: RailUiRevision {
                    sequence: 1,
                    writer_client_id: 0,
                },
                collapsed_groups: vec![],
                scroll_offset: 8,
                variables: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn bootstrap_snapshot_keeps_only_persistent_display_variables() {
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(
            andamento_shared::template_config::TemplateConfigCatalog::from_config(
                andamento_shared::template_config::parse_template_config_kdl(
                    r#"
                    version 1
                    display-variable "persistent" type="bool" default=true label="Persistent" icon="P"
                    display-variable "ephemeral" type="bool" default=true label="Ephemeral" icon="E" persist=false
                    "#,
                )
                .unwrap(),
            ),
        ));
        state.apply_rail_ui_action(RailUiAction::ToggleVariable {
            name: "persistent".to_owned(),
        });
        state.apply_rail_ui_action(RailUiAction::ToggleVariable {
            name: "ephemeral".to_owned(),
        });

        assert_eq!(
            state.bootstrap_snapshot().rail_ui_state.variables,
            BTreeMap::from([("persistent".to_owned(), DisplayVariableValue::Bool(false))])
        );
    }

    #[test]
    fn stale_bootstrap_snapshot_cannot_overwrite_newer_rail_ui_state() {
        let mut state = ControllerState::default();
        state.apply_rail_ui_action(RailUiAction::ScrollBy { delta: 8 });
        state.apply_rail_ui_action(RailUiAction::ScrollBy { delta: 2 });

        assert!(!state.apply_rail_ui_state(RailUiState {
            revision: RailUiRevision {
                sequence: 1,
                writer_client_id: 9,
            },
            collapsed_groups: vec![],
            scroll_offset: 99,
            variables: BTreeMap::new(),
        }));
        assert_eq!(
            state.rail_ui_state(),
            RailUiState {
                revision: RailUiRevision {
                    sequence: 2,
                    writer_client_id: 0,
                },
                collapsed_groups: vec![],
                scroll_offset: 10,
                variables: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn bootstrap_snapshot_carries_metadata_patches() {
        const KEY: &str = "tab.subject";
        let mut source = ControllerState::default();
        source.apply_metadata_patch(andamento_shared::MetadataPatch {
            target: EntityId::Tab(7),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                KEY.to_owned(),
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
            .find(|metadata| metadata.target == andamento_shared::ResolvedMetadataTarget::Tab(7))
            .expect("tab metadata");

        assert_eq!(
            metadata
                .values
                .get(KEY)
                .map(|entry| (&entry.value, entry.precedence, entry.ordinal)),
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
        state.register_rail(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 7,
                client_id: 1,
            },
            placement: PluginPlacement::Tab {
                tab_id: 1,
                pane_kind: PluginPaneKind::Tiled,
            },
        });
        state.register_rail(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 8,
                client_id: 1,
            },
            placement: PluginPlacement::Tab {
                tab_id: 1,
                pane_kind: PluginPaneKind::Tiled,
            },
        });

        state.retain_rails(&[8].into_iter().collect());

        assert_eq!(state.known_rail_count(), 1);
        assert_eq!(state.known_config_editor_count(), 0);
        assert_eq!(state.rail_plugin_ids(), vec![8]);
    }

    #[test]
    fn renderer_cleanup_keeps_session_rail_ui_state() {
        let path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/repo".to_owned()),
            label: Some("repo".to_owned()),
        }]);
        let mut state = ControllerState::default();
        state.apply_rail_ui_action(RailUiAction::ToggleGroup { path: path.clone() });
        state.apply_rail_ui_action(RailUiAction::ScrollBy { delta: 7 });

        state.retain_rails(&HashSet::new());

        assert_eq!(
            state.rail_ui_state(),
            RailUiState {
                revision: RailUiRevision {
                    sequence: 2,
                    writer_client_id: 0,
                },
                collapsed_groups: vec![path],
                scroll_offset: 7,
                variables: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn config_editor_target_prefers_same_client_and_tab() {
        let mut state = ControllerState::default();
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 30,
                client_id: 1,
            },
            placement: PluginPlacement::Tab {
                tab_id: 1,
                pane_kind: PluginPaneKind::Floating,
            },
        });
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 31,
                client_id: 2,
            },
            placement: PluginPlacement::Tab {
                tab_id: 2,
                pane_kind: PluginPaneKind::Floating,
            },
        });
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 32,
                client_id: 2,
            },
            placement: PluginPlacement::Tab {
                tab_id: 1,
                pane_kind: PluginPaneKind::Floating,
            },
        });

        assert_eq!(
            state.config_editor_target_for_client_tab(2, 1),
            Some(RendererHello {
                plugin_id: 32,
                client_id: 2
            })
        );
        assert_eq!(
            state.config_editor_target_for_client_tab(2, 2),
            Some(RendererHello {
                plugin_id: 31,
                client_id: 2
            })
        );
        assert_eq!(state.config_editor_target_for_client_tab(2, 3), None);
    }

    #[test]
    fn config_editor_target_does_not_reuse_other_tab_floating_editor() {
        let mut state = ControllerState::default();
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 30,
                client_id: 1,
            },
            placement: PluginPlacement::Tab {
                tab_id: 1,
                pane_kind: PluginPaneKind::Floating,
            },
        });
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 31,
                client_id: 2,
            },
            placement: PluginPlacement::Tab {
                tab_id: 9,
                pane_kind: PluginPaneKind::Floating,
            },
        });

        assert_eq!(state.config_editor_target_for_client_tab(2, 3), None);
    }

    #[test]
    fn config_editor_target_does_not_reuse_other_tab_tiled_editor() {
        let mut state = ControllerState::default();
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 30,
                client_id: 2,
            },
            placement: PluginPlacement::Tab {
                tab_id: 9,
                pane_kind: PluginPaneKind::Tiled,
            },
        });

        assert_eq!(state.config_editor_target_for_client_tab(2, 3), None);
    }

    #[test]
    fn config_editor_target_does_not_reuse_unknown_editor() {
        let mut state = ControllerState::default();
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 30,
                client_id: 1,
            },
            placement: PluginPlacement::Unknown,
        });
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 31,
                client_id: 2,
            },
            placement: PluginPlacement::Unknown,
        });

        assert_eq!(state.config_editor_target_for_client_tab(2, 3), None);
    }

    #[test]
    fn view_model_for_client_uses_that_clients_inspected_node() {
        let mut state = ControllerState::default();
        state.set_inspected_node(4, NodeKey::Tab(7));
        state.set_inspected_node(5, NodeKey::Root);

        assert_eq!(
            state.view_model_for_client(4).inspected_node,
            Some(NodeKey::Tab(7))
        );
        assert_eq!(
            state.view_model_for_client(5).inspected_node,
            Some(NodeKey::Root)
        );
    }

    #[test]
    fn registering_rail_places_it_under_client_and_tab() {
        let mut state = ControllerState::default();
        let hello = PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 10,
                client_id: 4,
            },
            placement: PluginPlacement::Tab {
                tab_id: 7,
                pane_kind: PluginPaneKind::Tiled,
            },
        };

        assert!(state.register_rail(hello));
        assert_eq!(
            state
                .client(4)
                .and_then(|client| client.tabs.get(&7))
                .map(|tab| tab.rails.contains_key(&10)),
            Some(true)
        );
    }

    #[test]
    fn registering_config_places_it_under_client_and_tab() {
        let mut state = ControllerState::default();
        let hello = PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 11,
                client_id: 4,
            },
            placement: PluginPlacement::Tab {
                tab_id: 7,
                pane_kind: PluginPaneKind::Floating,
            },
        };

        assert!(state.register_config_editor(hello));
        assert_eq!(
            state
                .client(4)
                .and_then(|client| client.tabs.get(&7))
                .map(|tab| tab.config_editors.contains_key(&11)),
            Some(true)
        );
    }

    #[test]
    fn rail_plugin_targets_include_rails_and_config_editors_across_clients() {
        let mut state = ControllerState::default();
        state.register_rail(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 1,
                client_id: 4,
            },
            placement: PluginPlacement::Tab {
                tab_id: 7,
                pane_kind: PluginPaneKind::Tiled,
            },
        });
        state.register_config_editor(PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 2,
                client_id: 4,
            },
            placement: PluginPlacement::Tab {
                tab_id: 7,
                pane_kind: PluginPaneKind::Floating,
            },
        });

        let targets = state.rail_plugin_targets();

        assert_eq!(
            targets,
            vec![
                RendererHello {
                    plugin_id: 1,
                    client_id: 4
                },
                RendererHello {
                    plugin_id: 2,
                    client_id: 4
                },
            ]
        );
    }
}
