#[derive(Debug, Clone)]
pub(crate) struct EvaluatedPlacement {
    pub entity: EntityRef,
    pub placement: Option<PlacementKey>,
    pub label: String,
    pub form: String,
    pub tier: Option<crate::template_config::AbbreviationTier>,
    pub metadata: BTreeMap<String, MetadataValue>,
    pub templates: ResolvedTemplateSlots,
    pub children: Vec<EvaluatedPlacement>,
}
#[derive(Debug)]
pub(crate) struct EvaluatedSection {
    pub definition: crate::template_config::SurfaceRegionDefinition,
    pub root: Option<ResolvedTemplateSlot>,
    pub entities: Vec<EvaluatedPlacement>,
}
#[derive(Debug, Default)]
pub(crate) struct PlacementAnnotation {
    pub layout: Option<String>,
    pub related_detail: Vec<crate::template_config::TemplateConfigRenderedField>,
}
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

use crate::host::PaneObservation;
use crate::metadata::{select_primary_entry, CandidateEntry, EntityId, MetadataStore};
use crate::{
    ControllerBootstrapSnapshot, ControllerViewModel, DisplayVariableValue, EffectiveNodeVariables,
    EffectiveVariableValue, EntityRef, LatentMaterializationState, LatentTab, MetadataControls,
    MetadataEntry, MetadataIdentity, MetadataSourceEntry, MetadataTriState, MetadataValue, NodeKey,
    ObservedMetadataIdentity, PaneTarget, PlacementKey, PlacementSegment, PluginPlacement,
    PluginRegistrationHello, Priority, RailConfig, RailUiAction, RailUiRevision, RailUiState,
    ReachableMetadataIdentity, RendererHello, ResolvedMetadata, ResolvedTemplateSlot,
    ResolvedTemplateSlots, SetPaneStatus, SortMode, TabCard, TabStatusSummary,
    TemplateConfigDiagnostics, VariableSetterProvenance, DISPLAY_FORM_COMPACT,
    NODE_VARIABLE_CONFIG_OVERRIDE_SETTER,
};

const SOURCE_ZELLIJ: &str = "zellij";
const SOURCE_LATENT_MATERIALIZER: &str = "andamento-latent-materializer";
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
// Opener-owned identity must outrank observational discovery such as cwd-derived associations.
const LATENT_MATERIALIZER_PRECEDENCE: i64 = 1_000;

type TabSeedMetadata = HashMap<u64, BTreeMap<String, MetadataEntry>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerTab {
    pub tab_id: u64,
    pub position: usize,
    pub name: String,
    pub active: bool,
}

/// Catalog entities indexed by `(fact key, indexable text)`.
///
/// Built once per model. Every placement predicate is answered by a lookup
/// here; nothing scans the catalog.
#[derive(Debug, Default)]
struct PlacementIndex {
    by_fact: BTreeMap<(String, String), Vec<usize>>,
    visibility: BTreeMap<String, Vec<bool>>,
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
                let Some(text) = crate::template_config::placement_index_text(&entry.value) else {
                    continue;
                };
                insert(key.clone(), text);
            }
        }
        Self {
            by_fact,
            visibility: BTreeMap::new(),
        }
    }

    /// Each policy is evaluated once over the catalog, never once per nested
    /// loop. Query evaluation then checks a bit alongside its posting position.
    fn evaluate_visibility(
        &mut self,
        entities: &[CatalogEntity],
        catalog: &crate::template_config::TemplateConfigCatalog,
        variables: &BTreeMap<String, crate::DisplayVariableValue>,
    ) {
        for policy in catalog.visibility() {
            let enabled: Vec<bool> = policy
                .rules
                .iter()
                .map(|rule| {
                    let value = variables.get(&rule.visible_when).or_else(|| {
                        catalog
                            .display_variables()
                            .iter()
                            .find(|variable| variable.name == rule.visible_when)
                            .map(|variable| &variable.default)
                    });
                    matches!(value, Some(crate::DisplayVariableValue::Bool(true)))
                })
                .collect();
            let visible = entities
                .iter()
                .map(|entity| {
                    let rule = policy.rules.iter().position(|rule| {
                        rule.predicates.iter().all(|predicate| {
                            let identity = match predicate.key.as_str() {
                                "entity.kind" => Some(&entity.entity.kind),
                                "entity.id" => Some(&entity.entity.id),
                                _ => None,
                            };
                            let fact = entity.values.get(&predicate.key);
                            if let Some(exists) = predicate.exists {
                                return exists == (identity.is_some() || fact.is_some());
                            }
                            let value = identity.cloned().or_else(|| {
                                fact.and_then(|entry| {
                                    crate::template_config::placement_index_text(&entry.value)
                                })
                            });
                            value
                                .as_ref()
                                .is_some_and(|value| Some(value) == predicate.value.as_ref())
                        })
                    });
                    rule.is_none_or(|position| enabled[position])
                })
                .collect();
            self.visibility.insert(policy.name.clone(), visible);
        }
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
    declarations: Vec<crate::template_config::NodeVariableDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityActivation {
    FocusTab { position: usize },
    Materialize(crate::MaterializeLatentRequest),
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
    request: crate::MaterializeLatentRequest,
    tab_id: Option<u64>,
}

#[cfg(test)]
thread_local! { static CATALOG_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[derive(Debug, Clone)]
struct CatalogEntity {
    entity: EntityRef,
    values: BTreeMap<String, MetadataEntry>,
    ordinal: i64,
}

/// Resolved catalog and lookup tables shared by every pass of one model build.
/// Its lifetime is the evaluation, so metadata/configuration/time changes cannot
/// leave stale derived state behind between snapshots.
struct CatalogEvaluation {
    entities: Vec<CatalogEntity>,
    by_entity: BTreeMap<EntityRef, usize>,
}

impl CatalogEvaluation {
    fn new(entities: Vec<CatalogEntity>) -> Self {
        let by_entity = entities
            .iter()
            .enumerate()
            .map(|(i, e)| (e.entity.clone(), i))
            .collect();
        Self {
            entities,
            by_entity,
        }
    }

    fn entity(&self, target: &EntityRef) -> Option<&CatalogEntity> {
        self.by_entity
            .get(target)
            .map(|index| &self.entities[*index])
    }
}

fn placement_entity_order(
    left: &CatalogEntity,
    right: &CatalogEntity,
    order: &[crate::template_config::PlacementOrder],
) -> Ordering {
    for key in order {
        let left_value = placement_sort_fact(left, &key.key);
        let right_value = placement_sort_fact(right, &key.key);
        let compared = match (left_value, right_value) {
            (Some(left), Some(right)) => match key.direction {
                crate::template_config::PlacementOrderDirection::Ascending => {
                    left.as_ref().cmp(right.as_ref())
                }
                crate::template_config::PlacementOrderDirection::Descending => {
                    right.as_ref().cmp(left.as_ref())
                }
            },
            (None, None) => Ordering::Equal,
            (None, Some(_)) => match key.absent {
                crate::template_config::PlacementOrderAbsent::First => Ordering::Less,
                crate::template_config::PlacementOrderAbsent::Last => Ordering::Greater,
            },
            (Some(_), None) => match key.absent {
                crate::template_config::PlacementOrderAbsent::First => Ordering::Greater,
                crate::template_config::PlacementOrderAbsent::Last => Ordering::Less,
            },
        };
        if compared != Ordering::Equal {
            return compared;
        }
    }
    left.entity.cmp(&right.entity)
}

fn placement_sort_fact<'a>(entity: &'a CatalogEntity, key: &str) -> Option<Cow<'a, MetadataValue>> {
    match key {
        KEY_ENTITY_KIND => Some(Cow::Owned(MetadataValue::Text(entity.entity.kind.clone()))),
        KEY_ENTITY_ID => Some(Cow::Owned(MetadataValue::Text(entity.entity.id.clone()))),
        _ => entity
            .values
            .get(key)
            .map(|entry| Cow::Borrowed(&entry.value)),
    }
}

#[derive(Debug, Default)]
struct ControllerRailUiState {
    revision: RailUiRevision,
    collapsed_placements: BTreeSet<PlacementKey>,
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

#[derive(Debug)]
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
    template_catalog: Option<crate::template_config::TemplateConfigCatalog>,
    template_config: TemplateConfigDiagnostics,
    receive_counter: u64,
    clock_ms: Option<u64>,
    pending_materialized_tab_names: HashMap<u64, String>,
    pending_latent_materializations: BTreeMap<String, PendingLatentMaterialization>,
    rail_ui: ControllerRailUiState,
    rail_ui_writer_client_id: u16,
    node_variable_overrides: BTreeMap<NodeKey, BTreeMap<String, String>>,
}
impl Default for ControllerState {
    fn default() -> Self {
        Self {
            tabs: Default::default(),
            pane_to_tab: Default::default(),
            panes: Default::default(),
            metadata: Default::default(),
            pane_statuses: Default::default(),
            pinned_tabs: Default::default(),
            clients: Default::default(),
            sort_mode: Default::default(),
            rail_config: Default::default(),
            template_catalog: Some(Default::default()),
            template_config: Default::default(),
            receive_counter: Default::default(),
            clock_ms: Default::default(),
            pending_materialized_tab_names: Default::default(),
            pending_latent_materializations: Default::default(),
            rail_ui: Default::default(),
            rail_ui_writer_client_id: Default::default(),
            node_variable_overrides: Default::default(),
        }
    }
}

impl ControllerState {
    /// Opt into host-supplied monotonic time. Legacy adapters retain receipt-clock behavior.
    pub fn advance_time(&mut self, now_ms: u64) -> bool {
        let before = self.now();
        let now = self.clock_ms.unwrap_or(0).max(now_ms);
        self.clock_ms = Some(now);
        self.metadata.expires_between(before, now)
    }
    fn now(&self) -> u64 {
        self.clock_ms.unwrap_or(self.receive_counter)
    }

    pub fn entity_is_opening(&self, entity: &EntityRef) -> bool {
        self.latent_tabs().iter().any(|tab| {
            &tab.entity == entity && tab.materialization == LatentMaterializationState::Opening
        })
    }

    /// Current host observations, without building a presentation.
    pub fn workspaces(&self) -> &[ControllerTab] {
        &self.tabs
    }

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
    pub fn observe_workspaces(&mut self, tabs: Vec<ControllerTab>) -> bool {
        let mut pending_materialized_tab_names =
            std::mem::take(&mut self.pending_materialized_tab_names);
        let mut next_tabs = Vec::with_capacity(tabs.len());
        for tab in tabs {
            let tab_id = tab.tab_id;
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
                tab_id: tab.tab_id,
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

    #[cfg(any(test, feature = "test-support"))]
    pub fn update_tabs(&mut self, tabs: Vec<ControllerTab>) {
        self.tabs = tabs;
        self.tabs.sort_by_key(|tab| tab.position);
    }

    #[allow(dead_code)]
    pub fn observe_panes(&mut self, observations: Vec<PaneObservation>) -> bool {
        let mut live_panes = HashSet::new();
        let previous_pane_to_tab = self.pane_to_tab.clone();
        self.pane_to_tab.clear();
        let previous_panes = self.panes.clone();
        self.panes.clear();
        for pane in observations {
            if !self.tabs.iter().any(|tab| tab.tab_id == pane.workspace_id) {
                continue;
            }
            let pane_target = pane.pane_id;
            live_panes.insert(pane_target);
            self.pane_to_tab.insert(pane_target, pane.workspace_id);
            self.panes.insert(
                pane_target,
                ControllerPane {
                    pane_id: pane_target,
                    tab_id: pane.workspace_id,
                    is_selectable: pane.is_selectable,
                    is_focused: pane.is_focused,
                    ordinal: pane.ordinal,
                    cwd: previous_panes.get(&pane_target).and_then(|p| p.cwd.clone()),
                    cwd_requested: previous_panes
                        .get(&pane_target)
                        .is_some_and(|p| p.cwd_requested),
                },
            );
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

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_pane_tab(&mut self, pane_id: PaneTarget, tab_id: u64) {
        self.pane_to_tab.insert(pane_id, tab_id);
    }

    #[cfg(any(test, feature = "test-support"))]
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
            RailUiAction::TogglePlacement { key } => {
                if !self.rail_ui.collapsed_placements.insert(key.clone()) {
                    self.rail_ui.collapsed_placements.remove(&key);
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
                        crate::template_config::TemplateVariableType::Bool,
                        DisplayVariableValue::Bool(value),
                    ) => DisplayVariableValue::Bool(!value),
                    (
                        crate::template_config::TemplateVariableType::Enum { values },
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
            collapsed_placements: self.rail_ui.collapsed_placements.iter().cloned().collect(),
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
            collapsed_placements: state.collapsed_placements.into_iter().collect(),
            scroll_offset: state.scroll_offset,
            variables: state.variables,
        };
        true
    }

    pub fn set_template_catalog(
        &mut self,
        catalog: Option<crate::template_config::TemplateConfigCatalog>,
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
    }

    pub fn set_template_config_diagnostics(&mut self, diagnostics: TemplateConfigDiagnostics) {
        self.template_config = diagnostics;
    }

    pub fn template_config_diagnostics(&self) -> &TemplateConfigDiagnostics {
        &self.template_config
    }

    pub fn apply_metadata_patch(&mut self, patch: crate::MetadataPatch) -> bool {
        let next_receive_counter = self.receive_counter.saturating_add(1);
        let outcome = self
            .metadata
            .apply_patch(patch, self.clock_ms.unwrap_or(next_receive_counter));
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
                .map(|(node, values)| crate::NodeVariableOverrides {
                    node: node.clone(),
                    values: values.clone(),
                })
                .collect(),
            metadata_patches: self.metadata.snapshot_patches(self.now()),
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
        let catalog = CatalogEvaluation::new(self.catalog_entities());
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

        let latent_tabs = self.latent_tabs_in(&catalog);
        let resolved_metadata = self.resolved_metadata_for_tabs(&tabs, &catalog);
        let observed_identities = observed_metadata_identities(&resolved_metadata);
        let region_catalog_entities = &catalog.entities;
        let (effective_variables, variable_warnings) =
            self.resolve_effective_variables(&resolved_metadata);
        let mut template_config = self.template_config.clone();
        template_config.effective_variables = effective_variables;
        for warning in variable_warnings {
            if !template_config.warnings.contains(&warning) {
                template_config.warnings.push(warning);
            }
        }
        let mut placement_index = PlacementIndex::build(region_catalog_entities);
        if let Some(config) = self.template_catalog.as_ref() {
            placement_index.evaluate_visibility(
                region_catalog_entities,
                config,
                &self.rail_ui.variables,
            );
        }
        let mut placement_layouts = BTreeMap::new();
        let surface_regions: Vec<EvaluatedSection> = self
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
                            self.evaluate_placement(
                                placement,
                                region_catalog_entities,
                                &placement_index,
                                &definition.form,
                                &mut placement_layouts,
                            )
                        } else {
                            vec![]
                        };
                        EvaluatedSection {
                            definition,
                            root: self.resolve_template_slot(
                                crate::template_config::TemplateConfigSlot::Compact,
                                crate::template_config::TemplateConfigNodeKind::Entity,
                                &metadata,
                            ),
                            entities,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(catalog) = self.template_catalog.as_ref() {
            let mut inherited = template_config
                .effective_variables
                .iter()
                .map(|entry| (entry.node.clone(), entry.values.clone()))
                .collect::<BTreeMap<_, _>>();
            let mut placement_variables = vec![];
            let mut warnings = BTreeSet::new();
            for entity in surface_regions.iter().flat_map(|region| &region.entities) {
                self.resolve_placement_variables(
                    catalog,
                    entity,
                    &NodeKey::Root,
                    &mut inherited,
                    &mut placement_variables,
                    &mut warnings,
                );
            }
            template_config
                .effective_variables
                .extend(placement_variables);
            for warning in warnings {
                if !template_config.warnings.contains(&warning) {
                    template_config.warnings.push(warning);
                }
            }
        }

        fn collect_matched(nodes: &[EvaluatedPlacement], matched: &mut BTreeSet<EntityRef>) {
            for node in nodes {
                matched.insert(node.entity.clone());
                collect_matched(&node.children, matched);
            }
        }
        let mut matched = BTreeSet::new();
        for section in &surface_regions {
            collect_matched(&section.entities, &mut matched);
        }
        let unmatched_entities = catalog
            .entities
            .iter()
            .filter(|entity| !matched.contains(&entity.entity))
            .map(|entity| entity.entity.clone())
            .collect();
        let mut model = ControllerViewModel {
            unmatched_entities,
            sort_mode: self.sort_mode,
            config: self.rail_config,
            template_config,
            resolved_metadata,
            observed_identities,
            tabs,
            metadata_controls: self
                .clients
                .get(&0)
                .map(|client| client.metadata_controls.clone())
                .unwrap_or_default(),
            inspected_node: None,
            collapsed_placements: self.rail_ui.collapsed_placements.iter().cloned().collect(),
            display_variables: self
                .template_catalog
                .as_ref()
                .map(|catalog| catalog.display_variables().to_vec())
                .unwrap_or_default(),
            display_variable_values: self.rail_ui.variables.clone(),
            presentation: None,
        };
        // Reuse this build's resolved facts. Re-running activation resolution for
        // every displayed entity would rebuild the catalog once per node.
        use crate::presentation::PresentationState;
        let tab_metadata = model
            .resolved_metadata
            .iter()
            .filter_map(|metadata| {
                if let EntityId::Tab(id) = metadata.target {
                    Some((id, &metadata.values))
                } else {
                    None
                }
            })
            .collect::<BTreeMap<_, _>>();
        let mut live = BTreeMap::new();
        for workspace in &self.tabs {
            if let Some(values) = tab_metadata.get(&workspace.tab_id) {
                let target = metadata_entry_text(values, KEY_ACTION_TARGET)
                    .map(str::to_owned)
                    .or_else(|| entity_ref_from_entries(values).map(|e| e.action_target()));
                if let Some(target) = target {
                    live.entry(target).or_insert(PresentationState::Live {
                        workspace_id: workspace.tab_id,
                        selected: workspace.active,
                    });
                }
            }
        }
        let latents = latent_tabs
            .iter()
            .map(|latent| (&latent.entity, latent))
            .collect::<BTreeMap<_, _>>();
        let states = region_catalog_entities
            .iter()
            .map(|entity| {
                let action_target = metadata_entry_text(&entity.values, KEY_ACTION_TARGET)
                    .map(str::to_owned)
                    .unwrap_or_else(|| entity.entity.action_target());
                let state = live.get(&action_target).cloned().unwrap_or_else(|| {
                    latents
                        .get(&entity.entity)
                        .map(|latent| {
                            if latent.materialization == LatentMaterializationState::Opening {
                                PresentationState::Opening
                            } else {
                                PresentationState::Latent {
                                    openable: latent.materialize_request().is_some(),
                                }
                            }
                        })
                        .unwrap_or_default()
                });
                (entity.entity.clone(), state)
            })
            .collect();
        model.presentation = Some(crate::presentation::SurfaceSnapshot::resolve(
            &model,
            &surface_regions,
            &placement_layouts,
            &states,
        ));
        if let Some(surface) = &mut model.presentation {
            surface.cover_workspaces(&self.tabs);
        }
        model
    }

    pub fn view_model_for_client(&self, client_id: u16) -> ControllerViewModel {
        let mut model = self.view_model();
        if let Some(client) = self.clients.get(&client_id) {
            model.metadata_controls = client.metadata_controls.clone();
            model.inspected_node = client.inspected_node.clone();
        }
        model
    }

    fn display_entity(&self, entity: &CatalogEntity) -> EvaluatedPlacement {
        let metadata = entity_facts(&entity.entity, &entity.values);
        let templates = ResolvedTemplateSlots {
            compact: self.resolve_template_slot(
                crate::template_config::TemplateConfigSlot::Compact,
                crate::template_config::TemplateConfigNodeKind::Entity,
                &metadata,
            ),
            detail: self.resolve_template_slot(
                crate::template_config::TemplateConfigSlot::Detail,
                crate::template_config::TemplateConfigNodeKind::Entity,
                &metadata,
            ),
            ..Default::default()
        };
        EvaluatedPlacement {
            entity: entity.entity.clone(),
            placement: None,
            tier: None,
            label: metadata_entry_text(&entity.values, KEY_DISPLAY_LABEL)
                .map(str::to_owned)
                .unwrap_or_else(|| entity.entity.id.clone()),
            form: DISPLAY_FORM_COMPACT.into(),
            metadata,
            templates,
            children: vec![],
        }
    }

    /// Evaluate a placement's single loop into the entities it places.
    ///
    /// Predicates intersect: the smallest posting list is taken first and the
    /// rest filter it, so cost tracks the most selective fact rather than the
    /// catalog size.
    fn evaluate_placement(
        &self,
        placement: &crate::template_config::PlacementDefinition,
        entities: &[CatalogEntity],
        index: &PlacementIndex,
        form: &str,
        layouts: &mut BTreeMap<PlacementKey, PlacementAnnotation>,
    ) -> Vec<EvaluatedPlacement> {
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
                    &PlacementKey::default(),
                    0,
                    layouts,
                )
            })
            .collect()
    }

    /// Entities a loop selects under the given bindings, in its declared order.
    fn placement_matches<'a>(
        &self,
        loop_definition: &crate::template_config::PlacementLoop,
        entities: &'a [CatalogEntity],
        index: &PlacementIndex,
        bindings: &BTreeMap<String, &'a CatalogEntity>,
        ancestors: &[crate::EntityRef],
    ) -> Vec<&'a CatalogEntity> {
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
                            crate::template_config::placement_index_text(&entry.value)
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
        let mut matches = seed
            .iter()
            .filter(|position| {
                rest.iter()
                    .all(|matches| matches.binary_search(position).is_ok())
            })
            .filter(|position| {
                loop_definition.visibility.as_ref().is_none_or(|name| {
                    index
                        .visibility
                        .get(name)
                        .and_then(|visible| visible.get(**position))
                        .copied()
                        .unwrap_or(false)
                })
            })
            .filter_map(|position| entities.get(*position))
            .filter(|entity| !ancestors.contains(&entity.entity))
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| placement_entity_order(left, right, &loop_definition.order));
        matches
    }

    /// Fields contributed to a placed entity's detail by loops declared in its
    /// detail template. The template binds the entity under its own name, as an
    /// applied template does, and each loop renders its fields per match.
    fn related_detail_fields<'a>(
        &self,
        display: &EvaluatedPlacement,
        entity: &'a CatalogEntity,
        entities: &'a [CatalogEntity],
        index: &PlacementIndex,
    ) -> Vec<crate::template_config::TemplateConfigRenderedField> {
        let (Some(catalog), Some(detail)) = (
            self.template_catalog.as_ref(),
            display.templates.detail.as_ref(),
        ) else {
            return vec![];
        };
        let Some(template) = catalog.placement_template(&detail.template_name, &display.metadata)
        else {
            return vec![];
        };
        let binding = detail
            .template_name
            .split('/')
            .next()
            .unwrap_or(&detail.template_name)
            .to_owned();
        let bindings = BTreeMap::from([(binding, entity)]);
        let ancestors = [entity.entity.clone()];
        template
            .loops
            .iter()
            .flat_map(|related_loop| {
                self.placement_matches(related_loop, entities, index, &bindings, &ancestors)
                    .into_iter()
                    .flat_map(move |related| {
                        let metadata = entity_facts(&related.entity, &related.values);
                        let context = crate::template_config::TemplateConfigMatchContext {
                            slot: crate::template_config::TemplateConfigSlot::Detail,
                            node_kind: crate::template_config::TemplateConfigNodeKind::Entity,
                            metadata: &metadata,
                            collapsed: false,
                            collapsible: false,
                            active_tab_name: None,
                        };
                        related_loop
                            .fields
                            .iter()
                            .filter_map(|spec| spec.render(context))
                            .collect::<Vec<_>>()
                    })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_placement_loop<'a>(
        &self,
        loop_definition: &crate::template_config::PlacementLoop,
        entities: &'a [CatalogEntity],
        index: &PlacementIndex,
        form: &str,
        bindings: &BTreeMap<String, &'a CatalogEntity>,
        ancestors: &[crate::EntityRef],
        parent_placement: &PlacementKey,
        depth: usize,
        layouts: &mut BTreeMap<PlacementKey, PlacementAnnotation>,
    ) -> Vec<EvaluatedPlacement> {
        const MAX_PLACEMENT_DEPTH: usize = 64;
        if depth >= MAX_PLACEMENT_DEPTH {
            return vec![];
        }
        let matches = self.placement_matches(loop_definition, entities, index, bindings, ancestors);
        matches
            .into_iter()
            .map(|entity| {
                let mut display = self.display_entity(entity);
                display.form = form.to_owned();
                display.tier = loop_definition.tier;
                if !loop_definition.fields.is_empty() {
                    display.templates.compact = Some(ResolvedTemplateSlot {
                        template_name: format!("placement:{}", loop_definition.binding),
                        fields: vec![],
                        setters: vec![],
                        effective_kdl: String::new(),
                        resolve_error: None,
                        render_ready: Some(crate::template_config::TemplateConfigRenderReady {
                            fields: loop_definition.fields.clone(),
                            controls: vec![],
                            chrome: Default::default(),
                        }),
                    });
                    if form != DISPLAY_FORM_COMPACT {
                        display.templates.detail = display.templates.compact.clone();
                    }
                }
                let mut placement = parent_placement.clone();
                placement.0.push(PlacementSegment {
                    loop_name: loop_definition.binding.clone(),
                    entity: entity.entity.clone(),
                });
                let related_detail = self.related_detail_fields(&display, entity, entities, index);
                layouts.insert(
                    placement.clone(),
                    PlacementAnnotation {
                        layout: loop_definition.layout.clone(),
                        related_detail,
                    },
                );
                display.placement = Some(placement.clone());
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
                                if form != DISPLAY_FORM_COMPACT {
                                    display.templates.detail = Some(slot);
                                }
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
                        } else if !requested.is_empty() {
                            let slot = ResolvedTemplateSlot {
                                template_name: "<resolve-error>".to_owned(),
                                fields: vec![],
                                render_ready: None,
                                setters: vec![],
                                effective_kdl: String::new(),
                                resolve_error: Some(format!(
                                    "unknown applied template {template_name}"
                                )),
                            };
                            display.templates.compact = Some(slot.clone());
                            display.templates.detail = Some(slot);
                            child_loops = &[];
                            nested_bindings.clear();
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
                            &placement,
                            depth + 1,
                            layouts,
                        )
                    })
                    .collect();
                display
            })
            .collect()
    }

    fn resolve_effective_variables(
        &self,
        resolved_metadata: &[ResolvedMetadata],
    ) -> (Vec<EffectiveNodeVariables>, Vec<String>) {
        let Some(catalog) = self.template_catalog.as_ref() else {
            return (vec![], vec![]);
        };
        let mut warnings = BTreeSet::new();
        let root = NodeKey::Root;
        let resolved = self.resolve_variables_at_node(
            catalog,
            &root,
            None,
            &node_metadata(&root, resolved_metadata),
            None,
            &mut warnings,
        );
        (
            vec![EffectiveNodeVariables {
                node: root,
                values: resolved.values,
                declarations: resolved.declarations,
            }],
            warnings.into_iter().collect(),
        )
    }

    fn resolve_placement_variables(
        &self,
        catalog: &crate::template_config::TemplateConfigCatalog,
        entity: &EvaluatedPlacement,
        parent: &NodeKey,
        inherited_by_node: &mut BTreeMap<NodeKey, BTreeMap<String, EffectiveVariableValue>>,
        output: &mut Vec<EffectiveNodeVariables>,
        warnings: &mut BTreeSet<String>,
    ) {
        let Some(key) = entity.placement.clone() else {
            return;
        };
        let node = NodeKey::Placement(key);
        let template = if entity.form == DISPLAY_FORM_COMPACT {
            entity.templates.compact.as_ref()
        } else {
            entity.templates.detail.as_ref()
        };
        let resolved = self.resolve_variables_at_node(
            catalog,
            &node,
            inherited_by_node.get(parent),
            &entity.metadata,
            template,
            warnings,
        );
        inherited_by_node.insert(node.clone(), resolved.values.clone());
        output.push(EffectiveNodeVariables {
            node: node.clone(),
            values: resolved.values,
            declarations: resolved.declarations,
        });
        for child in &entity.children {
            self.resolve_placement_variables(
                catalog,
                child,
                &node,
                inherited_by_node,
                output,
                warnings,
            );
        }
    }

    fn resolve_variables_at_node(
        &self,
        catalog: &crate::template_config::TemplateConfigCatalog,
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
                        origin: crate::template_config::TemplateConfigOrigin {
                            layer: crate::template_config::TemplateConfigLayerKind::User,
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
        self.latent_tabs_in(&CatalogEvaluation::new(self.catalog_entities()))
    }

    fn latent_tabs_in(&self, catalog: &CatalogEvaluation) -> Vec<LatentTab> {
        let entities = &catalog.entities;
        let live_targets = self.materialized_action_targets(catalog);
        entities
            .iter()
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
        #[cfg(test)]
        CATALOG_BUILDS.with(|count| count.set(count.get() + 1));
        let mut entities = self
            .metadata
            .targets()
            .filter_map(|target| {
                let EntityId::Entity(entity) = target else {
                    return None;
                };
                let values = self.metadata.resolved_entries_for(target, self.now());
                if values.is_empty() {
                    return None;
                }
                Some(CatalogEntity {
                    entity: entity.clone(),
                    values,
                    ordinal: self.metadata.target_ordinal(target).unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        entities.sort_by(|a, b| {
            a.ordinal
                .cmp(&b.ordinal)
                .then_with(|| a.entity.cmp(&b.entity))
        });
        entities
    }

    fn materialized_action_targets(&self, catalog: &CatalogEvaluation) -> BTreeSet<String> {
        let tab_seed_metadata = self.tab_seed_metadata_entries();
        self.tabs
            .iter()
            .filter_map(|tab| self.tab_entity_ref(tab.tab_id, &tab_seed_metadata))
            .map(|entity| {
                catalog
                    .entity(&entity)
                    .and_then(|candidate| metadata_entry_text(&candidate.values, KEY_ACTION_TARGET))
                    .map(str::to_owned)
                    .unwrap_or_else(|| entity.action_target())
            })
            .collect()
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

    pub fn can_materialize_latent(&self, request: &crate::MaterializeLatentRequest) -> bool {
        // This exact equality check is the security boundary for controller pipe callers:
        // accepting an action target alone would let a caller substitute its own recipe or path.
        self.latent_tabs()
            .iter()
            .filter_map(LatentTab::materialize_request)
            .any(|candidate| candidate == *request)
    }

    pub fn begin_latent_materialization(
        &mut self,
        request: &crate::MaterializeLatentRequest,
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
        request: &crate::MaterializeLatentRequest,
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
        request: &crate::MaterializeLatentRequest,
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
        request: &crate::MaterializeLatentRequest,
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

    pub(crate) fn managed_content(&self) -> BTreeMap<EntityRef, crate::managed::DesiredContent> {
        self.metadata
            .targets()
            .filter_map(|target| {
                let EntityId::Entity(entity) = target else {
                    return None;
                };
                Some((entity.clone(), self.desired_content(target)?))
            })
            .collect()
    }

    fn desired_content(&self, target: &EntityId) -> Option<crate::managed::DesiredContent> {
        use crate::managed::{DesiredContent, TerminalContent};
        let values = self.metadata.resolved_entries_for(target, self.now());
        let text = |key| metadata_entry_text(&values, key).map(str::to_owned);
        match text("workspace.primary.state").as_deref()? {
            "held" => Some(DesiredContent::Held),
            "ready" => Some(DesiredContent::Ready(TerminalContent {
                target: text("workspace.primary.target").filter(|s| !s.is_empty())?,
                command: text(KEY_MATERIALIZE_RECIPE).filter(|s| !s.is_empty())?,
                // The same fact materialization launches in, so a workspace opened
                // from this resolution compares equal to it.
                cwd: text(KEY_CHECKOUT_PATH),
            })),
            _ => None,
        }
    }

    /// The managed target a materialization of `recipe` would install, if the
    /// entity is ready and that recipe is its current resolution.
    pub(crate) fn managed_primary_target(
        &self,
        entity: &EntityRef,
        recipe: &str,
        cwd: Option<&str>,
    ) -> Option<String> {
        match self.desired_content(&EntityId::Entity(entity.clone()))? {
            crate::managed::DesiredContent::Ready(content)
                if content.command == recipe && content.cwd.as_deref() == cwd =>
            {
                Some(content.target)
            }
            _ => None,
        }
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
        request: &crate::MaterializeLatentRequest,
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
        self.apply_metadata_patch(crate::MetadataPatch {
            target: crate::MetadataTarget::Tab(tab_id),
            source_id: SOURCE_LATENT_MATERIALIZER.to_owned(),
            set: BTreeMap::from([
                (
                    KEY_ENTITY_KIND.to_owned(),
                    crate::MetadataValueUpdate {
                        value: MetadataValue::Text(entity.kind.clone()),
                        ttl_ms: None,
                        precedence: Some(LATENT_MATERIALIZER_PRECEDENCE),
                        ordinal: None,
                    },
                ),
                (
                    KEY_ENTITY_ID.to_owned(),
                    crate::MetadataValueUpdate {
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

    fn resolve_template_slot(
        &self,
        slot: crate::template_config::TemplateConfigSlot,
        node_kind: crate::template_config::TemplateConfigNodeKind,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> Option<ResolvedTemplateSlot> {
        let catalog = self.template_catalog.as_ref()?;
        let context = crate::template_config::TemplateConfigMatchContext {
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
        Some(ResolvedTemplateSlot {
            template_name: resolved.name.clone(),
            fields: vec![],
            render_ready: Some(resolved.render_ready()),
            setters: resolved.setters.clone(),
            effective_kdl: resolved.dump_kdl(),
            resolve_error: None,
        })
    }

    fn tab_primary_metadata_entry(&self, tab_id: u64, key: &str) -> Option<MetadataEntry> {
        let entries: Vec<CandidateEntry> = self
            .panes
            .values()
            .filter(|pane| pane.tab_id == tab_id)
            .flat_map(|pane| {
                self.metadata
                    .entries_for(&EntityId::Pane(pane.pane_id), key, self.now())
            })
            .collect();
        select_primary_entry(&entries).map(|candidate| candidate.entry)
    }

    fn resolved_metadata_for_tabs(
        &self,
        tabs: &[TabCard],
        catalog: &CatalogEvaluation,
    ) -> Vec<ResolvedMetadata> {
        let seeds = self.tab_seed_metadata_entries();
        let targets = std::iter::once(EntityId::Root)
            .chain(tabs.iter().map(|tab| EntityId::Tab(tab.tab_id)))
            .chain(
                catalog
                    .entities
                    .iter()
                    .map(|entity| EntityId::Entity(entity.entity.clone())),
            );
        targets
            .map(|target| {
                let seed = if let EntityId::Tab(id) = target {
                    seeds.get(&id).cloned().unwrap_or_default()
                } else {
                    BTreeMap::new()
                };
                let (values, source_entries, reachable_identities) =
                    self.resolve_target_metadata(&target, seed);
                ResolvedMetadata {
                    target,
                    values,
                    source_entries,
                    reachable_identities,
                }
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
            let current_values = self.metadata.resolved_entries_for(&current, self.now());
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
            for (key, entries) in self.metadata.source_entries_for(&current, self.now()) {
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

    fn refresh_pane_cwd_metadata(&mut self, pane_id: PaneTarget) -> bool {
        let entity_id = EntityId::Pane(pane_id);
        let Some(pane) = self.panes.get(&pane_id) else {
            let changed = self
                .metadata
                .source_entry(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ, self.now())
                .is_some();
            self.metadata.unset(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ);
            return changed;
        };
        let should_emit = matches!(pane_id, PaneTarget::Terminal(_)) && pane.is_selectable;
        let Some(cwd) = pane.cwd.clone().filter(|_| should_emit) else {
            let changed = self
                .metadata
                .source_entry(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ, self.now())
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
            .source_entry(&entity_id, KEY_PANE_CWD, SOURCE_ZELLIJ, self.now())
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
                updated_at: self.now(),
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

fn node_metadata(
    node: &NodeKey,
    resolved_metadata: &[ResolvedMetadata],
) -> BTreeMap<String, MetadataValue> {
    let target = match node {
        NodeKey::Root => crate::ResolvedMetadataTarget::Root,
        NodeKey::Tab(tab_id) => crate::ResolvedMetadataTarget::Tab(*tab_id),
        NodeKey::Entity(entity) => crate::ResolvedMetadataTarget::Entity(entity.clone()),
        NodeKey::Placement(key) => key
            .0
            .last()
            .map(|segment| crate::ResolvedMetadataTarget::Entity(segment.entity.clone()))
            .unwrap_or(crate::ResolvedMetadataTarget::Root),
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

fn entity_facts(
    entity: &EntityRef,
    values: &BTreeMap<String, MetadataEntry>,
) -> BTreeMap<String, MetadataValue> {
    let mut facts = values
        .iter()
        .map(|(key, entry)| (key.clone(), entry.value.clone()))
        .collect::<BTreeMap<_, _>>();
    // The target is the canonical entity identity. Producers must not have to
    // duplicate it in every patch's set map for placement queries and templates.
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

fn metadata_entry_text<'a>(
    values: &'a BTreeMap<String, MetadataEntry>,
    key: &str,
) -> Option<&'a str> {
    match values.get(key).map(|entry| &entry.value) {
        Some(MetadataValue::Text(value)) => Some(value),
        _ => None,
    }
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
    fn placement_loop(predicates: &[(&str, &str)]) -> crate::template_config::PlacementDefinition {
        crate::template_config::PlacementDefinition {
            name: "section".to_owned(),
            loops: vec![crate::template_config::PlacementLoop {
                binding: "item".to_owned(),
                visibility: None,
                predicates: predicates
                    .iter()
                    .map(|(key, value)| crate::template_config::PlacementPredicate {
                        key: (*key).to_owned(),
                        value: Some((*value).to_owned()),
                        of: None,
                    })
                    .collect(),
                order: vec![],
                fields: vec![],
                layout: None,
                tier: None,
                loops: vec![],
                apply_template: None,
            }],
        }
    }
    use crate::{
        PluginPaneKind, PluginPlacement, PluginRegistrationHello, RailStructure, StatusIcon,
    };

    type EntityId = crate::MetadataTarget;
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

    fn tab_info(tab_id: usize, position: usize, name: &str, active: bool) -> ControllerTab {
        ControllerTab {
            tab_id: tab_id as u64,
            position,
            name: name.into(),
            active,
        }
    }

    fn entity_ref(kind: &str, id: &str) -> crate::EntityRef {
        crate::EntityRef {
            kind: kind.to_owned(),
            id: id.to_owned(),
        }
    }

    #[test]
    fn cross_layer_variable_setter_warnings_reach_template_diagnostics() {
        let user = crate::template_config::parse_template_config_kdl(
            r#"set "child-layout" "compact-strip""#,
        )
        .expect("user setter");
        let bundled = crate::template_config::parse_template_config_kdl(
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
            crate::template_config::TemplateConfigCatalog::from_layers(vec![
                crate::template_config::TemplateConfigLayer::user("user.kdl", user),
                crate::template_config::TemplateConfigLayer::bundled("bundled.kdl", bundled),
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
                crate::MetadataValueUpdate {
                    value: MetadataValue::Text(kind.to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: Some(ordinal),
                },
            ),
            (
                KEY_ENTITY_ID.to_owned(),
                crate::MetadataValueUpdate {
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
                crate::MetadataValueUpdate {
                    value: MetadataValue::Text((*value).to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: Some(ordinal),
                },
            )
        }));
        state.apply_metadata_patch(crate::MetadataPatch {
            target: crate::MetadataTarget::Entity(entity),
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
        state.apply_metadata_patch(crate::MetadataPatch {
            target: crate::MetadataTarget::Entity(entity_ref(kind, id)),
            source_id: "flotilla-connector".to_owned(),
            set: facts
                .iter()
                .map(|(key, value)| {
                    (
                        (*key).to_owned(),
                        crate::MetadataValueUpdate {
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
            &mut BTreeMap::new(),
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
                &mut BTreeMap::new()
            )
            .is_empty());
        assert!(
            state
                .evaluate_placement(
                    &placement_loop(&[("entity.kind", "vessel"), ("status.attention", "true")]),
                    &entities,
                    &index,
                    "full",
                    &mut BTreeMap::new()
                )
                .is_empty(),
            "predicates intersect: an unmatched one empties the result"
        );
    }

    #[test]
    fn loops_over_the_same_kind_choose_independent_total_orders() {
        let config = crate::template_config::parse_template_config_kdl(
            r#"
version 1
placement "urgent" {
  for "item" kind="vessel" {
    order "status.rank" direction="descending" absent="last"
  }
}
placement "stable" {
  for "item" kind="vessel" {
    order "display.label" direction="ascending" absent="first"
  }
}
placement "missing-first" {
  for "item" kind="vessel" {
    order "status.rank" direction="ascending" absent="first"
  }
}
placement "identity-descending" {
  for "item" kind="vessel" {
    order "entity.id" direction="descending"
  }
}
"#,
        )
        .expect("two independently ordered loops");
        let mut state = directory_entity_state();
        apply_entity(
            &mut state,
            "vessel",
            "zeta-id",
            1,
            &[
                ("flotilla.vessel", "zeta-id"),
                ("display.label", "alpha"),
                ("status.rank", "1"),
            ],
        );
        apply_entity(
            &mut state,
            "vessel",
            "alpha-id",
            1,
            &[
                ("flotilla.vessel", "alpha-id"),
                ("display.label", "zeta"),
                ("status.rank", "9"),
            ],
        );
        apply_entity(
            &mut state,
            "vessel",
            "missing-id",
            1,
            &[
                ("flotilla.vessel", "missing-id"),
                ("display.label", "middle"),
            ],
        );
        let entities = state.catalog_entities();
        let index = PlacementIndex::build(&entities);
        let labels = |placement: &crate::template_config::PlacementDefinition| {
            state
                .evaluate_placement(placement, &entities, &index, "full", &mut BTreeMap::new())
                .into_iter()
                .map(|entity| entity.label)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            labels(&config.placements[0]),
            vec!["zeta", "alpha", "middle"]
        );
        assert_eq!(
            labels(&config.placements[1]),
            vec!["alpha", "middle", "zeta"]
        );
        assert_eq!(
            labels(&config.placements[2]),
            vec!["middle", "alpha", "zeta"],
            "absent-first is independent of ascending direction"
        );
        assert_eq!(
            labels(&config.placements[3]),
            vec!["alpha", "middle", "zeta"],
            "canonical entity identity facts can be ordered explicitly"
        );
    }

    #[test]
    fn undeclared_and_equal_order_falls_back_to_stable_entity_identity() {
        let mut state = directory_entity_state();
        for id in ["zeta-id", "alpha-id"] {
            apply_entity(
                &mut state,
                "vessel",
                id,
                1,
                &[("flotilla.vessel", id), ("display.label", "same")],
            );
        }
        let entities = state.catalog_entities();
        let index = PlacementIndex::build(&entities);
        let placed = state.evaluate_placement(
            &placement_loop(&[("entity.kind", "vessel")]),
            &entities,
            &index,
            "full",
            &mut BTreeMap::new(),
        );

        assert_eq!(
            placed
                .into_iter()
                .map(|item| item.entity.id)
                .collect::<Vec<_>>(),
            vec!["alpha-id", "zeta-id"]
        );
    }

    #[test]
    fn placement_state_is_independent_per_appearance_and_survives_model_pushes() {
        let entity = EntityRef {
            kind: "vessel".to_owned(),
            id: "dev/focus/worker@lab".to_owned(),
        };
        let key = |loop_name: &str| {
            PlacementKey(vec![PlacementSegment {
                loop_name: loop_name.to_owned(),
                entity: entity.clone(),
            }])
        };
        let tree = key("tree-item");
        let attention = key("attention-item");
        let mut state = directory_entity_state();

        assert!(state.apply_rail_ui_action(RailUiAction::TogglePlacement { key: tree.clone() }));
        assert_eq!(state.view_model().collapsed_placements, vec![tree.clone()]);
        assert!(!state.view_model().collapsed_placements.contains(&attention));

        state.set_sort_mode(SortMode::Controller);
        assert_eq!(state.view_model().collapsed_placements, vec![tree]);
    }

    #[test]
    fn placement_layout_variables_are_scoped_by_the_full_key() {
        let entity = EntityRef {
            kind: "vessel".to_owned(),
            id: "dev/focus/worker@lab".to_owned(),
        };
        let placement = |loop_name: &str| {
            NodeKey::Placement(PlacementKey(vec![PlacementSegment {
                loop_name: loop_name.to_owned(),
                entity: entity.clone(),
            }]))
        };
        let tree = placement("tree-item");
        let attention = placement("attention-item");
        let mut state = ControllerState::default();
        assert!(state.set_node_variable(
            tree.clone(),
            "child-layout".to_owned(),
            Some("cards".to_owned()),
        ));
        assert!(state.set_node_variable(
            attention.clone(),
            "child-layout".to_owned(),
            Some("strip".to_owned()),
        ));

        let overrides = state.bootstrap_snapshot().node_variable_overrides;
        assert!(overrides.iter().any(|item| item.node == tree
            && item.values.get("child-layout").map(String::as_str) == Some("cards")));
        assert!(overrides.iter().any(|item| item.node == attention
            && item.values.get("child-layout").map(String::as_str) == Some("strip")));
    }

    #[test]
    fn renaming_a_loop_deliberately_resets_its_placement_subtree_state() {
        let entity = EntityRef {
            kind: "vessel".to_owned(),
            id: "dev/focus/worker@lab".to_owned(),
        };
        let key = |loop_name: &str| {
            PlacementKey(vec![PlacementSegment {
                loop_name: loop_name.to_owned(),
                entity: entity.clone(),
            }])
        };
        let old = key("vessel");
        let renamed = key("worker");
        let mut state = ControllerState::default();
        state.apply_rail_ui_action(RailUiAction::TogglePlacement { key: old.clone() });

        assert!(state.rail_ui_state().collapsed_placements.contains(&old));
        assert!(!state
            .rail_ui_state()
            .collapsed_placements
            .contains(&renamed));
    }

    #[test]
    fn an_entity_without_workspace_or_recipe_falls_back_to_inspect() {
        // A catalog issue without a workspace or recipe has no activation;
        // the caller opens its inspector.
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
            "an entity without a workspace or recipe cannot activate"
        );
    }

    #[test]
    fn unchanged_tab_update_is_not_a_controller_change() {
        let mut state = ControllerState::default();
        let tabs = vec![tab_info(1, 0, "work", true), tab_info(2, 1, "logs", false)];

        assert!(state.observe_workspaces(tabs.clone()));
        assert!(!state.observe_workspaces(tabs));
    }

    #[test]
    fn tab_update_change_is_a_controller_change() {
        let mut state = ControllerState::default();
        state.observe_workspaces(vec![
            tab_info(1, 0, "work", true),
            tab_info(2, 1, "logs", false),
        ]);

        assert!(state.observe_workspaces(vec![
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
        let patch = crate::MetadataPatch {
            target: crate::MetadataTarget::Tab(1),
            source_id: "watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                crate::MetadataValueUpdate {
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
        let patch = crate::MetadataPatch {
            target: crate::MetadataTarget::Tab(1),
            source_id: "watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                crate::MetadataValueUpdate {
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
        let manifest = vec![PaneObservation {
            workspace_id: 1,
            pane_id: PaneTarget::Terminal(10),
            is_selectable: true,
            is_focused: false,
            ordinal: 0,
        }];

        assert!(state.observe_panes(manifest.clone()));
        state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into());
        let receive_counter = state.receive_counter;

        assert!(!state.observe_panes(manifest));
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
        let manifest = vec![PaneObservation {
            workspace_id: 1,
            pane_id: PaneTarget::Terminal(10),
            is_selectable: true,
            is_focused: false,
            ordinal: 0,
        }];
        state.observe_panes(manifest.clone());

        assert_eq!(state.terminal_panes_for_cwd_refresh(), vec![10]);
        state.mark_pane_cwd_requested(PaneTarget::Terminal(10));

        // A failed lookup must not be retried on the next manifest update.
        state.observe_panes(manifest.clone());
        assert_eq!(state.terminal_panes_for_cwd_refresh(), Vec::<u32>::new());

        // CwdChanged still lands and keeps the pane out of the refresh set.
        assert!(state.set_pane_cwd(PaneTarget::Terminal(10), "/repo/a".into()));
        state.observe_panes(manifest);
        assert_eq!(state.terminal_panes_for_cwd_refresh(), Vec::<u32>::new());
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
        state.apply_metadata_patch(crate::MetadataPatch {
            target: EntityId::Tab(1),
            source_id: "dir-watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                crate::MetadataValueUpdate {
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });
        state.apply_metadata_patch(crate::MetadataPatch {
            target: EntityId::Identity(crate::MetadataIdentity {
                key: "git.repo".to_owned(),
                value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
            }),
            source_id: "gh".to_owned(),
            set: BTreeMap::from([(
                "vcs.pr".to_owned(),
                crate::MetadataValueUpdate {
                    value: MetadataValue::Text("#45".to_owned()),
                    ttl_ms: None,
                    precedence: None,
                    ordinal: None,
                },
            )]),
            unset: vec![],
        });
        state.apply_metadata_patch(crate::MetadataPatch {
            target: EntityId::Identity(crate::MetadataIdentity {
                key: "vcs.pr".to_owned(),
                value: MetadataValue::Text("#45".to_owned()),
            }),
            source_id: "ci".to_owned(),
            set: BTreeMap::from([(
                "ci.status".to_owned(),
                crate::MetadataValueUpdate {
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
            .find(|metadata| metadata.target == crate::ResolvedMetadataTarget::Tab(1))
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
        state.apply_metadata_patch(crate::MetadataPatch {
            target: EntityId::Identity(MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text("/Users/robert/dev/katzensteg".to_owned()),
            }),
            source_id: "dir-watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                crate::MetadataValueUpdate {
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
            .find(|metadata| metadata.target == crate::ResolvedMetadataTarget::Tab(1))
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
        state.apply_metadata_patch(crate::MetadataPatch {
            target: EntityId::Identity(MetadataIdentity {
                key: KEY_PANE_CWD.to_owned(),
                value: MetadataValue::Text(cwd.clone()),
            }),
            source_id: "dir-watcher".to_owned(),
            set: BTreeMap::from([(
                "git.repo".to_owned(),
                crate::MetadataValueUpdate {
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
                && observed.target_count == 1
                && observed.nearest_distance == 1
        }));
        assert!(model.observed_identities.iter().any(|observed| {
            observed.identity
                == MetadataIdentity {
                    key: "git.repo".to_owned(),
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                }
                && observed.target_count == 1
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
                collapsed_placements: vec![],
                scroll_offset: 8,
                variables: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn bootstrap_snapshot_keeps_only_persistent_display_variables() {
        let mut state = ControllerState::default();
        state.set_template_catalog(Some(
            crate::template_config::TemplateConfigCatalog::from_config(
                crate::template_config::parse_template_config_kdl(
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
            collapsed_placements: vec![],
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
                collapsed_placements: vec![],
                scroll_offset: 10,
                variables: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn bootstrap_snapshot_carries_metadata_patches() {
        const KEY: &str = "tab.subject";
        let mut source = ControllerState::default();
        source.apply_metadata_patch(crate::MetadataPatch {
            target: EntityId::Tab(7),
            source_id: "test".to_owned(),
            set: BTreeMap::from([(
                KEY.to_owned(),
                crate::MetadataValueUpdate {
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
            .find(|metadata| metadata.target == crate::ResolvedMetadataTarget::Tab(7))
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
        target.observe_workspaces(vec![tab_info(1, 0, "main", true)]);
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
