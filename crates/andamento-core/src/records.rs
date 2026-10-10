//! Named records: the logical sidebar state Andamento owns, as KDL inside a
//! versioned envelope. The host decides where each record is stored and when
//! it is written; Andamento never touches the filesystem.
//!
//! ```kdl
//! andamento-record "dashboard" version=2 {
//!     display "show-finished" true
//!     collapsed {
//!         at "project" "project" "p" provider="sub-1"
//!     }
//!     order "tree" "vessel" {
//!         parent {
//!             at "project" "project" "p" provider="sub-1"
//!         }
//!         entity "vessel" "b" provider="sub-1"
//!         entity "vessel" "a" provider="sub-1"
//!     }
//!     variable "density" "compact" {
//!         at "project" "project" "p" provider="sub-1"
//!     }
//!     local ".group" "g1" provider="local" {
//!         fact ".section" {
//!             entity ".section" "s1" provider="local"
//!         }
//!         fact "display.label" "Pinned"
//!     }
//! }
//! ```
//!
//! Every entity names its provider, the Dashboard subscription its facts
//! come from (`local` for Andamento's own). Version 1 records, written before
//! entities had providers, still import: their sections, groups and refs
//! (`.section`, `.group`, `.ref`) get the `local` provider and every other
//! entity gets the sidebar's default provider, which is the provider ABI 2
//! calls stamp, so migrated keys match the facts a host still publishes the
//! old way. Export always writes the current version.
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
    sidebar_arrangement::{SidebarArrangement, SidebarDoc},
    slots::{
        ArrangementDoc, Baseline, ContentChange, ContentEdit, EditSet, Panel, PanelNode, SlotDef,
        SlotEdit, SoftOverride, StoredArrangement, WorkspaceSlots,
    },
    suggested_layout::{
        Axis, BaselineVersion, CommandLine, Content, LocalRecipe, RebindPolicy, ViewSpec,
    },
    target_resolution::{PortableResolution, SavedResolution, SavedResolutions},
    DisplayVariableValue, EntityRef, MetadataValue, PlacementKey, PlacementLoopKey,
    PlacementSegment, WorkspaceId,
};

/// The record format version this Andamento writes. It also reads versions 1
/// to 4. Version 3 adds a workspace's slots and arrangement; a version 2
/// workspace record has none. Version 4 adds the Dashboard's sidebar
/// arrangement; an earlier dashboard record has none, so it is placed by the
/// template's hints. A workspace record is unchanged in version 4. Version 5
/// makes a workspace's Workspace Overlay an explicit `overlay` edit set
/// (tombstones, rebind edits, name, mood and soft overrides) and records
/// departed slots and provider changes to an owned arrangement; a version 3
/// or 4 workspace record's `override` and `slot` nodes migrate into it. It
/// also records the template version a dashboard record was made against.
/// Version 6 adds a workspace's saved portable Target Resolutions
/// (`resolutions`); an earlier workspace record has none, and in one a node
/// of that name is kept as an unknown node. A dashboard record is unchanged
/// in version 6.
pub const RECORD_VERSION: i64 = 6;
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
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// The sidebar arrangement, once stored.
    pub sidebar: Option<SidebarArrangement>,
    /// The template version these keys were made against, if recorded.
    pub template: Option<String>,
    /// Nodes this version doesn't know, as KDL text.
    pub unknown: Vec<String>,
}

/// One workspace's logical state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceRecord {
    /// The entity the workspace was opened for.
    pub subject: Option<EntityRef>,
    /// The subject and the entities on its path, as last seen, so the row can
    /// be drawn where it was when no producer publishes them.
    pub retained: BTreeMap<EntityRef, SubjectRecord>,
    /// Its slots: the cached baseline and the Workspace Overlay's edits of
    /// it, the user's own slots among them.
    pub slots: WorkspaceSlots,
    /// Its arrangement document.
    pub arrangement: Option<StoredArrangement>,
    /// Portable Target Resolutions saved for its slots.
    pub resolutions: SavedResolutions,
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
        if let Some(template) = &self.template {
            let mut node = KdlNode::new("template");
            node.insert("version", template.clone());
            body.push(node);
        }
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
        if let Some(sidebar) = &self.sidebar {
            body.push(sidebar_node(sidebar));
        }
        envelope(&RecordName::Dashboard, body, &self.unknown)
    }

    /// Read a record. A version 1 record's entities get `default_provider`
    /// (see the module documentation).
    pub fn decode(text: &str, default_provider: &str) -> Result<Self, String> {
        let mut record = Self::default();
        let (version, nodes) = open_envelope(text, &RecordName::Dashboard)?;
        let read = Reader::new(version, default_provider);
        for node in nodes {
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
                    record.collapsed.insert(read.key(&node)?);
                }
                "order" => {
                    let mut parent = PlacementKey::default();
                    let mut entities = Vec::new();
                    for child in children(&node) {
                        match child.name().value() {
                            "parent" => parent = read.key(child)?,
                            "entity" => entities.push(read.entity(child)?),
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
                        .entry(read.key(&node)?)
                        .or_default()
                        .insert(name, value);
                }
                "local" => {
                    record.local.insert(read.entity(&node)?, read.facts(&node)?);
                }
                "sidebar" if version >= 4 => record.sidebar = Some(read_sidebar(&node)?),
                "template" if version >= 5 => {
                    record.template = Some(
                        node.get("version")
                            .and_then(|e| e.value().as_string())
                            .ok_or("a template version is a string")?
                            .to_owned(),
                    )
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
        if let Some(baseline) = &self.slots.baseline {
            let mut node = KdlNode::new("baseline");
            match &baseline.version {
                BaselineVersion::Published(version) => node.insert("version", version.clone()),
                BaselineVersion::PrimaryOnly => node.insert("primary-only", true),
            };
            let children = node.ensure_children();
            for slot in &baseline.slots {
                children.nodes_mut().push(slot_node("slot", slot));
            }
            if let Some(hint) = &baseline.hint {
                children.nodes_mut().push(doc_node("hint", hint));
            }
            body.push(node);
        }
        let edits = EditSet::new(&self.slots, self.arrangement.as_ref());
        let mut overlay = KdlNode::new("overlay");
        // The owned arrangement is the `arrangement` node below.
        push_edits(
            &mut overlay,
            &EditSet {
                arrangement: None,
                ..edits
            },
        );
        if overlay.children().is_some() {
            body.push(overlay);
        }
        if let Some(arrangement) = &self.arrangement {
            let mut node = doc_node("arrangement", &arrangement.doc);
            node.insert(
                "generation",
                i64::try_from(arrangement.generation).unwrap_or(i64::MAX),
            );
            node.insert("owned", arrangement.owned);
            if arrangement.provider_changed {
                node.insert("provider-changed", true);
            }
            for tab in &arrangement.placed {
                let mut placed = KdlNode::new("placed");
                placed.push(KdlEntry::new(tab.clone()));
                node.ensure_children().nodes_mut().push(placed);
            }
            body.push(node);
        }
        for (key, rebind) in &self.slots.departed {
            let mut node = KdlNode::new("departed");
            node.push(KdlEntry::new(key.clone()));
            node.insert("rebind", rebind_name(*rebind));
            body.push(node);
        }
        if !self.resolutions.is_empty() {
            body.push(resolutions_node(&self.resolutions));
        }
        envelope(&RecordName::Workspace(id), body, &self.unknown)
    }

    /// Read a record. A version 1 record's entities get `default_provider`
    /// (see the module documentation).
    pub fn decode(text: &str, id: WorkspaceId, default_provider: &str) -> Result<Self, String> {
        let mut record = Self::default();
        let mut soft = BTreeMap::new();
        let (version, nodes) = open_envelope(text, &RecordName::Workspace(id))?;
        let read = Reader::new(version, default_provider);
        for node in nodes {
            match node.name().value() {
                "subject" => record.subject = Some(read.entity(&node)?),
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
                        read.entity(&node)?,
                        SubjectRecord {
                            label,
                            ended,
                            last_seen_ms,
                            facts: read.facts(&node)?,
                        },
                    );
                }
                "baseline" if version >= 3 => {
                    let version = match (node.get("version"), node.get("primary-only")) {
                        (Some(entry), None) => BaselineVersion::Published(
                            entry
                                .value()
                                .as_string()
                                .ok_or("baseline version must be a string")?
                                .to_owned(),
                        ),
                        (None, Some(_)) => BaselineVersion::PrimaryOnly,
                        _ => return Err("a baseline has a version or is primary-only".into()),
                    };
                    let mut baseline = Baseline {
                        version,
                        slots: Vec::new(),
                        hint: None,
                    };
                    for child in children(&node) {
                        match child.name().value() {
                            "slot" => baseline.slots.push(read.slot(child)?),
                            "hint" => baseline.hint = Some(read_doc(child)?),
                            other => return Err(format!("unexpected {other:?} in baseline")),
                        }
                    }
                    record.slots.baseline = Some(baseline);
                }
                // Versions 3 and 4: an override set content and policy
                // together, and user slots sat in the envelope.
                "override" if (3..5).contains(&version) => {
                    let slot = read.slot(&node)?;
                    let against = children(&node)
                        .iter()
                        .find(|child| child.name().value() == "against")
                        .ok_or("an override records what it was made against")?;
                    record.slots.edits.insert(
                        slot.key,
                        SlotEdit {
                            content: Some(ContentEdit {
                                change: ContentChange::Override(slot.spec),
                                against: read.spec(against)?,
                            }),
                            rebind: Some(slot.rebind),
                        },
                    );
                }
                "slot" if (3..5).contains(&version) => record.slots.user.push(read.slot(&node)?),
                "overlay" if version >= 5 => {
                    let edits = read.edits(&node)?;
                    record.slots.edits = edits.slots;
                    record.slots.user = edits.added;
                    record.slots.name = edits.name;
                    record.slots.mood = edits.mood;
                    soft = edits.panels;
                }
                "resolutions" if version >= 6 => record.resolutions = read_resolutions(&node)?,
                "departed" if version >= 5 => {
                    record
                        .slots
                        .departed
                        .insert(string_arg(&node, 0)?, read_rebind(&node)?);
                }
                "arrangement" if version >= 3 => {
                    let generation = node
                        .get("generation")
                        .and_then(|e| e.value().as_i64())
                        .and_then(|n| u64::try_from(n).ok())
                        .filter(|n| *n > 0)
                        .ok_or("an arrangement needs a positive generation")?;
                    let owned = node
                        .get("owned")
                        .map(|e| e.value().as_bool().ok_or("owned must be a boolean"))
                        .transpose()?
                        .unwrap_or(false);
                    let placed = children(&node)
                        .iter()
                        .filter(|child| child.name().value() == "placed")
                        .map(|child| string_arg(child, 0))
                        .collect::<Result<_, _>>()?;
                    let provider_changed = node
                        .get("provider-changed")
                        .map(|e| {
                            e.value()
                                .as_bool()
                                .ok_or("provider-changed must be a boolean")
                        })
                        .transpose()?
                        .unwrap_or(false);
                    record.arrangement = Some(StoredArrangement {
                        generation,
                        owned,
                        doc: read_doc(&node)?,
                        placed,
                        soft: BTreeMap::new(),
                        provider_changed,
                    });
                }
                _ => record.unknown.push(canonical(node)),
            }
        }
        if !soft.is_empty() {
            let arrangement = record
                .arrangement
                .as_mut()
                .ok_or("soft overrides need an arrangement")?;
            if arrangement.owned {
                return Err("an owned arrangement has no soft overrides".into());
            }
            arrangement.soft = soft;
        }
        if version < 5 {
            // Normalise migrated overrides: a policy that is the baseline's
            // is no edit.
            for (key, edit) in record.slots.edits.iter_mut() {
                let slot = record.slots.baseline.as_ref().and_then(|b| b.slot(key));
                if let Some(slot) = slot {
                    if edit.rebind == Some(slot.rebind) {
                        edit.rebind = None;
                    }
                }
            }
        }
        Ok(record)
    }
}

/// A workspace's saved portable resolutions:
///
/// ```kdl
/// resolutions last-generation=3 {
///     resolution "reviewer" kind="cleat-session" generation=3 against="…" {
///         field "daemon" "D"
///         field "host" "feta"
///     }
/// }
/// ```
fn resolutions_node(resolutions: &SavedResolutions) -> KdlNode {
    let mut node = KdlNode::new("resolutions");
    node.insert(
        "last-generation",
        i64::try_from(resolutions.last_generation).unwrap_or(i64::MAX),
    );
    for (key, saved) in &resolutions.by_slot {
        let mut child = KdlNode::new("resolution");
        child.push(KdlEntry::new(key.clone()));
        child.insert("kind", saved.resolution.kind.clone());
        child.insert(
            "generation",
            i64::try_from(saved.generation).unwrap_or(i64::MAX),
        );
        child.insert("against", saved.against.clone());
        for (name, value) in &saved.resolution.fields {
            let mut field = KdlNode::new("field");
            field.push(KdlEntry::new(name.clone()));
            field.push(KdlEntry::new(value.clone()));
            child.ensure_children().nodes_mut().push(field);
        }
        node.ensure_children().nodes_mut().push(child);
    }
    node
}

fn read_resolutions(node: &KdlNode) -> Result<SavedResolutions, String> {
    let positive = |node: &KdlNode, name: &str| {
        node.get(name)
            .and_then(|e| e.value().as_i64())
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("{} needs a positive {name}", node.name().value()))
    };
    let string = |node: &KdlNode, name: &str| {
        node.get(name)
            .and_then(|e| e.value().as_string())
            .map(str::to_owned)
            .ok_or_else(|| format!("{} needs a string {name}", node.name().value()))
    };
    let mut out = SavedResolutions {
        last_generation: positive(node, "last-generation")?,
        ..SavedResolutions::default()
    };
    for child in children(node) {
        if child.name().value() != "resolution" {
            return Err(format!(
                "unexpected {:?} in resolutions",
                child.name().value()
            ));
        }
        let key = string_arg(child, 0)?;
        let mut resolution = PortableResolution {
            kind: string(child, "kind")?,
            fields: BTreeMap::new(),
        };
        for field in children(child) {
            if field.name().value() != "field" {
                return Err(format!(
                    "unexpected {:?} in resolution",
                    field.name().value()
                ));
            }
            let name = string_arg(field, 0)?;
            if resolution
                .fields
                .insert(name.clone(), string_arg(field, 1)?)
                .is_some()
            {
                return Err(format!("resolution field {name:?} is given twice"));
            }
        }
        resolution.check()?;
        let saved = SavedResolution {
            resolution,
            against: string(child, "against")?,
            generation: positive(child, "generation")?,
        };
        if saved.against.is_empty() {
            return Err("a saved resolution names the slot resolution it resolves".into());
        }
        if saved.generation > out.last_generation {
            return Err("a saved resolution's generation is past last-generation".into());
        }
        if out.by_slot.insert(key.clone(), saved).is_some() {
            return Err(format!("slot {key:?} has two saved resolutions"));
        }
    }
    Ok(out)
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
fn open_envelope(text: &str, expected: &RecordName) -> Result<(i64, Vec<KdlNode>), String> {
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
    if !(1..=RECORD_VERSION).contains(&version) {
        return Err(format!(
            "record version {version} is not supported; this Andamento reads versions 1 to {RECORD_VERSION}"
        ));
    }
    let name = string_arg(node, 0)?;
    if name != expected.to_string() {
        return Err(format!("record {name:?} imported as {expected}"));
    }
    Ok((version, children(node).to_vec()))
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
    node.insert("provider", entity.provider.clone());
    node
}

/// Reads entities, placement keys and facts of one record version.
struct Reader<'a> {
    version: i64,
    default_provider: &'a str,
}

impl<'a> Reader<'a> {
    fn new(version: i64, default_provider: &'a str) -> Self {
        Self {
            version,
            default_provider,
        }
    }

    /// An entity: its kind and ID from arguments `first` and `first + 1`,
    /// and its provider.
    fn entity_at(&self, node: &KdlNode, first: usize) -> Result<EntityRef, String> {
        let kind = string_arg(node, first)?;
        let id = string_arg(node, first + 1)?;
        let provider = match (self.version, node.get("provider")) {
            (_, Some(entry)) => entry
                .value()
                .as_string()
                .filter(|p| !p.is_empty())
                .ok_or("provider must be a nonempty string")?
                .to_owned(),
            (1, None) if crate::sidebar::LOCAL_KINDS.contains(&kind.as_str()) => {
                crate::LOCAL_PROVIDER.to_owned()
            }
            (1, None) => self.default_provider.to_owned(),
            (_, None) => return Err(format!("{} needs a provider", node.name().value())),
        };
        Ok(EntityRef::new(provider, kind, id))
    }

    fn entity(&self, node: &KdlNode) -> Result<EntityRef, String> {
        self.entity_at(node, 0)
    }

    fn key(&self, node: &KdlNode) -> Result<PlacementKey, String> {
        read_key(self, node)
    }

    fn facts(&self, node: &KdlNode) -> Result<BTreeMap<String, MetadataValue>, String> {
        read_facts(self, node)
    }
}

impl Reader<'_> {
    /// A slot: `<name> "<key>" rebind=".." presentation=".." { <content> }`.
    fn slot(&self, node: &KdlNode) -> Result<SlotDef, String> {
        Ok(SlotDef {
            key: string_arg(node, 0)?,
            spec: self.spec(node)?,
            rebind: read_rebind(node)?,
        })
    }

    /// An edit set's nodes (see [`push_edits`]).
    fn edits(&self, node: &KdlNode) -> Result<EditSet, String> {
        let mut edits = EditSet::default();
        for child in children(node) {
            match child.name().value() {
                "name" => edits.name = Some(string_arg(child, 0)?),
                "mood" => edits.mood = Some(string_arg(child, 0)?),
                "edit" => {
                    let key = string_arg(child, 0)?;
                    let rebind = child
                        .get("rebind")
                        .map(|_| read_rebind(child))
                        .transpose()?;
                    let mut change = None;
                    let mut against = None;
                    for part in children(child) {
                        let next = match part.name().value() {
                            "override" => ContentChange::Override(self.spec(part)?),
                            "tombstone" => ContentChange::Tombstone,
                            "against" => {
                                against = Some(self.spec(part)?);
                                continue;
                            }
                            other => return Err(format!("unexpected {other:?} in an edit")),
                        };
                        if change.replace(next).is_some() {
                            return Err(format!("edit {key:?} has two content changes"));
                        }
                    }
                    let content = match (change, against) {
                        (Some(change), Some(against)) => Some(ContentEdit { change, against }),
                        (None, None) => None,
                        (Some(_), None) => {
                            return Err(format!(
                                "edit {key:?} records no content it was made against"
                            ))
                        }
                        (None, Some(_)) => return Err(format!("edit {key:?} changes no content")),
                    };
                    let edit = SlotEdit { content, rebind };
                    if edit.is_empty() {
                        return Err(format!("edit {key:?} changes nothing"));
                    }
                    edits.slots.insert(key, edit);
                }
                "slot" => edits.added.push(self.slot(child)?),
                "panel" => {
                    let number = |name: &str| -> Result<Option<f64>, String> {
                        match child.get(name).map(|e| e.value()) {
                            None => Ok(None),
                            Some(KdlValue::Base10Float(n)) => Ok(Some(*n)),
                            Some(value) => value
                                .as_i64()
                                .map(|n| Some(n as f64))
                                .ok_or_else(|| format!("{name} is a number")),
                        }
                    };
                    let edit = SoftOverride {
                        weight: number("weight")?
                            .map(|w| {
                                (w.is_finite() && w > 0.0)
                                    .then_some(w)
                                    .ok_or("weights are positive")
                            })
                            .transpose()?,
                        selected: child
                            .get("selected")
                            .map(|e| {
                                e.value()
                                    .as_string()
                                    .map(str::to_owned)
                                    .ok_or("selected must be a string")
                            })
                            .transpose()?,
                    };
                    if edit.is_empty() {
                        return Err("a panel edit changes a weight or a selection".into());
                    }
                    edits.panels.insert(string_arg(child, 0)?, edit);
                }
                "arrangement" => edits.arrangement = Some(read_doc(child)?),
                other => return Err(format!("unexpected {other:?} in an overlay")),
            }
        }
        Ok(edits)
    }

    /// A View Spec: an optional `presentation` property, and one content child.
    fn spec(&self, node: &KdlNode) -> Result<ViewSpec, String> {
        let presentation = match node.get("presentation") {
            None => None,
            Some(entry) => Some(
                entry
                    .value()
                    .as_string()
                    .ok_or("presentation must be a string")?
                    .to_owned(),
            ),
        };
        let mut content = None;
        for child in children(node) {
            let cwd = || -> Result<Option<String>, String> {
                child
                    .get("cwd")
                    .map(|e| {
                        e.value()
                            .as_string()
                            .map(str::to_owned)
                            .ok_or_else(|| "cwd must be a string".to_owned())
                    })
                    .transpose()
            };
            let next = match child.name().value() {
                "facet" => Content::ProviderFacet {
                    facet: string_arg(child, 0)?,
                    entity: self.entity_at(child, 1)?,
                },
                "shell" => Content::Local(LocalRecipe::Command {
                    line: CommandLine::Shell(string_arg(child, 0)?),
                    cwd: cwd()?,
                }),
                "argv" => Content::Local(LocalRecipe::Command {
                    line: CommandLine::Argv(
                        (0..child
                            .entries()
                            .iter()
                            .filter(|e| e.name().is_none())
                            .count())
                            .map(|n| string_arg(child, n))
                            .collect::<Result<_, _>>()?,
                    ),
                    cwd: cwd()?,
                }),
                "file" => Content::Local(LocalRecipe::File {
                    path: string_arg(child, 0)?,
                }),
                "url" => Content::Local(LocalRecipe::Url {
                    url: string_arg(child, 0)?,
                }),
                "jackstay" => Content::Local(LocalRecipe::Jackstay {
                    launcher: string_arg(child, 0)?,
                    endpoint: string_arg(child, 1)?,
                }),
                "against" => continue,
                other => return Err(format!("unexpected {other:?} in a view spec")),
            };
            if content.replace(next).is_some() {
                return Err("a view spec has one content".into());
            }
        }
        Ok(ViewSpec {
            content: content.ok_or("a view spec needs content")?,
            presentation,
        })
    }
}

fn slot_node(name: &str, slot: &SlotDef) -> KdlNode {
    let mut node = KdlNode::new(name);
    node.push(KdlEntry::new(slot.key.clone()));
    if slot.rebind != RebindPolicy::Replace {
        node.insert("rebind", rebind_name(slot.rebind));
    }
    push_spec(&mut node, &slot.spec);
    node
}

fn rebind_name(rebind: RebindPolicy) -> &'static str {
    match rebind {
        RebindPolicy::Replace => "replace",
        RebindPolicy::KeepPrevious => "keep-previous",
        RebindPolicy::Ask => "ask",
    }
}

fn read_rebind(node: &KdlNode) -> Result<RebindPolicy, String> {
    match node.get("rebind").map(|e| e.value().as_string()) {
        None | Some(Some("replace")) => Ok(RebindPolicy::Replace),
        Some(Some("keep-previous")) => Ok(RebindPolicy::KeepPrevious),
        Some(Some("ask")) => Ok(RebindPolicy::Ask),
        _ => Err("rebind must be \"replace\", \"keep-previous\" or \"ask\"".into()),
    }
}

/// An edit set as children of `node`, in a fixed order:
///
/// ```kdl
/// name "Review"
/// mood "focused"
/// edit "reviewer" rebind="ask" {
///     override { argv "htop" }
///     against presentation="terminal" { facet "terminal" "vessel" "r" provider="sub-1" }
/// }
/// edit "logs" { tombstone; against { shell "tail -f log" } }
/// slot "u:1" presentation="web" { url "https://example.com" }
/// panel "main" weight=0.6 selected="reviewer"
/// arrangement { split "main" axis="row" { ... } }
/// ```
fn push_edits(node: &mut KdlNode, edits: &EditSet) {
    let mut out = Vec::new();
    for (name, value) in [("name", &edits.name), ("mood", &edits.mood)] {
        if let Some(value) = value {
            let mut child = KdlNode::new(name);
            child.push(KdlEntry::new(value.clone()));
            out.push(child);
        }
    }
    for (key, edit) in &edits.slots {
        let mut child = KdlNode::new("edit");
        child.push(KdlEntry::new(key.clone()));
        if let Some(rebind) = edit.rebind {
            child.insert("rebind", rebind_name(rebind));
        }
        if let Some(content) = &edit.content {
            let change = match &content.change {
                ContentChange::Override(spec) => {
                    let mut change = KdlNode::new("override");
                    push_spec(&mut change, spec);
                    change
                }
                ContentChange::Tombstone => KdlNode::new("tombstone"),
            };
            let mut against = KdlNode::new("against");
            push_spec(&mut against, &content.against);
            let parts = child.ensure_children().nodes_mut();
            parts.push(change);
            parts.push(against);
        }
        out.push(child);
    }
    for slot in &edits.added {
        out.push(slot_node("slot", slot));
    }
    for (id, edit) in &edits.panels {
        let mut child = KdlNode::new("panel");
        child.push(KdlEntry::new(id.clone()));
        if let Some(weight) = edit.weight {
            child.insert("weight", weight);
        }
        if let Some(selected) = &edit.selected {
            child.insert("selected", selected.clone());
        }
        out.push(child);
    }
    if let Some(arrangement) = &edits.arrangement {
        out.push(doc_node("arrangement", arrangement));
    }
    if !out.is_empty() {
        node.ensure_children().nodes_mut().extend(out);
    }
}

/// A workspace's edit set as an Overlay Sync proposal: the edits and the
/// baseline version they apply to. No sync protocol reads it yet.
///
/// ```kdl
/// overlay-proposal workspace="01920a6b-..." baseline="2" {
///     subject "vessel" "v" provider="sub-1"
///     edit "logs" { tombstone; against { shell "tail -f log" } }
///     panel "main" weight=0.6
/// }
/// ```
///
/// `baseline` is the Suggested Layout's `layout.version`; a baseline of
/// only the primary facts is `primary-only=true`, and a workspace with no
/// Suggested Layout has neither (an empty baseline).
pub fn encode_proposal(
    workspace: WorkspaceId,
    subject: Option<&EntityRef>,
    edits: &EditSet,
) -> String {
    let mut node = KdlNode::new("overlay-proposal");
    node.insert("workspace", workspace.to_string());
    match &edits.baseline {
        Some(BaselineVersion::Published(version)) => {
            node.insert("baseline", version.clone());
        }
        Some(BaselineVersion::PrimaryOnly) => {
            node.insert("primary-only", true);
        }
        None => {}
    }
    if let Some(subject) = subject {
        node.ensure_children()
            .nodes_mut()
            .push(entity_node("subject", subject));
    }
    push_edits(&mut node, edits);
    canonical(node)
}

/// Read a proposal written by [`encode_proposal`], for tests and tools.
pub fn decode_proposal(text: &str) -> Result<(WorkspaceId, EditSet), String> {
    let doc: KdlDocument = text.parse().map_err(|e| format!("{e}"))?;
    let [node] = doc.nodes() else {
        return Err("a proposal is one overlay-proposal node".into());
    };
    if node.name().value() != "overlay-proposal" {
        return Err("a proposal is one overlay-proposal node".into());
    }
    let workspace = node
        .get("workspace")
        .and_then(|e| e.value().as_string())
        .and_then(|id| id.parse().ok())
        .ok_or("a proposal names its workspace")?;
    let mut body = node.clone();
    if let Some(children) = body.children_mut() {
        children
            .nodes_mut()
            .retain(|child| child.name().value() != "subject");
    }
    let mut edits = Reader::new(RECORD_VERSION, "").edits(&body)?;
    edits.baseline = match (node.get("baseline"), node.get("primary-only")) {
        (Some(entry), None) => Some(BaselineVersion::Published(
            entry
                .value()
                .as_string()
                .ok_or("a baseline version is a string")?
                .to_owned(),
        )),
        (None, Some(_)) => Some(BaselineVersion::PrimaryOnly),
        (None, None) => None,
        _ => return Err("a proposal has one baseline".into()),
    };
    Ok((workspace, edits))
}

fn push_spec(node: &mut KdlNode, spec: &ViewSpec) {
    if let Some(presentation) = &spec.presentation {
        node.insert("presentation", presentation.clone());
    }
    let content = match &spec.content {
        Content::ProviderFacet { entity, facet } => {
            let mut content = entity_node("facet", entity);
            content
                .entries_mut()
                .insert(0, KdlEntry::new(facet.clone()));
            content
        }
        Content::Local(LocalRecipe::Command { line, cwd }) => {
            let mut content = match line {
                CommandLine::Shell(shell) => {
                    let mut content = KdlNode::new("shell");
                    content.push(KdlEntry::new(shell.clone()));
                    content
                }
                CommandLine::Argv(argv) => {
                    let mut content = KdlNode::new("argv");
                    for arg in argv {
                        content.push(KdlEntry::new(arg.clone()));
                    }
                    content
                }
            };
            if let Some(cwd) = cwd {
                content.insert("cwd", cwd.clone());
            }
            content
        }
        Content::Local(LocalRecipe::File { path }) => {
            let mut content = KdlNode::new("file");
            content.push(KdlEntry::new(path.clone()));
            content
        }
        Content::Local(LocalRecipe::Url { url }) => {
            let mut content = KdlNode::new("url");
            content.push(KdlEntry::new(url.clone()));
            content
        }
        Content::Local(LocalRecipe::Jackstay { launcher, endpoint }) => {
            let mut content = KdlNode::new("jackstay");
            content.push(KdlEntry::new(launcher.clone()));
            content.push(KdlEntry::new(endpoint.clone()));
            content
        }
    };
    node.ensure_children().nodes_mut().push(content);
}

/// An arrangement document as `<name> { <root panel> }`: `split "<id>"
/// axis="row|column" weight=1.0 { panels }` or `tabs "<id>" weight=1.0
/// selected="<slot>" { tab "<slot>" ... }`.
fn doc_node(name: &str, doc: &ArrangementDoc) -> KdlNode {
    let mut node = KdlNode::new(name);
    if let Some(root) = &doc.root {
        node.ensure_children().nodes_mut().push(panel_node(root));
    }
    node
}

fn panel_node(panel: &Panel) -> KdlNode {
    let mut node;
    match &panel.node {
        PanelNode::Split { axis, children } => {
            node = KdlNode::new("split");
            node.push(KdlEntry::new(panel.id.clone()));
            node.insert(
                "axis",
                match axis {
                    Axis::Row => "row",
                    Axis::Column => "column",
                },
            );
            node.insert("weight", panel.weight);
            let list = node.ensure_children();
            for child in children {
                list.nodes_mut().push(panel_node(child));
            }
        }
        PanelNode::Tabs { tabs, selected } => {
            node = KdlNode::new("tabs");
            node.push(KdlEntry::new(panel.id.clone()));
            node.insert("weight", panel.weight);
            if let Some(selected) = selected {
                node.insert("selected", selected.clone());
            }
            if !tabs.is_empty() {
                let list = node.ensure_children();
                for tab in tabs {
                    let mut tab_node = KdlNode::new("tab");
                    tab_node.push(KdlEntry::new(tab.clone()));
                    list.nodes_mut().push(tab_node);
                }
            }
        }
    }
    node
}

fn read_doc(node: &KdlNode) -> Result<ArrangementDoc, String> {
    let doc = ArrangementDoc {
        root: read_root(node)?,
    };
    doc.check()?;
    Ok(doc)
}

/// The one root panel among a node's children, if any, unchecked.
fn read_root(node: &KdlNode) -> Result<Option<Panel>, String> {
    let mut panels = read_panels(node)?.into_iter();
    let root = panels.next();
    if panels.next().is_some() {
        return Err("an arrangement has one root panel".into());
    }
    Ok(root)
}

/// The panels among a node's children (other than `placed`), unchecked.
fn read_panels(node: &KdlNode) -> Result<Vec<Panel>, String> {
    fn read_panel(node: &KdlNode) -> Result<Panel, String> {
        let weight = match node.get("weight").map(|e| e.value()) {
            None => 1.0,
            Some(KdlValue::Base10Float(weight)) => *weight,
            Some(value) => value
                .as_i64()
                .map(|n| n as f64)
                .ok_or("a panel weight is a number")?,
        };
        let id = string_arg(node, 0)?;
        let node = match node.name().value() {
            "split" => PanelNode::Split {
                axis: match node.get("axis").and_then(|e| e.value().as_string()) {
                    Some("row") => Axis::Row,
                    Some("column") => Axis::Column,
                    _ => return Err("a split's axis is \"row\" or \"column\"".into()),
                },
                children: children(node)
                    .iter()
                    .map(read_panel)
                    .collect::<Result<_, _>>()?,
            },
            "tabs" => PanelNode::Tabs {
                tabs: children(node)
                    .iter()
                    .map(|tab| match tab.name().value() {
                        "tab" => string_arg(tab, 0),
                        other => Err(format!("unexpected {other:?} in tabs")),
                    })
                    .collect::<Result<_, _>>()?,
                selected: node
                    .get("selected")
                    .map(|e| {
                        e.value()
                            .as_string()
                            .map(str::to_owned)
                            .ok_or("selected must be a string")
                    })
                    .transpose()?,
            },
            other => return Err(format!("unexpected {other:?} in an arrangement")),
        };
        Ok(Panel { id, weight, node })
    }
    children(node)
        .iter()
        .filter(|child| child.name().value() != "placed")
        .map(read_panel)
        .collect()
}

/// The sidebar arrangement as `sidebar generation=N owned=B { dock { <root
/// panel> }; floating { <panels> }; closed "<key>"; placed "<key>" }`.
fn sidebar_node(sidebar: &SidebarArrangement) -> KdlNode {
    let mut node = KdlNode::new("sidebar");
    node.insert(
        "generation",
        i64::try_from(sidebar.generation).unwrap_or(i64::MAX),
    );
    node.insert("owned", sidebar.owned);
    let list = node.ensure_children();
    list.nodes_mut().push(doc_node("dock", &sidebar.doc.dock));
    let mut floating = KdlNode::new("floating");
    for panel in &sidebar.doc.floating {
        floating
            .ensure_children()
            .nodes_mut()
            .push(panel_node(panel));
    }
    list.nodes_mut().push(floating);
    for (name, keys) in [("closed", &sidebar.closed), ("placed", &sidebar.placed)] {
        for key in keys {
            let mut child = KdlNode::new(name);
            child.push(KdlEntry::new(key.clone()));
            list.nodes_mut().push(child);
        }
    }
    node
}

/// A stored sidebar arrangement. A key may have two tabs: reconciling
/// removes the later ones, as it does for a host's commit.
fn read_sidebar(node: &KdlNode) -> Result<SidebarArrangement, String> {
    let generation = node
        .get("generation")
        .and_then(|e| e.value().as_i64())
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("a sidebar arrangement needs a positive generation")?;
    let owned = node
        .get("owned")
        .map(|e| e.value().as_bool().ok_or("owned must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    let mut sidebar = SidebarArrangement {
        generation,
        owned,
        ..SidebarArrangement::default()
    };
    for child in children(node) {
        match child.name().value() {
            "dock" => sidebar.doc.dock.root = read_root(child)?,
            "floating" => sidebar.doc.floating = read_panels(child)?,
            "closed" => {
                sidebar.closed.insert(string_arg(child, 0)?);
            }
            "placed" => {
                sidebar.placed.insert(string_arg(child, 0)?);
            }
            other => return Err(format!("unexpected {other:?} in the sidebar arrangement")),
        }
    }
    SidebarDoc::check(&sidebar.doc)?;
    Ok(sidebar)
}

/// A placement key, as `at "<loop>" "<kind>" "<id>" provider="<provider>"`
/// children, outermost first.
fn push_key(node: &mut KdlNode, key: &PlacementKey) {
    let children = node.ensure_children();
    for segment in &key.0 {
        let mut at = entity_node("at", &segment.entity);
        at.entries_mut()
            .insert(0, KdlEntry::new(segment.loop_name.clone()));
        children.nodes_mut().push(at);
    }
}

fn read_key(read: &Reader, node: &KdlNode) -> Result<PlacementKey, String> {
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
                entity: read.entity_at(at, 1)?,
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

fn read_facts(read: &Reader, node: &KdlNode) -> Result<BTreeMap<String, MetadataValue>, String> {
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
                        read.entity(entity)
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
        EntityRef::local(kind, id)
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
            sidebar: Some(SidebarArrangement {
                generation: 7,
                owned: true,
                doc: SidebarDoc {
                    dock: ArrangementDoc {
                        root: Some(Panel {
                            id: "1".into(),
                            weight: 1.0,
                            node: PanelNode::Split {
                                axis: Axis::Column,
                                children: vec![
                                    tabs("2", 0.25, &["tree", "u:notes"], Some("u:notes")),
                                    tabs("3", 0.75, &["123", ".section:s1"], None),
                                    tabs("4", 1.0, &[], None),
                                ],
                            },
                        }),
                    },
                    floating: vec![
                        tabs("5", 1.0, &["git"], Some("git")),
                        tabs("6", 1.0, &[".unplaced"], Some(".unplaced")),
                    ],
                },
                closed: BTreeSet::from(["closed".into(), "gone \"old\"".into()]),
                placed: BTreeSet::from(["git".into()]),
            }),
            template: Some("0123456789abcdef".into()),
            unknown: vec![],
        }
    }

    #[test]
    fn dashboard_round_trips() {
        let record = dashboard();
        let text = record.encode();
        assert_eq!(
            DashboardRecord::decode(&text, "local").unwrap(),
            record,
            "{text}"
        );
        assert_eq!(
            DashboardRecord::decode(&text, "local").unwrap().encode(),
            text
        );
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
            slots: WorkspaceSlots::default(),
            arrangement: None,
            resolutions: SavedResolutions::default(),
            unknown: vec![],
        };
        let text = record.encode(id);
        assert!(text.starts_with(
            "andamento-record \"workspace/01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b\" version=6"
        ));
        assert_eq!(
            WorkspaceRecord::decode(&text, id, "local").unwrap(),
            record,
            "{text}"
        );
        // A record for one workspace is not another's.
        assert!(WorkspaceRecord::decode(&text, WorkspaceId::from(7), "local").is_err());
    }

    fn spec(content: Content, presentation: Option<&str>) -> ViewSpec {
        ViewSpec {
            content,
            presentation: presentation.map(str::to_owned),
        }
    }

    fn tabs(id: &str, weight: f64, tabs: &[&str], selected: Option<&str>) -> Panel {
        Panel {
            id: id.into(),
            weight,
            node: PanelNode::Tabs {
                tabs: tabs.iter().map(|t| t.to_string()).collect(),
                selected: selected.map(str::to_owned),
            },
        }
    }

    /// Slots of every content kind, an override, user slots and an
    /// arrangement with a hint.
    fn workspace_with_slots() -> WorkspaceRecord {
        let convoy = EntityRef::new("sub-1", "convoy", "c");
        let notes = spec(
            Content::Local(LocalRecipe::File {
                path: "/notes \"x\".md".into(),
            }),
            Some("markdown"),
        );
        let reviewer = spec(
            Content::ProviderFacet {
                entity: EntityRef::new("sub-1", "vessel", "reviewer"),
                facet: "terminal".into(),
            },
            Some("terminal"),
        );
        let hint = ArrangementDoc {
            root: Some(Panel {
                id: "main".into(),
                weight: 1.0,
                node: PanelNode::Split {
                    axis: Axis::Row,
                    children: vec![
                        tabs("agents", 3.0, &["primary", "reviewer"], Some("reviewer")),
                        tabs("notes-view", 2.0, &["notes"], Some("notes")),
                    ],
                },
            }),
        };
        let mut arrangement = hint.clone();
        if let Some(Panel {
            node: PanelNode::Split { axis, children },
            ..
        }) = &mut arrangement.root
        {
            *axis = Axis::Column;
            children[0].weight = 0.625;
            children.push(tabs("7", 0.1, &["u:1", "u:2", "gone"], None));
        }
        WorkspaceRecord {
            subject: Some(convoy.clone()),
            retained: BTreeMap::new(),
            slots: WorkspaceSlots {
                baseline: Some(Baseline {
                    version: BaselineVersion::Published("1".into()),
                    slots: vec![
                        SlotDef {
                            key: "primary".into(),
                            spec: spec(
                                Content::ProviderFacet {
                                    entity: convoy,
                                    facet: "primary".into(),
                                },
                                None,
                            ),
                            rebind: RebindPolicy::Replace,
                        },
                        SlotDef {
                            key: "reviewer".into(),
                            spec: reviewer.clone(),
                            rebind: RebindPolicy::KeepPrevious,
                        },
                        SlotDef {
                            key: "notes".into(),
                            spec: notes.clone(),
                            rebind: RebindPolicy::Ask,
                        },
                    ],
                    hint: Some(hint),
                }),
                edits: BTreeMap::from([
                    (
                        "reviewer".into(),
                        SlotEdit {
                            content: Some(ContentEdit {
                                change: ContentChange::Override(spec(
                                    Content::Local(LocalRecipe::Command {
                                        line: CommandLine::Argv(vec!["htop".into(), "-d".into()]),
                                        cwd: Some("/srv".into()),
                                    }),
                                    None,
                                )),
                                against: reviewer,
                            }),
                            rebind: Some(RebindPolicy::Replace),
                        },
                    ),
                    (
                        "notes".into(),
                        SlotEdit {
                            content: Some(ContentEdit {
                                change: ContentChange::Tombstone,
                                against: notes.clone(),
                            }),
                            rebind: None,
                        },
                    ),
                    (
                        "primary".into(),
                        SlotEdit {
                            content: None,
                            rebind: Some(RebindPolicy::Ask),
                        },
                    ),
                ]),
                name: Some("Review \"two\"".into()),
                mood: Some("focused".into()),
                departed: BTreeMap::from([("old".into(), RebindPolicy::KeepPrevious)]),
                user: vec![
                    SlotDef {
                        key: "u:2".into(),
                        spec: spec(
                            Content::Local(LocalRecipe::Command {
                                line: CommandLine::Shell("make test".into()),
                                cwd: None,
                            }),
                            None,
                        ),
                        rebind: RebindPolicy::Replace,
                    },
                    SlotDef {
                        key: "u:1".into(),
                        spec: spec(
                            Content::Local(LocalRecipe::Url {
                                url: "https://example.com".into(),
                            }),
                            Some("web"),
                        ),
                        rebind: RebindPolicy::Replace,
                    },
                    SlotDef {
                        key: "u:3".into(),
                        spec: spec(
                            Content::Local(LocalRecipe::Jackstay {
                                launcher: "l".into(),
                                endpoint: "e".into(),
                            }),
                            None,
                        ),
                        rebind: RebindPolicy::KeepPrevious,
                    },
                ],
            },
            arrangement: Some(StoredArrangement {
                generation: 4,
                owned: true,
                doc: arrangement,
                placed: BTreeSet::from(["u:2".into()]),
                soft: BTreeMap::new(),
                provider_changed: true,
            }),
            resolutions: SavedResolutions {
                by_slot: BTreeMap::from([
                    (
                        "reviewer".into(),
                        SavedResolution {
                            resolution: PortableResolution {
                                kind: "cleat-session".into(),
                                fields: BTreeMap::from([
                                    ("host".into(), "feta".into()),
                                    ("session".into(), "S \"1\"".into()),
                                    ("daemon".into(), "D".into()),
                                ]),
                            },
                            against: "4:r#12:shell".into(),
                            generation: 5,
                        },
                    ),
                    (
                        "u:3".into(),
                        SavedResolution {
                            resolution: PortableResolution {
                                kind: "jackstay-endpoint".into(),
                                fields: BTreeMap::new(),
                            },
                            against: "0:8:jackstay".into(),
                            generation: 2,
                        },
                    ),
                ]),
                last_generation: 6,
            },
            unknown: vec![],
        }
    }

    #[test]
    fn workspace_slots_and_arrangement_round_trip() {
        let id = WorkspaceId::from(7);
        let record = workspace_with_slots();
        let text = record.encode(id);
        assert_eq!(
            WorkspaceRecord::decode(&text, id, "local").unwrap(),
            record,
            "{text}"
        );
        assert_eq!(
            WorkspaceRecord::decode(&text, id, "local")
                .unwrap()
                .encode(id),
            text
        );
        for expected in [
            r#"baseline version="1" {"#,
            r#"slot "reviewer" rebind="keep-previous" presentation="terminal" {"#,
            r#"facet "terminal" "vessel" "reviewer" provider="sub-1""#,
            r#"overlay {"#,
            r#"name "Review \"two\"""#,
            r#"mood "focused""#,
            r#"edit "notes" {"#,
            r#"tombstone"#,
            r#"edit "primary" rebind="ask""#,
            r#"edit "reviewer" rebind="replace" {"#,
            r#"override {"#,
            r#"argv "htop" "-d" cwd="/srv""#,
            r#"against presentation="terminal" {"#,
            r#"slot "u:2" {"#,
            r#"arrangement generation=4 owned=true provider-changed=true {"#,
            r#"departed "old" rebind="keep-previous""#,
            r#"split "main" axis="column" weight=1.0 {"#,
            r#"tabs "agents" weight=0.625 selected="reviewer" {"#,
            r#"tab "gone""#,
            r#"placed "u:2""#,
            r#"resolutions last-generation=6 {"#,
            r##"resolution "reviewer" kind="cleat-session" generation=5 against="4:r#12:shell" {"##,
            r#"field "daemon" "D""#,
            r#"field "session" "S \"1\"""#,
            r#"resolution "u:3" kind="jackstay-endpoint" generation=2 against="0:8:jackstay""#,
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        // Every generation given out is remembered, even with none saved.
        let mut cleared = WorkspaceRecord {
            resolutions: SavedResolutions {
                last_generation: 6,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = cleared.encode(id);
        assert!(text.contains("resolutions last-generation=6"), "{text}");
        assert_eq!(
            WorkspaceRecord::decode(&text, id, "local").unwrap(),
            cleared
        );
        cleared.resolutions.last_generation = 0;
        assert!(!cleared.encode(id).contains("resolutions"));
        // A primary-only baseline and an empty, uncommitted arrangement.
        let mut record = WorkspaceRecord::default();
        record.slots.baseline = Some(Baseline {
            version: BaselineVersion::PrimaryOnly,
            slots: vec![],
            hint: None,
        });
        record.arrangement = Some(StoredArrangement {
            generation: 1,
            ..Default::default()
        });
        let text = record.encode(id);
        assert!(!text.contains("overlay"), "{text}");
        assert_eq!(WorkspaceRecord::decode(&text, id, "local").unwrap(), record);
        // Soft overrides of an arrangement the overlay doesn't own.
        record.arrangement = Some(StoredArrangement {
            generation: 2,
            doc: ArrangementDoc {
                root: Some(tabs("p", 1.0, &["primary"], Some("primary"))),
            },
            soft: BTreeMap::from([
                (
                    "p".into(),
                    SoftOverride {
                        weight: Some(0.25),
                        selected: Some("primary".into()),
                    },
                ),
                (
                    "gone".into(),
                    SoftOverride {
                        weight: None,
                        selected: Some("x".into()),
                    },
                ),
            ]),
            ..Default::default()
        });
        let text = record.encode(id);
        assert!(
            text.contains(r#"panel "p" weight=0.25 selected="primary""#),
            "{text}"
        );
        assert_eq!(WorkspaceRecord::decode(&text, id, "local").unwrap(), record);
        // An owned arrangement has none; an edit says what it changes.
        for body in [
            r#"overlay { panel "p" weight=0.5; }; arrangement generation=1 owned=true { tabs "p"; }"#,
            r#"overlay { panel "p"; }; arrangement generation=1 { tabs "p"; }"#,
            r#"overlay { panel "p" weight=0.5; }"#,
            r#"overlay { edit "a"; }"#,
            r#"overlay { edit "a" { tombstone; }; }"#,
            r#"overlay { edit "a" { against { url "x"; }; }; }"#,
            r#"overlay { edit "a" { tombstone; override { url "y"; }; against { url "x"; }; }; }"#,
            r#"overlay { nope; }"#,
            r#"departed "a" rebind="never""#,
            r#"resolutions"#,
            r#"resolutions last-generation=0"#,
            r#"resolutions last-generation=1 { nope; }"#,
            r#"resolutions last-generation=1 { resolution "a" kind="k" against="x"; }"#,
            r#"resolutions last-generation=1 { resolution "a" kind="k" generation=2 against="x"; }"#,
            r#"resolutions last-generation=1 { resolution "a" kind="K" generation=1 against="x"; }"#,
            r#"resolutions last-generation=1 { resolution "a" kind="k" generation=1; }"#,
            r#"resolutions last-generation=1 { resolution "a" kind="k" generation=1 against=""; }"#,
            r#"resolutions last-generation=1 { resolution "a" kind="k" generation=1 against="x" { field "h" "1"; field "h" "2"; }; }"#,
            r#"resolutions last-generation=2 { resolution "a" kind="k" generation=1 against="x"; resolution "a" kind="k" generation=2 against="x"; }"#,
        ] {
            let text = format!("andamento-record \"workspace/7\" version=6 {{ {body}; }}");
            assert!(
                WorkspaceRecord::decode(&text, id, "local").is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn version_4_workspace_overrides_migrate_into_the_overlay() {
        let id = WorkspaceId::from(7);
        let v4 = r#"andamento-record "workspace/7" version=4 {
    baseline version="1" {
        slot "a" rebind="ask" { url "a"; }
        slot "b" { url "b"; }
    }
    override "a" rebind="ask" {
        url "mine"
        against { url "a"; }
    }
    override "b" rebind="keep-previous" {
        url "also mine"
        against { url "b"; }
    }
    slot "u:1" { url "u"; }
    arrangement generation=2 owned=true { tabs "1" { tab "a"; tab "b"; tab "u:1"; }; }
}"#;
        let record = WorkspaceRecord::decode(v4, id, "local").unwrap();
        // a's policy is the baseline's, so only its content is an edit.
        assert_eq!(record.slots.edits["a"].rebind, None);
        assert_eq!(
            record.slots.edits["b"].rebind,
            Some(RebindPolicy::KeepPrevious)
        );
        assert_eq!(record.slots.user.len(), 1);
        let slots = record.slots.slots();
        assert!(slots[0].detached && slots[1].detached);
        assert_eq!(slots[0].rebind, RebindPolicy::Ask);
        let text = record.encode(id);
        assert!(text.starts_with("andamento-record \"workspace/7\" version=6"));
        assert!(text.contains("overlay {"), "{text}");
        assert!(!text.contains("\n    override"), "{text}");
        assert_eq!(WorkspaceRecord::decode(&text, id, "local").unwrap(), record);
        // Version 5 records have no saved resolutions: a node of that name is
        // unknown, and kept.
        let v5 = r#"andamento-record "workspace/7" version=5 { resolutions last-generation=1; }"#;
        let record = WorkspaceRecord::decode(v5, id, "local").unwrap();
        assert!(record.resolutions.is_empty());
        assert_eq!(record.unknown.len(), 1);
        // In version 5, top-level override and slot nodes are unknown.
        let v5 = r#"andamento-record "workspace/7" version=5 { slot "u:1" { url "u"; }; }"#;
        let record = WorkspaceRecord::decode(v5, id, "local").unwrap();
        assert!(record.slots.user.is_empty());
        assert_eq!(record.unknown.len(), 1);
    }

    #[test]
    fn proposals_carry_the_edit_set_and_its_baseline() {
        let id = WorkspaceId::from(7);
        let record = workspace_with_slots();
        let edits = EditSet::new(&record.slots, record.arrangement.as_ref());
        let text = encode_proposal(id, record.subject.as_ref(), &edits);
        assert!(
            text.starts_with(r#"overlay-proposal workspace="7" baseline="1" {"#),
            "{text}"
        );
        assert!(text.contains("arrangement {"), "{text}");
        assert!(!text.contains("departed"), "{text}");
        assert_eq!(decode_proposal(&text).unwrap(), (id, edits));
        let empty = EditSet::default();
        let text = encode_proposal(id, None, &empty);
        assert_eq!(text.trim(), r#"overlay-proposal workspace="7""#);
        assert_eq!(decode_proposal(&text).unwrap(), (id, empty));
    }

    #[test]
    fn version_3_dashboard_records_have_no_sidebar_arrangement() {
        // Version 3 knew no sidebar arrangement: a node with its name was
        // unknown to it, and stays unknown.
        let v3 = r#"andamento-record "dashboard" version=3 {
    display "show-finished" true
    sidebar generation=1 {
        dock
    }
}"#;
        let record = DashboardRecord::decode(v3, "local").unwrap();
        assert_eq!(record.sidebar, None);
        assert_eq!(record.unknown.len(), 1);
        assert!(record
            .encode()
            .starts_with("andamento-record \"dashboard\" version=6"));
        // In version 4 it is read, duplicates and all (reconciling removes
        // them); a malformed one is rejected.
        let v4 = r#"andamento-record "dashboard" version=4 {
    sidebar generation=2 {
        dock {
            tabs "1" { tab "a"; tab "a"; }
        }
        floating {
            tabs "2" { tab "a"; }
        }
        closed "b"
    }
}"#;
        let sidebar = DashboardRecord::decode(v4, "local")
            .unwrap()
            .sidebar
            .unwrap();
        assert_eq!(sidebar.doc.tabs(), ["a", "a", "a"]);
        assert_eq!(sidebar.closed, BTreeSet::from(["b".to_owned()]));
        assert!(!sidebar.owned);
        for body in [
            "sidebar",
            "sidebar generation=0",
            "sidebar generation=1 { dock { tabs \"1\"; tabs \"2\"; }; }",
            "sidebar generation=1 { dock { tabs \"1\" weight=0; }; }",
            "sidebar generation=1 { floating { tabs \"1\"; tabs \"1\"; }; }",
            "sidebar generation=1 { nope; }",
        ] {
            let text = format!("andamento-record \"dashboard\" version=4 {{ {body}; }}");
            assert!(DashboardRecord::decode(&text, "local").is_err(), "{text}");
        }
    }

    #[test]
    fn version_2_workspace_records_have_no_slots() {
        let id = WorkspaceId::from(7);
        // Version 2 knew no slots: nodes with these names were unknown to it
        // and are kept as unknown, not read as slots.
        let v2 = r#"andamento-record "workspace/7" version=2 {
    subject "vessel" "b" provider="sub-1"
    slot "u:1" {
        url "https://example.com"
    }
}"#;
        let record = WorkspaceRecord::decode(v2, id, "local").unwrap();
        assert_eq!(record.subject, Some(EntityRef::new("sub-1", "vessel", "b")));
        assert!(record.slots.is_empty());
        assert_eq!(record.arrangement, None);
        assert_eq!(record.unknown.len(), 1);
        assert!(record
            .encode(id)
            .starts_with("andamento-record \"workspace/7\" version=6"));
        // Malformed slots and arrangements are rejected in version 3.
        for body in [
            r#"slot "u:1""#,
            r#"slot "u:1" rebind="sometimes" { url "x"; }"#,
            r#"slot "u:1" { url "x"; file "y"; }"#,
            r#"override "k" { url "x"; }"#,
            r#"arrangement owned=true"#,
            r#"arrangement generation=1 { tabs "a" { tab "x"; }; tabs "a"; }"#,
            r#"arrangement generation=1 { tabs "a" selected="y" { tab "x"; }; }"#,
            r#"arrangement generation=1 { split "a" axis="diagonal" { tabs "b"; }; }"#,
            r#"arrangement generation=1 { split "a" axis="row" { tabs "b" weight=0; }; }"#,
            r#"baseline { slot "p" { url "x"; }; }"#,
        ] {
            let text = format!("andamento-record \"workspace/7\" version=3 {{ {body}; }}");
            assert!(
                WorkspaceRecord::decode(&text, id, "local").is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn unknown_nodes_survive_a_round_trip() {
        let text = dashboard().encode();
        let extended = text.replacen(
            "{\n",
            "{\n    future-thing \"x\" mode=2 {\n        nested 1\n    }\n",
            1,
        );
        let record = DashboardRecord::decode(&extended, "local").unwrap();
        assert_eq!(record.unknown.len(), 1);
        let exported = record.encode();
        assert!(exported.contains("future-thing \"x\" mode=2"), "{exported}");
        assert!(exported.contains("nested 1"), "{exported}");
        assert_eq!(DashboardRecord::decode(&exported, "local").unwrap(), record);
        assert_eq!(
            DashboardRecord::decode(&exported, "local")
                .unwrap()
                .encode(),
            exported
        );
    }

    #[test]
    fn other_versions_names_and_shapes_are_rejected() {
        for text in [
            "andamento-record \"dashboard\" version=7",
            "andamento-record \"dashboard\" version=0",
            "andamento-record \"dashboard\"",
            // Version 2 entities name their provider.
            "andamento-record \"dashboard\" version=2 { collapsed { at \"p\" \"project\" \"p\"; }; }",
            "andamento-record \"dashboard\" version=2 { collapsed { at \"p\" \"project\" \"p\" provider=\"\"; }; }",
            "andamento-record \"workspace/7\" version=1",
            "something-else \"dashboard\" version=1",
            "andamento-record \"dashboard\" version=1\nandamento-record \"dashboard\" version=1",
            "andamento-record \"dashboard\" version=1 { display \"x\"; }",
            "andamento-record \"dashboard\" version=1 { collapsed { nope; }; }",
            "not { kdl",
        ] {
            assert!(DashboardRecord::decode(text, "local").is_err(), "{text}");
        }
        assert!(
            DashboardRecord::decode("andamento-record \"dashboard\" version=1", "local").is_ok()
        );
        assert!(
            DashboardRecord::decode("andamento-record \"dashboard\" version=2", "local").is_ok()
        );
    }

    #[test]
    fn version_1_records_migrate_to_the_default_provider() {
        let v1 = r#"andamento-record "dashboard" version=1 {
    collapsed {
        at "project" "project" "p"
    }
    order "tree" "vessel" {
        parent {
            at "project" "project" "p"
        }
        entity "vessel" "b"
    }
    local ".ref" "r1" {
        fact ".group" {
            entity ".group" "g1"
        }
        fact ".target" {
            entity "vessel" "b"
        }
    }
}"#;
        let record = DashboardRecord::decode(v1, "sub-1").unwrap();
        let project = PlacementKey(vec![PlacementSegment {
            loop_name: "project".into(),
            entity: EntityRef::new("sub-1", "project", "p"),
        }]);
        assert_eq!(record.collapsed, BTreeSet::from([project.clone()]));
        let (loop_key, order) = record.orders.iter().next().unwrap();
        assert_eq!(loop_key.parent, project);
        assert_eq!(order, &vec![EntityRef::new("sub-1", "vessel", "b")]);
        // Andamento's own sections, groups and refs are local; what a ref
        // points at gets the default.
        let facts = &record.local[&EntityRef::local(".ref", "r1")];
        assert_eq!(
            facts[".group"],
            MetadataValue::EntityRefs(vec![EntityRef::local(".group", "g1")])
        );
        assert_eq!(
            facts[".target"],
            MetadataValue::EntityRefs(vec![EntityRef::new("sub-1", "vessel", "b")])
        );
        // Export writes the current version, naming every provider; it reads back the same.
        let text = record.encode();
        assert!(
            text.starts_with("andamento-record \"dashboard\" version=6"),
            "{text}"
        );
        assert!(
            text.contains(r#"at "project" "project" "p" provider="sub-1""#),
            "{text}"
        );
        assert_eq!(DashboardRecord::decode(&text, "other").unwrap(), record);

        let id = WorkspaceId::from(7);
        let v1 = r#"andamento-record "workspace/7" version=1 {
    subject "vessel" "b"
    retained "vessel" "b" label="B" status="retained"
}"#;
        let record = WorkspaceRecord::decode(v1, id, "local").unwrap();
        assert_eq!(record.subject, Some(EntityRef::local("vessel", "b")));
        assert!(record
            .retained
            .contains_key(&EntityRef::local("vessel", "b")));
        assert!(record
            .encode(id)
            .contains(r#"subject "vessel" "b" provider="local""#));
    }

    #[test]
    fn the_same_kind_and_id_under_two_providers_are_two_entities() {
        let mut record = dashboard();
        record.orders.clear();
        record.orders.insert(
            PlacementLoopKey {
                region: "tree".into(),
                parent: PlacementKey::default(),
                binding: "vessel".into(),
            },
            vec![
                EntityRef::new("sub-1", "vessel", "v"),
                EntityRef::new("sub-2", "vessel", "v"),
            ],
        );
        let text = record.encode();
        let decoded = DashboardRecord::decode(&text, "local").unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.orders.values().next().unwrap().len(), 2);
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
