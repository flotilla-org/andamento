//! Entities carry the provider (Dashboard subscription) their facts come
//! from: the same kind and ID under two providers are two entities; a
//! provider's facts can be retracted at once or kept stale.
mod common;
use andamento_core::presentation::{PlacementNode, PresentationState};
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::workspace::WorkspaceRecord;
use andamento_core::{
    records::DashboardRecord, tab_workspace_id, EntityRef, MetadataPatch, MetadataTarget,
    MetadataValue, MetadataValueUpdate, Sidebar, WorkspaceId, LOCAL_PROVIDER,
};
use common::{nodes, refs, text, CONFIG};
use std::collections::BTreeMap;

const W42: WorkspaceId = tab_workspace_id(42);

/// A patch as a producer writes it: it names no provider.
fn patch(
    kind: &str,
    id: &str,
    ttl_ms: Option<u64>,
    facts: &[(&str, MetadataValue)],
) -> MetadataPatch {
    MetadataPatch {
        target: MetadataTarget::Entity(EntityRef::new("", kind, id)),
        source_id: "flotilla".into(),
        set: facts
            .iter()
            .map(|(key, value)| {
                (
                    key.to_string(),
                    MetadataValueUpdate {
                        value: value.clone(),
                        ttl_ms,
                        precedence: None,
                        ordinal: None,
                    },
                )
            })
            .collect(),
        unset: vec![],
    }
}
/// A project and a vessel in it, labelled with `tag`.
fn fleet(tag: &str, ttl_ms: Option<u64>) -> Vec<MetadataPatch> {
    vec![
        patch(
            "project",
            "p",
            ttl_ms,
            &[
                ("flotilla.project", text("p")),
                ("display.label", text(&format!("Project {tag}"))),
            ],
        ),
        patch(
            "vessel",
            "v",
            ttl_ms,
            &[
                ("flotilla.project", text("p")),
                ("flotilla.vessel", text("v")),
                ("display.label", text(&format!("Vessel {tag}"))),
                ("action.primary.recipe", text("printf hello")),
            ],
        ),
    ]
}
fn entity(provider: &str, kind: &str, id: &str) -> EntityRef {
    EntityRef::new(provider, kind, id)
}
fn tree(sidebar: &Sidebar) -> Vec<PlacementNode> {
    sidebar.snapshot().surface.sections[0].nodes.clone()
}
fn workspace(id: WorkspaceId) -> Workspace {
    Workspace {
        id,
        position: 0,
        name: "Worker".into(),
        selected: true,
    }
}
/// Activate `subject`, complete its materialization as W42, and observe it.
fn open(sidebar: &mut Sidebar, subject: EntityRef) {
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: subject.clone(),
        })
        .unwrap();
    let [HostEffect::Materialize {
        request_id, entity, ..
    }] = &effects[..]
    else {
        panic!("expected materialize, got {effects:?}");
    };
    assert_eq!(entity, &subject);
    sidebar.complete(*request_id, Ok(Some(W42)));
    sidebar.observe(vec![workspace(W42)], vec![]);
}

#[test]
fn the_same_kind_and_id_under_two_providers_are_two_entities() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    sidebar.apply(100, "sub-1", fleet("one", None)).unwrap();
    sidebar.apply(100, "sub-2", fleet("two", None)).unwrap();
    let projects = tree(&sidebar);
    assert_eq!(projects.len(), 2);
    for project in &projects {
        // Each project holds only its own provider's vessel, though both
        // vessels name the same project text.
        let tag = if project.entity.provider == "sub-1" {
            "one"
        } else {
            "two"
        };
        assert_eq!(project.label, format!("Project {tag}"));
        assert_eq!(project.children.len(), 1, "{project:?}");
        let vessel = &project.children[0];
        assert_eq!(vessel.entity.provider, project.entity.provider);
        assert_eq!(vessel.label, format!("Vessel {tag}"));
    }
    assert_ne!(projects[0].key, projects[1].key);
    assert_ne!(
        projects[0].children[0].loop_key,
        projects[1].children[0].loop_key
    );
    assert_ne!(
        projects[0].children[0].loop_key.encode(),
        projects[1].children[0].loop_key.encode()
    );
    let card = |provider| {
        sidebar
            .detail_card(&entity(provider, "vessel", "v"))
            .unwrap()
            .label
    };
    assert_eq!(
        (card("sub-1"), card("sub-2")),
        ("Vessel one".into(), "Vessel two".into())
    );

    // Opening one leaves the other latent: their activation targets differ.
    open(&mut sidebar, entity("sub-1", "vessel", "v"));
    for project in tree(&sidebar) {
        let state = &project.children[0].state;
        match project.entity.provider.as_str() {
            "sub-1" => assert!(matches!(state, PresentationState::Live { .. })),
            _ => assert!(
                matches!(state, PresentationState::Latent { .. }),
                "{state:?}"
            ),
        }
    }
    // The workspace's subject keeps its provider in its record.
    let record = WorkspaceRecord::decode(
        &sidebar.export_record(&format!("workspace/{W42}")).unwrap(),
        &mut Default::default(),
    )
    .unwrap();
    assert_eq!(
        record.subject(),
        Some(&EntityRef::new("sub-1", "vessel", "v"))
    );
}

#[test]
fn a_patch_never_names_its_own_provider() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let mut forged = fleet("one", None);
    for patch in &mut forged {
        let MetadataTarget::Entity(entity) = &mut patch.target else {
            unreachable!()
        };
        entity.provider = "sub-2".into();
    }
    // A reference among its values is the patch's provider's too.
    forged[1].set.insert(
        "flotilla.forge".into(),
        MetadataValueUpdate {
            value: refs(entity("sub-2", "forge", "f")),
            ttl_ms: None,
            precedence: None,
            ordinal: None,
        },
    );
    sidebar.apply(100, "sub-1", forged).unwrap();
    assert!(sidebar.apply(100, "", fleet("x", None)).is_err());
    let snapshot = sidebar.snapshot();
    let all = nodes(&snapshot.surface);
    assert!(all.iter().all(|n| n.entity.provider == "sub-1"), "{all:?}");
    let vessel = all.iter().find(|n| n.entity.kind == "vessel").unwrap();
    assert_eq!(
        vessel.facts["flotilla.forge"],
        refs(entity("sub-1", "forge", "f"))
    );
}

#[test]
fn a_reference_to_a_local_entity_stays_local() {
    // A producer may file its entity under one of Andamento's own groups;
    // stamping its provider must not claim the group.
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let mut patches = fleet("one", None);
    patches[1].set.insert(
        ".group".into(),
        MetadataValueUpdate {
            value: refs(entity(LOCAL_PROVIDER, ".group", "g")),
            ttl_ms: None,
            precedence: None,
            ordinal: None,
        },
    );
    sidebar.apply(100, "sub-1", patches).unwrap();
    let snapshot = sidebar.snapshot();
    let all = nodes(&snapshot.surface);
    let vessel = all.iter().find(|n| n.entity.kind == "vessel").unwrap();
    assert_eq!(
        vessel.facts[".group"],
        refs(entity(LOCAL_PROVIDER, ".group", "g"))
    );
}

#[test]
fn a_workspace_bound_by_a_providers_own_facts_is_that_providers() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    sidebar.apply(100, "sub-1", fleet("one", None)).unwrap();
    sidebar.apply(100, "sub-2", fleet("two", None)).unwrap();
    let mut binding = patch(
        "vessel",
        "v",
        None,
        &[("entity.kind", text("vessel")), ("entity.id", text("v"))],
    );
    binding.target = MetadataTarget::Tab(W42);
    sidebar.apply(100, "sub-2", [binding]).unwrap();
    sidebar.observe(vec![workspace(W42)], vec![]);
    sidebar.register_workspace(W42);
    for project in tree(&sidebar) {
        let live = matches!(project.children[0].state, PresentationState::Live { .. });
        assert_eq!(live, project.entity.provider == "sub-2", "{project:?}");
    }
    // Retracting the provider keeps the workspace on its subject.
    assert!(sidebar.retract_provider("sub-2"));
    let projects = tree(&sidebar);
    let retained = projects
        .iter()
        .find(|p| p.entity.provider == "sub-2")
        .expect("the open workspace's path is retained");
    assert_eq!(retained.label, "Project two");
    assert_eq!(
        retained.children[0].state,
        PresentationState::Live {
            workspace_id: W42,
            selected: true
        }
    );
    let record = WorkspaceRecord::decode(
        &sidebar.export_record(&format!("workspace/{W42}")).unwrap(),
        &mut Default::default(),
    )
    .unwrap();
    assert_eq!(
        record.subject(),
        Some(&EntityRef::new("sub-2", "vessel", "v"))
    );
}

#[test]
fn retracting_a_provider_removes_its_facts_and_retains_open_workspaces() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    sidebar.apply(100, "sub-1", fleet("one", None)).unwrap();
    let mut other = fleet("one", None);
    other.push(patch(
        "vessel",
        "u",
        None,
        &[
            ("flotilla.project", text("p")),
            ("display.label", text("Unopened")),
        ],
    ));
    sidebar.apply(100, "sub-1", other).unwrap();
    sidebar.apply(100, "sub-2", fleet("two", None)).unwrap();
    open(&mut sidebar, entity("sub-1", "vessel", "v"));
    let revision = sidebar.revision();

    assert!(sidebar.retract_provider("sub-1"));
    assert_ne!(sidebar.revision(), revision);
    // Retracting again, or a provider never seen, changes nothing.
    assert!(!sidebar.retract_provider("sub-1"));
    assert!(!sidebar.retract_provider("never"));

    let projects = tree(&sidebar);
    let one = projects
        .iter()
        .find(|p| p.entity.provider == "sub-1")
        .expect("the open workspace's path is retained");
    // The open vessel stays, live, on its path; the unopened one goes.
    assert_eq!(one.label, "Project one");
    assert_eq!(one.children.len(), 1);
    assert_eq!(one.children[0].label, "Vessel one");
    assert!(matches!(
        one.children[0].state,
        PresentationState::Live {
            workspace_id: W42,
            ..
        }
    ));
    // The other provider is untouched.
    let two = projects
        .iter()
        .find(|p| p.entity.provider == "sub-2")
        .unwrap();
    assert_eq!(two.children[0].label, "Vessel two");
    // The workspace record keeps the retained path, with providers.
    let record = WorkspaceRecord::decode(
        &sidebar.export_record(&format!("workspace/{W42}")).unwrap(),
        &mut Default::default(),
    )
    .unwrap();
    let retained = &record.subject.get().retained[&EntityRef::new("sub-1", "vessel", "v")];
    assert_eq!(retained.label.as_deref(), Some("Vessel one"));
    assert!(!retained.ended);

    // Closing the workspace lets the retained path go.
    sidebar.observe(vec![], vec![]);
    let projects = tree(&sidebar);
    assert!(projects.iter().all(|p| p.entity.provider == "sub-2"));
}

const REFS_CONFIG: &str = r#"
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
  for "ref" kind=".ref" {
    match ".group" of="group"
    apply-template
  }
}
template ".ref/line" {
  field "label" key="display.label"
}
"#;

#[test]
fn refs_keep_presenting_a_retracted_target() {
    let mut sidebar = Sidebar::new(REFS_CONFIG).unwrap();
    let section = EntityRef::local(".section", "s1");
    let group = EntityRef::local(".group", "g1");
    sidebar
        .set_local(
            section.clone(),
            BTreeMap::from([("display.label".into(), text("Pinned"))]),
        )
        .unwrap();
    sidebar
        .set_local(
            group.clone(),
            BTreeMap::from([(".section".into(), refs(section))]),
        )
        .unwrap();
    for provider in ["sub-1", "sub-2"] {
        sidebar
            .set_local(
                EntityRef::local(".ref", provider),
                BTreeMap::from([
                    (".group".into(), refs(group.clone())),
                    (".target".into(), refs(entity(provider, "vessel", "v"))),
                ]),
            )
            .unwrap();
    }
    // A local entity is local; another provider is refused.
    assert!(sidebar
        .set_local(entity("sub-1", ".group", "g2"), BTreeMap::new())
        .is_err());
    sidebar.apply(100, "sub-1", fleet("one", None)).unwrap();
    sidebar.apply(100, "sub-2", fleet("two", None)).unwrap();
    let labels = |sidebar: &Sidebar| {
        let snapshot = sidebar.snapshot();
        nodes(&snapshot.surface)
            .iter()
            .filter(|n| n.entity.kind == ".ref")
            .map(|n| (n.entity.id.clone(), n.label.clone()))
            .collect::<Vec<_>>()
    };
    let presented = vec![
        ("sub-1".to_owned(), "Vessel one".to_owned()),
        ("sub-2".to_owned(), "Vessel two".to_owned()),
    ];
    assert_eq!(labels(&sidebar), presented);
    assert!(sidebar.retract_provider("sub-1"));
    assert_eq!(labels(&sidebar), presented);
    // The pinned target is kept in the dashboard record with its provider.
    let dashboard = sidebar.export_record("dashboard").unwrap();
    let dashboard = DashboardRecord::decode(&dashboard).unwrap();
    let pin = &dashboard.state.local[&EntityRef::local(".ref", "sub-1")];
    assert_eq!(pin[".target"], refs(entity("sub-1", "vessel", "v")));
}

#[test]
fn a_stale_providers_facts_do_not_expire_and_are_flagged() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    sidebar
        .apply(100, "sub-1", fleet("one", Some(1_000)))
        .unwrap();
    sidebar
        .apply(100, "sub-2", fleet("two", Some(1_000)))
        .unwrap();
    // Facts that expired before the provider went stale stay expired.
    sidebar
        .apply(
            100,
            "sub-1",
            [patch(
                "vessel",
                "gone",
                Some(10),
                &[
                    ("flotilla.project", text("p")),
                    ("display.label", text("Gone")),
                ],
            )],
        )
        .unwrap();
    sidebar.tick(200);
    let stale_of = |sidebar: &Sidebar| {
        let snapshot = sidebar.snapshot();
        nodes(&snapshot.surface)
            .iter()
            .map(|n| (n.entity.provider.clone(), n.entity.id.clone(), n.stale))
            .collect::<Vec<_>>()
    };
    assert!(stale_of(&sidebar).iter().all(|(_, _, stale)| !stale));
    let revision = sidebar.revision();
    assert!(sidebar.set_provider_stale("sub-1", true));
    assert!(!sidebar.set_provider_stale("sub-1", true));
    assert!(sidebar.provider_is_stale("sub-1"));
    assert_ne!(sidebar.revision(), revision);

    // Long past the TTL: sub-2's facts expired, sub-1's are kept and stale.
    sidebar.tick(5_000);
    let state = stale_of(&sidebar);
    assert!(!state.is_empty());
    assert!(
        state
            .iter()
            .all(|(provider, id, stale)| provider == "sub-1" && *stale && id != "gone"),
        "{state:?}"
    );
    // The snapshot serializes the flag; fresh nodes leave it out.
    let json = serde_json::to_string(&sidebar.snapshot()).unwrap();
    assert!(json.contains(r#""stale":true"#));

    // Fresh again, each lease restarts now, so the facts live one more TTL.
    assert!(sidebar.set_provider_stale("sub-1", false));
    assert!(!sidebar.provider_is_stale("sub-1"));
    sidebar.tick(5_900);
    let state = stale_of(&sidebar);
    assert!(!state.is_empty() && state.iter().all(|(_, _, stale)| !stale));
    assert!(!serde_json::to_string(&sidebar.snapshot())
        .unwrap()
        .contains("\"stale\""));
    sidebar.tick(6_001);
    assert!(tree(&sidebar).is_empty());
}

#[test]
fn a_stale_ref_target_marks_the_ref_stale() {
    let mut sidebar = Sidebar::new(REFS_CONFIG).unwrap();
    let section = EntityRef::local(".section", "s1");
    let group = EntityRef::local(".group", "g1");
    sidebar
        .set_local(
            section.clone(),
            BTreeMap::from([("display.label".into(), text("Pinned"))]),
        )
        .unwrap();
    sidebar
        .set_local(
            group.clone(),
            BTreeMap::from([(".section".into(), refs(section))]),
        )
        .unwrap();
    sidebar
        .set_local(
            EntityRef::local(".ref", "r"),
            BTreeMap::from([
                (".group".into(), refs(group)),
                (".target".into(), refs(entity("sub-1", "vessel", "v"))),
            ]),
        )
        .unwrap();
    sidebar.apply(100, "sub-1", fleet("one", None)).unwrap();
    sidebar.set_provider_stale("sub-1", true);
    let snapshot = sidebar.snapshot();
    let all = nodes(&snapshot.surface);
    let pin = all.iter().find(|n| n.entity.kind == ".ref").unwrap();
    assert!(pin.stale && pin.entity.provider == "local");
    assert!(all
        .iter()
        .filter(|n| n.entity.kind != ".ref")
        .all(|n| !n.stale));
}

/// An entity that names its own action target is live in the workspace
/// materialized for it, whatever its provider: the tab is bound by the
/// entity's identity, and matched by the entity's target.
#[test]
fn an_entity_naming_its_own_action_target_goes_live_for_any_provider() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let mut patches = fleet("one", None);
    patches[1].set.insert(
        "action.primary.target".into(),
        MetadataValueUpdate {
            value: text("vessel:v"),
            ttl_ms: None,
            precedence: None,
            ordinal: None,
        },
    );
    sidebar.apply(100, "sub-1", patches).unwrap();
    open(&mut sidebar, entity("sub-1", "vessel", "v"));
    let snapshot = sidebar.snapshot();
    let vessel = nodes(&snapshot.surface)
        .into_iter()
        .find(|n| n.entity.kind == "vessel")
        .unwrap();
    assert!(
        matches!(vessel.state, PresentationState::Live { workspace_id, .. } if workspace_id == W42),
        "{:?}",
        vessel.state
    );
}
