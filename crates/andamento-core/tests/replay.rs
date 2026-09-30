use andamento_core::{
    presentation::PlacementNode,
    replay::{read, Frame, Recorder, Replay},
    MetadataValue, Sidebar,
};
use std::io::Cursor;
const CONFIG: &str = r#"
region "tree" source="tree" root-template="heading" form="compact" placement="tree"
region "convoys" source="tree" root-template="heading" form="compact" placement="convoys"
region "vessels" source="tree" root-template="heading" form="compact" placement="vessels"
region "repos" source="tree" root-template="heading" form="compact" placement="repos"
region "issues" source="tree" root-template="heading" form="compact" placement="issues"
template "heading" { field "label" source="literal" value="Replay"; }
placement "tree" {
  for "project" kind="project" {
    apply-template "project"
  }
}
placement "convoys" {
  for "convoy" kind="convoy" { apply-template "line"; }
}
placement "vessels" {
  for "vessel" kind="vessel" { apply-template "line"; }
}
placement "repos" {
  for "repo" kind="repo" { apply-template "line"; }
}
placement "issues" {
  for "issue" kind="issue" { apply-template "line"; }
}
template "project" {
  field "label" key="display.label"
  for "role" kind="role" {
    match "flotilla.project" of="project"
    apply-template "line"
  }
}
template "line" { field "label" key="display.label"; }
"#;
fn frames(name: &str) -> Vec<Frame> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    read(Cursor::new(std::fs::read(path).unwrap())).unwrap()
}
fn catalog(nodes: &[PlacementNode]) -> serde_json::Value {
    serde_json::Value::Array(
        nodes
            .iter()
            .map(|node| {
                serde_json::json!({
                    "entity":node.entity, "facts":node.facts, "children":catalog(&node.children)
                })
            })
            .collect(),
    )
}
#[test]
fn legacy_and_generated_catalog_snapshots() {
    for name in [
        "flotilla-connector-patches",
        "rail-scene-legacy",
        "rail-scene",
    ] {
        let frames = frames(&format!("{name}.jsonl"));
        let mut replay = Replay::new(frames.clone()).unwrap();
        let mut sidebar = Sidebar::new(CONFIG).unwrap();
        replay.advance_to(&mut sidebar, 0).unwrap();
        let mut direct = Sidebar::new(CONFIG).unwrap();
        direct.apply(0, frames.into_iter().map(|f| f.patch));
        assert_eq!(sidebar.snapshot().surface, direct.snapshot().surface);
        insta::assert_debug_snapshot!(
            name,
            sidebar
                .snapshot()
                .surface
                .sections
                .iter()
                .map(|s| catalog(&s.nodes))
                .collect::<Vec<_>>()
        );
    }
}
#[test]
fn scripted_roll_keeps_role_under_project_through_hold_handover_and_expiry() {
    let mut replay = Replay::new(frames("scripted-roll.jsonl")).unwrap();
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let mut key = None;
    for (at, desired, target) in [
        (0, "ready", Some("coder-1")),
        (1000, "held", None),
        (2000, "ready", Some("coder-2")),
    ] {
        assert_eq!(replay.step(&mut sidebar).unwrap(), Some(at));
        let snapshot = sidebar.snapshot();
        let project = snapshot.surface.sections[0]
            .nodes
            .iter()
            .find(|n| n.entity.kind == "project")
            .unwrap();
        assert_eq!(project.entity.id, "flotilla/andamento@fleet");
        assert_eq!(project.children.len(), 1);
        let role = &project.children[0];
        assert_eq!(role.entity.kind, "role");
        if let Some(key) = &key {
            assert_eq!(&role.key, key);
        } else {
            key = Some(role.key.clone());
        }
        assert_eq!(
            role.facts.get("workspace.primary.state"),
            Some(&MetadataValue::Text(desired.into()))
        );
        let actual = role.facts.get("workspace.primary.target");
        if let Some(target) = target {
            assert!(matches!(actual, Some(MetadataValue::Text(s)) if s.contains(target)));
        } else {
            assert!(actual.is_none());
        }
    }
    assert_eq!(replay.step(&mut sidebar).unwrap(), None);
    replay.advance_to(&mut sidebar, 32_001).unwrap();
    assert!(sidebar.snapshot().surface.sections[0].nodes.is_empty());
    assert!(replay.advance_to(&mut sidebar, 2000).is_err());
}
#[test]
fn malformed_decreasing_and_truncated_streams_are_rejected() {
    assert!(read(Cursor::new("{\n"))
        .unwrap_err()
        .to_string()
        .contains("line 1"));
    let frame = frames("flotilla-connector-patches.jsonl").remove(0);
    let mut a = frame.clone();
    a.offset_ms = 10;
    let mut b = frame;
    b.offset_ms = 9;
    let raw = format!(
        "{}\n{}\n",
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    assert!(read(Cursor::new(raw)).is_err());
    assert!(Replay::new(vec![a, b]).is_err());
}
#[test]
fn recorder_round_trips_wire_values() {
    let patch = frames("flotilla-connector-patches.jsonl").remove(0).patch;
    let mut bytes = vec![];
    let mut recorder = Recorder::new(&mut bytes);
    recorder.record(patch.clone()).unwrap();
    recorder.record(patch.clone()).unwrap();
    let first: serde_json::Value =
        serde_json::from_slice(bytes.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(first["patch"]["type"], "metadata-patch");
    let recorded = read(Cursor::new(bytes)).unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].patch, patch);
    assert!(recorded[0].offset_ms <= recorded[1].offset_ms);
}

#[test]
fn advancing_over_a_gap_does_not_refresh_old_facts() {
    let mut input = frames("rail-scene-legacy.jsonl");
    // Deliver one project late, leaving the other entities at time zero.
    let index = input
        .iter()
        .position(|frame| {
            matches!(&frame.patch.target,
        andamento_core::MetadataTarget::Entity(entity) if entity.kind == "project")
        })
        .unwrap();
    let mut late = input.remove(index);
    late.offset_ms = 40_000;
    let target = late.patch.target.clone();
    input.push(late);
    let mut replay = Replay::new(input).unwrap();
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    replay.advance_to(&mut sidebar, 0).unwrap();
    assert_eq!(
        sidebar.snapshot().surface.sections[1].nodes[0].entity.id,
        "flotilla/adhoc-triage@fleet"
    ); // Explicit -100 ordinal wins.
    replay.advance_to(&mut sidebar, 40_000).unwrap();
    let snapshot = sidebar.snapshot();
    let nodes = &snapshot.surface.sections[0].nodes;
    assert_eq!(nodes.len(), 1);
    assert_eq!(
        andamento_core::MetadataTarget::Entity(nodes[0].entity.clone()),
        target
    );
    assert!(snapshot.surface.sections[1..]
        .iter()
        .all(|s| s.nodes.is_empty()));
    replay.advance_to(&mut sidebar, 70_001).unwrap();
    assert!(sidebar
        .snapshot()
        .surface
        .sections
        .iter()
        .all(|s| s.nodes.is_empty()));
}
