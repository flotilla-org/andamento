//! Helpers shared by the integration tests.
#![allow(dead_code)]
use andamento_core::presentation::{PlacementNode, SurfaceSnapshot};
use andamento_core::{
    tab_workspace_id, EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate,
    Sidebar, WorkspaceId,
};
use std::collections::BTreeSet;

/// The subscription test facts come from.
pub const PROVIDER: &str = "test";
/// The fixture template catalog.
pub const CONFIG: &str = include_str!("../fixtures/sidebar.kdl");
/// A workspace a host generated an ID for.
pub const WORKER: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c";

pub fn worker() -> WorkspaceId {
    WORKER.parse().unwrap()
}

/// An entity of the test subscription.
pub fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::new(PROVIDER, kind, id)
}

/// The `.workspace` entity Andamento presents for an unplaced workspace.
pub fn fallback(tab: u64) -> EntityRef {
    let id = tab_workspace_id(tab).to_string();
    EntityRef::local(".workspace", id)
}

pub fn text(s: &str) -> MetadataValue {
    MetadataValue::Text(s.into())
}

pub fn list(values: &[&str]) -> MetadataValue {
    MetadataValue::StringList(values.iter().map(|v| v.to_string()).collect())
}

pub fn refs(entity: EntityRef) -> MetadataValue {
    MetadataValue::EntityRefs(vec![entity])
}

pub fn update(value: MetadataValue) -> MetadataValueUpdate {
    MetadataValueUpdate {
        value,
        ttl_ms: None,
        precedence: None,
        ordinal: None,
    }
}

/// A patch setting `facts` on `subject`.
pub fn patch(subject: EntityRef, facts: &[(&str, MetadataValue)]) -> MetadataPatch {
    MetadataPatch {
        target: MetadataTarget::Entity(subject),
        source_id: "fixture".into(),
        set: facts
            .iter()
            .map(|(key, value)| (key.to_string(), update(value.clone())))
            .collect(),
        unset: vec![],
    }
}

/// Every node, in preorder.
pub fn nodes(surface: &SurfaceSnapshot) -> Vec<&PlacementNode> {
    fn collect<'a>(nodes: &'a [PlacementNode], out: &mut Vec<&'a PlacementNode>) {
        for node in nodes {
            out.push(node);
            collect(&node.children, out);
        }
    }
    let mut out = vec![];
    for section in &surface.sections {
        collect(&section.nodes, &mut out);
    }
    out
}

pub fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

/// Applying the test subscription's facts.
pub trait Feed {
    fn feed(&mut self, now_ms: u64, patches: impl IntoIterator<Item = MetadataPatch>);
}

impl Feed for Sidebar {
    fn feed(&mut self, now_ms: u64, patches: impl IntoIterator<Item = MetadataPatch>) {
        self.apply(now_ms, PROVIDER, patches).unwrap();
    }
}
