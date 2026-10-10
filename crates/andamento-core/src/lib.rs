use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub mod detail;
pub mod host;
pub mod managed;
mod metadata;
pub mod presentation;
#[doc(hidden)]
pub mod profile;
pub mod replay;
pub mod sidebar;
pub mod state;
pub use sidebar::Sidebar;
pub mod template_config;
mod workspace_id;
pub use workspace_id::WorkspaceId;

pub const MSG_RENDERER_HELLO: &str = "andamento-renderer-hello";
pub const MSG_CONFIG_EDITOR_HELLO: &str = "andamento-config-editor-hello";
pub const MSG_REQUEST_STATE: &str = "andamento-request-state";
pub const MSG_VIEW_MODEL: &str = "andamento-view-model";
pub const MSG_TOGGLE_PIN: &str = "andamento-toggle-pin";
pub const MSG_SET_SORT_MODE: &str = "andamento-set-sort-mode";
pub const MSG_SET_RAIL_CONFIG: &str = "andamento-set-rail-config";
pub const MSG_SET_PANE_STATUS: &str = "andamento-set-pane-status";
pub const MSG_CLEAR_PANE_STATUS: &str = "andamento-clear-pane-status";
pub const MSG_APPLY_METADATA_PATCH: &str = "andamento-apply-metadata-patch";
pub const MSG_CONTROLLER_BOOTSTRAP_REQUEST: &str = "andamento-controller-bootstrap-request";
pub const MSG_CONTROLLER_BOOTSTRAP_STATE: &str = "andamento-controller-bootstrap-state";
pub const MSG_OBSERVED_IDENTITIES: &str = "andamento-observed-identities";
pub const MSG_STATS_COLLECT: &str = "andamento-stats-collect";
pub const MSG_STATS_REQUEST: &str = "andamento-stats-request";
pub const MSG_STATS_REPORT: &str = "andamento-stats-report";
pub const MSG_RAIL_SIZE_OBSERVED: &str = "andamento-rail-size-observed";
pub const MSG_RAIL_SIZE_TARGET: &str = "andamento-rail-size-target";
pub const MSG_RAIL_UI_ACTION: &str = "andamento-rail-ui-action";
pub const MSG_RAIL_UI_STATE: &str = "andamento-rail-ui-state";
pub const MSG_REQUEST_RAIL_UI_STATE: &str = "andamento-request-rail-ui-state";
pub const MSG_SET_METADATA_VISIBILITY: &str = "andamento-set-metadata-visibility";
pub const MSG_SET_NODE_VARIABLE: &str = "andamento-set-node-variable";
pub const MSG_CONFIG_INSPECT: &str = "andamento-config-inspect";
pub const MSG_MATERIALIZE_LATENT: &str = "andamento-materialize-latent";
pub const MSG_ACTIVATE_ENTITY: &str = "andamento-activate-entity";
pub const NODE_VARIABLE_CONFIG_OVERRIDE_SETTER: &str = "config override";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    Idle,
    Info,
    Success,
    Waiting,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum PaneTarget {
    Terminal(u32),
    Plugin(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetPaneStatus {
    pub pane_id: PaneTarget,
    pub priority: Priority,
    pub title: String,
    pub detail: Option<String>,
    pub icon: Option<StatusIcon>,
    pub timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ExternalMessage {
    SetPaneStatus(SetPaneStatus),
    ClearPaneStatus { pane_id: PaneTarget },
    MetadataPatch(MetadataPatch),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererHello {
    pub plugin_id: u32,
    pub client_id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRegistrationHello {
    pub identity: RendererHello,
    pub placement: PluginPlacement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PluginPlacement {
    Tab {
        tab_id: WorkspaceId,
        pane_kind: PluginPaneKind,
    },
    Background,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginPaneKind {
    Tiled,
    Floating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigInspectRequest {
    pub client_id: u16,
    pub origin_tab_id: WorkspaceId,
    pub node_key: NodeKey,
    pub config_plugin_url: String,
    pub controller_plugin_url: String,
}

/// Activate the entity behind a row.
///
/// Entities without an observed workspace or a materialization recipe use the
/// inspector fallback, so every placed entity remains inspectable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityActivationRequest {
    pub entity: EntityRef,
    pub inspect_fallback: ConfigInspectRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataVisibilitySetRequest {
    pub client_id: u16,
    pub node_key: NodeKey,
    pub state: Option<MetadataTriState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeVariableSetRequest {
    pub client_id: u16,
    pub node_key: NodeKey,
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum RailUiAction {
    TogglePlacement {
        key: PlacementKey,
    },
    ToggleVariable {
        name: String,
    },
    ScrollBy {
        delta: isize,
    },
    SetScrollOffset {
        offset: isize,
    },
    ResetScroll,
    /// Host-owned sibling order for one loop invocation. An empty order clears
    /// it. Entities the order doesn't name keep data order relative to it.
    SetSiblingOrder {
        loop_key: PlacementLoopKey,
        order: Vec<EntityRef>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RailUiRevision {
    pub sequence: u64,
    pub writer_client_id: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailUiState {
    #[serde(default)]
    pub revision: RailUiRevision,
    #[serde(default)]
    pub collapsed_placements: Vec<PlacementKey>,
    #[serde(default)]
    pub scroll_offset: isize,
    #[serde(default)]
    pub variables: BTreeMap<String, DisplayVariableValue>,
    /// Host-owned sibling orders. A list of pairs because JSON map keys must be
    /// strings; on restore, empty orders are dropped and a later key wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sibling_orders: Vec<(PlacementLoopKey, Vec<EntityRef>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DisplayVariableValue {
    Bool(bool),
    Enum(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum RailSize {
    Fixed(usize),
    Percent(f64),
}

impl RailSize {
    pub fn within_tolerance(self, other: Self) -> bool {
        match (self, other) {
            (Self::Fixed(left), Self::Fixed(right)) => left.abs_diff(right) <= 1,
            (Self::Percent(left), Self::Percent(right)) => (left - right).abs() < 0.001,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RailSizeObserved {
    pub rail: RendererHello,
    pub size: RailSize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RailSizeTarget {
    pub client_id: u16,
    pub size: RailSize,
    pub version: u64,
}

#[cfg(test)]
mod rail_size_tests {
    use super::RailSize;

    #[test]
    fn rail_size_tolerance_accepts_nearby_values_of_the_same_kind() {
        assert!(RailSize::Fixed(20).within_tolerance(RailSize::Fixed(21)));
        assert!(RailSize::Percent(18.75).within_tolerance(RailSize::Percent(18.7505)));
    }

    #[test]
    fn rail_size_tolerance_rejects_different_kinds_and_meaningful_changes() {
        assert!(!RailSize::Fixed(20).within_tolerance(RailSize::Percent(20.0)));
        assert!(!RailSize::Fixed(20).within_tolerance(RailSize::Fixed(22)));
        assert!(!RailSize::Percent(18.75).within_tolerance(RailSize::Percent(18.752)));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerBootstrapSnapshot {
    pub sort_mode: SortMode,
    pub config: RailConfig,
    pub pinned_tabs: Vec<WorkspaceId>,
    pub pane_statuses: Vec<SetPaneStatus>,
    #[serde(default)]
    pub node_variable_overrides: Vec<NodeVariableOverrides>,
    #[serde(default)]
    pub metadata_patches: Vec<MetadataPatch>,
    #[serde(default)]
    pub rail_ui_state: RailUiState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabStatusSummary {
    pub priority: Priority,
    pub title: String,
    pub detail: Option<String>,
    pub icon: Option<StatusIcon>,
    pub source_pane: PaneTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum StatusIcon {
    Builtin(String),
    PngFile(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabCard {
    pub tab_id: WorkspaceId,
    pub position: usize,
    pub name: String,
    pub active: bool,
    pub pinned: bool,
    pub status: Option<TabStatusSummary>,
    #[serde(default)]
    pub templates: ResolvedTemplateSlots,
    /// The currently-focused pane on this tab, if any. Useful for
    /// templates and debugging.
    #[serde(default)]
    pub active_pane: Option<PaneTarget>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatentMaterializationState {
    #[default]
    Ready,
    Opening,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatentTab {
    /// Catalog entity whose workspace is not currently materialized.
    pub entity: EntityRef,
    /// Stable producer-owned identity used to deduplicate materializations.
    pub action_target: String,
    pub name: String,
    /// The opener-owned lifecycle while this catalog entry has no live tab.
    #[serde(default)]
    pub materialization: LatentMaterializationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Producer provenance rendered as a compact source badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize_recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_path: Option<String>,
    #[serde(default)]
    pub templates: ResolvedTemplateSlots,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializeLatentRequest {
    pub action_target: String,
    pub name: String,
    pub recipe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_path: Option<String>,
}

impl LatentTab {
    pub fn materialize_request(&self) -> Option<MaterializeLatentRequest> {
        if self.materialization == LatentMaterializationState::Opening {
            return None;
        }
        Some(MaterializeLatentRequest {
            action_target: self.action_target.clone(),
            name: self.name.clone(),
            recipe: self.materialize_recipe.clone()?,
            checkout_path: self.checkout_path.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortMode {
    Controller,
    Position,
    PinnedFirst,
    LatestStatus,
}

impl Default for SortMode {
    fn default() -> Self {
        Self::Position
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RailStructure {
    JoinedCells,
    SplitAroundActive,
    BoxPerTab,
}

impl Default for RailStructure {
    fn default() -> Self {
        Self::JoinedCells
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum MetadataValue {
    Text(String),
    Bool(bool),
    Integer(i64),
    StringList(Vec<String>),
    EntityRefs(Vec<EntityRef>),
    /// Structured producer metadata, not a UI identity or a grouping rule.
    /// Placement state is keyed exclusively by `PlacementKey`.
    GroupPath(Vec<MetadataPathSegmentValue>),
}

#[derive(Debug, Clone, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MetadataPathSegmentValue {
    pub key: String,
    pub value: MetadataPathValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl PartialEq for MetadataPathSegmentValue {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.value == other.value
    }
}

impl Eq for MetadataPathSegmentValue {}

impl std::hash::Hash for MetadataPathSegmentValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key.hash(state);
        self.value.hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum MetadataPathValue {
    Text(String),
    Bool(bool),
    Integer(i64),
    StringList(Vec<String>),
}

impl From<MetadataPathValue> for MetadataValue {
    fn from(value: MetadataPathValue) -> Self {
        match value {
            MetadataPathValue::Text(value) => MetadataValue::Text(value),
            MetadataPathValue::Bool(value) => MetadataValue::Bool(value),
            MetadataPathValue::Integer(value) => MetadataValue::Integer(value),
            MetadataPathValue::StringList(values) => MetadataValue::StringList(values),
        }
    }
}

impl MetadataPathValue {
    pub fn display(&self) -> String {
        match self {
            MetadataPathValue::Text(value) => value.clone(),
            MetadataPathValue::Bool(value) => value.to_string(),
            MetadataPathValue::Integer(value) => value.to_string(),
            MetadataPathValue::StringList(values) => values.join(", "),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MetadataIdentity {
    pub key: String,
    pub value: MetadataValue,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    pub kind: String,
    pub id: String,
}

/// The identity of one rendered appearance of an entity.
///
/// Each segment records the loop that selected the entity at that depth.
/// Placement identity remains outside metadata's target space.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlacementKey(pub Vec<PlacementSegment>);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlacementSegment {
    pub loop_name: String,
    pub entity: EntityRef,
}

/// One invocation of a declared loop beneath a particular parent appearance.
/// Siblings share this identity; a different parent or binding creates another.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlacementLoopKey {
    pub region: String,
    pub parent: PlacementKey,
    pub binding: String,
}

impl PlacementKey {
    pub fn loop_key(&self, region: &str) -> Option<PlacementLoopKey> {
        let (last, parent) = self.0.split_last()?;
        Some(PlacementLoopKey {
            region: region.to_owned(),
            parent: PlacementKey(parent.to_vec()),
            binding: last.loop_name.clone(),
        })
    }
}

impl PlacementLoopKey {
    /// Stable text for hosts: each part is length-prefixed (`len:text`), in
    /// the order region, then each parent segment's loop name, entity kind and
    /// id, then the loop binding. Hosts treat it as opaque and compare it for
    /// equality; only Andamento decodes it.
    pub fn encode(&self) -> String {
        std::iter::once(&self.region)
            .chain(
                self.parent
                    .0
                    .iter()
                    .flat_map(|s| [&s.loop_name, &s.entity.kind, &s.entity.id]),
            )
            .chain(std::iter::once(&self.binding))
            .map(|s| format!("{}:{}", s.len(), s))
            .collect()
    }

    /// Inverse of [`encode`](Self::encode), for keys a host hands back.
    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let (len, tail) = rest.split_once(':')?;
            let len: usize = len.parse().ok()?;
            if !tail.is_char_boundary(len.min(tail.len())) || tail.len() < len {
                return None;
            }
            parts.push(tail[..len].to_owned());
            rest = &tail[len..];
        }
        if parts.len() < 2 || (parts.len() - 2) % 3 != 0 {
            return None;
        }
        let binding = parts.pop()?;
        let region = parts.remove(0);
        let parent = parts
            .chunks(3)
            .map(|c| PlacementSegment {
                loop_name: c[0].clone(),
                entity: EntityRef {
                    kind: c[1].clone(),
                    id: c[2].clone(),
                },
            })
            .collect();
        Some(Self {
            region,
            parent: PlacementKey(parent),
            binding,
        })
    }
}

impl EntityRef {
    pub fn action_target(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum MetadataTarget {
    Root,
    Pane(PaneTarget),
    Tab(WorkspaceId),
    Entity(EntityRef),
    Identity(MetadataIdentity),
}

/// Controller-internal target space used in resolved view-model metadata.
///
/// Group paths are derived presentation nodes, never metadata-patch wire
/// targets. Keeping them here lets inspect/config address a rendered group
/// without making group assertions representable on the external wire.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum ResolvedMetadataTarget {
    Root,
    Pane(PaneTarget),
    Tab(WorkspaceId),
    Entity(EntityRef),
    Identity(MetadataIdentity),
}

impl From<MetadataTarget> for ResolvedMetadataTarget {
    fn from(target: MetadataTarget) -> Self {
        match target {
            MetadataTarget::Root => Self::Root,
            MetadataTarget::Pane(pane) => Self::Pane(pane),
            MetadataTarget::Tab(tab) => Self::Tab(tab),
            MetadataTarget::Entity(entity) => Self::Entity(entity),
            MetadataTarget::Identity(identity) => Self::Identity(identity),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataValueUpdate {
    pub value: MetadataValue,
    pub ttl_ms: Option<u64>,
    pub precedence: Option<i64>,
    pub ordinal: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataPatch {
    pub target: MetadataTarget,
    pub source_id: String,
    #[serde(default, deserialize_with = "decode_metadata_updates")]
    pub set: BTreeMap<String, MetadataValueUpdate>,
    #[serde(default)]
    pub unset: Vec<String>,
}

// Future value kinds must not discard the known facts in the same connector patch.
fn decode_metadata_updates<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, MetadataValueUpdate>, D::Error> {
    let updates = BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
    updates
        .into_iter()
        .filter_map(|(key, update)| {
            let kind = update
                .get("value")
                .and_then(|v| v.get("type"))
                .and_then(|v| v.as_str());
            match kind {
                Some(
                    "text" | "bool" | "integer" | "string-list" | "group-path" | "entity-refs",
                )
                | None => Some(
                    serde_json::from_value(update)
                        .map(|update| (key, update))
                        .map_err(serde::de::Error::custom),
                ),
                Some(_) => None,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataEntry {
    pub value: MetadataValue,
    pub updated_at: u64,
    pub ttl_ms: Option<u64>,
    pub precedence: i64,
    pub ordinal: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedMetadata {
    pub target: ResolvedMetadataTarget,
    #[serde(default)]
    pub values: BTreeMap<String, MetadataEntry>,
    #[serde(default)]
    pub source_entries: BTreeMap<String, Vec<MetadataSourceEntry>>,
    #[serde(default)]
    pub reachable_identities: Vec<ReachableMetadataIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataSourceEntry {
    pub source_id: String,
    pub entry: MetadataEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachableMetadataIdentity {
    pub identity: MetadataIdentity,
    pub distance: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedMetadataIdentity {
    pub identity: MetadataIdentity,
    pub target_count: usize,
    pub nearest_distance: usize,
}

/// Conventional form that uses the rail's wrapped ribbon layout.
///
/// Form names remain an open string vocabulary; only this form has distinct
/// built-in geometry. Other names use the full surface.
pub const DISPLAY_FORM_COMPACT: &str = "compact";

/// Conventional name for the default full surface.
pub const DISPLAY_FORM_FULL: &str = "full";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTemplateSlots {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact: Option<ResolvedTemplateSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ResolvedTemplateSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTemplateSlot {
    pub template_name: String,
    #[serde(default)]
    pub fields: Vec<ResolvedTemplateField>,
    /// Parsed render data paired with `effective_kdl` by the controller.
    ///
    /// Keeping both representations in the snapshot lets the rail render live
    /// state without reparsing the inspectable KDL on every redraw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_ready: Option<template_config::TemplateConfigRenderReady>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setters: Vec<template_config::ResolvedVariableSetter>,
    /// Flattened template evidence shown by inspect.
    #[serde(default)]
    pub effective_kdl: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTemplateField {
    pub text: String,
    pub priority: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResolvedTemplateFieldSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResolvedTemplateFieldSource {
    pub key: String,
    pub value: MetadataValue,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateConfigDiagnostics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub state: TemplateConfigState,
    #[serde(default)]
    pub template_count: usize,
    #[serde(default)]
    pub template_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub effective_variables: Vec<EffectiveNodeVariables>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigState {
    #[default]
    NotConfigured,
    PendingPermission,
    Loaded,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailRgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailConfig {
    #[serde(default)]
    pub structure: RailStructure,
    #[serde(default)]
    pub segment_between_color: Option<RailRgbColor>,
}

impl Default for RailConfig {
    fn default() -> Self {
        Self {
            structure: RailStructure::default(),
            segment_between_color: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerViewModel {
    /// Catalog entries no placement loop selected; exposed in the existing inspector.
    pub unmatched_entities: Vec<EntityRef>,
    pub sort_mode: SortMode,
    pub config: RailConfig,
    #[serde(default)]
    pub template_config: TemplateConfigDiagnostics,
    pub tabs: Vec<TabCard>,
    #[serde(default)]
    pub resolved_metadata: Vec<ResolvedMetadata>,
    #[serde(default)]
    pub observed_identities: Vec<ObservedMetadataIdentity>,
    #[serde(default)]
    pub metadata_controls: MetadataControls,
    #[serde(default)]
    pub inspected_node: Option<NodeKey>,
    #[serde(default)]
    pub collapsed_placements: Vec<PlacementKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sibling_orders: Vec<(PlacementLoopKey, Vec<EntityRef>)>,
    #[serde(default)]
    pub display_variables: Vec<template_config::TemplateVariableDefinition>,
    #[serde(default)]
    pub display_variable_values: BTreeMap<String, DisplayVariableValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<presentation::SurfaceSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataTriState {
    Clean,
    Meta,
    MetaChildren,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "value")]
pub enum NodeKey {
    Root,
    Tab(WorkspaceId),
    Entity(EntityRef),
    Placement(PlacementKey),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeVariableOverrides {
    pub node: NodeKey,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveNodeVariables {
    pub node: NodeKey,
    #[serde(default)]
    pub values: BTreeMap<String, EffectiveVariableValue>,
    /// Declarations in scope at this node. The inspector renders a control per
    /// declaration, so what is settable here is carried with what is set.
    #[serde(default)]
    pub declarations: Vec<template_config::NodeVariableDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveVariableValue {
    pub value: String,
    pub provenance: VariableSetterProvenance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overridden: Vec<VariableSetterProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariableSetterProvenance {
    pub setter: String,
    pub ancestor: NodeKey,
    pub origin: template_config::TemplateConfigOrigin,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataControls {
    /// Sparse storage: an entry exists only when the user has explicitly set
    /// a state on that node. Absence = inherit from above (defaults hidden).
    /// We use HashMap rather than BTreeMap because NodeKey is not Ord.
    #[serde(
        default,
        serialize_with = "serialize_node_map",
        deserialize_with = "deserialize_node_map"
    )]
    pub per_node: HashMap<NodeKey, MetadataTriState>,
}

fn serialize_node_map<S>(
    map: &HashMap<NodeKey, MetadataTriState>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let entries: Vec<(&NodeKey, &MetadataTriState)> = map.iter().collect();
    entries.serialize(serializer)
}

fn deserialize_node_map<'de, D>(
    deserializer: D,
) -> Result<HashMap<NodeKey, MetadataTriState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let entries: Vec<(NodeKey, MetadataTriState)> = Vec::deserialize(deserializer)?;
    Ok(entries.into_iter().collect())
}

impl MetadataTriState {
    pub fn glyph(self) -> char {
        match self {
            MetadataTriState::Clean => '○',
            MetadataTriState::Meta => '◐',
            MetadataTriState::MetaChildren => '●',
        }
    }
}

impl MetadataControls {
    /// Cycle order: Absent → Meta → MetaChildren → Clean → Absent.
    pub fn cycle(&mut self, key: NodeKey) {
        let next = match self.per_node.get(&key) {
            None => Some(MetadataTriState::Meta),
            Some(MetadataTriState::Meta) => Some(MetadataTriState::MetaChildren),
            Some(MetadataTriState::MetaChildren) => Some(MetadataTriState::Clean),
            Some(MetadataTriState::Clean) => None,
        };
        match next {
            Some(state) => {
                self.per_node.insert(key, state);
            }
            None => {
                self.per_node.remove(&key);
            }
        }
    }

    pub fn effective_show(&self, key: &NodeKey, ancestor_meta_children: bool) -> bool {
        match self.per_node.get(key) {
            Some(MetadataTriState::Meta) | Some(MetadataTriState::MetaChildren) => true,
            Some(MetadataTriState::Clean) => false,
            None => ancestor_meta_children,
        }
    }

    pub fn propagates_to_children(&self, key: &NodeKey, ancestor_meta_children: bool) -> bool {
        match self.per_node.get(key) {
            Some(MetadataTriState::MetaChildren) => true,
            Some(MetadataTriState::Meta) => false,
            Some(MetadataTriState::Clean) => false,
            None => ancestor_meta_children,
        }
    }
}

impl ControllerViewModel {
    pub fn tab_by_id(&self, tab_id: WorkspaceId) -> Option<&TabCard> {
        self.tabs.iter().find(|tab| tab.tab_id == tab_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsCollectRequest {
    pub requester: RendererHello,
    pub collection_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStatsSpan {
    pub name: String,
    pub count: u64,
    pub total_us: u64,
    pub max_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStatsSnapshot {
    pub collection_id: u64,
    pub plugin_id: u32,
    pub client_id: u16,
    pub plugin_kind: String,
    #[serde(default)]
    pub counters: BTreeMap<String, u64>,
    #[serde(default)]
    pub spans: Vec<PluginStatsSpan>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PluginStatsRecorder {
    counters: BTreeMap<String, u64>,
    spans: BTreeMap<String, PluginStatsSpan>,
}

impl PluginStatsRecorder {
    pub fn increment(&mut self, name: impl Into<String>) {
        let name = name.into();
        *self.counters.entry(name).or_insert(0) += 1;
    }

    pub fn add(&mut self, name: impl Into<String>, value: u64) {
        let name = name.into();
        let counter = self.counters.entry(name).or_insert(0);
        *counter = counter.saturating_add(value);
    }

    pub fn record_span_elapsed(&mut self, name: impl Into<String>, started_at: Instant) {
        self.record_span_us(name, started_at.elapsed().as_micros() as u64);
    }

    pub fn record_span_us(&mut self, name: impl Into<String>, elapsed_us: u64) {
        let name = name.into();
        let entry = self.spans.entry(name.clone()).or_insert(PluginStatsSpan {
            name,
            count: 0,
            total_us: 0,
            max_us: 0,
        });
        entry.count = entry.count.saturating_add(1);
        entry.total_us = entry.total_us.saturating_add(elapsed_us);
        entry.max_us = entry.max_us.max(elapsed_us);
    }

    pub fn snapshot(
        &self,
        collection_id: u64,
        identity: RendererHello,
        plugin_kind: impl Into<String>,
    ) -> PluginStatsSnapshot {
        PluginStatsSnapshot {
            collection_id,
            plugin_id: identity.plugin_id,
            client_id: identity.client_id,
            plugin_kind: plugin_kind.into(),
            counters: self.counters.clone(),
            spans: self.spans.values().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_message_names_use_andamento_namespace() {
        let message_names = [
            MSG_RENDERER_HELLO,
            MSG_CONFIG_EDITOR_HELLO,
            MSG_REQUEST_STATE,
            MSG_VIEW_MODEL,
            MSG_TOGGLE_PIN,
            MSG_SET_SORT_MODE,
            MSG_SET_RAIL_CONFIG,
            MSG_SET_PANE_STATUS,
            MSG_CLEAR_PANE_STATUS,
            MSG_APPLY_METADATA_PATCH,
            MSG_CONTROLLER_BOOTSTRAP_REQUEST,
            MSG_CONTROLLER_BOOTSTRAP_STATE,
            MSG_OBSERVED_IDENTITIES,
            MSG_STATS_COLLECT,
            MSG_STATS_REQUEST,
            MSG_STATS_REPORT,
            MSG_RAIL_SIZE_OBSERVED,
            MSG_RAIL_SIZE_TARGET,
            MSG_SET_METADATA_VISIBILITY,
            MSG_SET_NODE_VARIABLE,
            MSG_CONFIG_INSPECT,
            MSG_MATERIALIZE_LATENT,
        ];

        let old_prefix = ["ta", "bs-"].concat();

        for message_name in message_names {
            assert!(
                message_name.starts_with("andamento-"),
                "{message_name} should use the andamento namespace"
            );
            assert!(
                !message_name.starts_with(&old_prefix),
                "{message_name} should not use the old tabs namespace"
            );
        }
    }

    #[test]
    fn priority_orders_status_importance() {
        assert!(Priority::Error > Priority::Warning);
        assert!(Priority::Warning > Priority::Waiting);
        assert!(Priority::Waiting > Priority::Info);
        assert!(Priority::Info > Priority::Idle);
    }

    #[test]
    fn external_set_pane_status_round_trips_json() {
        let message = ExternalMessage::SetPaneStatus(SetPaneStatus {
            pane_id: PaneTarget::Terminal(7),
            priority: Priority::Waiting,
            title: "Claude waiting".to_owned(),
            detail: Some("Needs approval".to_owned()),
            icon: Some(StatusIcon::PngFile(PathBuf::from("/tmp/waiting.png"))),
            timestamp_ms: Some(42),
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert!(encoded.contains(r#""icon":{"kind":"png-file","value":"/tmp/waiting.png"}"#));
        let decoded: ExternalMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn config_inspect_request_round_trips_json() {
        let request = ConfigInspectRequest {
            client_id: 4,
            origin_tab_id: WorkspaceId::from(7),
            node_key: NodeKey::Tab(WorkspaceId::from(7)),
            config_plugin_url: "andamento-config".to_owned(),
            controller_plugin_url: "andamento-controller".to_owned(),
        };

        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: ConfigInspectRequest = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, request);
    }

    #[test]
    fn plugin_registration_hello_round_trips_json() {
        let hello = PluginRegistrationHello {
            identity: RendererHello {
                plugin_id: 11,
                client_id: 4,
            },
            placement: PluginPlacement::Tab {
                tab_id: WorkspaceId::from(7),
                pane_kind: PluginPaneKind::Floating,
            },
        };

        let encoded = serde_json::to_string(&hello).unwrap();
        let decoded: PluginRegistrationHello = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, hello);
    }

    #[test]
    fn rail_config_defaults_to_joined_cells() {
        let config = RailConfig::default();

        assert_eq!(config.structure, RailStructure::JoinedCells);
    }

    #[test]
    fn plugin_stats_snapshot_round_trips_json() {
        let mut counters = BTreeMap::new();
        counters.insert("pipe.view-model".to_owned(), 3);
        let snapshot = PluginStatsSnapshot {
            collection_id: 9,
            plugin_id: 4,
            client_id: 2,
            plugin_kind: "rail".to_owned(),
            counters,
            spans: vec![PluginStatsSpan {
                name: "decode.view-model".to_owned(),
                count: 3,
                total_us: 120,
                max_us: 70,
            }],
        };

        let encoded = serde_json::to_string(&snapshot).unwrap();
        let decoded: PluginStatsSnapshot = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn stats_collect_request_round_trips_json() {
        let request = StatsCollectRequest {
            requester: RendererHello {
                plugin_id: 10,
                client_id: 1,
            },
            collection_id: 11,
        };

        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: StatsCollectRequest = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, request);
    }

    #[test]
    fn stats_recorder_accumulates_counters_and_spans() {
        let mut recorder = PluginStatsRecorder::default();

        recorder.increment("pipe.view-model");
        recorder.increment("pipe.view-model");
        recorder.add("view-model.payload-bytes", 100);
        recorder.add("view-model.payload-bytes", 250);
        recorder.record_span_us("decode.view-model", 10);
        recorder.record_span_us("decode.view-model", 25);

        let snapshot = recorder.snapshot(
            1,
            RendererHello {
                plugin_id: 2,
                client_id: 3,
            },
            "rail",
        );

        assert_eq!(snapshot.counters["pipe.view-model"], 2);
        assert_eq!(snapshot.counters["view-model.payload-bytes"], 350);
        assert_eq!(snapshot.spans[0].name, "decode.view-model");
        assert_eq!(snapshot.spans[0].count, 2);
        assert_eq!(snapshot.spans[0].total_us, 35);
        assert_eq!(snapshot.spans[0].max_us, 25);
    }

    #[test]
    fn placement_key_round_trips_as_an_ordered_loop_entity_path() {
        let key = PlacementKey(vec![
            PlacementSegment {
                loop_name: "project".to_owned(),
                entity: EntityRef {
                    kind: "project".to_owned(),
                    id: "andamento".to_owned(),
                },
            },
            PlacementSegment {
                loop_name: "vessel".to_owned(),
                entity: EntityRef {
                    kind: "vessel".to_owned(),
                    id: "andamento/work".to_owned(),
                },
            },
        ]);
        let encoded = serde_json::to_string(&key).unwrap();
        let decoded: PlacementKey = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn metadata_target_entity_round_trips_json() {
        let target = MetadataTarget::Entity(EntityRef {
            kind: "convoy".to_owned(),
            id: "dev/cutover@kiwi".to_owned(),
        });

        let encoded = serde_json::to_string(&target).unwrap();
        let decoded: MetadataTarget = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, target);
        assert_eq!(
            encoded,
            r#"{"kind":"entity","value":{"kind":"convoy","id":"dev/cutover@kiwi"}}"#
        );
    }

    #[test]
    fn metadata_target_identity_round_trips_json() {
        let target = MetadataTarget::Identity(MetadataIdentity {
            key: "git.repo".to_owned(),
            value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
        });

        let encoded = serde_json::to_string(&target).unwrap();
        let decoded: MetadataTarget = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, target);
    }

    #[test]
    fn metadata_patch_round_trips_json() {
        let patch = MetadataPatch {
            target: MetadataTarget::Entity(EntityRef {
                kind: "project".to_owned(),
                id: "dev/zellij@fleet".to_owned(),
            }),
            source_id: "flotilla".to_owned(),
            set: std::collections::BTreeMap::from([(
                "summary.local_llm".to_owned(),
                MetadataValueUpdate {
                    value: MetadataValue::Text("running tests".to_owned()),
                    ttl_ms: Some(30_000),
                    precedence: Some(10),
                    ordinal: Some(2),
                },
            )]),
            unset: vec!["old.summary".to_owned()],
        };

        let encoded = serde_json::to_string(&patch).unwrap();
        let decoded: MetadataPatch = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, patch);
    }

    #[test]
    fn external_metadata_patch_message_round_trips_json() {
        let patch = MetadataPatch {
            target: MetadataTarget::Tab(WorkspaceId::from(7)),
            source_id: "flotilla".to_owned(),
            set: std::collections::BTreeMap::from([(
                "tab.subject".to_owned(),
                MetadataValueUpdate {
                    value: MetadataValue::Text("checkout".to_owned()),
                    ttl_ms: None,
                    precedence: Some(1),
                    ordinal: None,
                },
            )]),
            unset: vec![],
        };
        let message = ExternalMessage::MetadataPatch(patch.clone());

        let encoded = serde_json::to_string(&message).unwrap();
        let decoded: ExternalMessage = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, ExternalMessage::MetadataPatch(patch));
    }

    #[test]
    fn rail_config_round_trips_json() {
        let config = RailConfig {
            structure: RailStructure::BoxPerTab,
            segment_between_color: Some(RailRgbColor {
                red: 1,
                green: 2,
                blue: 3,
            }),
        };

        let encoded = serde_json::to_string(&config).unwrap();
        let decoded: RailConfig = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, config);
    }
}

#[cfg(test)]
mod cutover_tests;

#[cfg(test)]
mod visibility_tests;
