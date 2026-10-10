//! Named records (#142): Andamento owns the sidebar's logical state and
//! exports it as versioned KDL, which a fresh sidebar imports before it has
//! observed anything or received a fact.
use andamento_core::presentation::{PlacementNode, PresentationState, SurfaceSnapshot};
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::{
    DisplayVariableValue, EntityRef, MetadataPatch, MetadataTarget, MetadataValue,
    MetadataValueUpdate, Sidebar, WorkspaceId,
};
use std::collections::BTreeMap;

const LOCAL: &str = r#"
display-variable "show-finished" type="bool" default=false label="Finished" icon="F"
display-variable "mode" type="enum" default="wide" label="Mode" icon="M" persist=false {
  value "wide"
  value "narrow"
}
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

fn config() -> String {
    format!("{}\n{LOCAL}", include_str!("fixtures/sidebar.kdl"))
}

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
}
fn text(s: &str) -> MetadataValue {
    MetadataValue::Text(s.into())
}
fn refs(entity: EntityRef) -> MetadataValue {
    MetadataValue::EntityRefs(vec![entity])
}
fn patch(subject: EntityRef, facts: &[(&str, MetadataValue)], ttl: Option<u64>) -> MetadataPatch {
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
                        ttl_ms: ttl,
                        precedence: None,
                        ordinal: None,
                    },
                )
            })
            .collect(),
        unset: vec![],
    }
}
fn vessel(id: &str, label: &str) -> MetadataPatch {
    patch(
        entity("vessel", id),
        &[
            ("entity.kind", text("vessel")),
            ("entity.id", text(id)),
            ("flotilla.project", text("p")),
            ("display.label", text(label)),
            ("status.state", text("waiting")),
            ("action.primary.recipe", text("printf hello")),
        ],
        None,
    )
}
fn project() -> MetadataPatch {
    patch(
        entity("project", "p"),
        &[
            ("flotilla.project", text("p")),
            ("display.label", text("Project P")),
        ],
        None,
    )
}

const WORKER: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c";
fn worker() -> WorkspaceId {
    WORKER.parse().unwrap()
}
fn workspace(selected: bool) -> Workspace {
    Workspace {
        id: worker(),
        position: 0,
        name: "Worker".into(),
        selected,
    }
}

fn nodes(surface: &SurfaceSnapshot) -> Vec<&PlacementNode> {
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
fn tree(sidebar: &Sidebar) -> PlacementNode {
    sidebar
        .snapshot()
        .surface
        .sections
        .into_iter()
        .find(|s| s.name == "tree")
        .and_then(|s| s.nodes.into_iter().next())
        .expect("a project row")
}
fn ended(node: &PlacementNode) -> bool {
    node.facts.get("presentation.ended") == Some(&MetadataValue::Bool(true))
}
fn issues_checked(sidebar: &Sidebar) -> bool {
    sidebar.snapshot().surface.display_values["show-issues"] == DisplayVariableValue::Bool(true)
}

/// A session that opens a workspace for vessel v, arranges the sidebar, makes
/// a local section, group and pin, and sees v's producer remove it.
fn arranged_session() -> Sidebar {
    let mut sidebar = Sidebar::new(&config()).unwrap();
    sidebar.apply(100, [project(), vessel("v", "Vee"), vessel("w", "Double")]);
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "v"),
        })
        .unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else {
        panic!("expected materialize: {effects:?}");
    };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![workspace(true)], vec![]);

    let project_row = tree(&sidebar);
    let vessels = project_row.children[0].loop_key.clone();
    sidebar.set_sibling_order(vessels, vec![entity("vessel", "w"), entity("vessel", "v")]);
    sidebar
        .dispatch(Action::SetVariable {
            key: project_row.key.clone(),
            name: "density".into(),
            value: Some("compact".into()),
        })
        .unwrap();
    sidebar
        .dispatch(Action::TogglePlacement {
            key: project_row.key,
        })
        .unwrap();
    sidebar
        .set_display_variable("show-issues", Some(DisplayVariableValue::Bool(false)))
        .unwrap();
    sidebar
        .set_display_variable("show-finished", Some(DisplayVariableValue::Bool(true)))
        .unwrap();
    sidebar
        .set_local(
            entity(".section", "s1"),
            BTreeMap::from([("display.label".into(), text("Mine"))]),
        )
        .unwrap();
    sidebar
        .set_local(
            entity(".group", "g1"),
            BTreeMap::from([
                ("display.label".into(), text("Pinned")),
                (".section".into(), refs(entity(".section", "s1"))),
            ]),
        )
        .unwrap();
    sidebar
        .set_local(
            entity(".ref", "r1"),
            BTreeMap::from([
                (".group".into(), refs(entity(".group", "g1"))),
                (".target".into(), refs(entity("vessel", "w"))),
                (".position".into(), MetadataValue::Integer(1)),
            ]),
        )
        .unwrap();
    // The producer removes v: the open workspace keeps it, ended.
    let mut removal = patch(entity("vessel", "v"), &[], None);
    removal.unset = [
        "entity.kind",
        "entity.id",
        "flotilla.project",
        "display.label",
        "status.state",
        "action.primary.recipe",
    ]
    .map(String::from)
    .to_vec();
    sidebar.apply(150, [removal]);
    sidebar.snapshot();
    sidebar.apply(150, []);
    let v = nodes(&sidebar.snapshot().surface)
        .into_iter()
        .find(|n| n.entity == entity("vessel", "v"))
        .cloned()
        .expect("v stays while its workspace is open");
    assert!(ended(&v));
    sidebar
}

fn export_all(sidebar: &mut Sidebar) -> Vec<(String, String)> {
    sidebar
        .record_names()
        .into_iter()
        .map(|name| {
            let text = sidebar.export_record(&name).unwrap();
            (name, text)
        })
        .collect()
}

#[test]
fn records_are_named_for_the_dashboard_and_registered_workspaces() {
    let mut sidebar = arranged_session();
    assert_eq!(
        sidebar.record_names(),
        vec!["dashboard".to_owned(), format!("workspace/{WORKER}")]
    );
    let workspace = sidebar
        .export_record(&format!("workspace/{WORKER}"))
        .unwrap();
    assert!(
        workspace.contains("subject \"vessel\" \"v\" provider=\"local\""),
        "{workspace}"
    );
    assert!(
        workspace
            .contains("retained \"vessel\" \"v\" provider=\"local\" label=\"Vee\" status=\"ended\" last-seen=150"),
        "{workspace}"
    );
    assert!(workspace.contains(
        "retained \"project\" \"p\" provider=\"local\" label=\"Project P\" status=\"retained\""
    ));
    assert!(workspace.contains("fact \"flotilla.project\" \"p\""));
    // Only facts placement reads are kept, not the full fact set.
    assert!(!workspace.contains("action.primary.recipe"), "{workspace}");
    let dashboard = sidebar.export_record("dashboard").unwrap();
    for wanted in [
        "display \"show-finished\" true",
        "display \"show-issues\" false",
        "order \"tree\" \"vessel\"",
        "variable \"density\" \"compact\"",
        "local \".ref\" \"r1\"",
    ] {
        assert!(dashboard.contains(wanted), "{wanted} in {dashboard}");
    }
    // A display variable that doesn't persist is presentation, not saved.
    assert!(!dashboard.contains("\"mode\""), "{dashboard}");
    assert!(sidebar.export_record("workspace/7").is_err());
    assert!(sidebar.export_record("nonsense").is_err());
}

#[test]
fn importing_before_the_first_observation_restores_the_sidebar_without_facts() {
    let mut before = arranged_session();
    let records = export_all(&mut before);

    let mut sidebar = Sidebar::new(&config()).unwrap();
    for (name, text) in &records {
        sidebar.import_record(name, text).unwrap();
    }
    assert!(sidebar.registered_workspaces().contains(&worker()));
    // Records round-trip before anything is observed.
    assert_eq!(export_all(&mut sidebar), records);
    sidebar.observe(vec![workspace(true)], vec![]);
    sidebar.apply(200, []);

    // No producer has published anything: the project and the ended vessel
    // are drawn on their path from the record, live for the workspace.
    let project_row = tree(&sidebar);
    assert_eq!(project_row.entity, entity("project", "p"));
    assert_eq!(project_row.label, "Project P");
    assert!(project_row.collapsed, "row collapse is restored");
    let v = &project_row.children[0];
    assert_eq!(
        (v.entity.clone(), v.label.as_str()),
        (entity("vessel", "v"), "Vee")
    );
    assert!(ended(v));
    assert!(matches!(
        v.state,
        PresentationState::Live { workspace_id, selected: true } if workspace_id == worker()
    ));
    assert_eq!(
        project_row.variables.as_ref().unwrap().values["density"].value,
        "compact",
        "placement variables are restored"
    );
    assert!(!issues_checked(&sidebar), "display variables are restored");

    // Local sections, groups and pins are back, owned by Andamento.
    let snapshot = sidebar.snapshot();
    let all = nodes(&snapshot.surface);
    for local in [
        entity(".section", "s1"),
        entity(".group", "g1"),
        entity(".ref", "r1"),
    ] {
        assert!(all.iter().any(|n| n.entity == local), "{local:?}");
    }
    assert_eq!(sidebar.local_entities().len(), 3);

    // When w is published again, the saved sibling order applies.
    sidebar.apply(300, [vessel("w", "Double")]);
    let order = tree(&sidebar)
        .children
        .iter()
        .map(|n| n.entity.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(order, ["w", "v"]);

    // Opening the restored ended row focuses the same workspace.
    let key = tree(&sidebar).children[1].key.clone();
    let effects = sidebar.dispatch(Action::ActivatePlacement { key }).unwrap();
    assert!(
        matches!(&effects[..], [HostEffect::Focus { workspace_id, .. }] if *workspace_id == worker()),
        "{effects:?}"
    );
}

#[test]
fn a_record_imported_after_observation_still_restores_the_path() {
    let mut before = arranged_session();
    let records = export_all(&mut before);
    let mut sidebar = Sidebar::new(&config()).unwrap();
    sidebar.observe(vec![workspace(true)], vec![]);
    for (name, text) in records.iter().rev() {
        sidebar.import_record(name, text).unwrap();
    }
    let v = &tree(&sidebar).children[0];
    assert_eq!(v.entity, entity("vessel", "v"));
    assert!(ended(v));
}

#[test]
fn closing_a_registered_workspace_keeps_its_record_and_forgetting_drops_it() {
    let mut sidebar = arranged_session();
    let name = format!("workspace/{WORKER}");
    let open = sidebar.export_record(&name).unwrap();
    sidebar.observe(vec![], vec![]);
    assert_eq!(sidebar.export_record(&name).unwrap(), open);
    sidebar.forget_workspace(worker());
    assert_eq!(sidebar.record_names(), ["dashboard"]);
    assert!(sidebar.record_generation(&name).is_err());
}

#[test]
fn unknown_nodes_survive_export_import_export() {
    let mut sidebar = arranged_session();
    for name in sidebar.record_names() {
        let text = sidebar.export_record(&name).unwrap();
        let extended = text.replacen(
            "{\n",
            "{\n    from-the-future \"x\" level=3 {\n        detail #true\n    }\n",
            1,
        );
        // KDL 1 has no #true; use a form it reads.
        let extended = extended.replace("#true", "true");
        let mut fresh = Sidebar::new(&config()).unwrap();
        fresh.import_record(&name, &extended).unwrap();
        let again = fresh.export_record(&name).unwrap();
        assert!(again.contains("from-the-future \"x\" level=3"), "{again}");
        assert!(again.contains("detail true"), "{again}");
        let mut third = Sidebar::new(&config()).unwrap();
        third.import_record(&name, &again).unwrap();
        assert_eq!(third.export_record(&name).unwrap(), again);
    }
}

#[test]
fn other_versions_and_bad_records_are_rejected_without_change() {
    let mut sidebar = arranged_session();
    let records = export_all(&mut sidebar);
    let generations = records
        .iter()
        .map(|(name, _)| sidebar.record_generation(name).unwrap())
        .collect::<Vec<_>>();
    let revision = sidebar.revision();
    for (name, text) in &records {
        let future = text.replacen("version=4", "version=5", 1);
        let error = sidebar.import_record(name, &future).unwrap_err();
        assert!(error.contains("version 5"), "{error}");
        // A record for another name, broken KDL and a malformed known node.
        assert!(sidebar
            .import_record(name, "andamento-record \"workspace/9\" version=1")
            .is_err());
        assert!(sidebar.import_record(name, "{").is_err());
        let broken = text.replacen(
            "{\n",
            "{\n    collapsed {\n        at 1\n    }\n    retained 1\n    display\n",
            1,
        );
        assert!(sidebar.import_record(name, &broken).is_err());
    }
    // A workspace record names a local entity kind only Andamento's own.
    assert!(sidebar
        .import_record(
            "dashboard",
            "andamento-record \"dashboard\" version=1 { local \"vessel\" \"x\"; }"
        )
        .is_err());
    assert!(sidebar
        .import_record("workspace/9", "andamento-record \"workspace/9\" version=5")
        .is_err());
    assert!(!sidebar
        .registered_workspaces()
        .contains(&WorkspaceId::from(9)));
    assert_eq!(sidebar.revision(), revision);
    assert_eq!(export_all(&mut sidebar), records);
    for ((name, _), generation) in records.iter().zip(generations) {
        assert_eq!(sidebar.record_generation(name).unwrap(), generation);
    }
}

#[test]
fn generations_change_only_with_content() {
    let mut sidebar = Sidebar::new(&config()).unwrap();
    let dashboard = sidebar.record_generation("dashboard").unwrap();
    assert_ne!(dashboard, 0);
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), dashboard);
    // Facts, time and topology are not dashboard content.
    sidebar.apply(100, [project(), vessel("v", "Vee")]);
    sidebar.apply(5_000, []);
    let effects = sidebar
        .dispatch(Action::Activate {
            entity: entity("vessel", "v"),
        })
        .unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else {
        panic!("{effects:?}");
    };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![workspace(true)], vec![]);
    sidebar.observe(vec![workspace(false)], vec![]);
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), dashboard);
    // Setting a value it already has changes nothing; a real change does.
    sidebar
        .set_display_variable("show-issues", Some(DisplayVariableValue::Bool(true)))
        .unwrap();
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), dashboard);
    sidebar
        .set_display_variable("mode", Some(DisplayVariableValue::Enum("narrow".into())))
        .unwrap();
    assert_eq!(
        sidebar.record_generation("dashboard").unwrap(),
        dashboard,
        "a variable that doesn't persist is not content"
    );
    sidebar
        .set_display_variable("show-issues", Some(DisplayVariableValue::Bool(false)))
        .unwrap();
    let changed = sidebar.record_generation("dashboard").unwrap();
    assert!(changed > dashboard);
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), changed);

    // A workspace's record changes with its subject's label, not with a
    // heartbeat that repeats the same facts at a later time.
    let name = format!("workspace/{WORKER}");
    let workspace = sidebar.record_generation(&name).unwrap();
    sidebar.apply(6_000, [project(), vessel("v", "Vee")]);
    sidebar.apply(7_000, []);
    assert_eq!(sidebar.record_generation(&name).unwrap(), workspace);
    sidebar.apply(8_000, [vessel("v", "Renamed")]);
    let renamed = sidebar.record_generation(&name).unwrap();
    assert!(renamed > workspace);
    assert!(sidebar
        .export_record(&name)
        .unwrap()
        .contains("label=\"Renamed\""));

    // Importing what is already there is no change.
    let text = sidebar.export_record("dashboard").unwrap();
    sidebar.import_record("dashboard", &text).unwrap();
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), changed);
}

#[test]
fn display_variables_and_local_entities_are_set_directly() {
    let mut sidebar = Sidebar::new(&config()).unwrap();
    // No snapshot or retry is needed, and invalid values change nothing.
    assert!(sidebar
        .set_display_variable("missing", Some(DisplayVariableValue::Bool(true)))
        .is_err());
    assert!(sidebar
        .set_display_variable("show-issues", Some(DisplayVariableValue::Enum("x".into())))
        .is_err());
    assert!(sidebar
        .set_display_variable("mode", Some(DisplayVariableValue::Enum("tall".into())))
        .is_err());
    sidebar
        .set_display_variable("show-issues", Some(DisplayVariableValue::Bool(false)))
        .unwrap();
    assert!(!issues_checked(&sidebar));
    sidebar.set_display_variable("show-issues", None).unwrap();
    assert!(issues_checked(&sidebar), "None returns to the default");

    assert!(sidebar
        .set_local(entity("vessel", "v"), BTreeMap::new())
        .is_err());
    assert!(sidebar
        .set_local(
            entity(".group", "g"),
            BTreeMap::from([("list".into(), MetadataValue::StringList(vec![]))])
        )
        .is_err());
    sidebar
        .set_local(
            entity(".section", "s"),
            BTreeMap::from([
                ("display.label".into(), text("One")),
                ("extra".into(), text("x")),
            ]),
        )
        .unwrap();
    let section = |sidebar: &Sidebar| {
        nodes(&sidebar.snapshot().surface)
            .into_iter()
            .find(|n| n.entity == entity(".section", "s"))
            .cloned()
    };
    assert_eq!(section(&sidebar).unwrap().label, "One");
    // Setting replaces every fact; a dropped one is unset.
    sidebar
        .set_local(
            entity(".section", "s"),
            BTreeMap::from([("display.label".into(), text("Two"))]),
        )
        .unwrap();
    let card = sidebar.detail_card(&entity(".section", "s")).unwrap();
    assert_eq!(card.label, "Two");
    assert!(!card.fields.iter().any(|f| f.name == "extra"));
    assert!(sidebar.remove_local(&entity(".section", "s")));
    assert!(!sidebar.remove_local(&entity(".section", "s")));
    assert!(section(&sidebar).is_none());
}
