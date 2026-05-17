use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MSG_RENDERER_HELLO: &str = "tabs-renderer-hello";
pub const MSG_CONFIG_EDITOR_HELLO: &str = "tabs-config-editor-hello";
pub const MSG_REQUEST_STATE: &str = "tabs-request-state";
pub const MSG_VIEW_MODEL: &str = "tabs-view-model";
pub const MSG_TOGGLE_PIN: &str = "tabs-toggle-pin";
pub const MSG_SET_SORT_MODE: &str = "tabs-set-sort-mode";
pub const MSG_SET_RAIL_CONFIG: &str = "tabs-set-rail-config";
pub const MSG_SET_PANE_STATUS: &str = "tabs-set-pane-status";
pub const MSG_CLEAR_PANE_STATUS: &str = "tabs-clear-pane-status";
pub const MSG_CONTROLLER_BOOTSTRAP_REQUEST: &str = "tabs-controller-bootstrap-request";
pub const MSG_CONTROLLER_BOOTSTRAP_STATE: &str = "tabs-controller-bootstrap-state";

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererHello {
    pub plugin_id: u32,
    pub client_id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerBootstrapSnapshot {
    pub sort_mode: SortMode,
    pub config: RailConfig,
    pub pinned_tabs: Vec<u64>,
    pub pane_statuses: Vec<SetPaneStatus>,
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GroupPath(pub Vec<GroupSegment>);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GroupSegment {
    pub key: String,
    pub value: MetadataValue,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum MetadataTarget {
    Pane(PaneTarget),
    Tab(u64),
    Group(GroupPath),
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
    },
    Tab {
        tab: TabCard,
        indent: usize,
    },
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
    pub tabs: Vec<TabCard>,
    #[serde(default)]
    pub rows: Vec<RailRow>,
    #[serde(default)]
    pub resolved_metadata: Vec<ResolvedMetadata>,
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
            }],
            rows: vec![],
            resolved_metadata: vec![],
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
    fn controller_view_model_with_group_rows_round_trips_json() {
        let group_path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
        }]);
        let model = ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig {
                structure: RailStructure::JoinedCells,
                sizing: RailSizingPreset::Compact,
                grouping: RailGroupingMode::Directory,
                view: RailViewMode::Normal,
            },
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
            }],
            rows: vec![
                RailRow::GroupHeader {
                    group_id: "cwd:/Users/robert/dev/zellij".to_owned(),
                    path: group_path.clone(),
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
                        grouping: Some(TabGroupingInfo {
                            key: "cwd:/Users/robert/dev/zellij".to_owned(),
                            path: group_path,
                            label: "zellij".to_owned(),
                            full_label: "/Users/robert/dev/zellij".to_owned(),
                        }),
                    },
                    indent: 2,
                },
            ],
            resolved_metadata: vec![],
        };

        let encoded = serde_json::to_string(&model).unwrap();
        let decoded: ControllerViewModel = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, model);
    }

    #[test]
    fn group_path_round_trips_json() {
        let path = GroupPath(vec![
            GroupSegment {
                key: "project.name".to_owned(),
                value: MetadataValue::Text("zellij".to_owned()),
            },
            GroupSegment {
                key: "zellij.pane.cwd".to_owned(),
                value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
            },
        ]);

        let encoded = serde_json::to_string(&path).unwrap();
        let decoded: GroupPath = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, path);
    }

    #[test]
    fn metadata_target_group_round_trips_json() {
        let target = MetadataTarget::Group(GroupPath(vec![GroupSegment {
            key: "project.name".to_owned(),
            value: MetadataValue::Text("zellij".to_owned()),
        }]));

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
