//! Named records: the logical sidebar state Andamento owns, as bytes the host
//! stores without reading: `dashboard` ([`DashboardRecord`]) and
//! `workspace/<id>` ([`WorkspaceRecord`](crate::workspace::WorkspaceRecord)). The host decides where each record is stored and
//! when it is written; Andamento never touches the filesystem. A record is
//! its struct serialised as JSON: private to this Andamento, unversioned, and
//! free to change from one build to the next.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    sidebar_arrangement::SidebarArrangement, DisplayVariableValue, EntityRef, MetadataValue, PlacementKey, PlacementLoopKey, WorkspaceId,
};

/// The name of a record: `dashboard`, or `workspace/<id>` for a registered
/// workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordName {
    Dashboard,
    Workspace(WorkspaceId),
}

impl RecordName {
    pub fn parse(name: &str) -> Result<Self, String> {
        if name == "dashboard" {
            return Ok(Self::Dashboard);
        }
        name.strip_prefix("workspace/")
            .and_then(|id| id.parse().ok())
            .map(Self::Workspace)
            .ok_or_else(|| format!("unknown record name {name:?}"))
    }
}

impl fmt::Display for RecordName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dashboard => f.write_str("dashboard"),
            Self::Workspace(id) => write!(f, "workspace/{id}"),
        }
    }
}

/// The `dashboard` record: the Dashboard's state and its sidebar
/// arrangement.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DashboardRecord {
    #[serde(flatten)]
    pub state: DashboardState,
    /// The sidebar arrangement, once stored.
    pub sidebar: Option<SidebarArrangement>,
}

/// The Dashboard's state other than its sidebar arrangement.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DashboardState {
    /// Persisted display variables.
    pub display: BTreeMap<String, DisplayVariableValue>,
    /// Collapsed rows, by placement key.
    pub collapsed: BTreeSet<PlacementKey>,
    /// Sibling orders, by loop key.
    #[serde(with = "pairs")]
    pub orders: BTreeMap<PlacementLoopKey, Vec<EntityRef>>,
    /// Placement variables set on rows (`SetVariable`).
    #[serde(with = "pairs")]
    pub variables: BTreeMap<PlacementKey, BTreeMap<String, String>>,
    /// Local sections, groups and refs, with their facts.
    #[serde(with = "pairs")]
    pub local: BTreeMap<EntityRef, BTreeMap<String, MetadataValue>>,
}

/// The least needed to draw a row with no facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectRecord {
    pub label: Option<String>,
    /// Ended (`status="ended"`), else retained (`status="retained"`).
    pub ended: bool,
    /// Host time (the `now_ms` it supplies) when the subject's facts last
    /// appeared, changed or went away.
    pub last_seen_ms: Option<u64>,
    /// The facts the configured placement reads (match, `in`, order and
    /// visibility keys), so the row is placed on its path.
    pub facts: BTreeMap<String, MetadataValue>,
}

impl DashboardRecord {
    pub fn encode(&self) -> String {
        encode(self)
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let record: Self = decode(text, "dashboard")?;
        if let Some(sidebar) = &record.sidebar {
            sidebar.doc.check()?;
        }
        for facts in record.state.local.values() {
            if !facts.values().all(recordable) {
                return Err("local facts are text, booleans, integers or entity references".into());
            }
        }
        Ok(record)
    }
}

/// A map with structured keys as a list of pairs (JSON keys are strings).
pub(crate) mod pairs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<K: Serialize, V: Serialize, S: Serializer>(map: &BTreeMap<K, V>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(map)
    }

    pub fn deserialize<'de, K: Ord + Deserialize<'de>, V: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<K, V>, D::Error> {
        Ok(Vec::<(K, V)>::deserialize(deserializer)?.into_iter().collect())
    }
}

pub(crate) fn encode<T: Serialize>(record: &T) -> String {
    serde_json::to_string(record).expect("records serialise")
}

pub(crate) fn decode<T: DeserializeOwned>(text: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|error| format!("not a {what} record: {error}"))
}

/// Whether a fact value can be written to a record. Lists of text and group
/// paths are producer detail no placement reads.
pub fn recordable(value: &MetadataValue) -> bool {
    matches!(value, MetadataValue::Text(_) | MetadataValue::Bool(_) | MetadataValue::Integer(_) | MetadataValue::EntityRefs(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sidebar_arrangement::SidebarDoc,
        slots::{ArrangementDoc, Panel, PanelNode},
        suggested_layout::Axis,
        PlacementSegment,
    };

    fn entity(kind: &str, id: &str) -> EntityRef {
        EntityRef::new("sub-1", kind, id)
    }

    fn dashboard() -> DashboardRecord {
        let project = PlacementKey(vec![PlacementSegment { loop_name: "project".into(), entity: entity("project", "p") }]);
        let tabs = |id: &str, keys: &[&str]| Panel {
            id: id.into(),
            weight: 0.5,
            node: PanelNode::Tabs { tabs: keys.iter().map(|key| (*key).into()).collect(), selected: keys.first().map(|key| (*key).into()) },
        };
        DashboardRecord {
            state: DashboardState {
                display: BTreeMap::from([
                    ("show-finished".into(), DisplayVariableValue::Bool(true)),
                    ("mode".into(), DisplayVariableValue::Enum("wide".into())),
                ]),
                collapsed: BTreeSet::from([project.clone()]),
                orders: BTreeMap::from([(
                    PlacementLoopKey { region: "tree".into(), parent: project.clone(), binding: "vessel".into() },
                    // The same kind and ID under two providers are two entities.
                    vec![entity("vessel", "b"), EntityRef::new("sub-2", "vessel", "b")],
                )]),
                variables: BTreeMap::from([(project, BTreeMap::from([("density".into(), "compact".into())]))]),
                local: BTreeMap::from([(
                    EntityRef::local(".group", "g1"),
                    BTreeMap::from([
                        (".section".into(), MetadataValue::EntityRefs(vec![EntityRef::local(".section", "s1")])),
                        (".position".into(), MetadataValue::Integer(12)),
                    ]),
                )]),
            },
            sidebar: Some(SidebarArrangement {
                owned: true,
                doc: SidebarDoc {
                    dock: ArrangementDoc {
                        root: Some(Panel {
                            id: "1".into(),
                            weight: 1.0,
                            node: PanelNode::Split {
                                axis: Axis::Column,
                                children: vec![tabs("2", &["tree", "u:notes"]), tabs("3", &["git"])],
                            },
                        }),
                    },
                },
                closed: BTreeSet::from(["closed".into()]),
                ..SidebarArrangement::default()
            }),
        }
    }

    #[test]
    fn records_round_trip() {
        let record = dashboard();
        assert_eq!(DashboardRecord::decode(&record.encode()), Ok(record));
    }

    #[test]
    fn malformed_records_are_rejected_with_a_reason() {
        let mut bad_doc = dashboard();
        let sidebar = bad_doc.sidebar.as_mut().unwrap();
        let Some(PanelNode::Split { children, .. }) = sidebar.doc.dock.root.as_mut().map(|root| &mut root.node) else {
            unreachable!("the fixture's dock is a split")
        };
        children[1].weight = -1.0;
        let mut bad_fact = dashboard();
        bad_fact.state.local.values_mut().next().unwrap().insert("list".into(), MetadataValue::StringList(vec!["x".into()]));
        for (text, reason) in [
            ("".to_owned(), "not a dashboard record"),
            ("{}".to_owned(), "not a dashboard record: missing field"),
            (bad_doc.encode(), "weights are positive"),
            (bad_fact.encode(), "local facts are text"),
        ] {
            let error = DashboardRecord::decode(&text).unwrap_err();
            assert!(error.contains(reason), "{text}: {error}");
        }
    }

    #[test]
    fn names_parse_and_print() {
        let name = "workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b";
        for name in ["dashboard", name] {
            assert_eq!(RecordName::parse(name).unwrap().to_string(), name);
        }
        for name in ["", "workspace/", "workspace/7", "dashboards", "workspace"] {
            assert!(RecordName::parse(name).is_err(), "{name}");
        }
    }
}
