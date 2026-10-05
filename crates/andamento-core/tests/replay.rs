use andamento_core::{
    presentation::PlacementNode,
    replay::{read, Frame, Recorder, Replay},
    MetadataValue, Sidebar,
};
use std::io::Cursor;
const CONFIG: &str = r#"
region "tree" root-template="heading" form="compact" placement="tree"
region "convoys" root-template="heading" form="compact" placement="convoys"
region "vessels" root-template="heading" form="compact" placement="vessels"
region "repos" root-template="heading" form="compact" placement="repos"
region "issues" root-template="heading" form="compact" placement="issues"
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

#[test]
fn equal_offsets_are_one_ordered_observation() {
    let mut input = frames("rail-scene-legacy.jsonl");
    let mut replacement = input[0].clone();
    replacement
        .patch
        .set
        .get_mut("display.label")
        .unwrap()
        .value = MetadataValue::Text("last writer".into());
    input.push(replacement);
    let mut direct = Sidebar::new(CONFIG).unwrap();
    direct.apply(0, input.iter().map(|f| f.patch.clone()));
    let mut replay = Replay::new(input).unwrap();
    let mut stepped = Sidebar::new(CONFIG).unwrap();
    assert_eq!(replay.step(&mut stepped).unwrap(), Some(0));
    assert_eq!(stepped.snapshot(), direct.snapshot()); // Includes revision, not only surface.
    assert_eq!(replay.step(&mut stepped).unwrap(), None);
}

#[test]
fn incomplete_or_unknown_envelope_fields_are_rejected() {
    let valid = serde_json::to_value(frames("flotilla-connector-patches.jsonl").remove(0)).unwrap();
    for field in ["offset_ms", "patch"] {
        let mut incomplete = valid.clone();
        incomplete.as_object_mut().unwrap().remove(field);
        assert!(read(Cursor::new(incomplete.to_string())).is_err());
    }
    let mut unknown = valid;
    unknown["extra"] = serde_json::json!(true);
    assert!(read(Cursor::new(unknown.to_string())).is_err());
}

#[test]
fn cli_emit_snapshot_and_usage() {
    use std::process::Command;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = env!("CARGO_BIN_EXE_andamento-replay");
    let capture = root.join("fixtures/scripted-roll.jsonl");
    let emitted = Command::new(binary)
        .arg("emit")
        .arg(&capture)
        .output()
        .unwrap();
    assert!(emitted.status.success());
    let parsed = read(Cursor::new(emitted.stdout)).unwrap();
    let expected = frames("scripted-roll.jsonl");
    assert_eq!(parsed.len(), expected.len());
    for (actual, expected) in parsed.iter().zip(expected) {
        assert_eq!(actual.patch, expected.patch);
        assert_eq!(actual.offset_ms, 0);
    }
    let snapshot = Command::new(binary)
        .arg("snapshot")
        .arg(&capture)
        .arg(root.join("crates/andamento-core/tests/fixtures/sidebar.kdl"))
        .arg("32001")
        .output()
        .unwrap();
    assert!(snapshot.status.success());
    let snapshot: andamento_core::sidebar::Snapshot =
        serde_json::from_slice(&snapshot.stdout).unwrap();
    assert!(snapshot.surface.sections.iter().all(|s| s.nodes.is_empty()));
    let invalid = Command::new(binary).arg("unknown").output().unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("usage:"));
}

// A declared standing role must appear exactly once on its owning project row
// after every observation, independently of attempts (andamento#105).
// Keep scripts/check-standing-role.py in sync with these placement assertions.
fn assert_standing_role_placement(input: Vec<Frame>) {
    let offsets: std::collections::BTreeSet<_> = input.iter().map(|f| f.offset_ms).collect();
    assert!(
        !offsets.is_empty(),
        "regression stream must contain observations"
    );
    let mut replay = Replay::new(input).unwrap();
    let mut sidebar = Sidebar::new(include_str!("../../../fixtures/standing-role.kdl")).unwrap();
    let mut role_key = None;
    for at in offsets {
        assert_eq!(replay.step(&mut sidebar).unwrap(), Some(at));
        let snapshot = sidebar.snapshot();
        let projects = &snapshot.surface.sections[0].nodes;
        let roles: Vec<_> = projects
            .iter()
            .flat_map(|project| project.children.iter().map(move |role| (project, role)))
            .filter(|(_, role)| role.entity.kind == "role")
            .collect();
        assert_eq!(roles.len(), 1, "role dissociated or duplicated at {at} ms");
        let (project, role) = roles[0];
        assert_eq!(project.entity.id, "flotilla/andamento@fleet", "{at} ms");
        assert_eq!(role.entity.id, "flotilla/andamento/coder@fleet", "{at} ms");
        if let Some(key) = &role_key {
            assert_eq!(&role.key, key, "role placement identity changed at {at} ms");
        } else {
            role_key = Some(role.key.clone());
        }
    }
    assert_eq!(replay.step(&mut sidebar).unwrap(), None);
}

#[test]
#[ignore = "andamento#105: connector projection withdraws the declared role's project join during a held gap"]
fn declared_role_stays_on_project_through_scripted_gap() {
    // Keep the desired contract even while the pinned connector violates it.
    // Regenerate after the connector fix, then remove this ignore when green.
    // Scripted, with no awareness/project-repository rows: not full #105 acceptance.
    assert_standing_role_placement(frames("scripted-gap.jsonl"));
}

#[test]
#[ignore = "andamento#105: connector empty publication withdraws role and project joins before restart reassertion"]
fn declared_role_stays_on_project_through_scripted_restart() {
    // This models an empty query publication, not transport silence or TTL expiry.
    // Temporary empty observations must not mean declaration deletion.
    // Scripted, with no awareness/project-repository rows: not full #105 acceptance.
    assert_standing_role_placement(frames("scripted-restart.jsonl"));
}

#[test]
fn declared_role_parent_retained_control_stays_on_project() {
    // Single-variable control: preserve only the parent publication while the
    // connector removes the attempt. The role's held/ready facts are untouched.
    // Pin 87f03e9 is known broken: require one withdrawal so fixture/filter drift
    // cannot silently erase the control. Update this expectation with the pin fix.
    let mut input = frames("scripted-gap.jsonl");
    let before = input.len();
    input.retain(|frame| {
        !(frame.offset_ms == 1000
            && matches!(&frame.patch.target,
                andamento_core::MetadataTarget::Entity(entity) if entity.kind == "project")
            && frame
                .patch
                .unset
                .iter()
                .any(|key| key == "flotilla.project")
            && !frame.patch.set.contains_key("flotilla.project"))
    });
    assert_eq!(
        before - input.len(),
        1,
        "known-broken connector pin must withdraw exactly one parent join"
    );
    assert_standing_role_placement(input);
}
