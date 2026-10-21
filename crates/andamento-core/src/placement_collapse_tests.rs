//! A placement loop may declare that its placements start collapsed. The
//! collapse set then holds placements toggled away from their default.
use crate::presentation::{PlacementNode, SurfaceSnapshot};
use crate::records::DashboardRecord;
use crate::sidebar::Action;
use crate::{EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, Sidebar};
use std::rc::Rc;

const PROVIDER: &str = "test";

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::new(PROVIDER, kind, id)
}

fn patch(subject: EntityRef, facts: &[(&str, &str)]) -> MetadataPatch {
    let update =
        |value: &str| MetadataValueUpdate { value: MetadataValue::Text(value.into()), ttl_ms: None, precedence: None, ordinal: None };
    MetadataPatch {
        target: MetadataTarget::Entity(subject),
        source_id: "fixture".into(),
        set: facts.iter().map(|(key, value)| (key.to_string(), update(value))).collect(),
        unset: vec![],
    }
}

fn nodes(surface: &SurfaceSnapshot) -> Vec<&PlacementNode> {
    fn collect<'a>(nodes: &'a [Rc<PlacementNode>], out: &mut Vec<&'a PlacementNode>) {
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

const CONFIG: &str = r#"
region "tree" root-template="tree/title" form="compact" placement="tree"
template "tree/title" slot="compact" node-kind="entity" {
  field "label" source="literal" value="Projects"
}
placement "tree" {
  for "project" kind="project" {
    apply-template
  }
}
template "project/line" {
  field "label" key="display.label"
  for "convoy" kind="convoy" default-collapsed=true {
    match "flotilla.project" of="project"
    apply-template
  }
}
template "convoy/line" {
  field "label" key="display.label"
  for "vessel" kind="vessel" {
    match "flotilla.convoy" of="convoy"
    apply-template
  }
}
template "vessel/line" {
  field "label" key="display.label"
}
"#;

fn facts() -> Vec<MetadataPatch> {
    vec![
        patch(entity("project", "p"), &[("flotilla.project", "p"), ("display.label", "P")]),
        patch(entity("convoy", "c"), &[("flotilla.project", "p"), ("flotilla.convoy", "c"), ("display.label", "C")]),
        patch(entity("vessel", "v"), &[("flotilla.project", "p"), ("flotilla.convoy", "c"), ("display.label", "V")]),
    ]
}

fn sidebar() -> Sidebar {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    sidebar.apply(100, PROVIDER, facts()).unwrap();
    sidebar
}

fn node(sidebar: &Sidebar, kind: &str) -> PlacementNode {
    nodes(&sidebar.snapshot().surface).into_iter().find(|node| node.entity.kind == kind).cloned().expect(kind)
}

#[test]
fn a_default_collapsed_loop_starts_collapsed_and_toggles_open_and_closed() {
    let mut sidebar = sidebar();
    let convoy = node(&sidebar, "convoy");
    assert!(convoy.collapsed, "a default-collapsed placement starts collapsed");
    assert_eq!(convoy.children.len(), 1, "collapse keeps its children for the host to hide");

    sidebar.dispatch(Action::TogglePlacement { key: convoy.key.clone() }).unwrap();
    assert!(!node(&sidebar, "convoy").collapsed, "toggling expands it");
    sidebar.dispatch(Action::TogglePlacement { key: convoy.key.clone() }).unwrap();
    assert!(node(&sidebar, "convoy").collapsed, "toggling again collapses it");
}

#[test]
fn loops_without_the_property_are_unchanged() {
    let mut sidebar = sidebar();
    let project = node(&sidebar, "project");
    assert!(!project.collapsed && !node(&sidebar, "vessel").collapsed, "other loops start expanded");
    sidebar.dispatch(Action::TogglePlacement { key: project.key.clone() }).unwrap();
    assert!(node(&sidebar, "project").collapsed, "toggling an ordinary placement collapses it");
    assert!(node(&sidebar, "convoy").collapsed, "an ancestor's toggle does not toggle its default-collapsed children");
}

#[test]
fn a_nested_placement_keeps_its_own_state_while_its_parent_collapses_and_opens() {
    let mut sidebar = sidebar();
    let project = node(&sidebar, "project").key;
    let convoy = node(&sidebar, "convoy").key;
    sidebar.dispatch(Action::TogglePlacement { key: convoy }).unwrap();
    sidebar.dispatch(Action::TogglePlacement { key: project.clone() }).unwrap();
    assert!(node(&sidebar, "project").collapsed);
    assert!(!node(&sidebar, "convoy").collapsed, "collapsing the parent leaves the opened convoy's state alone");
    sidebar.dispatch(Action::TogglePlacement { key: project }).unwrap();
    assert!(!node(&sidebar, "project").collapsed && !node(&sidebar, "convoy").collapsed, "reopening the parent shows it still open");
}

#[test]
fn an_expanded_default_collapsed_placement_survives_a_record_round_trip() {
    let mut before = sidebar();
    let convoy = node(&before, "convoy").key;
    before.dispatch(Action::TogglePlacement { key: convoy.clone() }).unwrap();
    let record = before.export_record("dashboard").unwrap();
    // The record holds the toggle, which for this loop means expanded.
    assert!(DashboardRecord::decode(&record).unwrap().state.collapsed.contains(&convoy));

    let mut after = Sidebar::new(CONFIG).unwrap();
    after.import_record("dashboard", &record).unwrap();
    after.apply(100, PROVIDER, facts()).unwrap();
    assert!(!node(&after, "convoy").collapsed, "the restored convoy stays expanded");
    assert!(!node(&after, "project").collapsed);
}

#[test]
fn default_collapsed_must_be_a_boolean() {
    let config = CONFIG.replace("default-collapsed=true", "default-collapsed=\"yes\"");
    let error = Sidebar::new(&config).err().expect("a non-boolean default-collapsed is rejected");
    assert!(error.contains("default-collapsed must be a boolean"), "{error}");
}
