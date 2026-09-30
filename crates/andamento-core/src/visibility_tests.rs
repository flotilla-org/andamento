use crate::state::ControllerState;
use crate::template_config::{
    parse_template_config_kdl, TemplateConfigCatalog, TemplateConfigLayer,
};
use crate::{
    DisplayVariableValue, EntityRef, MetadataPatch, MetadataTarget, MetadataValue,
    MetadataValueUpdate, RailUiAction,
};

const CONFIG: &str = r#"
display-variable "finished" type="bool" default=false label="Finished" icon="F" persist=true
display-variable "attempts" type="bool" default=false label="Attempts" icon="A" persist=true
visibility "activity" {
    when kind="vessel" visible-when="finished" { match "phase" value="done"; }
    when kind="vessel" visible-when="attempts" { match "role" exists=true; }
}
region "tree" root-template="heading" placement="tree"
region "attention" root-template="heading" placement="attention"
region "audit" root-template="heading" placement="audit"
template "heading" slot="compact" node-kind="entity" { field "label" source="literal" value="Items"; }
placement "tree" { for "vessel" kind="vessel" visibility="activity" { field "label" key="entity.id"; }; }
placement "attention" { for "alert" visibility="activity" { match "attention" value="true"; }; }
placement "audit" { for "item" kind="vessel"; }
"#;

fn config(input: &str) -> TemplateConfigCatalog {
    TemplateConfigCatalog::from_config(parse_template_config_kdl(input).unwrap())
}
fn publish(state: &mut ControllerState, id: &str, facts: &[(&str, MetadataValue)]) {
    state.apply_metadata_patch(MetadataPatch {
        target: MetadataTarget::Entity(EntityRef {
            kind: "vessel".into(),
            id: id.into(),
        }),
        source_id: "test".into(),
        set: facts
            .iter()
            .map(|(key, value)| {
                (
                    key.to_string(),
                    MetadataValueUpdate {
                        value: value.clone(),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                )
            })
            .collect(),
        unset: vec![],
    });
}
fn ids(state: &ControllerState, section: &str) -> Vec<String> {
    state
        .view_model()
        .presentation
        .unwrap()
        .sections
        .iter()
        .find(|item| item.name == section)
        .unwrap()
        .nodes
        .iter()
        .map(|node| node.entity.id.clone())
        .collect()
}
fn toggle(state: &mut ControllerState, name: &str) {
    assert!(state.apply_rail_ui_action(RailUiAction::ToggleVariable { name: name.into() }));
}

#[test]
fn policy_is_opt_in_shared_by_aliases_and_preserves_first_match_precedence() {
    let mut state = ControllerState::default();
    state.set_template_catalog(Some(config(CONFIG)));
    for id in ["active", "done", "standing"] {
        publish(&mut state, id, &[("attention", MetadataValue::Bool(true))]);
    }
    publish(
        &mut state,
        "done",
        &[
            ("phase", MetadataValue::Text("done".into())),
            ("role", MetadataValue::Text("governor".into())),
        ],
    );
    // Existence includes non-scalar facts, unlike equality query predicates.
    publish(
        &mut state,
        "standing",
        &[("role", MetadataValue::StringList(vec!["governor".into()]))],
    );
    assert_eq!(ids(&state, "tree"), ["active"]);
    assert_eq!(ids(&state, "attention"), ["active"]);
    assert_eq!(ids(&state, "audit"), ["active", "done", "standing"]);
    let original_key = state.view_model().presentation.unwrap().sections[0].nodes[0]
        .key
        .clone();
    toggle(&mut state, "attempts");
    assert_eq!(ids(&state, "tree"), ["active", "standing"]);
    toggle(&mut state, "finished");
    toggle(&mut state, "attempts");
    assert_eq!(ids(&state, "tree"), ["active", "done"]);
    assert_eq!(ids(&state, "attention"), ["active", "done"]);
    toggle(&mut state, "finished");
    assert_eq!(
        original_key,
        state.view_model().presentation.unwrap().sections[0].nodes[0].key
    );

    // Changing facts recomputes visibility without changing toggle state.
    publish(
        &mut state,
        "done",
        &[("phase", MetadataValue::Text("active".into()))],
    );
    assert_eq!(ids(&state, "tree"), ["active"]); // still a standing attempt
    let mut restored = ControllerState::default();
    restored.set_template_catalog(Some(config(CONFIG)));
    toggle(&mut state, "attempts");
    restored.apply_rail_ui_state(state.rail_ui_state());
    publish(
        &mut restored,
        "standing",
        &[("role", MetadataValue::Bool(false))],
    );
    assert_eq!(ids(&restored, "tree"), ["standing"]);
}

#[test]
fn policies_and_boolean_defaults_follow_layer_precedence() {
    let bundled = parse_template_config_kdl(CONFIG).unwrap();
    let user = parse_template_config_kdl(&CONFIG.replace("default=false", "default=true")).unwrap();
    let mut state = ControllerState::default();
    state.set_template_catalog(Some(TemplateConfigCatalog::from_layers(vec![
        TemplateConfigLayer::bundled("bundled", bundled),
        TemplateConfigLayer::user("user", user),
    ])));
    publish(
        &mut state,
        "done",
        &[("phase", MetadataValue::Text("done".into()))],
    );
    assert_eq!(ids(&state, "tree"), ["done"]);
    assert_eq!(
        state.rail_ui_state().variables.get("finished"),
        Some(&DisplayVariableValue::Bool(true))
    );
}

#[test]
fn invalid_visibility_configuration_is_rejected() {
    for (old, new) in [
        ("visibility=\"activity\"", "visibility=\"typo\""),
        ("visible-when=\"finished\"", "visible-when=\"typo\""),
        ("match \"role\" exists=true", "match \"role\""),
        (
            "match \"role\" exists=true",
            "match \"role\" exists=true value=\"x\"",
        ),
        (
            "match \"role\" exists=true",
            "match \"role\" exists=\"yes\"",
        ),
        ("match \"role\" exists=true", "match \"role\" of=\"vessel\""),
        (
            "when kind=\"vessel\" visible-when=\"attempts\"",
            "oops kind=\"vessel\" visible-when=\"attempts\"",
        ),
        ("visibility \"activity\"", "visibility \"\""),
    ] {
        assert!(
            parse_template_config_kdl(&CONFIG.replace(old, new)).is_err(),
            "accepted {new}"
        );
    }
    let nonbool = CONFIG.replace(
        "type=\"bool\" default=false label=\"Finished\" icon=\"F\" persist=true",
        "type=\"enum\" default=\"yes\" label=\"Finished\" icon=\"F\" { value \"yes\"; }",
    );
    assert!(parse_template_config_kdl(&nonbool).is_err());
    let duplicate = format!("{CONFIG}\nvisibility \"activity\" {{}}\n");
    assert!(parse_template_config_kdl(&duplicate).is_err());
    // The new policy must not loosen indexed query validation.
    assert!(parse_template_config_kdl(&CONFIG.replace(
        "match \"attention\" value=\"true\"",
        "match \"attention\" exists=true"
    ))
    .is_err());
}

#[test]
fn json_uses_the_same_policy_validation() {
    use crate::template_config::parse_template_config_json;
    let mut document = serde_json::json!({
        "version": 1,
        "display-variables": [{"name":"show", "type":"bool", "default":true, "label":"Show", "icon":"S", "persist":true}],
        "visibility": [{"name":"policy", "rules":[{"visible-when":"show", "predicates":[{"key":"flag", "exists":true}]}]}]
    });
    // Use a parsed declaration so this test is independent of the variable's wire enum shape.
    let declaration = parse_template_config_kdl(
        "display-variable \"show\" type=\"bool\" default=true label=\"Show\" icon=\"S\"",
    )
    .unwrap()
    .display_variables
    .remove(0);
    document["display-variables"] = serde_json::to_value(vec![declaration]).unwrap();
    assert!(parse_template_config_json(&document.to_string()).is_ok());
    document["visibility"][0]["rules"][0]["predicates"][0]["value"] = "true".into();
    assert!(parse_template_config_json(&document.to_string()).is_err());
}
