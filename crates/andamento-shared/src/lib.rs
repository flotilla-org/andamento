use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub mod grouping_config;
pub mod template_config;

pub const MSG_RENDERER_HELLO: &str = "tabs-renderer-hello";
pub const MSG_CONFIG_EDITOR_HELLO: &str = "tabs-config-editor-hello";
pub const MSG_REQUEST_STATE: &str = "tabs-request-state";
pub const MSG_VIEW_MODEL: &str = "tabs-view-model";
pub const MSG_TOGGLE_PIN: &str = "tabs-toggle-pin";
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
    pub pinned_tabs: Vec<u64>,
    pub pane_statuses: Vec<SetPaneStatus>,
    #[serde(default)]
    pub metadata_patches: Vec<MetadataPatch>,
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
    pub tab_id: u64,
    pub position: usize,
    pub name: String,
    pub active: bool,
    pub pinned: bool,
    pub status: Option<TabStatusSummary>,
    #[serde(default)]
    pub grouping: Option<TabGroupingInfo>,
    #[serde(default)]
    pub templates: ResolvedTemplateSlots,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabGroupingInfo {
    pub key: String,
    #[serde(default)]
    pub path: GroupPath,
    pub label: String,
    pub full_label: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RailSizingPreset {
    Compact,
    Large,
    ActiveLarge,
    PinnedLarge,
}

impl Default for RailSizingPreset {
    fn default() -> Self {
        Self::ActiveLarge
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum MetadataValue {
    Text(String),
    Bool(bool),
    Integer(i64),
    StringList(Vec<String>),
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

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GroupPath(pub Vec<GroupSegment>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSegment {
    pub key: String,
    pub value: MetadataValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl PartialEq for GroupSegment {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.value == other.value
    }
}

impl Eq for GroupSegment {}

impl PartialOrd for GroupSegment {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GroupSegment {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.value.cmp(&other.value))
    }
}

impl std::hash::Hash for GroupSegment {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key.hash(state);
        self.value.hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MetadataIdentity {
    pub key: String,
    pub value: MetadataValue,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum MetadataTarget {
    Pane(PaneTarget),
    Tab(u64),
    Group(GroupPath),
    Identity(MetadataIdentity),
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
    #[serde(default)]
    pub set: BTreeMap<String, MetadataValueUpdate>,
    #[serde(default)]
    pub unset: Vec<String>,
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
    pub target: MetadataTarget,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RailViewMode {
    Normal,
    Metadata,
}

impl Default for RailViewMode {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RailRow {
    GroupHeader {
        group_id: String,
        #[serde(default)]
        path: GroupPath,
        label: String,
        full_label: String,
        tab_count: usize,
        #[serde(default)]
        templates: ResolvedTemplateSlots,
    },
    Tab {
        tab_id: u64,
        indent: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_path: Option<GroupPath>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTemplateSlots {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_header: Option<ResolvedTemplateSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<ResolvedTemplateSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_status: Option<ResolvedTemplateSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTemplateSlot {
    pub template_name: String,
    #[serde(default)]
    pub fields: Vec<ResolvedTemplateField>,
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
pub struct RailConfig {
    #[serde(default)]
    pub structure: RailStructure,
    #[serde(default)]
    pub sizing: RailSizingPreset,
    #[serde(default)]
    pub grouping: RailGroupingMode,
    #[serde(default)]
    pub view: RailViewMode,
}

impl Default for RailConfig {
    fn default() -> Self {
        Self {
            structure: RailStructure::default(),
            sizing: RailSizingPreset::default(),
            grouping: RailGroupingMode::default(),
            view: RailViewMode::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerViewModel {
    pub sort_mode: SortMode,
    pub config: RailConfig,
    #[serde(default)]
    pub template_config: TemplateConfigDiagnostics,
    pub tabs: Vec<TabCard>,
    #[serde(default)]
    pub rows: Vec<RailRow>,
    #[serde(default)]
    pub resolved_metadata: Vec<ResolvedMetadata>,
    #[serde(default)]
    pub observed_identities: Vec<ObservedMetadataIdentity>,
}

impl ControllerViewModel {
    pub fn tab_by_id(&self, tab_id: u64) -> Option<&TabCard> {
        self.tabs.iter().find(|tab| tab.tab_id == tab_id)
    }

    pub fn tab_for_row(&self, row: &RailRow) -> Option<&TabCard> {
        match row {
            RailRow::Tab { tab_id, .. } => self.tab_by_id(*tab_id),
            RailRow::GroupHeader { .. } => None,
        }
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
    fn controller_view_model_round_trips_json() {
        let model = ControllerViewModel {
            sort_mode: SortMode::Controller,
            config: RailConfig::default(),
            template_config: TemplateConfigDiagnostics::default(),
            tabs: vec![TabCard {
                tab_id: 10,
                position: 0,
                name: "work".to_owned(),
                active: true,
                pinned: true,
                status: Some(TabStatusSummary {
                    priority: Priority::Error,
                    title: "Tests failed".to_owned(),
                    detail: None,
                    icon: Some(StatusIcon::PngFile(PathBuf::from("/tmp/error.png"))),
                    source_pane: PaneTarget::Terminal(3),
                }),
                grouping: None,
                templates: ResolvedTemplateSlots::default(),
            }],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![ObservedMetadataIdentity {
                identity: MetadataIdentity {
                    key: "git.repo".to_owned(),
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                },
                target_count: 2,
                nearest_distance: 1,
            }],
        };
        let encoded = serde_json::to_string(&model).unwrap();
        let decoded: ControllerViewModel = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, model);
    }

    #[test]
    fn rail_config_defaults_to_joined_active_large() {
        let config = RailConfig::default();

        assert_eq!(config.structure, RailStructure::JoinedCells);
        assert_eq!(config.sizing, RailSizingPreset::ActiveLarge);
    }

    #[test]
    fn rail_config_defaults_to_directory_grouping_off() {
        let config = RailConfig::default();

        assert_eq!(config.grouping, RailGroupingMode::None);
    }

    #[test]
    fn rail_config_defaults_to_normal_view() {
        let config = RailConfig::default();

        assert_eq!(config.view, RailViewMode::Normal);
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
    fn controller_view_model_with_group_rows_round_trips_json() {
        let group_path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
            label: None,
        }]);
        let model = ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig {
                structure: RailStructure::JoinedCells,
                sizing: RailSizingPreset::Compact,
                grouping: RailGroupingMode::Directory,
                view: RailViewMode::Normal,
            },
            template_config: TemplateConfigDiagnostics::default(),
            tabs: vec![TabCard {
                tab_id: 1,
                position: 0,
                name: "server".to_owned(),
                active: true,
                pinned: false,
                status: None,
                grouping: Some(TabGroupingInfo {
                    key: "cwd:/Users/robert/dev/zellij".to_owned(),
                    path: group_path.clone(),
                    label: "zellij".to_owned(),
                    full_label: "/Users/robert/dev/zellij".to_owned(),
                }),
                templates: ResolvedTemplateSlots::default(),
            }],
            rows: vec![
                RailRow::GroupHeader {
                    group_id: "cwd:/Users/robert/dev/zellij".to_owned(),
                    path: group_path.clone(),
                    label: "zellij".to_owned(),
                    full_label: "/Users/robert/dev/zellij".to_owned(),
                    tab_count: 1,
                    templates: ResolvedTemplateSlots::default(),
                },
                RailRow::Tab {
                    tab_id: 1,
                    indent: 2,
                    parent_path: Some(group_path.clone()),
                },
            ],
            resolved_metadata: vec![],
            observed_identities: vec![],
        };

        let encoded = serde_json::to_string(&model).unwrap();
        let decoded: ControllerViewModel = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, model);
        assert_eq!(
            decoded
                .tab_for_row(&decoded.rows[1])
                .map(|tab| tab.name.as_str()),
            Some("server")
        );
    }

    #[test]
    fn group_path_round_trips_json() {
        let path = GroupPath(vec![
            GroupSegment {
                key: "project.name".to_owned(),
                value: MetadataValue::Text("zellij".to_owned()),
                label: None,
            },
            GroupSegment {
                key: "zellij.pane.cwd".to_owned(),
                value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
                label: None,
            },
        ]);

        let encoded = serde_json::to_string(&path).unwrap();
        let decoded: GroupPath = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, path);
    }

    #[test]
    fn group_path_identity_ignores_display_labels() {
        let identity = GroupPath(vec![GroupSegment {
            key: "git.repo".to_owned(),
            value: MetadataValue::Text("zellij-org/zellij".to_owned()),
            label: None,
        }]);
        let labelled = GroupPath(vec![GroupSegment {
            key: "git.repo".to_owned(),
            value: MetadataValue::Text("zellij-org/zellij".to_owned()),
            label: Some("zellij".to_owned()),
        }]);

        assert_eq!(identity, labelled);
    }

    #[test]
    fn metadata_value_group_path_round_trips_json() {
        let value = MetadataValue::GroupPath(vec![MetadataPathSegmentValue {
            key: "git.repo".to_owned(),
            value: MetadataPathValue::Text("flotilla-org/flotilla".to_owned()),
            label: Some("flotilla".to_owned()),
        }]);

        let encoded = serde_json::to_string(&value).unwrap();
        let decoded: MetadataValue = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, value);
        assert_eq!(
            encoded,
            r#"{"type":"group-path","value":[{"key":"git.repo","value":{"type":"text","value":"flotilla-org/flotilla"},"label":"flotilla"}]}"#
        );
    }

    #[test]
    fn metadata_target_group_round_trips_json() {
        let target = MetadataTarget::Group(GroupPath(vec![GroupSegment {
            key: "project.name".to_owned(),
            value: MetadataValue::Text("zellij".to_owned()),
            label: None,
        }]));

        let encoded = serde_json::to_string(&target).unwrap();
        let decoded: MetadataTarget = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, target);
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
            target: MetadataTarget::Group(GroupPath(vec![GroupSegment {
                key: "project.name".to_owned(),
                value: MetadataValue::Text("zellij".to_owned()),
                label: None,
            }])),
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
            target: MetadataTarget::Tab(7),
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
    fn parses_hierarchical_grouping_rules_from_kdl() {
        let config = crate::grouping_config::parse_grouping_config_kdl(
            r#"
            version 1

            grouping "proj-repo-branch" {
              priority 100
              level key="andamento.project" optional=true
              level key="git.repo" label-key="repo.name"
              level key="git.branch"
            }

            grouping "directory" priority=10 {
              level key="zellij.pane.cwd"
            }
            "#,
        )
        .expect("grouping config parses");

        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].name, "proj-repo-branch");
        assert_eq!(config.rules[0].priority, 100);
        assert_eq!(config.rules[0].levels[0].key, "andamento.project");
        assert!(config.rules[0].levels[0].optional);
        assert_eq!(
            config.rules[0].levels[1].label_key.as_deref(),
            Some("repo.name")
        );
        assert_eq!(config.rules[1].name, "directory");
        assert_eq!(config.rules[1].priority, 10);
    }

    #[test]
    fn rail_config_round_trips_json() {
        let config = RailConfig {
            structure: RailStructure::BoxPerTab,
            sizing: RailSizingPreset::Compact,
            grouping: RailGroupingMode::None,
            view: RailViewMode::Metadata,
        };

        let encoded = serde_json::to_string(&config).unwrap();
        let decoded: RailConfig = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, config);
    }
}
