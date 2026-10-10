use andamento_core::presentation::UNPLACED_WORKSPACES_SECTION;
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::{
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, Sidebar,
    WorkspaceId,
};
use std::collections::BTreeMap;

const CONFIG: &str = include_str!("fixtures/sidebar.kdl");
// ABI 2-style IDs, named for use in patterns.
const W7: WorkspaceId = WorkspaceId::from_u64(7);
const W8: WorkspaceId = WorkspaceId::from_u64(8);
const W42: WorkspaceId = WorkspaceId::from_u64(42);
const W43: WorkspaceId = WorkspaceId::from_u64(43);
const W80: WorkspaceId = WorkspaceId::from_u64(80);
fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
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
        id: WorkspaceId::from(42),
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
    assert!(sidebar.complete(request, Ok(Some(WorkspaceId::from(42)))));
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
            workspace_id: WorkspaceId::from(42),
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
            workspace_id: W42,
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
    assert!(sidebar.complete(request, Ok(Some(WorkspaceId::from(42)))));
    assert!(!sidebar.complete(request, Err("late failure".into())));
    assert!(matches!(
        sidebar
            .dispatch(Action::Activate {
                entity: entity("vessel", "v")
            })
            .unwrap()[0],
        HostEffect::Focus {
            workspace_id: W42,
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

#[test]
fn unchanged_heartbeats_and_ticks_reuse_snapshot_until_expiry() {
    let mut sidebar = sidebar();
    let mut heartbeat = patch(entity("vessel", "v"), &[("status.state", text("running"))]);
    heartbeat.set.get_mut("status.state").unwrap().ttl_ms = Some(100);
    sidebar.apply(100, [heartbeat.clone()]);
    let revision = sidebar.revision();
    let snapshot = sidebar.snapshot_shared() as *const _;
    sidebar.apply(150, [heartbeat]);
    sidebar.apply(201, []); // The original deadline has been replaced.
    assert_eq!(sidebar.revision(), revision);
    assert_eq!(sidebar.snapshot_shared() as *const _, snapshot);
    sidebar.apply(250, []); // Entries are live through the deadline, inclusively.
    assert_eq!(sidebar.revision(), revision);
    sidebar.apply(251, []);
    assert_ne!(sidebar.revision(), revision);
    let expired = sidebar.snapshot();
    sidebar.apply(300, []);
    assert_eq!(sidebar.snapshot(), expired);
}

#[test]
fn recipe_only_changes_invalidate_snapshot_and_activation() {
    let mut sidebar = sidebar();
    let revision = sidebar.revision();
    let before = sidebar.snapshot();
    sidebar.apply(
        100,
        [patch(
            entity("vessel", "v"),
            &[("action.primary.recipe", text("printf changed"))],
        )],
    );
    assert_ne!(sidebar.revision(), revision);
    assert_eq!(
        before.surface.sections[0].nodes[0].children[0].content,
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].content
    );
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "v"),
        })
        .unwrap();
    assert!(
        matches!(&effects[0], HostEffect::Materialize { recipe, .. } if recipe == "printf changed")
    );
}

#[test]
fn identical_topology_is_unchanged_but_selection_invalidates() {
    let mut sidebar = sidebar();
    let mut workspaces = vec![Workspace {
        id: WorkspaceId::from(7),
        position: 0,
        name: "Workspace".into(),
        selected: true,
    }];
    sidebar.observe(workspaces.clone(), vec![]);
    let before = sidebar.snapshot();
    sidebar.observe(workspaces.clone(), vec![]);
    assert_eq!(sidebar.snapshot(), before);
    workspaces[0].selected = false;
    sidebar.observe(workspaces, vec![]);
    assert_ne!(sidebar.revision(), before.revision);
}

#[test]
fn removing_last_expired_fact_invalidates_entity_membership() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let mut fact = patch(
        entity("project", "p"),
        &[("display.label", text("Project"))],
    );
    fact.set.get_mut("display.label").unwrap().ttl_ms = Some(10);
    sidebar.apply(100, [fact]);
    sidebar.apply(111, []);
    let revision = sidebar.revision();
    sidebar.apply(
        111,
        [MetadataPatch {
            target: MetadataTarget::Entity(entity("project", "p")),
            source_id: "fixture".into(),
            set: BTreeMap::new(),
            unset: vec!["display.label".into()],
        }],
    );
    assert_ne!(sidebar.revision(), revision);
    assert!(sidebar.snapshot().surface.sections[0].nodes.is_empty());
}

fn fallback(sidebar: &Sidebar) -> Vec<andamento_core::presentation::PlacementNode> {
    sidebar
        .snapshot()
        .surface
        .sections
        .into_iter()
        .find(|s| s.name == UNPLACED_WORKSPACES_SECTION)
        .map(|s| s.nodes)
        .unwrap_or_default()
}

#[test]
fn unplaced_inventory_is_focusable_by_id_and_tracks_rename_selection_and_close() {
    let mut sidebar = sidebar();
    let mut first = workspace();
    let mut second = first.clone();
    second.id = WorkspaceId::from(43);
    second.position = 1;
    second.selected = false;
    sidebar.observe(vec![first.clone(), second.clone()], vec![]);
    let nodes = fallback(&sidebar);
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].label, nodes[1].label);
    assert_ne!(nodes[0].key, nodes[1].key);
    let key = nodes[1].key.clone();
    let effects = sidebar
        .dispatch(Action::ActivatePlacement { key: key.clone() })
        .unwrap();
    let request_id = match effects[0] {
        HostEffect::Focus {
            workspace_id: W43,
            request_id,
        } => request_id,
        ref effect => panic!("{effect:?}"),
    };
    sidebar.complete(request_id, Ok(None));
    first.selected = false;
    second.selected = true;
    second.name = "Renamed".into();
    sidebar.observe(vec![first.clone(), second], vec![]);
    let nodes = fallback(&sidebar);
    assert_eq!(nodes[1].key, key);
    assert_eq!(nodes[1].label, "Renamed");
    assert_eq!(
        nodes[1].state,
        andamento_core::presentation::PresentationState::Live {
            workspace_id: WorkspaceId::from(43),
            selected: true,
        }
    );
    sidebar.observe(vec![first], vec![]);
    assert_eq!(fallback(&sidebar).len(), 1);
    assert!(sidebar.dispatch(Action::ActivatePlacement { key }).is_err());
    sidebar.observe(vec![], vec![]);
    assert!(fallback(&sidebar).is_empty());
    // Hosts anchor workspace creation to this header, so it outlives its rows.
    let snapshot = sidebar.snapshot();
    let section = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == UNPLACED_WORKSPACES_SECTION)
        .expect("fallback section stays present when empty");
    assert!(section.nodes.is_empty() && section.is_empty_workspace_fallback());
}

#[test]
fn placements_cover_aliases_and_collapsed_children_but_filtered_workspaces_fall_back() {
    let mut sidebar = sidebar();
    let request = open(&mut sidebar);
    sidebar.complete(request, Ok(Some(WorkspaceId::from(42))));
    sidebar.observe(vec![workspace()], vec![]);
    assert!(fallback(&sidebar).is_empty());
    let parent = sidebar.snapshot().surface.sections[0].nodes[0].key.clone();
    sidebar
        .dispatch(Action::TogglePlacement { key: parent })
        .unwrap();
    assert!(fallback(&sidebar).is_empty());
    // Hide every provider placement while retaining the observed workspace and
    // its binding. Reconfiguring back restores both aliases without a fallback.
    sidebar
        .configure(&CONFIG.replace("kind=\"vessel\"", "kind=\"unselected-kind\""))
        .unwrap();
    let nodes = fallback(&sidebar);
    assert_eq!(nodes.len(), 1);
    assert!(matches!(
        sidebar
            .dispatch(Action::ActivatePlacement {
                key: nodes[0].key.clone()
            })
            .unwrap()
            .as_slice(),
        [HostEffect::Focus {
            workspace_id: W42,
            ..
        }]
    ));
    sidebar.configure(CONFIG).unwrap();
    assert!(fallback(&sidebar).is_empty());
}

#[test]
fn removed_provider_facts_do_not_remove_open_workspaces() {
    let mut sidebar = sidebar();
    let request = open(&mut sidebar);
    sidebar.complete(request, Ok(Some(WorkspaceId::from(42))));
    sidebar.observe(vec![workspace()], vec![]);
    let mut remove = patch(entity("vessel", "v"), &[]);
    remove.unset = vec![
        "flotilla.project",
        "flotilla.vessel",
        "display.label",
        "status.attention",
        "status.state",
        "action.primary.target",
        "action.primary.recipe",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    sidebar.apply(101, [remove]);
    // Losing facts without a producer identity retraction is unobserved.
    let snapshot = sidebar.snapshot();
    let nodes = workspace_nodes(&snapshot.surface);
    assert!(!nodes.is_empty());
    assert!(fallback(&sidebar).is_empty());
    assert!(nodes
        .iter()
        .all(|node| node.entity == entity("vessel", "v")));
    let key = nodes[0].key.clone();
    assert!(matches!(
        sidebar
            .dispatch(Action::ActivatePlacement { key })
            .unwrap()
            .as_slice(),
        [HostEffect::Focus {
            workspace_id: W42,
            ..
        }]
    ));
}

#[test]
fn compact_placement_template_preserves_independent_detail_template() {
    let mut sidebar = sidebar();
    sidebar
        .configure(&format!(
            r#"{CONFIG}
        template "vessel/detail" slot="detail" node-kind="entity" {{
            field "host" key="flotilla.vessel.host" prefix="Host: "
        }}
    "#
        ))
        .unwrap();
    sidebar.apply(
        101,
        [patch(
            entity("vessel", "v"),
            &[("flotilla.vessel.host", text("remote"))],
        )],
    );
    let snapshot = sidebar.snapshot();
    let vessel = &snapshot.surface.sections[0].nodes[0].children[0];
    assert!(vessel.content.text().contains("Worker <one>"));
    assert_eq!(vessel.detail.text(), "Host: remote");
    assert_eq!(
        snapshot.surface.sections[1].nodes[0].detail.text(),
        "Host: remote"
    );
}

#[test]
fn live_placement_focus_is_deduplicated_until_completion_and_can_retry() {
    let mut sidebar = sidebar();
    let request = open(&mut sidebar);
    sidebar.complete(request, Ok(Some(WorkspaceId::from(42))));
    sidebar.observe(vec![workspace()], vec![]);
    let key = sidebar.snapshot().surface.sections[0].nodes[0].children[0]
        .key
        .clone();
    let action = Action::ActivatePlacement { key };
    for outcome in [Err("focus failed".into()), Ok(None)] {
        let effects = sidebar.dispatch(action.clone()).unwrap();
        let request_id = match effects.as_slice() {
            [HostEffect::Focus {
                request_id,
                workspace_id: W42,
            }] => *request_id,
            other => panic!("expected one focus, got {other:?}"),
        };
        let revision = sidebar.snapshot_shared().revision;
        assert!(sidebar.dispatch(action.clone()).unwrap().is_empty());
        assert!(sidebar
            .dispatch(Action::Activate {
                entity: entity("vessel", "v")
            })
            .unwrap()
            .is_empty());
        assert_eq!(sidebar.snapshot_shared().revision, revision);
        assert!(sidebar.complete(request_id, outcome));
        assert!(!sidebar.complete(request_id, Ok(None)));
    }
}

#[test]
fn managed_primary_reconciles_resolution_not_global_revision() {
    use andamento_core::managed::{ContentState, TerminalContent};
    let mut sidebar = sidebar();
    let subject = entity("vessel", "v");
    let original = TerminalContent {
        target: "one".into(),
        command: "printf hello".into(),
        cwd: None,
    };
    let publish = |sidebar: &mut Sidebar, time, target: &str| {
        sidebar.apply(
            time,
            [patch(
                subject.clone(),
                &[
                    ("workspace.primary.state", text("ready")),
                    ("workspace.primary.target", text(target)),
                    ("action.primary.recipe", text(&format!("attach {target}"))),
                ],
            )],
        );
    };
    sidebar.observe(vec![workspace()], vec![]);
    publish(&mut sidebar, 101, "two");
    let first = sidebar
        .managed
        .plan(WorkspaceId::from(42), subject.clone(), original.clone())
        .update
        .unwrap();
    sidebar.apply(
        102,
        [patch(
            subject.clone(),
            &[("display.label", text("Renamed"))],
        )],
    );
    assert_eq!(
        sidebar
            .managed
            .plan(WorkspaceId::from(42), subject.clone(), original.clone())
            .update,
        Some(first.clone())
    );
    publish(&mut sidebar, 103, "three");
    assert!(!sidebar.managed.valid(WorkspaceId::from(42), first.token));
    assert!(!sidebar
        .managed
        .complete(WorkspaceId::from(42), first.token, true));
    let second = sidebar
        .managed
        .plan(WorkspaceId::from(42), subject.clone(), original.clone())
        .update
        .unwrap();
    assert!(sidebar
        .managed
        .complete(WorkspaceId::from(42), second.token, false));
    assert_eq!(
        sidebar
            .managed
            .plan(WorkspaceId::from(42), subject.clone(), original.clone())
            .state,
        ContentState::Failed
    );
    sidebar.managed.retry(WorkspaceId::from(42));
    let third = sidebar
        .managed
        .plan(WorkspaceId::from(42), subject.clone(), original.clone())
        .update
        .unwrap();
    assert!(sidebar
        .managed
        .complete(WorkspaceId::from(42), third.token, true));
    assert_eq!(
        sidebar
            .managed
            .plan(
                WorkspaceId::from(42),
                subject.clone(),
                third.content.clone()
            )
            .state,
        ContentState::Current
    );
    sidebar.apply(
        104,
        [patch(
            subject.clone(),
            &[("workspace.primary.state", text("held"))],
        )],
    );
    assert_eq!(
        sidebar
            .managed
            .plan(
                WorkspaceId::from(42),
                subject.clone(),
                third.content.clone()
            )
            .state,
        ContentState::Held
    );
    publish(&mut sidebar, 105, "four");
    let fourth = sidebar
        .managed
        .plan(
            WorkspaceId::from(42),
            subject.clone(),
            third.content.clone(),
        )
        .update
        .unwrap();
    let mut expired = patch(subject.clone(), &[]);
    expired.unset = vec!["workspace.primary.state".into()];
    sidebar.apply(106, [expired]);
    assert!(!sidebar.managed.valid(WorkspaceId::from(42), fourth.token));
    assert_eq!(
        sidebar
            .managed
            .plan(
                WorkspaceId::from(42),
                subject.clone(),
                third.content.clone()
            )
            .state,
        ContentState::Unavailable
    );
    publish(&mut sidebar, 107, "four");
    let reconnected = sidebar
        .managed
        .plan(
            WorkspaceId::from(42),
            subject.clone(),
            third.content.clone(),
        )
        .update
        .unwrap();
    sidebar.observe(vec![], vec![]);
    assert!(!sidebar
        .managed
        .valid(WorkspaceId::from(42), reconnected.token));
    let reopened = sidebar
        .managed
        .plan(WorkspaceId::from(42), subject, original)
        .update
        .unwrap();
    assert_ne!(reconnected.token, reopened.token);
}

#[test]
fn managed_content_is_independent_of_placement_and_expires_without_removal() {
    use andamento_core::managed::{ContentState, TerminalContent};
    let mut sidebar = Sidebar::new("").unwrap();
    let subject = entity("project-role", "p/governor");
    let applied = TerminalContent {
        target: "old".into(),
        command: "old".into(),
        cwd: None,
    };
    let mut desired = patch(
        subject.clone(),
        &[
            ("workspace.primary.state", text("ready")),
            ("workspace.primary.target", text("new")),
            ("action.primary.recipe", text("attach new")),
        ],
    );
    for value in desired.set.values_mut() {
        value.ttl_ms = Some(10);
    }
    sidebar.apply(100, [desired.clone()]);
    let update = sidebar
        .managed
        .plan(WorkspaceId::from(42), subject.clone(), applied.clone())
        .update
        .unwrap();
    sidebar.apply(111, []);
    assert!(!sidebar.managed.valid(WorkspaceId::from(42), update.token));
    assert_eq!(
        sidebar
            .managed
            .plan(WorkspaceId::from(42), subject.clone(), applied.clone())
            .state,
        ContentState::Unavailable
    );
    sidebar.apply(112, [desired]);
    let new = sidebar
        .managed
        .plan(WorkspaceId::from(42), subject, applied)
        .update
        .unwrap();
    assert_ne!(new.token, update.token);
}

#[test]
fn materializing_the_current_resolution_records_its_managed_target() {
    use andamento_core::managed::{ContentState, TerminalContent};
    let mut sidebar = sidebar();
    let subject = entity("vessel", "v");
    sidebar.observe(vec![], vec![]);
    let materialize = |sidebar: &mut Sidebar| {
        let effects = sidebar
            .dispatch(Action::Activate {
                entity: subject.clone(),
            })
            .unwrap();
        match &effects[..] {
            [HostEffect::Materialize {
                request_id,
                primary_target,
                ..
            }] => (*request_id, primary_target.clone()),
            other => panic!("expected materialize, got {other:?}"),
        }
    };

    // Without managed facts, a materialization carries no managed target.
    let (request_id, target) = materialize(&mut sidebar);
    assert_eq!(target, None);
    assert!(sidebar.complete(request_id, Err("abandoned".into())));

    sidebar.apply(
        101,
        [patch(
            subject.clone(),
            &[
                ("workspace.primary.state", text("ready")),
                ("workspace.primary.target", text("attempt-2")),
            ],
        )],
    );
    let (_, target) = materialize(&mut sidebar);
    assert_eq!(target.as_deref(), Some("attempt-2"));

    // A host that records the target sees its fresh content as current.
    let applied = TerminalContent {
        target: "attempt-2".into(),
        command: "printf hello".into(),
        cwd: None,
    };
    assert_eq!(
        sidebar
            .managed
            .plan(WorkspaceId::from(7), subject.clone(), applied)
            .state,
        ContentState::Current
    );
}

#[test]
fn detail_templates_list_related_entities_through_their_own_loops() {
    const CONFIG: &str = r#"
region "tree" root-template="root" form="compact" placement="tree"
template "root" slot="compact" node-kind="entity" {
  field "label" source="literal" value="Projects"
}
placement "tree" {
  for "project" kind="project" {
    order "display.label"
    apply-template "project/native"
  }
}
template "project/native" {
  field "label" key="display.label"
}
template "project/detail" slot="detail" node-kind="entity" {
  field "label" key="display.label"
  for "repository" kind="project_repository" {
    match "flotilla.project" of="project"
    order "display.label"
    field "repo" key="display.label" prefix="Repository: "
    field "subpath" key="flotilla.membership.subpath" prefix="Path: "
  }
}
"#;
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let project = |id: &str| {
        patch(
            entity("project", id),
            &[("flotilla.project", text(id)), ("display.label", text(id))],
        )
    };
    let membership = |id: &str, project: &str, label: &str, subpath: Option<&str>| {
        let mut facts = vec![
            ("flotilla.project", text(project)),
            ("display.label", text(label)),
        ];
        if let Some(subpath) = subpath {
            facts.push(("flotilla.membership.subpath", text(subpath)));
        }
        patch(entity("project_repository", id), &facts)
    };
    sidebar.apply(
        100,
        [
            project("alpha"),
            project("beta"),
            membership("m2", "alpha", "zeta", None),
            membership("m1", "alpha", "shared", Some("docs")),
            membership("m3", "beta", "shared", None),
        ],
    );
    let snapshot = sidebar.snapshot();
    let details = |id: &str| {
        let node = snapshot.surface.sections[0]
            .nodes
            .iter()
            .find(|node| node.entity.id == id)
            .unwrap();
        node.detail
            .fields
            .iter()
            .map(|field| field.value.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        details("alpha"),
        [
            "alpha",
            "Repository: shared",
            "Path: docs",
            "Repository: zeta"
        ]
    );
    assert_eq!(details("beta"), ["beta", "Repository: shared"]);
    // Related entities contribute detail only; they are not placed as rows.
    assert!(snapshot.surface.sections[0]
        .nodes
        .iter()
        .all(|node| node.children.is_empty()));
}

#[test]
fn materialized_target_is_recorded_when_the_resolution_has_a_working_directory() {
    use andamento_core::managed::{ContentState, TerminalContent};
    let mut sidebar = sidebar();
    let subject = entity("vessel", "v");
    sidebar.observe(vec![], vec![]);
    sidebar.apply(
        101,
        [patch(
            subject.clone(),
            &[
                ("workspace.primary.state", text("ready")),
                ("workspace.primary.target", text("attempt-2")),
                ("git.root", text("/work/repo")),
            ],
        )],
    );
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: subject.clone(),
        })
        .unwrap();
    let [HostEffect::Materialize {
        cwd,
        primary_target,
        ..
    }] = &effects[..]
    else {
        panic!("expected materialize, got {effects:?}");
    };
    assert_eq!(cwd.as_deref(), Some("/work/repo"));
    assert_eq!(primary_target.as_deref(), Some("attempt-2"));
    let applied = TerminalContent {
        target: "attempt-2".into(),
        command: "printf hello".into(),
        cwd: Some("/work/repo".into()),
    };
    assert_eq!(
        sidebar
            .managed
            .plan(WorkspaceId::from(7), subject, applied)
            .state,
        ContentState::Current
    );
}

#[test]
fn retracted_and_expired_entities_leave_the_catalog() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let placed = |sidebar: &mut Sidebar, id: &str| {
        let snapshot = sidebar.snapshot();
        let mut stack = snapshot
            .surface
            .sections
            .iter()
            .flat_map(|section| section.nodes.iter())
            .collect::<Vec<_>>();
        while let Some(node) = stack.pop() {
            if node.entity.id == id {
                return true;
            }
            stack.extend(node.children.iter());
        }
        false
    };
    let mut retracted = patch(
        entity("project", "gone"),
        &[
            ("flotilla.project", text("gone")),
            ("display.label", text("Gone")),
        ],
    );
    sidebar.apply(101, [retracted.clone()]);
    assert!(placed(&mut sidebar, "gone"));
    retracted.set.clear();
    retracted.unset = vec!["flotilla.project".into(), "display.label".into()];
    sidebar.apply(102, [retracted]);
    assert!(
        !placed(&mut sidebar, "gone"),
        "an entity with no remaining facts is not a catalog entry"
    );

    let mut expiring = patch(
        entity("project", "stale"),
        &[
            ("flotilla.project", text("stale")),
            ("display.label", text("Stale")),
        ],
    );
    for update in expiring.set.values_mut() {
        update.ttl_ms = Some(1_000);
    }
    sidebar.apply(200, [expiring]);
    assert!(placed(&mut sidebar, "stale"));
    sidebar.apply(1_300, []);
    assert!(
        !placed(&mut sidebar, "stale"),
        "expired facts retract the entity"
    );
}

fn directory_state(
    sidebar: &Sidebar,
    subject: &EntityRef,
) -> andamento_core::presentation::PresentationState {
    fn find(
        nodes: &[andamento_core::presentation::PlacementNode],
        subject: &EntityRef,
    ) -> Option<andamento_core::presentation::PresentationState> {
        nodes.iter().find_map(|n| {
            if &n.entity == subject {
                Some(n.state.clone())
            } else {
                find(&n.children, subject)
            }
        })
    }
    sidebar
        .snapshot()
        .surface
        .sections
        .iter()
        .find_map(|s| find(&s.nodes, subject))
        .unwrap()
}
fn directory_workspace(id: u64) -> Workspace {
    Workspace {
        id: WorkspaceId::from(id),
        position: id as usize,
        name: "user terminal".into(),
        selected: true,
    }
}
#[test]
fn directory_observations_focus_any_entity_kind_without_persisting_identity() {
    use andamento_core::presentation::PresentationState;
    let mut sidebar = sidebar();
    let subject = entity("vessel", "v");
    sidebar.apply(
        100,
        [patch(subject.clone(), &[("git.root", text("/repo"))])],
    );
    sidebar.observe(vec![directory_workspace(7)], vec![]);
    sidebar.observe_workdirs(vec![
        (WorkspaceId::from(7), "/repo".into()),
        (WorkspaceId::from(7), "/other".into()),
        (WorkspaceId::from(99), "/repo".into()),
    ]);
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Live {
            workspace_id: W7,
            ..
        }
    ));
    let revision = sidebar.revision();
    sidebar.observe_workdirs(vec![
        (WorkspaceId::from(7), "/other".into()),
        (WorkspaceId::from(7), "/repo".into()),
    ]);
    assert_eq!(sidebar.revision(), revision);
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: subject.clone(),
        })
        .unwrap();
    let HostEffect::Focus {
        request_id,
        workspace_id: W7,
    } = effects[0]
    else {
        panic!("expected focus")
    };
    sidebar.complete(request_id, Ok(None));
    // Exact matching: a subdirectory is not a host-normalized repository root.
    sidebar.observe_workdirs(vec![(WorkspaceId::from(7), "/repo/subdir".into())]);
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Latent { openable: true }
    ));
    sidebar.observe_workdirs(vec![(WorkspaceId::from(7), "/repo".into())]);
    sidebar.observe(vec![], vec![]);
    sidebar.observe(vec![directory_workspace(7)], vec![]);
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Latent { openable: true }
    ));
}
#[test]
fn explicit_binding_wins_and_directory_association_tracks_fact_updates() {
    use andamento_core::presentation::PresentationState;
    let mut sidebar = sidebar();
    let subject = entity("vessel", "v");
    sidebar.apply(
        100,
        [patch(subject.clone(), &[("git.root", text("/repo"))])],
    );
    sidebar.observe(vec![directory_workspace(7), directory_workspace(8)], vec![]);
    sidebar.observe_workdirs(vec![(WorkspaceId::from(7), "/repo".into())]);
    let mut binding = patch(
        subject.clone(),
        &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
    );
    binding.target = MetadataTarget::Tab(WorkspaceId::from(8));
    sidebar.apply(100, [binding]);
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Live {
            workspace_id: W8,
            ..
        }
    ));
    sidebar.observe(vec![directory_workspace(7)], vec![]);
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Live {
            workspace_id: W7,
            ..
        }
    ));
    sidebar.apply(
        101,
        [patch(subject.clone(), &[("git.root", text("/changed"))])],
    );
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Latent { .. }
    ));
    sidebar.observe_workdirs(vec![(WorkspaceId::from(7), "/changed".into())]);
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Live { .. }
    ));
    sidebar.observe_workdirs(vec![]);
    assert!(matches!(
        directory_state(&sidebar, &subject),
        PresentationState::Latent { .. }
    ));
}
#[test]
fn multiple_directories_associate_multiple_entities_but_not_explicit_workspace() {
    use andamento_core::presentation::PresentationState;
    let mut sidebar = sidebar();
    let first = entity("vessel", "v");
    let second = entity("vessel", "second");
    sidebar.apply(
        100,
        [
            patch(first.clone(), &[("git.root", text("/one"))]),
            patch(
                second.clone(),
                &[
                    ("git.root", text("/two")),
                    ("flotilla.project", text("p")),
                    ("action.primary.recipe", text("exec sh")),
                ],
            ),
        ],
    );
    sidebar.observe(vec![directory_workspace(7)], vec![]);
    sidebar.observe_workdirs(vec![
        (WorkspaceId::from(7), "/one".into()),
        (WorkspaceId::from(7), "/two".into()),
    ]);
    for subject in [&first, &second] {
        assert!(matches!(
            directory_state(&sidebar, subject),
            PresentationState::Live {
                workspace_id: W7,
                ..
            }
        ));
    }
    let mut binding = patch(
        first.clone(),
        &[
            ("entity.kind", text("unrelated")),
            ("entity.id", text("explicit")),
        ],
    );
    binding.target = MetadataTarget::Tab(WorkspaceId::from(7));
    sidebar.apply(101, [binding]);
    for subject in [&first, &second] {
        assert!(matches!(
            directory_state(&sidebar, subject),
            PresentationState::Latent { .. }
        ));
    }
}

fn retained_sidebar() -> Sidebar {
    let mut sidebar = sidebar();
    sidebar.configure(&format!("{CONFIG}\ndisplay-variable \"show-finished\" type=\"bool\" default=false label=\"Show finished\" icon=\"F\"\n")).unwrap();
    let request = open(&mut sidebar);
    sidebar.complete(request, Ok(Some(WorkspaceId::from(42))));
    sidebar.observe(vec![workspace()], vec![]);
    sidebar
}

fn workspace_nodes(
    surface: &andamento_core::presentation::SurfaceSnapshot,
) -> Vec<&andamento_core::presentation::PlacementNode> {
    fn collect<'a>(
        nodes: &'a [andamento_core::presentation::PlacementNode],
        result: &mut Vec<&'a andamento_core::presentation::PlacementNode>,
    ) {
        for node in nodes {
            if matches!(
                node.state,
                andamento_core::presentation::PresentationState::Live {
                    workspace_id: W42,
                    ..
                }
            ) {
                result.push(node);
            }
            collect(&node.children, result);
        }
    }
    let mut result = vec![];
    for section in &surface.sections {
        collect(&section.nodes, &mut result);
    }
    result
}

#[test]
fn ended_workspace_lifecycle_scenarios() {
    // Generate every authoritative end signal, repeated publication, toggles,
    // a long empty tick, and user close. Retention has no timeout yet (#123).
    for signal in ["landed", "abandoned", "cancelled", "superseded", "removed"] {
        let mut sidebar = retained_sidebar();
        sidebar.apply(
            101,
            [patch(
                entity("vessel", "v"),
                &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
            )],
        );
        let end = if signal == "superseded" {
            patch(
                entity("vessel", "v"),
                &[("flotilla.convoy.superseded", MetadataValue::Bool(true))],
            )
        } else if signal == "removed" {
            let mut removal = patch(entity("vessel", "v"), &[("source", text("flotilla"))]);
            removal.unset = vec![
                "entity.kind".into(),
                "entity.id".into(),
                "display.label".into(),
                "flotilla.project".into(),
                "flotilla.vessel".into(),
                "status.state".into(),
                "status.attention".into(),
                "action.primary.target".into(),
                "action.primary.recipe".into(),
            ];
            removal
        } else {
            patch(
                entity("vessel", "v"),
                &[("flotilla.convoy.phase", text(signal))],
            )
        };
        for time in [102, 103] {
            sidebar.apply(time, [end.clone()]);
            // Open ended subjects stay on their original path, marked ended,
            // with Show finished off; a generic fallback is insufficient.
            let snapshot = sidebar.snapshot();
            let nodes = workspace_nodes(&snapshot.surface);
            assert!(!nodes.is_empty(), "{signal} at {time}");
            assert!(
                nodes.iter().all(|node| node.entity == entity("vessel", "v")
                    && node.facts.get("presentation.ended") == Some(&MetadataValue::Bool(true))),
                "{signal} at {time}: {nodes:?}"
            );
        }
        sidebar
            .dispatch(Action::ToggleDisplayVariable {
                name: "show-finished".into(),
            })
            .unwrap();
        sidebar.apply(u64::MAX - 1, []);
        let snapshot = sidebar.snapshot();
        // Show finished exposes the original subject and its project path, marked ended.
        let nodes = workspace_nodes(&snapshot.surface);
        assert!(!nodes.is_empty(), "{signal}");
        assert!(
            nodes.iter().all(|node| node.entity == entity("vessel", "v")
                && node.facts.get("presentation.ended") == Some(&MetadataValue::Bool(true))),
            "{signal}: {nodes:?}"
        );
        assert!(snapshot.surface.sections[0].nodes[0]
            .children
            .iter()
            .any(|n| n.entity == entity("vessel", "v")));
        assert!(fallback(&sidebar).is_empty());
        // Reopening a hidden ended row focuses the same workspace, preserving host panels.
        let effects = sidebar
            .dispatch(Action::ActivatePlacement {
                key: nodes[0].key.clone(),
            })
            .unwrap();
        assert!(matches!(
            effects.as_slice(),
            [HostEffect::Focus {
                workspace_id: W42,
                ..
            }]
        ));
        sidebar.observe(vec![], vec![]);
        // Closing is the only retention expiry; there is no phantom live workspace.
        assert!(workspace_nodes(&sidebar.snapshot().surface).is_empty());
    }
}

#[test]
fn disconnect_expiry_and_reconnect_preserve_subject_path_without_ending() {
    let mut sidebar = retained_sidebar();
    let mut publication = patch(
        entity("vessel", "v"),
        &[
            ("entity.kind", text("vessel")),
            ("entity.id", text("v")),
            ("flotilla.project", text("p")),
            ("display.label", text("Worker")),
            ("action.primary.target", text("vessel:v")),
            ("action.primary.recipe", text("printf hello")),
        ],
    );
    for update in publication.set.values_mut() {
        update.ttl_ms = Some(10);
    }
    sidebar.apply(101, [publication.clone()]);
    // A disconnect is an empty drain. Even beyond the fact lease, the bound
    // workspace remains under its subject and does not require Show finished.
    for time in [111, 112, 1000] {
        sidebar.apply(time, []);
        let snapshot = sidebar.snapshot();
        let nodes = workspace_nodes(&snapshot.surface);
        assert!(!nodes.is_empty());
        // The original project path must survive, even if Attention remains.
        assert!(
            snapshot.surface.sections[0].nodes[0]
                .children
                .iter()
                .any(|n| n.entity == entity("vessel", "v")),
            "lost project path at {time}"
        );
        assert!(nodes
            .iter()
            .all(|n| n.entity == entity("vessel", "v")
                && !n.facts.contains_key("presentation.ended")));
        assert!(fallback(&sidebar).is_empty());
    }
    sidebar.apply(1001, [publication]);
    // Reasserting after reconnect updates the same subject, without moving it.
    let snapshot = sidebar.snapshot();
    assert!(workspace_nodes(&snapshot.surface)
        .iter()
        .all(|n| n.entity == entity("vessel", "v") && !n.facts.contains_key("presentation.ended")));
}

#[test]
fn unrelated_source_removal_and_subjectless_workspaces_are_unaffected() {
    let mut sidebar = retained_sidebar();
    let mut removal = patch(entity("vessel", "v"), &[]);
    removal.source_id = "unrelated".into();
    removal.unset = vec!["entity.kind".into(), "entity.id".into()];
    sidebar.apply(101, [removal]);
    // Only the producer owning identity can remove the subject.
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
    sidebar.observe(
        vec![
            workspace(),
            Workspace {
                id: WorkspaceId::from(99),
                position: 1,
                name: "Unsaved notes".into(),
                selected: false,
            },
        ],
        vec![],
    );
    // Workspaces with no subject retain normal fallback coverage.
    assert!(sidebar
        .snapshot()
        .surface
        .sections
        .iter()
        .any(|s| s.name == UNPLACED_WORKSPACES_SECTION
            && s.nodes.iter().any(|n| n.label == "Unsaved notes")));
}

#[test]
fn nonterminal_convoy_phases_do_not_end_workspace() {
    // Failed/interrupted convoys can resume, and a standing role outlives its
    // terminal attempt. Generate all non-ending convoy phases explicitly.
    for phase in [
        "pending",
        "active",
        "interrupted",
        "failed",
        "anchored",
        "landing",
    ] {
        let mut sidebar = retained_sidebar();
        sidebar.apply(
            101,
            [patch(
                entity("vessel", "v"),
                &[("flotilla.convoy.phase", text(phase))],
            )],
        );
        assert!(
            !workspace_nodes(&sidebar.snapshot().surface).is_empty(),
            "{phase}"
        );
    }
}

#[test]
fn standing_role_outlives_terminal_attempt_phase() {
    // Lifted attempt facts are not a terminal phase of the standing role.
    let mut sidebar = Sidebar::new(&format!("{}\ndisplay-variable \"show-finished\" type=\"bool\" default=false label=\"Show finished\" icon=\"F\"\n",
        CONFIG.replace("vessel", "role"))).unwrap();
    sidebar.apply(
        100,
        [
            patch(
                entity("project", "p"),
                &[
                    ("flotilla.project", text("p")),
                    ("display.label", text("Project")),
                ],
            ),
            patch(
                entity("role", "r"),
                &[
                    ("flotilla.project", text("p")),
                    ("display.label", text("Governor")),
                    ("action.primary.recipe", text("exec /bin/sh")),
                    ("flotilla.convoy.phase", text("landed")),
                ],
            ),
        ],
    );
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: entity("role", "r"),
        })
        .unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    sidebar.complete(*request_id, Ok(Some(WorkspaceId::from(42))));
    sidebar.observe(vec![workspace()], vec![]);
    sidebar.apply(101, []);
    let snapshot = sidebar.snapshot();
    assert!(!workspace_nodes(&snapshot.surface).is_empty());
    assert!(workspace_nodes(&snapshot.surface)
        .iter()
        .all(|n| n.entity == entity("role", "r") && !n.facts.contains_key("presentation.ended")));
}

#[test]
fn authoritative_ancestor_removal_retains_workspace_path_as_ended() {
    let mut sidebar = retained_sidebar();
    sidebar.apply(
        101,
        [patch(
            entity("project", "p"),
            &[("entity.kind", text("project")), ("entity.id", text("p"))],
        )],
    );
    let mut removal = patch(entity("project", "p"), &[("source", text("flotilla"))]);
    removal.unset = vec![
        "entity.kind".into(),
        "entity.id".into(),
        "flotilla.project".into(),
        "display.label".into(),
    ];
    sidebar.apply(102, [removal]);
    // A removed ancestor is authoritative, unlike a lease-expired ancestor.
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
    sidebar
        .dispatch(Action::ToggleDisplayVariable {
            name: "show-finished".into(),
        })
        .unwrap();
    let snapshot = sidebar.snapshot();
    let project = &snapshot.surface.sections[0].nodes[0];
    assert_eq!(project.entity, entity("project", "p"));
    assert!(project.facts.get("presentation.ended") == Some(&MetadataValue::Bool(true)));
    assert!(workspace_nodes(&snapshot.surface)
        .iter()
        .any(|n| n.entity == entity("vessel", "v")
            && n.facts.get("presentation.ended") == Some(&MetadataValue::Bool(true))));
}

#[test]
fn reassertion_after_authoritative_removal_is_stale_for_the_same_identity() {
    let mut sidebar = retained_sidebar();
    let publication = patch(
        entity("vessel", "v"),
        &[
            ("entity.kind", text("vessel")),
            ("entity.id", text("v")),
            ("display.label", text("Worker")),
            ("flotilla.project", text("p")),
            ("action.primary.target", text("vessel:v")),
            ("action.primary.recipe", text("printf hello")),
        ],
    );
    sidebar.apply(101, [publication.clone()]);
    let mut removal = patch(entity("vessel", "v"), &[]);
    removal.unset = vec!["entity.kind".into(), "entity.id".into()];
    sidebar.apply(102, [removal]);
    sidebar.apply(103, [publication]);
    // An authoritative end is terminal for this identity. A new generation
    // needs a new subject ID; reconnect reassertions do not revive the old one.
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
    sidebar
        .dispatch(Action::ToggleDisplayVariable {
            name: "show-finished".into(),
        })
        .unwrap();
    assert!(workspace_nodes(&sidebar.snapshot().surface)
        .iter()
        .any(|n| n.facts.get("presentation.ended") == Some(&MetadataValue::Bool(true))));
}

#[test]
fn lease_expiry_inside_a_patch_batch_preserves_the_original_path() {
    let mut sidebar = retained_sidebar();
    let mut publication = patch(
        entity("project", "p"),
        &[
            ("entity.kind", text("project")),
            ("entity.id", text("p")),
            ("flotilla.project", text("p")),
            ("display.label", text("Project P")),
        ],
    );
    for fact in publication.set.values_mut() {
        fact.ttl_ms = Some(10);
    }
    sidebar.apply(101, [publication]);
    sidebar.apply(
        500,
        [patch(
            entity("unrelated", "heartbeat"),
            &[("source", text("unrelated"))],
        )],
    );
    // Capture is before the clock advance even when a batch also has patches.
    let snapshot = sidebar.snapshot();
    assert!(snapshot.surface.sections[0].nodes[0]
        .children
        .iter()
        .any(|n| n.entity == entity("vessel", "v")));
    assert!(workspace_nodes(&snapshot.surface)
        .iter()
        .all(|n| !n.facts.contains_key("presentation.ended")));
}

#[test]
fn closing_many_removed_workspaces_releases_retained_catalog_history() {
    let mut sidebar = retained_sidebar();
    sidebar.observe(vec![], vec![]);
    sidebar
        .dispatch(Action::ToggleDisplayVariable {
            name: "show-finished".into(),
        })
        .unwrap();
    // Generate many sequential identities on the same client. After each close,
    // its removed subject must leave the surface rather than grow retained history.
    for index in 0..24 {
        let subject = entity("vessel", &format!("history-{index}"));
        let publication = patch(
            subject.clone(),
            &[
                ("entity.kind", text("vessel")),
                ("entity.id", text(&subject.id)),
                ("display.label", text(&subject.id)),
                ("flotilla.project", text("p")),
                ("action.primary.recipe", text("exec /bin/sh")),
            ],
        );
        sidebar.apply(200 + index * 2, [publication.clone()]);
        let effects = sidebar
            .dispatch(Action::Activate {
                entity: subject.clone(),
            })
            .unwrap();
        let [HostEffect::Materialize { request_id, .. }] = effects.as_slice() else {
            panic!("{effects:?}")
        };
        sidebar.complete(*request_id, Ok(Some(WorkspaceId::from(42))));
        sidebar.observe(vec![workspace()], vec![]);
        let mut removal = patch(subject.clone(), &[("source", text("flotilla"))]);
        removal.unset = publication.set.keys().cloned().collect();
        sidebar.apply(201 + index * 2, [removal]);
        assert!(workspace_nodes(&sidebar.snapshot().surface)
            .iter()
            .any(|n| n.entity == subject));
        sidebar.observe(vec![], vec![]);
        let snapshot = sidebar.snapshot();
        assert!(
            !snapshot.surface.sections[0].nodes[0]
                .children
                .iter()
                .any(|n| n.entity.id.starts_with("history-")),
            "retention grew after close {index}"
        );
    }
}

#[test]
fn individual_fact_unsets_are_not_resurrected_from_retained_history() {
    let mut sidebar = retained_sidebar();
    let mut update = patch(entity("vessel", "v"), &[]);
    update.unset = vec!["status.state".into(), "status.attention".into()];
    sidebar.apply(101, [update]);
    // Explicit fact withdrawal on a live subject removes those facts, even
    // though the workspace still retains its subject and project path.
    let snapshot = sidebar.snapshot();
    let nodes = workspace_nodes(&snapshot.surface);
    assert!(!nodes.is_empty());
    assert!(nodes.iter().all(
        |n| !n.facts.contains_key("status.state") && !n.facts.contains_key("status.attention")
    ));
    assert!(snapshot.surface.sections[1].nodes.is_empty());
}

#[test]
fn shared_ancestor_survives_one_child_end_and_close() {
    let mut sidebar = retained_sidebar();
    sidebar.apply(
        101,
        [
            patch(
                entity("vessel", "sibling"),
                &[
                    ("entity.kind", text("vessel")),
                    ("entity.id", text("sibling")),
                    ("display.label", text("Sibling")),
                    ("flotilla.project", text("p")),
                    ("action.primary.recipe", text("exec /bin/sh")),
                ],
            ),
            patch(
                entity("vessel", "v"),
                &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
            ),
        ],
    );
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "sibling"),
        })
        .unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    sidebar.complete(*request_id, Ok(Some(WorkspaceId::from(80))));
    let sibling = Workspace {
        id: WorkspaceId::from(80),
        position: 1,
        name: "Sibling".into(),
        selected: false,
    };
    sidebar.observe(vec![workspace(), sibling.clone()], vec![]);
    let mut removal = patch(entity("vessel", "v"), &[("source", text("flotilla"))]);
    removal.unset = vec![
        "entity.kind",
        "entity.id",
        "display.label",
        "flotilla.project",
        "flotilla.vessel",
        "status.state",
        "status.attention",
        "action.primary.target",
        "action.primary.recipe",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    sidebar.apply(102, [removal]);
    // Ending one child must not end its shared parent or sibling workspace.
    let snapshot = sidebar.snapshot();
    let project = &snapshot.surface.sections[0].nodes[0];
    assert!(!project.facts.contains_key("presentation.ended"));
    assert!(!workspace_nodes(&snapshot.surface).is_empty());
    assert!(project.children.iter().any(|n| n.entity.id == "sibling"
        && matches!(
            n.state,
            andamento_core::presentation::PresentationState::Live {
                workspace_id: W80,
                ..
            }
        )
        && !n.facts.contains_key("presentation.ended")));
    sidebar
        .dispatch(Action::ToggleDisplayVariable {
            name: "show-finished".into(),
        })
        .unwrap();
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
    sidebar.observe(vec![sibling], vec![]);
    // Closing just the ended workspace releases its history while the shared
    // ancestor remains present for the other open workspace.
    let snapshot = sidebar.snapshot();
    let project = &snapshot.surface.sections[0].nodes[0];
    assert!(project.children.iter().all(|n| n.entity.id != "v"));
    assert!(project.children.iter().any(|n| n.entity.id == "sibling"));
}

#[test]
fn one_producer_retraction_preserves_other_identity_owner() {
    let mut sidebar = retained_sidebar();
    let identity = patch(
        entity("vessel", "v"),
        &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
    );
    let mut other = identity.clone();
    other.source_id = "other".into();
    sidebar.apply(101, [identity, other]);
    let mut remove = patch(entity("vessel", "v"), &[]);
    remove.unset = vec!["entity.kind".into(), "entity.id".into()];
    sidebar.apply(102, [remove.clone()]);
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
    assert!(workspace_nodes(&sidebar.snapshot().surface)
        .iter()
        .all(|n| !n.facts.contains_key("presentation.ended")));
    remove.source_id = "other".into();
    sidebar.apply(103, [remove]);
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
    sidebar
        .dispatch(Action::ToggleDisplayVariable {
            name: "show-finished".into(),
        })
        .unwrap();
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
}

#[test]
fn individual_fact_expiry_is_not_resurrected_on_live_subject() {
    let mut sidebar = retained_sidebar();
    let mut update = patch(
        entity("vessel", "v"),
        &[("status.attention", MetadataValue::Bool(true))],
    );
    update.set.get_mut("status.attention").unwrap().ttl_ms = Some(10);
    sidebar.apply(101, [update]);
    assert!(workspace_nodes(&sidebar.snapshot().surface)
        .iter()
        .any(|n| n.facts.contains_key("status.attention")));
    sidebar.apply(112, []);
    let snapshot = sidebar.snapshot();
    assert!(!workspace_nodes(&snapshot.surface).is_empty());
    assert!(workspace_nodes(&snapshot.surface)
        .iter()
        .all(|n| !n.facts.contains_key("status.attention")
            && !n.facts.contains_key("presentation.ended")));
    assert!(snapshot.surface.sections[1].nodes.is_empty());
}

#[test]
fn removal_after_first_topology_needs_no_intervening_snapshot() {
    let mut sidebar = sidebar();
    sidebar.configure(&format!("{CONFIG}\ndisplay-variable \"show-finished\" type=\"bool\" default=false label=\"Show finished\" icon=\"F\"\n")).unwrap();
    sidebar.apply(
        101,
        [patch(
            entity("vessel", "v"),
            &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
        )],
    );
    let request = open(&mut sidebar);
    sidebar.complete(request, Ok(Some(WorkspaceId::from(42))));
    sidebar.observe(vec![workspace()], vec![]);
    let mut removal = patch(entity("vessel", "v"), &[]);
    removal.unset = vec![
        "entity.kind".into(),
        "entity.id".into(),
        "display.label".into(),
    ];
    sidebar.apply(102, [removal]);
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
    sidebar
        .dispatch(Action::ToggleDisplayVariable {
            name: "show-finished".into(),
        })
        .unwrap();
    assert!(!workspace_nodes(&sidebar.snapshot().surface).is_empty());
}

#[test]
fn shared_action_targets_do_not_steal_subject_workspace_or_selection() {
    // Both subjects remain bound to their own workspace when their activation
    // targets collide. Generate both selections and both collapse states.
    use andamento_core::presentation::PresentationState;
    let mut sidebar = retained_sidebar();
    let other = entity("vessel", "other");
    sidebar.apply(
        101,
        [patch(
            other.clone(),
            &[
                ("flotilla.project", text("p")),
                ("display.label", text("Other subject")),
                ("action.primary.target", text("vessel:v")),
            ],
        )],
    );
    let mut binding = patch(
        other.clone(),
        &[
            ("entity.kind", text("vessel")),
            ("entity.id", text("other")),
        ],
    );
    binding.target = MetadataTarget::Tab(WorkspaceId::from(43));
    sidebar.apply(102, [binding]);
    for selected in [42, 43] {
        for collapsed in [false, true] {
            sidebar.observe(
                vec![
                    Workspace {
                        id: WorkspaceId::from(42),
                        position: 0,
                        name: "Vessel".into(),
                        selected: selected == 42,
                    },
                    Workspace {
                        id: WorkspaceId::from(43),
                        position: 1,
                        name: "Other".into(),
                        selected: selected == 43,
                    },
                ],
                vec![],
            );
            let snapshot = sidebar.snapshot();
            let project = &snapshot.surface.sections[0].nodes[0];
            if project.collapsed != collapsed {
                sidebar
                    .dispatch(Action::TogglePlacement {
                        key: project.key.clone(),
                    })
                    .unwrap();
            }
            let snapshot = sidebar.snapshot();
            let project = &snapshot.surface.sections[0].nodes[0];
            for (subject, id) in [(entity("vessel", "v"), 42), (other.clone(), 43)] {
                let node = project
                    .children
                    .iter()
                    .find(|node| node.entity == subject)
                    .unwrap();
                assert_eq!(
                    node.state,
                    PresentationState::Live {
                        workspace_id: WorkspaceId::from(id),
                        selected: selected == id
                    }
                );
            }
        }
    }
}

#[test]
fn duplicate_subject_bindings_prefer_selection_then_inventory_order() {
    // Duplicated subject identities prefer the selected workspace irrespective
    // of inventory order; without selection, the first inventory ID wins.
    use andamento_core::presentation::PresentationState;
    let mut sidebar = retained_sidebar();
    let mut binding = patch(
        entity("vessel", "v"),
        &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
    );
    binding.target = MetadataTarget::Tab(WorkspaceId::from(43));
    sidebar.apply(101, [binding]);
    for order in [[42, 43], [43, 42]] {
        for selected in [None, Some(42), Some(43)] {
            sidebar.observe(
                order
                    .iter()
                    .enumerate()
                    .map(|(position, id)| Workspace {
                        id: WorkspaceId::from(*id),
                        position,
                        name: "Duplicate".into(),
                        selected: selected == Some(*id),
                    })
                    .collect(),
                vec![],
            );
            let snapshot = sidebar.snapshot();
            let node = &snapshot.surface.sections[0].nodes[0].children[0];
            assert_eq!(
                node.state,
                PresentationState::Live {
                    workspace_id: WorkspaceId::from(selected.unwrap_or(order[0])),
                    selected: selected.is_some()
                }
            );
        }
    }
}

#[test]
fn unbound_action_target_keeps_selection_on_its_alias() {
    // Action-only host metadata has no subject identity; the matching entity
    // remains selected so a workspace with an unbound subject is reachable.
    use andamento_core::presentation::PresentationState;
    let mut sidebar = sidebar();
    let mut binding = patch(
        entity("vessel", "v"),
        &[("action.primary.target", text("vessel:v"))],
    );
    binding.target = MetadataTarget::Tab(WorkspaceId::from(42));
    sidebar.apply(101, [binding]);
    sidebar.observe(vec![workspace()], vec![]);
    assert_eq!(
        sidebar.snapshot().surface.sections[0].nodes[0].children[0].state,
        PresentationState::Live {
            workspace_id: WorkspaceId::from(42),
            selected: true
        }
    );
}

#[test]
fn open_ended_path_overrides_non_finished_visibility_policy_until_close() {
    // Reachability of an open ended subject takes precedence over arbitrary
    // display filters, not only Show finished. Close restores normal filtering.
    let mut sidebar = retained_sidebar();
    let filtered_config = CONFIG.replace(
        "for \"vessel\" kind=\"vessel\"",
        "for \"vessel\" kind=\"vessel\" visibility=\"private\"",
    );
    assert_ne!(
        filtered_config, CONFIG,
        "visibility fixture replacement must match"
    );
    let config = filtered_config
        + "\ndisplay-variable \"private\" type=\"bool\" default=false label=\"Private\" icon=\"P\"\nvisibility \"private\" { when kind=\"vessel\" visible-when=\"private\"; }\n";
    sidebar.configure(&config).unwrap();
    sidebar.apply(
        102,
        [patch(
            entity("vessel", "v"),
            &[("flotilla.convoy.phase", text("landed"))],
        )],
    );
    let snapshot = sidebar.snapshot();
    assert!(snapshot.surface.sections[0].nodes[0]
        .children
        .iter()
        .any(|node| node.entity == entity("vessel", "v")
            && node.facts.get("presentation.ended") == Some(&MetadataValue::Bool(true))));
    sidebar.observe(vec![], vec![]);
    assert!(sidebar.snapshot().surface.sections[0].nodes[0]
        .children
        .is_empty());
}

fn vessel_patch(id: &str, label: &str) -> MetadataPatch {
    patch(
        entity("vessel", id),
        &[
            ("flotilla.project", text("p")),
            ("flotilla.vessel", text(id)),
            ("display.label", text(label)),
        ],
    )
}

fn vessel_ids(sidebar: &Sidebar) -> Vec<String> {
    sidebar.snapshot().surface.sections[0].nodes[0]
        .children
        .iter()
        .map(|n| n.entity.id.clone())
        .collect()
}

#[test]
fn host_sibling_order_reorders_a_run_and_merges_changes_by_data_order() {
    // Two sidebars with the same data: `data` never has a host order, so it
    // shows Andamento's own order for comparison.
    let mut ordered = sidebar();
    let mut data = sidebar();
    for s in [&mut ordered, &mut data] {
        s.apply(
            101,
            ["a", "b", "c"].map(|id| vessel_patch(id, &id.to_uppercase())),
        );
    }
    let base = vessel_ids(&data);
    assert_eq!(base.len(), 4);
    let first = ordered.snapshot().surface.sections[0].nodes[0].children[0].clone();
    let loop_key = first.loop_key.clone();
    // Every sibling in the run shares the loop key the host uses.
    assert!(ordered.snapshot().surface.sections[0].nodes[0]
        .children
        .iter()
        .all(|n| n.loop_key == loop_key));

    // A full order replaces data order.
    let reversed: Vec<String> = base.iter().rev().cloned().collect();
    ordered.set_sibling_order(
        loop_key.clone(),
        reversed.iter().map(|id| entity("vessel", id)).collect(),
    );
    assert_eq!(vessel_ids(&ordered), reversed);

    // A sibling the order doesn't name goes right after its nearest data-order
    // predecessor that the order places.
    for s in [&mut ordered, &mut data] {
        s.apply(102, [vessel_patch("bb", "BB")]);
    }
    let data_now = vessel_ids(&data);
    let at = data_now.iter().position(|id| id == "bb").unwrap();
    let mut expected = reversed.clone();
    match at.checked_sub(1).map(|i| &data_now[i]) {
        Some(before) => {
            let p = expected.iter().position(|id| id == before).unwrap();
            expected.insert(p + 1, "bb".into());
        }
        None => expected.insert(0, "bb".into()),
    }
    assert_eq!(vessel_ids(&ordered), expected);

    // A sibling that leaves the run drops out; the rest keep the saved order.
    let leave = MetadataPatch {
        unset: vec!["flotilla.project".into()],
        ..patch(entity("vessel", "c"), &[])
    };
    ordered.apply(103, [leave]);
    expected.retain(|id| id != "c");
    assert_eq!(vessel_ids(&ordered), expected);

    // Other runs are untouched, and an empty order returns to data order.
    assert_eq!(ordered.snapshot().surface.sections[1].nodes.len(), 1);
    ordered.set_sibling_order(loop_key.clone(), vec![]);
    let mut data_without_c = vessel_ids(&data);
    data_without_c.retain(|id| id != "c");
    assert_eq!(vessel_ids(&ordered), data_without_c);

    // A repeated name places its row once, at its first mention.
    let last = data_without_c.last().unwrap().clone();
    let repeated = [&last, &data_without_c[0], &last].map(|id| entity("vessel", id));
    ordered.set_sibling_order(loop_key, repeated.to_vec());
    let shown = vessel_ids(&ordered);
    assert_eq!(shown.len(), data_without_c.len());
    assert_eq!(shown[..2], [last, data_without_c[0].clone()]);
}

#[test]
fn placement_loop_keys_round_trip_through_their_host_text() {
    let key = andamento_core::PlacementLoopKey {
        region: "tree".into(),
        parent: andamento_core::PlacementKey(vec![andamento_core::PlacementSegment {
            loop_name: "project".into(),
            entity: entity("project", "a:b 3:x é"),
        }]),
        binding: "vessel".into(),
    };
    let text = key.encode();
    assert_eq!(andamento_core::PlacementLoopKey::decode(&text), Some(key));
    for bad in [
        "",
        "4:tree",
        "x:tree",
        "9:tree5:vessel",
        "4:tree1:a1:b5:vessel",
        "99999999999999999999:tree5:vessel",
    ] {
        assert_eq!(andamento_core::PlacementLoopKey::decode(bad), None, "{bad}");
    }
}

const HOST_ENTITY_CONFIG: &str = r#"
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
  for "workspace" kind=".workspace" {
    match "flotilla.project" of="project"
    apply-template
  }
}
template ".workspace/line" {
  field "label" key="display.label"
}
"#;

fn tab_patch(tab: u64, facts: &[(&str, MetadataValue)]) -> MetadataPatch {
    let mut patch = patch(entity("unused", "unused"), facts);
    patch.target = MetadataTarget::Tab(WorkspaceId::from(tab));
    patch
}

fn project_children(sidebar: &Sidebar) -> Vec<andamento_core::presentation::PlacementNode> {
    sidebar
        .snapshot()
        .surface
        .sections
        .into_iter()
        .find(|s| s.name == "tree")
        .and_then(|s| s.nodes.into_iter().next())
        .map(|project| project.children)
        .unwrap_or_default()
}

/// A host entity (`.host.kind`/`.host.id` tab metadata) is an entity the host owns
/// for one workspace: placed like any entity and live for its tab, covering
/// it, but not its subject, so closing the tab retains nothing.
#[test]
fn host_entities_place_and_cover_their_workspace_without_subject_lifecycle() {
    let mut sidebar = Sidebar::new(HOST_ENTITY_CONFIG).unwrap();
    let local = entity(".workspace", "w1");
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
                local.clone(),
                &[
                    ("display.label", text("Local")),
                    ("flotilla.project", text("p")),
                ],
            ),
            tab_patch(
                42,
                &[(".host.kind", text(".workspace")), (".host.id", text("w1"))],
            ),
        ],
    );
    sidebar.observe(vec![workspace()], vec![]);
    let live = andamento_core::presentation::PresentationState::Live {
        workspace_id: WorkspaceId::from(42),
        selected: true,
    };
    // Homed with its project: placed there, live, and so not left over.
    let children = project_children(&sidebar);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].entity, local);
    assert_eq!(children[0].state, live);
    assert!(fallback(&sidebar).is_empty());
    let card = sidebar
        .detail_card(&local)
        .expect("a host entity has details");
    assert_eq!(card.workspace_id, Some(WorkspaceId::from(42)));

    // Unhomed: left over, under its own identity rather than a synthetic one.
    let mut unhome = patch(local.clone(), &[]);
    unhome.unset = vec!["flotilla.project".into()];
    sidebar.apply(200, [unhome]);
    assert!(project_children(&sidebar).is_empty());
    let nodes = fallback(&sidebar);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].entity, local);
    assert_eq!(nodes[0].state, live);

    // Closed: nothing is retained for it, as it was never the tab's subject.
    sidebar.observe(vec![], vec![]);
    assert!(fallback(&sidebar).is_empty());
    let snapshot = sidebar.snapshot();
    assert!(snapshot
        .surface
        .sections
        .iter()
        .all(|s| s.nodes.iter().all(|n| n.entity != local)));
}

/// Two tabs naming one host entity both stay covered, under distinct keys;
/// a tab with only half of the host entity metadata keeps the synthetic one.
#[test]
fn shared_or_incomplete_host_entities_keep_every_workspace_covered() {
    let mut sidebar = Sidebar::new(HOST_ENTITY_CONFIG).unwrap();
    let local = entity(".workspace", "w1");
    let host = [(".host.kind", text(".workspace")), (".host.id", text("w1"))];
    sidebar.apply(
        100,
        [
            patch(local.clone(), &[("display.label", text("Local"))]),
            tab_patch(42, &host),
            tab_patch(43, &host),
            tab_patch(44, &[(".host.kind", text(".workspace"))]),
        ],
    );
    let mut second = workspace();
    second.id = WorkspaceId::from(43);
    second.position = 1;
    second.selected = false;
    let mut third = second.clone();
    third.id = WorkspaceId::from(44);
    third.position = 2;
    sidebar.observe(vec![workspace(), second, third], vec![]);
    let nodes = fallback(&sidebar);
    assert_eq!(nodes.len(), 3);
    let keys = nodes
        .iter()
        .map(|n| n.key.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys.len(), 3, "each covered tab keeps a distinct key");
    assert_eq!(nodes.iter().filter(|n| n.entity == local).count(), 1);
    assert!(nodes.iter().any(|n| n.entity == entity(".workspace", "44")));
}

/// A tab may have both a subject and a host entity: each is live for it.
#[test]
fn a_tab_with_a_subject_and_a_host_entity_makes_both_live() {
    let mut sidebar = Sidebar::new(HOST_ENTITY_CONFIG).unwrap();
    let subject = entity(".workspace", "subject");
    let local = entity(".workspace", "w1");
    let homed = |label: &str| {
        [
            ("display.label", text(label)),
            ("flotilla.project", text("p")),
        ]
    };
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
            patch(subject.clone(), &homed("Subject")),
            patch(local.clone(), &homed("Local")),
            tab_patch(
                42,
                &[
                    ("entity.kind", text(".workspace")),
                    ("entity.id", text("subject")),
                    (".host.kind", text(".workspace")),
                    (".host.id", text("w1")),
                ],
            ),
        ],
    );
    sidebar.observe(vec![workspace()], vec![]);
    let live = andamento_core::presentation::PresentationState::Live {
        workspace_id: WorkspaceId::from(42),
        selected: true,
    };
    let children = project_children(&sidebar);
    for wanted in [&subject, &local] {
        let node = children
            .iter()
            .find(|n| &n.entity == wanted)
            .expect("placed");
        assert_eq!(node.state, live, "{wanted:?} is live for the tab");
    }
    // Either one focuses the tab, rather than falling back to Inspect.
    for wanted in [subject, local] {
        let effects = sidebar
            .dispatch(Action::Activate {
                entity: wanted.clone(),
            })
            .unwrap();
        assert!(
            matches!(
                effects[..],
                [HostEffect::Focus {
                    workspace_id: W42,
                    ..
                }]
            ),
            "{wanted:?} focuses its tab: {effects:?}"
        );
    }
}

const LOCAL_SECTIONS_CONFIG: &str = r#"
region "local" root-template="local/title" form="compact" placement="local"
region "attention" root-template="attention/title" form="compact" placement="attention"
template "local/title" slot="compact" node-kind="entity" {
  field "label" source="literal" value="Local"
}
template "attention/title" slot="compact" node-kind="entity" {
  field "label" source="literal" value="Attention"
}
placement "local" {
  for "section" kind=".section" layout="section" {
    apply-template "section/local"
  }
}
template "section/local" {
  field "label" key="display.label"
  for "group" kind=".group" {
    match ".section" of="section"
    apply-template "group/local"
  }
}
template "group/local" {
  field "label" key="display.label"
  for "workspace" kind=".workspace" {
    match ".group" of="group"
    apply-template
  }
  for "ref" kind=".ref" {
    match ".group" of="group"
    apply-template
  }
}
template ".workspace/line" {
  field "label" key="display.label"
}
template ".ref/line" {
  field "label" key="display.label"
  field "status" key="status.state"
}
template "vessel/line" {
  field "label" key="display.label"
}
placement "attention" {
  for "attention" {
    match "status.attention" value="true"
    apply-template "vessel/line"
  }
}
"#;

fn refs(entity: EntityRef) -> MetadataValue {
    MetadataValue::EntityRefs(vec![entity])
}

/// Sections and groups people make (drag-model.md, "Sections and groups as
/// data"): a `layout="section"` loop makes each `.section` its own section;
/// groups, workspaces and `.ref` ghosts nest by entity-reference facts; a ref
/// presents its target without being placed by the target's facts; the
/// `.default` group covers tabs nothing places.
#[test]
fn local_sections_groups_and_refs_nest_and_present_their_targets() {
    let mut sidebar = Sidebar::new(LOCAL_SECTIONS_CONFIG).unwrap();
    let section = entity(".section", "s1");
    let group = entity(".group", "g1");
    let local = entity(".workspace", "w1");
    let vessel = entity("vessel", "v");
    sidebar.apply(
        100,
        [
            patch(section.clone(), &[("display.label", text("Pinned"))]),
            patch(
                group.clone(),
                &[
                    ("display.label", text("Workspaces")),
                    (".section", refs(section.clone())),
                    (".default", MetadataValue::Bool(true)),
                ],
            ),
            patch(
                vessel.clone(),
                &[
                    ("display.label", text("Worker")),
                    ("status.attention", MetadataValue::Bool(true)),
                    ("status.state", text("waiting")),
                    ("action.primary.recipe", text("printf hello")),
                ],
            ),
            patch(
                entity(".ref", "r1"),
                &[
                    (".group", refs(group.clone())),
                    (".target", refs(vessel.clone())),
                ],
            ),
            patch(
                entity(".ref", "r2"),
                &[
                    (".group", refs(group.clone())),
                    (".target", refs(vessel.clone())),
                ],
            ),
            patch(
                local.clone(),
                &[
                    ("display.label", text("Scratch")),
                    (".group", refs(group.clone())),
                ],
            ),
            tab_patch(
                42,
                &[(".host.kind", text(".workspace")), (".host.id", text("w1"))],
            ),
        ],
    );
    let mut leftover = workspace();
    leftover.id = WorkspaceId::from(43);
    leftover.position = 1;
    leftover.selected = false;
    sidebar.observe(vec![workspace(), leftover], vec![]);
    let snapshot = sidebar.snapshot();
    let local_section = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "local")
        .unwrap();
    assert_eq!(local_section.nodes.len(), 1);
    let made = &local_section.nodes[0];
    assert_eq!(
        (made.entity.clone(), made.layout.as_deref()),
        (section, Some("section"))
    );
    assert_eq!(made.children.len(), 1);
    let items = &made.children[0].children;
    let entities = items.iter().map(|n| n.entity.clone()).collect::<Vec<_>>();
    // Homed workspace, both ghosts (one entity, two refs), then the leftover tab.
    assert_eq!(
        entities,
        vec![
            local.clone(),
            entity(".ref", "r1"),
            entity(".ref", "r2"),
            entity(".workspace", "43")
        ]
    );
    assert_eq!(
        items[1].label, "Worker",
        "a ref presents its target's label"
    );
    assert_eq!(
        items[1].state,
        andamento_core::presentation::PresentationState::Latent { openable: true },
        "a ref of an openable target is openable"
    );
    assert_ne!(items[1].key, items[2].key, "each ghost keeps its own key");
    assert_eq!(
        items[0].state,
        andamento_core::presentation::PresentationState::Live {
            workspace_id: WorkspaceId::from(42),
            selected: true
        }
    );
    assert!(
        fallback(&sidebar).is_empty(),
        "the default group covers leftover tabs"
    );
    // Attention places the vessel once; its ghosts aren't placed by its facts.
    let attention = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "attention")
        .unwrap();
    assert_eq!(
        attention
            .nodes
            .iter()
            .map(|n| n.entity.clone())
            .collect::<Vec<_>>(),
        vec![vessel.clone()]
    );
    // Details and activation follow the target.
    assert_eq!(
        sidebar.detail_card(&entity(".ref", "r1")).unwrap().label,
        "Worker"
    );
    let effects = sidebar
        .dispatch(Action::ActivatePlacement {
            key: items[1].key.clone(),
        })
        .unwrap();
    assert!(
        matches!(&effects[..], [HostEffect::Materialize { entity: subject, .. }] if subject == &vessel),
        "activating a ghost opens its target: {effects:?}"
    );
    // A ghost of an open target is live for the target's workspace.
    sidebar.apply(
        200,
        [tab_patch(
            44,
            &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
        )],
    );
    let mut opened = workspace();
    opened.id = WorkspaceId::from(44);
    opened.position = 2;
    opened.selected = false;
    let mut first = workspace();
    first.selected = true;
    let mut second = first.clone();
    second.id = WorkspaceId::from(43);
    second.position = 1;
    second.selected = false;
    sidebar.observe(vec![first, second, opened], vec![]);
    let snapshot = sidebar.snapshot();
    let local_section = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "local")
        .unwrap();
    let ghost = local_section.nodes[0].children[0]
        .children
        .iter()
        .find(|n| n.entity == entity(".ref", "r1"))
        .unwrap();
    assert_eq!(
        ghost.state,
        andamento_core::presentation::PresentationState::Live {
            workspace_id: WorkspaceId::from(44),
            selected: false
        }
    );
}

/// With several default groups, the first placed one covers leftover tabs;
/// one that isn't placed (its section is missing) doesn't hide it.
#[test]
fn the_first_placed_default_group_covers_leftover_tabs() {
    let mut sidebar = Sidebar::new(LOCAL_SECTIONS_CONFIG).unwrap();
    let section = entity(".section", "s1");
    let stray = entity(".group", "a-stray");
    let placed = entity(".group", "b-placed");
    let default = |of: &EntityRef| {
        [
            (".section", refs(of.clone())),
            (".default", MetadataValue::Bool(true)),
        ]
    };
    sidebar.apply(
        100,
        [
            patch(section.clone(), &[("display.label", text("Workspaces"))]),
            patch(stray.clone(), &default(&entity(".section", "missing"))),
            patch(placed.clone(), &default(&section)),
        ],
    );
    sidebar.observe(vec![workspace()], vec![]);
    let snapshot = sidebar.snapshot();
    let local = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "local")
        .unwrap();
    let group = &local.nodes[0].children[0];
    assert_eq!(group.entity, placed);
    assert_eq!(
        group
            .children
            .iter()
            .map(|n| n.entity.clone())
            .collect::<Vec<_>>(),
        vec![entity(".workspace", "42")]
    );
    assert!(fallback(&sidebar).is_empty());
}

/// Only the boolean `true` marks a default group; text that says "true"
/// doesn't, and leftover tabs stay in the `.unplaced` section.
#[test]
fn only_a_boolean_default_marks_the_default_group() {
    let mut sidebar = Sidebar::new(LOCAL_SECTIONS_CONFIG).unwrap();
    let section = entity(".section", "s1");
    sidebar.apply(
        100,
        [
            patch(section.clone(), &[("display.label", text("Workspaces"))]),
            patch(
                entity(".group", "g1"),
                &[
                    (".section", refs(section.clone())),
                    (".default", text("true")),
                ],
            ),
        ],
    );
    sidebar.observe(vec![workspace()], vec![]);
    assert_eq!(fallback(&sidebar).len(), 1, "the tab stays left over");
}

/// Refs don't chain: a ref to a ref keeps its own facts.
#[test]
fn a_ref_to_a_ref_presents_nothing_further() {
    let mut sidebar = Sidebar::new(LOCAL_SECTIONS_CONFIG).unwrap();
    let section = entity(".section", "s1");
    let group = entity(".group", "g1");
    let vessel = entity("vessel", "v");
    sidebar.apply(
        100,
        [
            patch(section.clone(), &[("display.label", text("Pinned"))]),
            patch(group.clone(), &[(".section", refs(section.clone()))]),
            patch(vessel.clone(), &[("display.label", text("Worker"))]),
            patch(
                entity(".ref", "inner"),
                &[(".group", refs(group.clone())), (".target", refs(vessel))],
            ),
            patch(
                entity(".ref", "outer"),
                &[
                    (".group", refs(group.clone())),
                    (".target", refs(entity(".ref", "inner"))),
                ],
            ),
        ],
    );
    let snapshot = sidebar.snapshot();
    let local = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "local")
        .unwrap();
    let items = &local.nodes[0].children[0].children;
    let inner = items
        .iter()
        .find(|n| n.entity == entity(".ref", "inner"))
        .unwrap();
    let outer = items
        .iter()
        .find(|n| n.entity == entity(".ref", "outer"))
        .unwrap();
    assert_eq!(inner.label, "Worker");
    assert_ne!(
        outer.label, "Worker",
        "a ref to a ref doesn't present the inner ref's target"
    );
}

/// A ref whose target is missing, or whose `.target` isn't a single
/// reference, keeps only its own facts and stays in its group.
#[test]
fn a_ref_without_a_usable_target_keeps_its_own_facts() {
    let mut sidebar = Sidebar::new(LOCAL_SECTIONS_CONFIG).unwrap();
    let section = entity(".section", "s1");
    let group = entity(".group", "g1");
    sidebar.apply(
        100,
        [
            patch(section.clone(), &[("display.label", text("Pinned"))]),
            patch(group.clone(), &[(".section", refs(section.clone()))]),
            patch(entity("vessel", "v"), &[("display.label", text("Worker"))]),
            patch(
                entity(".ref", "missing"),
                &[
                    (".group", refs(group.clone())),
                    (".target", refs(entity("vessel", "gone"))),
                ],
            ),
            patch(
                entity(".ref", "malformed"),
                &[
                    (".group", refs(group.clone())),
                    (
                        ".target",
                        MetadataValue::EntityRefs(vec![
                            entity("vessel", "v"),
                            entity("vessel", "v"),
                        ]),
                    ),
                ],
            ),
        ],
    );
    let snapshot = sidebar.snapshot();
    let local = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "local")
        .unwrap();
    let items = &local.nodes[0].children[0].children;
    for id in ["missing", "malformed"] {
        let node = items
            .iter()
            .find(|n| n.entity == entity(".ref", id))
            .expect("still placed");
        assert_ne!(node.label, "Worker", "{id} presents no target");
    }
}

/// A ref's place is its own: it keeps its own `.`-prefixed facts and takes
/// none of its target's. One without its own `.group` isn't placed in its
/// target's, and one ordered by `.position` sorts by its own.
#[test]
fn a_ref_takes_its_place_from_its_own_facts_not_its_targets() {
    const ORDERED: &str = r#"
region "local" root-template="local/title" form="compact" placement="local"
template "local/title" slot="compact" node-kind="entity" {
  field "label" source="literal" value="Local"
}
placement "local" {
  for "section" kind=".section" layout="section" {
    apply-template "section/local"
  }
}
template "section/local" {
  field "label" key="display.label"
  for "group" kind=".group" {
    match ".section" of="section"
    apply-template "group/local"
  }
}
template "group/local" {
  field "label" key="display.label"
  for "item" {
    match ".group" of="group"
    order ".position" natural=true
    apply-template "local/item"
  }
}
template "local/item" {
  field "label" key="display.label"
}
"#;
    let mut sidebar = Sidebar::new(ORDERED).unwrap();
    let section = entity(".section", "s1");
    let group = entity(".group", "g1");
    let home = entity(".workspace", "w1");
    let item = |label: &str, position: &str| {
        vec![
            ("display.label", text(label)),
            (".group", refs(group.clone())),
            (".position", text(position)),
        ]
    };
    sidebar.apply(
        100,
        [
            patch(section.clone(), &[("display.label", text("Pinned"))]),
            patch(group.clone(), &[(".section", refs(section.clone()))]),
            patch(home.clone(), &item("Home", "0")),
            patch(entity(".workspace", "w2"), &item("Other", "3")),
            patch(
                entity(".ref", "placed"),
                &[
                    (".group", refs(group.clone())),
                    (".target", refs(home.clone())),
                    (".position", text("5")),
                ],
            ),
            patch(
                entity(".ref", "unplaced"),
                &[(".target", refs(home.clone()))],
            ),
        ],
    );
    let snapshot = sidebar.snapshot();
    let local = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "local")
        .unwrap();
    let items = &local.nodes[0].children[0].children;
    let order = items
        .iter()
        .map(|n| n.entity.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        ["w1", "w2", "placed"],
        "the ref sorts by its own position and only it is placed"
    );
    assert_eq!(items[2].label, "Home", "it still presents its target");
}

/// A ref the host retracts (unsets its facts) leaves its group, even after
/// it opened its target, whose open workspace makes its row live.
#[test]
fn a_retracted_ref_leaves_its_group() {
    let mut sidebar = Sidebar::new(LOCAL_SECTIONS_CONFIG).unwrap();
    let section = entity(".section", "s1");
    let group = entity(".group", "g1");
    let vessel = entity("vessel", "v");
    let r = entity(".ref", "r1");
    sidebar.apply(
        100,
        [
            patch(section.clone(), &[("display.label", text("Pinned"))]),
            patch(group.clone(), &[(".section", refs(section.clone()))]),
            patch(
                vessel.clone(),
                &[
                    ("display.label", text("Worker")),
                    ("action.primary.recipe", text("printf hello")),
                ],
            ),
            patch(
                r.clone(),
                &[
                    (".group", refs(group.clone())),
                    (".target", refs(vessel.clone())),
                    (".position", text("1")),
                ],
            ),
        ],
    );
    let ref_key = |sidebar: &Sidebar| {
        let snapshot = sidebar.snapshot();
        let local = snapshot
            .surface
            .sections
            .iter()
            .find(|s| s.name == "local")
            .cloned()
            .unwrap();
        local.nodes[0].children[0]
            .children
            .iter()
            .find(|n| n.entity == r)
            .map(|n| n.key.clone())
    };
    // Open the target through the ghost, as a click on its row does.
    let key = ref_key(&sidebar).expect("placed");
    let effects = sidebar.dispatch(Action::ActivatePlacement { key }).unwrap();
    let request_id = match &effects[..] {
        [HostEffect::Materialize {
            request_id,
            entity: subject,
            ..
        }] if subject == &vessel => *request_id,
        other => panic!("expected the target to open, got {other:?}"),
    };
    sidebar.complete(request_id, Ok(Some(WorkspaceId::from(42))));
    // The host names the opened workspace's subject, as Wheelhouse does.
    sidebar.apply(
        150,
        [tab_patch(
            42,
            &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
        )],
    );
    sidebar.observe(vec![workspace()], vec![]);
    assert!(ref_key(&sidebar).is_some());
    // Hosts observe every frame; retained paths are recorded from a snapshot.
    sidebar.observe(vec![workspace()], vec![]);
    let mut retract = patch(r.clone(), &[]);
    retract.unset = vec![".group".into(), ".target".into(), ".position".into()];
    sidebar.apply(200, [retract]);
    sidebar.observe(vec![workspace()], vec![]);
    assert!(
        ref_key(&sidebar).is_none(),
        "a retracted ref is no longer placed"
    );
}
