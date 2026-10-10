//! Named records: the logical sidebar state Andamento owns, as KDL inside a
//! versioned envelope. The host decides where each record is stored and when
//! it is written; Andamento never touches the filesystem.
//!
//! ```kdl
//! andamento-record "dashboard" version=1 {
//!     display "show-finished" true
//!     collapsed {
//!         at "project" "project" "p"
//!     }
//!     order "tree" "vessel" {
//!         parent {
//!             at "project" "project" "p"
//!         }
//!         entity "vessel" "b"
//!         entity "vessel" "a"
//!     }
//!     variable "density" "compact" {
//!         at "project" "project" "p"
//!     }
//!     local ".group" "g1" {
//!         fact ".section" {
//!             entity ".section" "s1"
//!         }
//!         fact "display.label" "Pinned"
//!     }
//! }
//! ```
//!
//! Nodes this version doesn't know, directly inside the envelope, are kept and
//! written back unchanged, after the known ones. Unknown properties or
//! children of known nodes are not kept: a later version that needs them adds
//! a node, or raises the version.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use crate::{
    DisplayVariableValue, EntityRef, MetadataValue, PlacementKey, PlacementLoopKey,
    PlacementSegment, WorkspaceId,
};

/// The record format version this Andamento reads and writes.
pub const RECORD_VERSION: i64 = 1;
const ENVELOPE: &str = "andamento-record";

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

/// The Dashboard's sidebar state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardRecord {
    /// Persisted display variables.
    pub display: BTreeMap<String, DisplayVariableValue>,
    /// Collapsed rows, by placement key.
    pub collapsed: BTreeSet<PlacementKey>,
    /// Sibling orders, by loop key.
    pub orders: BTreeMap<PlacementLoopKey, Vec<EntityRef>>,
    /// Placement variables set on rows (`SetVariable`).
    pub variables: BTreeMap<PlacementKey, BTreeMap<String, String>>,
    /// Local sections, groups and refs, with their facts.
    pub local: BTreeMap<EntityRef, BTreeMap<String, MetadataValue>>,
    /// Nodes this version doesn't know, as KDL text.
    pub unknown: Vec<String>,
}

/// One workspace's logical state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceRecord {
    /// The entity the workspace was opened for.
    pub subject: Option<EntityRef>,
    /// The subject and the entities on its path, as last seen, so the row can
    /// be drawn where it was when no producer publishes them.
    pub retained: BTreeMap<EntityRef, SubjectRecord>,
    /// Nodes this version doesn't know, as KDL text.
    pub unknown: Vec<String>,
}

/// The least needed to draw a row with no facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
        let mut body = Vec::new();
        for (name, value) in &self.display {
            let mut node = KdlNode::new("display");
            node.push(KdlEntry::new(name.clone()));
            node.push(KdlEntry::new(match value {
                DisplayVariableValue::Bool(value) => KdlValue::Bool(*value),
                DisplayVariableValue::Enum(value) => KdlValue::String(value.clone()),
            }));
            body.push(node);
        }
        for key in &self.collapsed {
            let mut node = KdlNode::new("collapsed");
            push_key(&mut node, key);
            body.push(node);
        }
        for (loop_key, entities) in &self.orders {
            let mut node = KdlNode::new("order");
            node.push(KdlEntry::new(loop_key.region.clone()));
            node.push(KdlEntry::new(loop_key.binding.clone()));
            let children = node.ensure_children();
            if !loop_key.parent.0.is_empty() {
                let mut parent = KdlNode::new("parent");
                push_key(&mut parent, &loop_key.parent);
                children.nodes_mut().push(parent);
            }
            for entity in entities {
                children.nodes_mut().push(entity_node("entity", entity));
            }
            body.push(node);
        }
        for (key, values) in &self.variables {
            for (name, value) in values {
                let mut node = KdlNode::new("variable");
                node.push(KdlEntry::new(name.clone()));
                node.push(KdlEntry::new(value.clone()));
                push_key(&mut node, key);
                body.push(node);
            }
        }
        for (entity, facts) in &self.local {
            let mut node = entity_node("local", entity);
            push_facts(&mut node, facts);
            body.push(node);
        }
        envelope(&RecordName::Dashboard, body, &self.unknown)
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let mut record = Self::default();
        for node in open_envelope(text, &RecordName::Dashboard)? {
            match node.name().value() {
                "display" => {
                    let name = string_arg(&node, 0)?;
                    let value = match arg(&node, 1)? {
                        KdlValue::Bool(value) => DisplayVariableValue::Bool(*value),
                        value => DisplayVariableValue::Enum(
                            value
                                .as_string()
                                .ok_or("display value must be a boolean or a string")?
                                .to_owned(),
                        ),
                    };
                    record.display.insert(name, value);
                }
                "collapsed" => {
                    record.collapsed.insert(read_key(&node)?);
                }
                "order" => {
                    let mut parent = PlacementKey::default();
                    let mut entities = Vec::new();
                    for child in children(&node) {
                        match child.name().value() {
                            "parent" => parent = read_key(child)?,
                            "entity" => entities.push(read_entity(child)?),
                            other => return Err(format!("unexpected {other:?} in order")),
                        }
                    }
                    let key = PlacementLoopKey {
                        region: string_arg(&node, 0)?,
                        parent,
                        binding: string_arg(&node, 1)?,
                    };
                    if !entities.is_empty() {
                        record.orders.insert(key, entities);
                    }
                }
                "variable" => {
                    let name = string_arg(&node, 0)?;
                    let value = string_arg(&node, 1)?;
                    record
                        .variables
                        .entry(read_key(&node)?)
                        .or_default()
                        .insert(name, value);
                }
                "local" => {
                    record.local.insert(read_entity(&node)?, read_facts(&node)?);
                }
                _ => record.unknown.push(canonical(node)),
            }
        }
        Ok(record)
    }
}

impl WorkspaceRecord {
    pub fn encode(&self, id: WorkspaceId) -> String {
        let mut body = Vec::new();
        if let Some(subject) = &self.subject {
            body.push(entity_node("subject", subject));
        }
        for (entity, subject) in &self.retained {
            let mut node = entity_node("retained", entity);
            if let Some(label) = &subject.label {
                node.insert("label", label.clone());
            }
            node.insert("status", if subject.ended { "ended" } else { "retained" });
            if let Some(seen) = subject.last_seen_ms {
                node.insert("last-seen", i64::try_from(seen).unwrap_or(i64::MAX));
            }
            push_facts(&mut node, &subject.facts);
            body.push(node);
        }
        envelope(&RecordName::Workspace(id), body, &self.unknown)
    }

    pub fn decode(text: &str, id: WorkspaceId) -> Result<Self, String> {
        let mut record = Self::default();
        for node in open_envelope(text, &RecordName::Workspace(id))? {
            match node.name().value() {
                "subject" => record.subject = Some(read_entity(&node)?),
                "retained" => {
                    let ended = match node.get("status").map(|e| e.value().as_string()) {
                        None | Some(Some("retained")) => false,
                        Some(Some("ended")) => true,
                        _ => return Err("retained status must be \"retained\" or \"ended\"".into()),
                    };
                    let label = match node.get("label") {
                        None => None,
                        Some(entry) => Some(
                            entry
                                .value()
                                .as_string()
                                .ok_or("retained label must be a string")?
                                .to_owned(),
                        ),
                    };
                    let last_seen_ms = match node.get("last-seen") {
                        None => None,
                        Some(entry) => Some(
                            entry
                                .value()
                                .as_i64()
                                .and_then(|n| u64::try_from(n).ok())
                                .ok_or("last-seen must be a nonnegative integer")?,
                        ),
                    };
                    record.retained.insert(
                        read_entity(&node)?,
                        SubjectRecord {
                            label,
                            ended,
                            last_seen_ms,
                            facts: read_facts(&node)?,
                        },
                    );
                }
                _ => record.unknown.push(canonical(node)),
            }
        }
        Ok(record)
    }
}

/// Whether a fact value can be written to a record. Lists of text and group
/// paths are producer detail no placement reads.
pub fn recordable(value: &MetadataValue) -> bool {
    matches!(
        value,
        MetadataValue::Text(_)
            | MetadataValue::Bool(_)
            | MetadataValue::Integer(_)
            | MetadataValue::EntityRefs(_)
    )
}

fn envelope(name: &RecordName, body: Vec<KdlNode>, unknown: &[String]) -> String {
    let mut node = KdlNode::new(ENVELOPE);
    node.push(KdlEntry::new(name.to_string()));
    node.insert("version", RECORD_VERSION);
    let children = node.ensure_children();
    children.nodes_mut().extend(body);
    for text in unknown {
        // Kept text came from a parsed node, so it parses again.
        if let Ok(document) = text.parse::<KdlDocument>() {
            children
                .nodes_mut()
                .extend(document.nodes().iter().cloned());
        }
    }
    let mut document = KdlDocument::new();
    document.nodes_mut().push(node);
    document.fmt();
    document.to_string()
}

/// The envelope's body, after checking its name and version. Nothing is
/// applied until the whole record has been read.
fn open_envelope(text: &str, expected: &RecordName) -> Result<Vec<KdlNode>, String> {
    let document = text
        .parse::<KdlDocument>()
        .map_err(|e| format!("record is not valid KDL: {e}"))?;
    let [node] = document.nodes() else {
        return Err(format!("a record is one {ENVELOPE} node"));
    };
    if node.name().value() != ENVELOPE {
        return Err(format!("a record is one {ENVELOPE} node"));
    }
    let version = node
        .get("version")
        .and_then(|e| e.value().as_i64())
        .ok_or("record has no integer version")?;
    if version != RECORD_VERSION {
        return Err(format!(
            "record version {version} is not supported; this Andamento reads version {RECORD_VERSION}"
        ));
    }
    let name = string_arg(node, 0)?;
    if name != expected.to_string() {
        return Err(format!("record {name:?} imported as {expected}"));
    }
    Ok(children(node).to_vec())
}

fn canonical(mut node: KdlNode) -> String {
    node.clear_fmt_recursive();
    let mut document = KdlDocument::new();
    document.nodes_mut().push(node);
    document.fmt();
    document.to_string()
}

fn children(node: &KdlNode) -> &[KdlNode] {
    node.children().map(|c| c.nodes()).unwrap_or_default()
}

fn arg(node: &KdlNode, index: usize) -> Result<&KdlValue, String> {
    node.entries()
        .iter()
        .filter(|entry| entry.name().is_none())
        .nth(index)
        .map(|entry| entry.value())
        .ok_or_else(|| format!("{} needs argument {}", node.name().value(), index + 1))
}

fn string_arg(node: &KdlNode, index: usize) -> Result<String, String> {
    arg(node, index)?
        .as_string()
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "{} argument {} must be a string",
                node.name().value(),
                index + 1
            )
        })
}

fn entity_node(name: &str, entity: &EntityRef) -> KdlNode {
    let mut node = KdlNode::new(name);
    node.push(KdlEntry::new(entity.kind.clone()));
    node.push(KdlEntry::new(entity.id.clone()));
    node
}

fn read_entity(node: &KdlNode) -> Result<EntityRef, String> {
    Ok(EntityRef {
        kind: string_arg(node, 0)?,
        id: string_arg(node, 1)?,
    })
}

/// A placement key, as `at "<loop>" "<kind>" "<id>"` children, outermost first.
fn push_key(node: &mut KdlNode, key: &PlacementKey) {
    let children = node.ensure_children();
    for segment in &key.0 {
        let mut at = entity_node("at", &segment.entity);
        at.entries_mut()
            .insert(0, KdlEntry::new(segment.loop_name.clone()));
        children.nodes_mut().push(at);
    }
}

fn read_key(node: &KdlNode) -> Result<PlacementKey, String> {
    children(node)
        .iter()
        .map(|at| {
            if at.name().value() != "at" {
                return Err(format!(
                    "unexpected {:?} in a placement key",
                    at.name().value()
                ));
            }
            Ok(PlacementSegment {
                loop_name: string_arg(at, 0)?,
                entity: EntityRef {
                    kind: string_arg(at, 1)?,
                    id: string_arg(at, 2)?,
                },
            })
        })
        .collect::<Result<_, _>>()
        .map(PlacementKey)
}

/// Facts as `fact "<key>" <value>` children; entity references as `entity`
/// children of the fact.
fn push_facts(node: &mut KdlNode, facts: &BTreeMap<String, MetadataValue>) {
    if facts.values().all(|value| !recordable(value)) {
        return;
    }
    let children = node.ensure_children();
    for (key, value) in facts {
        let mut fact = KdlNode::new("fact");
        fact.push(KdlEntry::new(key.clone()));
        match value {
            MetadataValue::Text(text) => fact.push(KdlEntry::new(text.clone())),
            MetadataValue::Bool(flag) => fact.push(KdlEntry::new(*flag)),
            MetadataValue::Integer(number) => fact.push(KdlEntry::new(*number)),
            MetadataValue::EntityRefs(refs) => {
                let list = fact.ensure_children();
                for entity in refs {
                    list.nodes_mut().push(entity_node("entity", entity));
                }
            }
            MetadataValue::StringList(_) | MetadataValue::GroupPath(_) => continue,
        }
        children.nodes_mut().push(fact);
    }
}

fn read_facts(node: &KdlNode) -> Result<BTreeMap<String, MetadataValue>, String> {
    let mut facts = BTreeMap::new();
    for fact in children(node) {
        if fact.name().value() != "fact" {
            return Err(format!("unexpected {:?} among facts", fact.name().value()));
        }
        let key = string_arg(fact, 0)?;
        let value = if let Some(list) = fact.children() {
            MetadataValue::EntityRefs(
                list.nodes()
                    .iter()
                    .map(|entity| {
                        if entity.name().value() != "entity" {
                            return Err("an entity-reference fact lists entity nodes".to_owned());
                        }
                        read_entity(entity)
                    })
                    .collect::<Result<_, _>>()?,
            )
        } else {
            match arg(fact, 1)? {
                KdlValue::Bool(flag) => MetadataValue::Bool(*flag),
                value if value.is_i64_value() => {
                    MetadataValue::Integer(value.as_i64().expect("an integer value"))
                }
                value => MetadataValue::Text(
                    value
                        .as_string()
                        .ok_or("a fact is text, a boolean, an integer or entity references")?
                        .to_owned(),
                ),
            }
        };
        facts.insert(key, value);
    }
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(kind: &str, id: &str) -> EntityRef {
        EntityRef {
            kind: kind.into(),
            id: id.into(),
        }
    }
    fn key(segments: &[(&str, &str, &str)]) -> PlacementKey {
        PlacementKey(
            segments
                .iter()
                .map(|(loop_name, kind, id)| PlacementSegment {
                    loop_name: (*loop_name).into(),
                    entity: entity(kind, id),
                })
                .collect(),
        )
    }

    fn dashboard() -> DashboardRecord {
        let project = key(&[("project", "project", "p")]);
        DashboardRecord {
            display: BTreeMap::from([
                ("show-finished".into(), DisplayVariableValue::Bool(true)),
                (
                    "mode".into(),
                    DisplayVariableValue::Enum("wide \"one\"".into()),
                ),
            ]),
            collapsed: BTreeSet::from([project.clone(), key(&[])]),
            orders: BTreeMap::from([
                (
                    PlacementLoopKey {
                        region: "tree".into(),
                        parent: project.clone(),
                        binding: "vessel".into(),
                    },
                    vec![entity("vessel", "b"), entity("vessel", "a")],
                ),
                (
                    PlacementLoopKey {
                        region: "tree".into(),
                        parent: PlacementKey::default(),
                        binding: "project".into(),
                    },
                    vec![entity("project", "p")],
                ),
            ]),
            variables: BTreeMap::from([(
                project,
                BTreeMap::from([("density".into(), "compact".into())]),
            )]),
            local: BTreeMap::from([(
                entity(".group", "g1"),
                BTreeMap::from([
                    (
                        ".section".into(),
                        MetadataValue::EntityRefs(vec![entity(".section", "s1")]),
                    ),
                    (".default".into(), MetadataValue::Bool(true)),
                    ("display.label".into(), MetadataValue::Text("12".into())),
                    (".position".into(), MetadataValue::Integer(12)),
                    ("empty".into(), MetadataValue::EntityRefs(vec![])),
                ]),
            )]),
            unknown: vec![],
        }
    }

    #[test]
    fn dashboard_round_trips() {
        let record = dashboard();
        let text = record.encode();
        assert_eq!(DashboardRecord::decode(&text).unwrap(), record, "{text}");
        assert_eq!(DashboardRecord::decode(&text).unwrap().encode(), text);
    }

    #[test]
    fn workspace_round_trips() {
        let id: WorkspaceId = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b".parse().unwrap();
        let record = WorkspaceRecord {
            subject: Some(entity("vessel", "v")),
            retained: BTreeMap::from([
                (
                    entity("vessel", "v"),
                    SubjectRecord {
                        label: Some("Worker".into()),
                        ended: true,
                        last_seen_ms: Some(102),
                        facts: BTreeMap::from([(
                            "flotilla.project".into(),
                            MetadataValue::Text("p".into()),
                        )]),
                    },
                ),
                (entity("project", "p"), SubjectRecord::default()),
            ]),
            unknown: vec![],
        };
        let text = record.encode(id);
        assert!(text.starts_with(
            "andamento-record \"workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b\" version=1"
        ));
        assert_eq!(
            WorkspaceRecord::decode(&text, id).unwrap(),
            record,
            "{text}"
        );
        // A record for one workspace is not another's.
        assert!(WorkspaceRecord::decode(&text, WorkspaceId::from(7)).is_err());
    }

    #[test]
    fn unknown_nodes_survive_a_round_trip() {
        let text = dashboard().encode();
        let extended = text.replacen(
            "{\n",
            "{\n    future-thing \"x\" mode=2 {\n        nested 1\n    }\n",
            1,
        );
        let record = DashboardRecord::decode(&extended).unwrap();
        assert_eq!(record.unknown.len(), 1);
        let exported = record.encode();
        assert!(exported.contains("future-thing \"x\" mode=2"), "{exported}");
        assert!(exported.contains("nested 1"), "{exported}");
        assert_eq!(DashboardRecord::decode(&exported).unwrap(), record);
        assert_eq!(
            DashboardRecord::decode(&exported).unwrap().encode(),
            exported
        );
    }

    #[test]
    fn other_versions_names_and_shapes_are_rejected() {
        for text in [
            "andamento-record \"dashboard\" version=2",
            "andamento-record \"dashboard\"",
            "andamento-record \"workspace/7\" version=1",
            "something-else \"dashboard\" version=1",
            "andamento-record \"dashboard\" version=1\nandamento-record \"dashboard\" version=1",
            "andamento-record \"dashboard\" version=1 { display \"x\"; }",
            "andamento-record \"dashboard\" version=1 { collapsed { nope; }; }",
            "not { kdl",
        ] {
            assert!(DashboardRecord::decode(text).is_err(), "{text}");
        }
        assert!(DashboardRecord::decode("andamento-record \"dashboard\" version=1").is_ok());
    }

    #[test]
    fn names_parse_and_print() {
        for name in [
            "dashboard",
            "workspace/7",
            "workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b",
        ] {
            assert_eq!(RecordName::parse(name).unwrap().to_string(), name);
        }
        for name in ["", "workspace/", "workspace/x", "dashboards", "workspace"] {
            assert!(RecordName::parse(name).is_err(), "{name}");
        }
    }
}
