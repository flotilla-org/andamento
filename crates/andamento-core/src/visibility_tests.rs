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
        "display-variables": [{"name":"show", "type":{"kind":"bool"}, "default":true, "label":"Show", "icon":"S", "persist":true}],
        "visibility": [{"name":"policy", "rules":[{"visible-when":"show", "predicates":[{"key":"flag", "exists":true}]}]}]
    });
    assert!(parse_template_config_json(&document.to_string()).is_ok());
    document["visibility"][0]["rules"][0]["predicates"][0]["value"] = "true".into();
    assert!(parse_template_config_json(&document.to_string()).is_err());
}

#[test]
fn absence_and_identity_rules_hide_parent_subtrees_until_enabled() {
    let input = CONFIG.replace(
        "when kind=\"vessel\" visible-when=\"finished\" { match \"phase\" value=\"done\"; }",
        "when visible-when=\"finished\" { match \"entity.id\" value=\"parent\"; match \"exempt\" exists=false; }",
    ).replace(
        "for \"vessel\" kind=\"vessel\" visibility=\"activity\" { field \"label\" key=\"entity.id\"; }",
        "for \"vessel\" kind=\"vessel\" visibility=\"activity\" { match \"entity.id\" value=\"parent\"; apply-template \"parent/line\"; }",
    ) + "\ntemplate \"parent/line\" { for \"child\" kind=\"vessel\" { match \"entity.id\" value=\"child\"; }; }\n";
    let mut state = ControllerState::default();
    state.set_template_catalog(Some(config(&input)));
    for id in ["parent", "child"] {
        publish(
            &mut state,
            id,
            &[("display.label", MetadataValue::Text(id.into()))],
        );
    }
    assert!(ids(&state, "tree").is_empty());
    toggle(&mut state, "finished");
    let model = state.view_model();
    let nodes = &model.presentation.as_ref().unwrap().sections[0].nodes;
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].entity.id, "parent");
    assert_eq!(nodes[0].children[0].entity.id, "child");
    toggle(&mut state, "finished");
    assert!(ids(&state, "tree").is_empty());
    publish(
        &mut state,
        "parent",
        &[("exempt", MetadataValue::Bool(false))],
    );
    assert_eq!(ids(&state, "tree"), ["parent"]); // present false is not absence
}

#[test]
fn unused_policies_are_not_evaluated_and_non_boolean_overrides_are_diagnosed() {
    let input = format!(
        "{CONFIG}\nvisibility \"unused\" {{ when kind=\"vessel\" visible-when=\"finished\"; }}"
    );
    let bundled = parse_template_config_kdl(&input).unwrap();
    let user = parse_template_config_kdl("display-variable \"finished\" type=\"enum\" default=\"yes\" label=\"Finished\" icon=\"F\" { value \"yes\"; }").unwrap();
    let catalog = TemplateConfigCatalog::from_layers(vec![
        TemplateConfigLayer::bundled("bundled", bundled),
        TemplateConfigLayer::user("user", user),
    ]);
    assert_eq!(catalog.visibility().len(), 1);
    assert_eq!(catalog.visibility()[0].name, "activity");
    let mut state = ControllerState::default();
    state.set_template_catalog(Some(catalog));
    publish(
        &mut state,
        "done",
        &[("phase", MetadataValue::Text("done".into()))],
    );
    assert!(ids(&state, "tree").is_empty());
    let warnings = state.view_model().template_config.warnings;
    assert!(warnings.iter().any(|warning| warning
        .contains("visibility activity requires boolean display variable finished")));
    assert!(!warnings.iter().any(|warning| warning.contains("unused")));
}

// Both boolean display variables must be true; defaults, toggles, and first-match
// precedence apply to the conjunction just as they do to a single gate.
#[test]
fn conjunctive_visibility_covers_boolean_truth_table() {
    let input = CONFIG.replace(
        "visible-when=\"finished\"",
        "visible-when=\"finished\" and-visible-when=\"attempts\"",
    );
    let mut state = ControllerState::default();
    state.set_template_catalog(Some(config(&input)));
    publish(
        &mut state,
        "done",
        &[("phase", MetadataValue::Text("done".into()))],
    );
    for (finished, attempts) in [(false, false), (true, false), (true, true), (false, true)] {
        assert_eq!(
            ids(&state, "tree"),
            if finished && attempts {
                vec!["done"]
            } else {
                vec![]
            }
        );
        toggle(
            &mut state,
            if finished == attempts {
                "finished"
            } else {
                "attempts"
            },
        );
    }
    assert!(parse_template_config_kdl(&input.replace(
        "and-visible-when=\"attempts\"",
        "and-visible-when=\"missing\""
    ))
    .is_err());
}

// Misconfigured layer overrides must report every variable in a conjunction,
// even when the first variable already hides the matching entities.
#[test]
fn conjunction_reports_both_non_boolean_overrides() {
    let input = CONFIG.replace(
        "visible-when=\"finished\"",
        "visible-when=\"finished\" and-visible-when=\"attempts\"",
    );
    let user = parse_template_config_kdl(r#"
        display-variable "finished" type="enum" default="yes" label="Finished" icon="F" { value "yes"; }
        display-variable "attempts" type="enum" default="yes" label="Attempts" icon="A" { value "yes"; }
    "#).unwrap();
    let mut state = ControllerState::default();
    state.set_template_catalog(Some(TemplateConfigCatalog::from_layers(vec![
        TemplateConfigLayer::bundled("bundled", parse_template_config_kdl(&input).unwrap()),
        TemplateConfigLayer::user("user", user),
    ])));
    let warnings = state.view_model().template_config.warnings;
    for name in ["finished", "attempts"] {
        assert!(warnings
            .iter()
            .any(|warning| warning.contains(&format!("boolean display variable {name}"))));
    }
}
