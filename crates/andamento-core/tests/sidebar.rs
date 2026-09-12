use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::{
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, Sidebar,
};
use std::collections::BTreeMap;

const CONFIG: &str = include_str!("fixtures/sidebar.kdl");
fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef {
        kind: kind.into(),
        id: id.into(),
    }
}
fn patch(subject: EntityRef, facts: &[(&str, MetadataValue)]) -> MetadataPatch {
    MetadataPatch {
        target: MetadataTarget::Entity(subject),
        source_id: "fixture".into(),
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
    }
}
fn text(s: &str) -> MetadataValue {
    MetadataValue::Text(s.into())
}
fn sidebar() -> Sidebar {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    sidebar.apply(
        100,
        [
            patch(
                entity("project", "p"),
                &[
                    ("flotilla.project", text("p")),
                    ("display.label", text("Project P")),
                ],
            ),
            patch(
                entity("vessel", "v"),
                &[
                    ("flotilla.project", text("p")),
                    ("flotilla.vessel", text("v")),
                    ("display.label", text("Worker <one>")),
                    ("status.attention", MetadataValue::Bool(true)),
                    ("status.state", text("waiting")),
                    ("action.primary.target", text("vessel:v")),
                    ("action.primary.recipe", text("printf hello")),
                ],
            ),
        ],
    );
    sidebar
}

#[test]
fn snapshots_carry_typed_content_layout_and_independent_placement_state() {
    let mut sidebar = sidebar();
    let before = sidebar.snapshot();
    assert!(
        before.surface.diagnostics.is_empty(),
        "{:?}",
        before.surface.diagnostics
    );
    let project = &before.surface.sections[0].nodes[0];
    let vessel = &project.children[0];
    let attention = &before.surface.sections[1].nodes[0];
    assert_eq!(vessel.entity, attention.entity);
    assert_ne!(vessel.key, attention.key);
    assert_eq!(vessel.layout.as_deref(), Some("inline"));
    assert_eq!(
        vessel.content.fields[0].source.as_ref().unwrap().value,
        text("Worker <one>")
    );
    sidebar
        .dispatch(Action::SetVariable {
            key: vessel.key.clone(),
            name: "density".into(),
            value: Some("compact".into()),
        })
        .unwrap();
    let updated = sidebar.snapshot();
    assert!(updated
        .surface
        .node(&vessel.key)
        .unwrap()
        .content
        .text()
        .contains("compact"));
    assert!(updated
        .surface
        .node(&attention.key)
        .unwrap()
        .content
        .text()
        .contains("roomy"));
    sidebar
        .dispatch(Action::TogglePlacement {
            key: project.key.clone(),
        })
        .unwrap();
    let collapsed = sidebar.snapshot();
    assert!(collapsed.surface.node(&project.key).unwrap().collapsed);
    assert!(!collapsed.surface.node(&attention.key).unwrap().collapsed);
    assert_eq!(
        collapsed.surface.node(&project.key).unwrap().children.len(),
        1
    );
    assert!(serde_json::to_string(&collapsed).is_ok());
}

#[test]
fn invalid_variable_and_config_updates_leave_the_surface_unchanged() {
    let mut sidebar = sidebar();
    let before = sidebar.snapshot();
    let key = before.surface.sections[0].nodes[0].key.clone();
    assert!(sidebar
        .dispatch(Action::SetVariable {
            key,
            name: "density".into(),
            value: Some("nonsense".into())
        })
        .is_err());
    assert!(sidebar.configure("not { valid").is_err());
    assert_eq!(before, sidebar.snapshot());
}

fn open(sidebar: &mut Sidebar) -> u64 {
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "v"),
        })
        .unwrap();
    match &effects[..] {
        [HostEffect::Materialize {
            request_id,
            recipe,
            entity: subject,
            ..
        }] => {
            assert_eq!(recipe, "printf hello");
            assert_eq!(subject, &entity("vessel", "v"));
            *request_id
        }
        other => panic!("expected materialize, got {other:?}"),
    }
}
fn workspace() -> Workspace {
    Workspace {
        id: 42,
        position: 0,
        name: "Tab 1".into(),
        selected: true,
    }
}

#[test]
fn host_results_preserve_identity_and_closing_returns_to_latent() {
    use andamento_core::presentation::PresentationState;
    let mut sidebar = sidebar();
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].state,
        PresentationState::Latent { openable: true }
    );
    let request = open(&mut sidebar);
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].state,
        PresentationState::Opening
    );
    assert!(sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "v")
        })
        .unwrap()
        .is_empty());
    assert!(sidebar.complete(request, Ok(Some(42))));
    // Acknowledgement can precede the topology observation.
    assert!(sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "v")
        })
        .unwrap()
        .is_empty());
    sidebar.observe(vec![workspace()], vec![]);
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].state,
        PresentationState::Live {
            workspace_id: 42,
            selected: true
        }
    );
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "v"),
        })
        .unwrap();
    let id = match effects[0] {
        HostEffect::Focus {
            request_id,
            workspace_id: 42,
        } => request_id,
        ref other => panic!("{other:?}"),
    };
    sidebar.complete(id, Ok(None));
    sidebar.observe(vec![], vec![]);
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].state,
        PresentationState::Latent { openable: true }
    );
    assert!(open(&mut sidebar) > request);
}

#[test]
fn separate_clients_keep_independent_ui_state() {
    let mut first = sidebar();
    let second = sidebar();
    let key = first.snapshot().surface.sections[0].nodes[0].key.clone();
    first
        .dispatch(Action::TogglePlacement { key: key.clone() })
        .unwrap();
    first
        .dispatch(Action::ToggleDisplayVariable {
            name: "show-issues".into(),
        })
        .unwrap();
    assert!(first.snapshot().surface.node(&key).unwrap().collapsed);
    assert!(!second.snapshot().surface.node(&key).unwrap().collapsed);
    assert_ne!(
        first.snapshot().surface.display_values,
        second.snapshot().surface.display_values
    );
}

#[test]
fn topology_can_precede_acknowledgement_and_stale_results_are_ignored() {
    let mut sidebar = sidebar();
    let request = open(&mut sidebar);
    sidebar.observe(vec![workspace()], vec![]);
    assert!(sidebar.complete(request, Ok(Some(42))));
    assert!(!sidebar.complete(request, Err("late failure".into())));
    assert!(matches!(
        sidebar
            .dispatch(Action::Activate {
                entity: entity("vessel", "v")
            })
            .unwrap()[0],
        HostEffect::Focus {
            workspace_id: 42,
            ..
        }
    ));
}

#[test]
fn failed_materialization_is_visible_serializable_and_retryable() {
    let mut sidebar = sidebar();
    let request = open(&mut sidebar);
    sidebar.complete(request, Err("host unavailable".into()));
    let snapshot = sidebar.snapshot();
    assert_eq!(snapshot.errors[0].message, "host unavailable");
    assert_eq!(
        snapshot.surface.sections[0].nodes[0].children[0].entity,
        entity("vessel", "v")
    );
    serde_json::to_string(&snapshot).unwrap();
    assert!(open(&mut sidebar) > request);
    assert!(sidebar.snapshot().errors.is_empty());
}

#[test]
fn host_time_expires_overrides_without_further_facts_and_retries_refresh_ttl() {
    let mut sidebar = sidebar();
    let override_patch = MetadataPatch {
        target: MetadataTarget::Entity(entity("vessel", "v")),
        source_id: "temporary".into(),
        set: BTreeMap::from([(
            "display.label".into(),
            MetadataValueUpdate {
                value: text("Temporary"),
                ttl_ms: Some(10),
                precedence: Some(100),
                ordinal: None,
            },
        )]),
        unset: vec![],
    };
    sidebar.apply(1000, [override_patch.clone()]);
    sidebar.apply(1005, [override_patch]);
    sidebar.apply(1011, []);
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].label,
        "Temporary"
    );
    sidebar.apply(1016, []);
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].label,
        "Worker <one>"
    );
    sidebar.apply(1, []); // A host clock regression does not revive expired facts.
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].label,
        "Worker <one>"
    );
}

#[test]
fn loop_keys_distinguish_invocations_and_survive_collapse() {
    let mut sidebar = sidebar();
    sidebar.apply(
        101,
        [patch(
            entity("vessel", "v2"),
            &[
                ("flotilla.project", text("p")),
                ("flotilla.vessel", text("v2")),
                ("display.label", text("Second worker")),
            ],
        )],
    );
    let before = sidebar.snapshot();
    let project = &before.surface.sections[0].nodes[0];
    assert_eq!(project.children.len(), 2);
    assert_eq!(project.children[0].loop_key, project.children[1].loop_key);
    assert_ne!(project.loop_key, project.children[0].loop_key);
    assert_ne!(
        before.surface.sections[1].nodes[0].loop_key,
        project.children[0].loop_key
    );
    sidebar
        .dispatch(Action::TogglePlacement {
            key: project.key.clone(),
        })
        .unwrap();
    let after = sidebar.snapshot();
    assert_eq!(
        after.surface.sections[0].nodes[0].children[0].loop_key,
        project.children[0].loop_key
    );
}

#[test]
fn sibling_loop_bindings_must_be_distinct() {
    let config = CONFIG.replace(
        "template \"project/line\" {",
        "template \"project/line\" {\n  for \"vessel\" kind=\"issue\" {}\n",
    );
    let error = Sidebar::new(&config)
        .err()
        .expect("duplicate sibling binding must fail");
    assert!(
        error.contains("duplicate sibling loop binding vessel"),
        "{error}"
    );
}

#[test]
fn native_content_retains_declared_empty_columns() {
    let config = CONFIG.replace(
        "field \"status\" key=\"status.state\"",
        "field \"missing\" key=\"missing.fact\"\n  field \"status\" key=\"status.state\"",
    );
    let mut sidebar = sidebar();
    sidebar.configure(&config).unwrap();
    let snapshot = sidebar.snapshot();
    let fields = &snapshot.surface.sections[0].nodes[0].children[0]
        .content
        .fields;
    assert_eq!(fields[0].value, "Worker <one>");
    assert_eq!(fields[1].value, "");
    assert_eq!(fields[2].value, "waiting");
}

#[test]
fn abbreviation_uses_placement_variable_and_preserves_full_fallback() {
    let mut sidebar = Sidebar::new(include_str!("fixtures/abbreviation.kdl")).unwrap();
    sidebar.apply(
        0,
        [patch(
            entity("vessel", "v"),
            &[
                ("display.label", text("Full worker label")),
                ("flotilla.vessel", text("v")),
                ("display.label.medium", text("Worker")),
            ],
        )],
    );
    let before = sidebar.snapshot();
    let node = &before.surface.sections[0].nodes[0];
    assert_eq!(node.content.text(), "Worker", "{node:#?}");
    assert_eq!(node.label, "Full worker label");
    assert_eq!(node.detail.text(), "Full worker label");
    sidebar
        .dispatch(Action::SetVariable {
            key: node.key.clone(),
            name: "item.tier".into(),
            value: Some("short".into()),
        })
        .unwrap();
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0]
            .content
            .text(),
        "Worker"
    );
    sidebar.apply(
        1,
        [MetadataPatch {
            target: MetadataTarget::Entity(entity("vessel", "v")),
            source_id: "fixture".into(),
            set: BTreeMap::new(),
            unset: vec!["display.label.medium".into()],
        }],
    );
    let after = sidebar.snapshot();
    assert_eq!(
        after.surface.sections[0].nodes[0].content.text(),
        "Full worker label"
    );
    assert_eq!(after.surface.sections[0].nodes[0].key, node.key);
}
