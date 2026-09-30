use crate::presentation::PlacementNode;
use crate::state::ControllerState;
use crate::{EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, NodeKey, RailUiAction};
use std::collections::BTreeMap;

fn state() -> ControllerState {
    let mut state = ControllerState::default();
    for line in include_str!("../tests/fixtures/default.jsonl").lines() {
        state.apply_metadata_patch(serde_json::from_str(line).unwrap());
    }
    state
}
fn outline(nodes: &[PlacementNode], depth: usize, out: &mut String) {
    for node in nodes {
        out.push_str(&format!("{}{}:{} [{}] {}\n", "  ".repeat(depth), node.entity.kind,
            node.entity.id, node.layout.as_deref().unwrap_or("lines"), node.content.text()));
        outline(&node.children, depth + 1, out);
    }
}
fn patch(kind: &str, id: &str, key: &str, value: MetadataValue) -> MetadataPatch {
    MetadataPatch { target: MetadataTarget::Entity(EntityRef { kind: kind.into(), id: id.into() }),
        source_id: "fixture".into(), set: BTreeMap::from([(key.into(), MetadataValueUpdate {
            value, ttl_ms: None, precedence: None, ordinal: None })]), unset: vec![] }
}

#[test]
fn bundled_defaults_are_placement_templates_without_a_configuration_switch() {
    let model = state().view_model();
    let mut text = String::new();
    for section in &model.presentation.as_ref().unwrap().sections {
        text.push_str(&format!("section {} pinned={}\n", section.name, section.pinned));
        outline(&section.nodes, 1, &mut text);
    }
    insta::assert_snapshot!("default_placement_tree", text);
    assert_eq!(model.unmatched_entities, vec![EntityRef { kind: "repo".into(), id: "r".into() }]);
    let wire = serde_json::to_string(&model).unwrap();
    for retired in ["collapsed_groups", "group-header", "tab-title", "tab-status", "loop-binding", "loop-tier", "surface_regions", "placement_layout"] {
        assert!(!wire.contains(retired), "retired wire field {retired}");
    }
}

#[test]
fn catalog_coverage_does_not_depend_on_grouping_facts_or_collapse() {
    let mut state = state();
    state.apply_metadata_patch(patch("novel", "no-hierarchy", "display.label", MetadataValue::Text("Unknown".into())));
    let model = state.view_model();
    assert!(model.unmatched_entities.iter().any(|e| e.id == "no-hierarchy"));
    let project = model.presentation.unwrap().sections[1].nodes[0].key.clone();
    state.apply_rail_ui_action(RailUiAction::TogglePlacement { key: project });
    assert_eq!(state.view_model().unmatched_entities, model.unmatched_entities);
    state.apply_metadata_patch(patch("novel", "no-hierarchy", "status.attention", MetadataValue::Bool(true)));
    assert!(!state.view_model().unmatched_entities.iter().any(|e| e.id == "no-hierarchy"));
}

#[test]
fn one_entity_in_two_sections_has_independent_state_and_typed_tier() {
    let mut state = state();
    state.apply_metadata_patch(patch("convoy", "c", "status.attention", MetadataValue::Bool(true)));
    let before = state.view_model().presentation.unwrap();
    let tree = &before.sections[1].nodes[0].children[2];
    let attention = &before.sections[0].nodes[0];
    assert_eq!(tree.entity, attention.entity);
    assert_ne!(tree.key, attention.key);
    let key = tree.key.clone();
    state.apply_rail_ui_action(RailUiAction::TogglePlacement { key: key.clone() });
    state.set_node_variable(NodeKey::Placement(key.clone()), "convoy.tier".into(), Some("short".into()));
    let after = state.view_model().presentation.unwrap();
    let tree = after.node(&key).unwrap();
    assert!(tree.collapsed);
    assert_eq!(tree.content.text(), "PC");
    assert!(tree.detail.text().contains("Placement cutover"));
    assert!(!after.sections[0].nodes[0].collapsed);
    assert!(!tree.facts.keys().any(|k| k.starts_with("andamento.placement.")));
}

#[test]
fn defaults_keep_the_nested_shape_and_do_not_fuse_single_children() {
    let model = state().view_model();
    let surface = model.presentation.unwrap();
    let convoy = &surface.sections[1].nodes[0].children[2];
    assert_eq!(convoy.entity.kind, "convoy");
    assert_eq!(convoy.children[0].entity.kind, "vessel");
    assert_eq!(convoy.children[0].children[0].entity.kind, "session");
}

#[test]
fn old_configuration_and_ui_identity_are_rejected() {
    use crate::template_config::parse_template_config_kdl;
    for config in ["grouping \"old\" { level key=\"entity.id\"; }",
        "region \"old\" source=\"tree\" root-template=\"root\"",
        "region \"old\" attention-key=\"status.attention\" root-template=\"root\"",
        "template \"old\" slot=\"group-header\" node-kind=\"group\" { field \"label\" key=\"display.label\"; }"] {
        assert!(parse_template_config_kdl(config).is_err(), "{config}");
    }
    assert!(serde_json::from_str::<NodeKey>(r#"{"kind":"group","value":[]}"#).is_err());
    assert!(serde_json::from_str::<RailUiAction>(r#"{"action":"toggle-group","path":[]}"#).is_err());
}
