//! Named records (#142): Andamento owns the sidebar's logical state and
//! exports it as opaque records, which a fresh sidebar imports before it has
//! observed anything or received a fact.
mod common;
use andamento_core::presentation::{PlacementNode, PresentationState};
use andamento_core::records::{DashboardRecord, DashboardState};
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::workspace::WorkspaceRecord;
use andamento_core::{tab_workspace_id, DisplayVariableValue, EntityRef, MetadataPatch, MetadataValue, Sidebar};
use common::{entity, nodes, refs, text, worker, Feed, WORKER};
use std::rc::Rc;

/// A patch setting `facts` on `subject`, each with `ttl`.
fn patch(subject: EntityRef, facts: &[(&str, MetadataValue)], ttl: Option<u64>) -> MetadataPatch {
    let mut patch = common::patch(subject, facts);
    for update in patch.set.values_mut() {
        update.ttl_ms = ttl;
    }
    patch
}
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
    patch(entity("project", "p"), &[("flotilla.project", text("p")), ("display.label", text("Project P"))], None)
}

fn workspace(selected: bool) -> Workspace {
    Workspace { id: worker(), position: 0, name: "Worker".into(), selected }
}

fn tree(sidebar: &Sidebar) -> Rc<PlacementNode> {
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
    sidebar.feed(100, [project(), vessel("v", "Vee"), vessel("w", "Double")]);
    let effects = sidebar.dispatch(Action::Activate { entity: entity("vessel", "v") }).unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else {
        panic!("expected materialize: {effects:?}");
    };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![workspace(true)], vec![]);

    let project_row = tree(&sidebar);
    let vessels = project_row.children[0].loop_key.clone();
    sidebar.set_sibling_order(vessels, vec![entity("vessel", "w"), entity("vessel", "v")]).unwrap();
    sidebar.dispatch(Action::SetVariable { key: project_row.key.clone(), name: "density".into(), value: Some("compact".into()) }).unwrap();
    sidebar.dispatch(Action::TogglePlacement { key: project_row.key.clone() }).unwrap();
    sidebar.set_display_variable("show-issues", Some(DisplayVariableValue::Bool(false))).unwrap();
    sidebar.set_display_variable("show-finished", Some(DisplayVariableValue::Bool(true))).unwrap();
    sidebar.set_local(EntityRef::local(".section", "s1"), BTreeMap::from([("display.label".into(), text("Mine"))])).unwrap();
    sidebar
        .set_local(
            EntityRef::local(".group", "g1"),
            BTreeMap::from([("display.label".into(), text("Pinned")), (".section".into(), refs(EntityRef::local(".section", "s1")))]),
        )
        .unwrap();
    sidebar
        .set_local(
            EntityRef::local(".ref", "r1"),
            BTreeMap::from([
                (".group".into(), refs(EntityRef::local(".group", "g1"))),
                (".target".into(), refs(entity("vessel", "w"))),
                (".position".into(), MetadataValue::Integer(1)),
            ]),
        )
        .unwrap();
    // The producer removes v: the open workspace keeps it, ended.
    let mut removal = patch(entity("vessel", "v"), &[], None);
    removal.unset = ["entity.kind", "entity.id", "flotilla.project", "display.label", "status.state", "action.primary.recipe"]
        .map(String::from)
        .to_vec();
    sidebar.feed(150, [removal]);
    sidebar.snapshot();
    sidebar.tick(150);
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

fn ended_workspace_records(kind: &str) -> (String, Vec<(String, String)>) {
    let config = config().replace("vessel", kind);
    let subject = entity(kind, "v");
    let mut sidebar = Sidebar::new(&config).unwrap();
    let mut publication = vessel("v", "Worker");
    publication.target = andamento_core::MetadataTarget::Entity(subject.clone());
    publication.set.get_mut("entity.kind").unwrap().value = text(kind);
    sidebar.feed(100, [project(), publication.clone()]);
    let effects = sidebar.dispatch(Action::Activate { entity: subject }).unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else { panic!("{effects:?}") };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![workspace(true)], vec![]);
    let mut removal = patch(entity(kind, "v"), &[], None);
    removal.unset = publication.set.keys().cloned().collect();
    sidebar.feed(150, [removal]);
    assert!(nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && ended(node)));
    (config, export_all(&mut sidebar))
}

#[test]
fn fresh_lifecycle_overrides_saved_ended_when_opening_and_after_another_restart() {
    for (kind, phase) in [("role", "landed"), ("vessel", "active"), ("convoy", "pending"), ("convoy", "interrupted"), ("convoy", "landing")]
    {
        let (config, records) = ended_workspace_records(kind);
        let mut sidebar = Sidebar::new(&config).unwrap();
        for (name, record) in &records {
            sidebar.import_record(name, record).unwrap();
        }
        sidebar.feed(
            200,
            [
                project(),
                patch(
                    entity(kind, "v"),
                    &[
                        ("entity.kind", text(kind)),
                        ("entity.id", text("v")),
                        ("flotilla.project", text("p")),
                        ("display.label", text("Worker")),
                        ("flotilla.convoy.phase", text(phase)),
                    ],
                    None,
                ),
            ],
        );
        assert!(
            nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && !ended(node)),
            "{kind} before open"
        );
        sidebar.observe(vec![workspace(true)], vec![]);
        assert!(
            nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && !ended(node)),
            "{kind} after open"
        );
        let name = format!("workspace/{WORKER}");
        let record = sidebar.export_record(&name).unwrap();
        let decoded = WorkspaceRecord::decode(&record, &mut Default::default()).unwrap();
        assert!(!decoded.subject.get().retained[&entity(kind, "v")].ended, "{kind} persisted correction");
        // Losing the connection or restarting without facts retains the corrected history.
        sidebar.set_provider_stale(common::PROVIDER, true);
        assert!(
            nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && !ended(node)),
            "{kind} disconnected"
        );
        let mut restored = Sidebar::new(&config).unwrap();
        restored.import_record(&name, &record).unwrap();
        restored.observe(vec![workspace(true)], vec![]);
        assert!(
            nodes(&restored.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && !ended(node)),
            "{kind} second restart"
        );
    }
}

#[test]
fn saved_terminal_phase_without_ended_marker_waits_for_fresh_evidence() {
    let (config, records) = ended_workspace_records("vessel");
    let name = format!("workspace/{WORKER}");
    let encoded = &records.iter().find(|(key, _)| *key == name).unwrap().1;
    let mut clock = Default::default();
    let mut record = WorkspaceRecord::decode(encoded, &mut clock).unwrap();
    let mut subject = record.subject.get().clone();
    let retained = subject.retained.get_mut(&entity("vessel", "v")).unwrap();
    retained.ended = false;
    retained.facts.insert("flotilla.convoy.phase".into(), text("landed"));
    record.follow_subject(&mut clock, subject);
    let mut sidebar = Sidebar::new(&config).unwrap();
    sidebar.import_record(&name, &record.encode()).unwrap();
    sidebar.observe(vec![workspace(true)], vec![]);
    assert!(nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity("vessel", "v") && !ended(node)));
    sidebar.feed(200, [patch(entity("vessel", "v"), &[("flotilla.convoy.phase", text("landed"))], None)]);
    assert!(nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity("vessel", "v") && ended(node)));
}

#[test]
fn terminal_evidence_received_while_stale_waits_for_reconnect() {
    let mut sidebar = arranged_session();
    sidebar.feed(200, [vessel("v", "Worker"), patch(entity("vessel", "v"), &[("flotilla.convoy.phase", text("active"))], None)]);
    assert!(nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity("vessel", "v") && !ended(node)));
    assert!(sidebar.set_provider_stale(common::PROVIDER, true));
    sidebar.feed(201, [patch(entity("vessel", "v"), &[("flotilla.convoy.phase", text("landed"))], None)]);
    assert!(nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity("vessel", "v") && !ended(node)));
    assert!(sidebar.set_provider_stale(common::PROVIDER, false));
    assert!(nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity("vessel", "v") && ended(node)));
}

#[test]
fn stale_lifecycle_waits_for_reconnect_before_correcting_saved_history() {
    for kind in ["role", "vessel", "convoy"] {
        let (config, records) = ended_workspace_records(kind);
        let mut sidebar = Sidebar::new(&config).unwrap();
        for (name, record) in &records {
            sidebar.import_record(name, record).unwrap();
        }
        sidebar.set_provider_stale(common::PROVIDER, true);
        sidebar.feed(
            200,
            [
                project(),
                patch(
                    entity(kind, "v"),
                    &[
                        ("entity.kind", text(kind)),
                        ("entity.id", text("v")),
                        ("flotilla.project", text("p")),
                        ("display.label", text("Worker")),
                        ("flotilla.convoy.phase", text("active")),
                    ],
                    None,
                ),
            ],
        );
        sidebar.observe(vec![workspace(true)], vec![]);
        assert!(nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && ended(node)), "{kind} stale");
        sidebar.set_provider_stale(common::PROVIDER, false);
        assert!(
            nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && !ended(node)),
            "{kind} reconnected"
        );
    }
}

#[test]
fn identity_and_recipe_without_live_lifecycle_do_not_revive_ended_workspaces() {
    for kind in ["vessel", "convoy"] {
        for (phase, superseded) in [
            (None, false),
            (Some("landed"), false),
            (Some("abandoned"), false),
            (Some("cancelled"), false),
            (Some("failed"), false),
            (Some("anchored"), false),
            (Some("unrecognized"), false),
            (Some("active"), true),
        ] {
            let (config, records) = ended_workspace_records(kind);
            let mut sidebar = Sidebar::new(&config).unwrap();
            for (name, record) in &records {
                sidebar.import_record(name, record).unwrap();
            }
            let mut publication = vessel("v", "Worker");
            publication.target = andamento_core::MetadataTarget::Entity(entity(kind, "v"));
            publication.set.get_mut("entity.kind").unwrap().value = text(kind);
            sidebar.feed(200, [project(), publication]);
            if let Some(phase) = phase {
                sidebar.feed(201, [patch(entity(kind, "v"), &[("flotilla.convoy.phase", text(phase))], None)]);
            }
            if superseded {
                sidebar.feed(202, [patch(entity(kind, "v"), &[("flotilla.convoy.superseded", MetadataValue::Bool(true))], None)]);
            }
            sidebar.observe(vec![workspace(true)], vec![]);
            assert!(
                nodes(&sidebar.snapshot().surface).iter().any(|node| node.entity == entity(kind, "v") && ended(node)),
                "{kind} {phase:?}"
            );
        }
    }
}

#[test]
fn historical_ancestor_cannot_end_a_live_workspace_or_end_it_on_disconnect() {
    for kind in ["role", "vessel", "convoy"] {
        let config = config().replace("vessel", kind);
        let subject = entity(kind, "v");
        let mut sidebar = Sidebar::new(&config).unwrap();
        sidebar.feed(
            100,
            [
                patch(
                    entity("project", "p"),
                    &[
                        ("entity.kind", text("project")),
                        ("entity.id", text("p")),
                        ("flotilla.project", text("p")),
                        ("display.label", text("Project")),
                    ],
                    None,
                ),
                patch(
                    subject.clone(),
                    &[
                        ("entity.kind", text(kind)),
                        ("entity.id", text("v")),
                        ("flotilla.project", text("p")),
                        ("display.label", text("Worker")),
                        ("flotilla.convoy.phase", text("active")),
                        ("action.primary.recipe", text("printf hello")),
                    ],
                    None,
                ),
            ],
        );
        let effects = sidebar.dispatch(Action::Activate { entity: subject.clone() }).unwrap();
        let [HostEffect::Materialize { request_id, .. }] = effects[..] else { panic!("{effects:?}") };
        sidebar.complete(request_id, Ok(Some(worker())));
        sidebar.observe(vec![workspace(true)], vec![]);
        let mut removal = patch(entity("project", "p"), &[], None);
        removal.unset = ["entity.kind", "entity.id", "flotilla.project", "display.label"].map(str::to_owned).to_vec();
        sidebar.feed(200, [removal]);
        for stale in [false, true, false] {
            sidebar.set_provider_stale(common::PROVIDER, stale);
            let snapshot = sidebar.snapshot();
            assert!(nodes(&snapshot.surface).iter().any(|node| node.entity == entity("project", "p") && ended(node)));
            assert!(nodes(&snapshot.surface).iter().any(|node| node.entity == subject && !ended(node)), "{kind} stale={stale}");
        }
    }
}

#[test]
fn records_are_named_for_the_dashboard_and_registered_workspaces() {
    let sidebar = arranged_session();
    assert_eq!(sidebar.record_names(), vec!["dashboard".to_owned(), format!("workspace/{WORKER}")]);
    let workspace =
        WorkspaceRecord::decode(&sidebar.export_record(&format!("workspace/{WORKER}")).unwrap(), &mut Default::default()).unwrap();
    assert_eq!(workspace.subject(), Some(&entity("vessel", "v")));
    let vessel = &workspace.subject.get().retained[&entity("vessel", "v")];
    assert_eq!((vessel.label.as_deref(), vessel.ended, vessel.last_seen_ms), (Some("Vee"), true, Some(150)));
    let project = &workspace.subject.get().retained[&entity("project", "p")];
    assert_eq!((project.label.as_deref(), project.ended), (Some("Project P"), false));
    // Only facts placement reads are kept, not the full fact set.
    for retained in workspace.subject.get().retained.values() {
        assert_eq!(retained.facts.keys().collect::<Vec<_>>(), ["flotilla.project"]);
    }
    let dashboard = DashboardRecord::decode(&sidebar.export_record("dashboard").unwrap()).unwrap();
    // A display variable that doesn't persist is presentation, not saved.
    assert_eq!(
        dashboard.state.display,
        BTreeMap::from([
            ("show-finished".into(), DisplayVariableValue::Bool(true)),
            ("show-issues".into(), DisplayVariableValue::Bool(false)),
        ])
    );
    assert_eq!(dashboard.state.orders.len(), 1);
    assert_eq!(dashboard.state.variables.len(), 1);
    assert!(dashboard.state.local.contains_key(&EntityRef::local(".ref", "r1")));
    let unknown = format!("workspace/{}", tab_workspace_id(7));
    assert!(sidebar.export_record(&unknown).is_err());
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
    sidebar.tick(200);

    // No producer has published anything: the project and the ended vessel
    // are drawn on their path from the record, live for the workspace.
    let project_row = tree(&sidebar);
    assert_eq!(project_row.entity, entity("project", "p"));
    assert_eq!(project_row.label, "Project P");
    assert!(project_row.collapsed, "row collapse is restored");
    let v = &project_row.children[0];
    assert_eq!((v.entity.clone(), v.label.as_str()), (entity("vessel", "v"), "Vee"));
    assert!(ended(v));
    assert!(matches!(
        v.state,
        PresentationState::Live { workspace_id, selected: true } if workspace_id == worker()
    ));
    assert_eq!(project_row.variables.as_ref().unwrap().values["density"].value, "compact", "placement variables are restored");
    assert!(!issues_checked(&sidebar), "display variables are restored");

    // Local sections, groups and pins are back, owned by Andamento.
    let snapshot = sidebar.snapshot();
    let all = nodes(&snapshot.surface);
    for local in [EntityRef::local(".section", "s1"), EntityRef::local(".group", "g1"), EntityRef::local(".ref", "r1")] {
        assert!(all.iter().any(|n| n.entity == local), "{local:?}");
    }
    assert_eq!(sidebar.local_entities().len(), 3);

    // When w is published again, the saved sibling order applies.
    sidebar.feed(300, [vessel("w", "Double")]);
    let order = tree(&sidebar).children.iter().map(|n| n.entity.id.clone()).collect::<Vec<_>>();
    assert_eq!(order, ["w", "v"]);

    // Opening the restored ended row focuses the same workspace.
    let key = tree(&sidebar).children[1].key.clone();
    let effects = sidebar.dispatch(Action::ActivatePlacement { key }).unwrap();
    assert!(matches!(&effects[..], [HostEffect::Focus { workspace_id, .. }] if *workspace_id == worker()), "{effects:?}");
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
fn bad_records_are_rejected_without_change() {
    let mut sidebar = arranged_session();
    let records = export_all(&mut sidebar);
    let generations = records.iter().map(|(name, _)| sidebar.record_generation(name).unwrap()).collect::<Vec<_>>();
    let revision = sidebar.revision();
    let not_local = DashboardRecord {
        state: DashboardState { local: BTreeMap::from([(entity("vessel", "x"), BTreeMap::new())]), ..DashboardState::default() },
        sidebar: None,
    };
    let workspace = format!("workspace/{WORKER}");
    for (name, text, reason) in [
        ("dashboard", "{".to_owned(), "not a dashboard record"),
        ("dashboard", not_local.encode(), "\"vessel\" is not a local entity kind"),
        (workspace.as_str(), "[]".to_owned(), "not a workspace record"),
        ("workspace/9", "{}".to_owned(), "unknown record name"),
    ] {
        let error = sidebar.import_record(name, &text).unwrap_err();
        assert!(error.contains(reason), "{name}: {error}");
    }
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
    sidebar.feed(100, [project(), vessel("v", "Vee")]);
    sidebar.tick(5_000);
    let effects = sidebar.dispatch(Action::Activate { entity: entity("vessel", "v") }).unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else {
        panic!("{effects:?}");
    };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![workspace(true)], vec![]);
    sidebar.observe(vec![workspace(false)], vec![]);
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), dashboard);
    // Setting a value it already has changes nothing; a real change does.
    sidebar.set_display_variable("show-issues", Some(DisplayVariableValue::Bool(true))).unwrap();
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), dashboard);
    sidebar.set_display_variable("mode", Some(DisplayVariableValue::Enum("narrow".into()))).unwrap();
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), dashboard, "a variable that doesn't persist is not content");
    sidebar.set_display_variable("show-issues", Some(DisplayVariableValue::Bool(false))).unwrap();
    let changed = sidebar.record_generation("dashboard").unwrap();
    assert!(changed > dashboard);
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), changed);

    // A workspace's record changes with its subject's label, not with a
    // heartbeat that repeats the same facts at a later time.
    let name = format!("workspace/{WORKER}");
    let workspace = sidebar.record_generation(&name).unwrap();
    sidebar.feed(6_000, [project(), vessel("v", "Vee")]);
    sidebar.tick(7_000);
    assert_eq!(sidebar.record_generation(&name).unwrap(), workspace);
    sidebar.feed(8_000, [vessel("v", "Renamed")]);
    let renamed = sidebar.record_generation(&name).unwrap();
    assert!(renamed > workspace);
    let record = WorkspaceRecord::decode(&sidebar.export_record(&name).unwrap(), &mut Default::default()).unwrap();
    assert_eq!(record.subject.get().retained[&entity("vessel", "v")].label.as_deref(), Some("Renamed"));

    // Importing what is already there is no change.
    let text = sidebar.export_record("dashboard").unwrap();
    sidebar.import_record("dashboard", &text).unwrap();
    assert_eq!(sidebar.record_generation("dashboard").unwrap(), changed);
}

#[test]
fn display_variables_and_local_entities_are_set_directly() {
    let mut sidebar = Sidebar::new(&config()).unwrap();
    // No snapshot or retry is needed, and invalid values change nothing.
    for (name, value, reason) in [
        ("missing", DisplayVariableValue::Bool(true), "unknown display variable"),
        ("show-issues", DisplayVariableValue::Enum("x".into()), "not allowed"),
        ("mode", DisplayVariableValue::Enum("tall".into()), "not allowed"),
    ] {
        let error = sidebar.set_display_variable(name, Some(value)).unwrap_err();
        assert!(error.contains(reason), "{name}: {error}");
    }
    sidebar.set_display_variable("show-issues", Some(DisplayVariableValue::Bool(false))).unwrap();
    assert!(!issues_checked(&sidebar));
    sidebar.set_display_variable("show-issues", None).unwrap();
    assert!(issues_checked(&sidebar), "None returns to the default");

    for (entity, facts, reason) in [
        (entity("vessel", "v"), BTreeMap::new(), "local entities have the \"local\" provider"),
        (EntityRef::local("vessel", "v"), BTreeMap::new(), "local entities are"),
        (EntityRef::local(".group", "g"), BTreeMap::from([("list".into(), MetadataValue::StringList(vec![]))]), "local facts are text"),
        (
            EntityRef::local(".group", "g"),
            BTreeMap::from([(".section".into(), refs(EntityRef::new("", ".section", "s")))]),
            "names no provider",
        ),
    ] {
        let error = sidebar.set_local(entity, facts).unwrap_err();
        assert!(error.contains(reason), "{error}");
    }
    sidebar
        .set_local(EntityRef::local(".section", "s"), BTreeMap::from([("display.label".into(), text("One")), ("extra".into(), text("x"))]))
        .unwrap();
    let section =
        |sidebar: &Sidebar| nodes(&sidebar.snapshot().surface).into_iter().find(|n| n.entity == EntityRef::local(".section", "s")).cloned();
    assert_eq!(section(&sidebar).unwrap().label, "One");
    // Setting replaces every fact; a dropped one is unset.
    sidebar.set_local(EntityRef::local(".section", "s"), BTreeMap::from([("display.label".into(), text("Two"))])).unwrap();
    let card = sidebar.detail_card(&EntityRef::local(".section", "s")).unwrap();
    assert_eq!(card.label, "Two");
    assert!(!card.fields.iter().any(|f| f.name == "extra"));
    assert!(sidebar.remove_local(&EntityRef::local(".section", "s")));
    assert!(!sidebar.remove_local(&EntityRef::local(".section", "s")));
    assert!(section(&sidebar).is_none());
}
