//! The Dashboard's sidebar arrangement (#148) through the sidebar: it follows
//! configuration and local sections, commits at an expected generation
//! without invalidating the snapshot, and survives a restart as part of the
//! dashboard record.
use andamento_core::sidebar_arrangement::SidebarDoc;
use andamento_core::slots::{ArrangementDoc, ArrangementError, Panel, PanelNode};
use andamento_core::suggested_layout::Axis;
use andamento_core::{EntityRef, MetadataValue, Sidebar};
use std::collections::{BTreeMap, BTreeSet};

const TEMPLATE: &str = r#"
template "t" slot="compact" node-kind="entity" {
  field "label" source="literal" value="T"
}
"#;

const LOCAL: &str = r#"
region "local" root-template="t" form="compact" placement="local" default-host="sidebar" order=50
placement "local" {
  for "section" kind=".section" layout="section" {
    order ".position" natural=true
    apply-template "t"
  }
}
"#;

fn region(name: &str, attributes: &str) -> String {
    format!("region \"{name}\" root-template=\"t\" form=\"compact\" {attributes}\n")
}

fn config(regions: &[(&str, &str)]) -> String {
    let mut text = TEMPLATE.to_owned();
    for (name, attributes) in regions {
        text.push_str(&region(name, attributes));
    }
    text
}

fn ab() -> String {
    config(&[
        ("b", "default-host=\"sidebar\" order=20"),
        ("a", "default-host=\"sidebar\" order=10"),
    ])
}

fn tabs(id: &str, keys: &[&str]) -> Panel {
    Panel {
        id: id.into(),
        weight: 1.0,
        node: PanelNode::Tabs {
            tabs: keys.iter().map(|k| k.to_string()).collect(),
            selected: keys.first().map(|k| k.to_string()),
        },
    }
}

fn dock(children: Vec<Panel>) -> SidebarDoc {
    SidebarDoc {
        dock: ArrangementDoc {
            root: Some(Panel {
                id: "root".into(),
                weight: 1.0,
                node: PanelNode::Split {
                    axis: Axis::Column,
                    children,
                },
            }),
        },
        floating: Vec::new(),
    }
}

/// Each dock root child's tabs.
fn docked(sidebar: &Sidebar) -> Vec<Vec<String>> {
    let (view, _) = sidebar.sidebar_arrangement();
    match view.doc.dock.root.map(|root| root.node) {
        Some(PanelNode::Split { children, .. }) => children
            .iter()
            .map(|child| child.tabs().into_iter().map(str::to_owned).collect())
            .collect(),
        other => panic!("expected a split dock: {other:?}"),
    }
}

fn keys(keys: &[&[&str]]) -> Vec<Vec<String>> {
    keys.iter()
        .map(|panel| panel.iter().map(|k| k.to_string()).collect())
        .collect()
}

fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

fn section(id: &str, position: &str) -> (EntityRef, BTreeMap<String, MetadataValue>) {
    (
        EntityRef::local(".section", id),
        BTreeMap::from([
            ("display.label".into(), MetadataValue::Text(id.into())),
            (".position".into(), MetadataValue::Text(position.into())),
        ]),
    )
}

#[test]
fn configuration_places_sections_and_commits_leave_the_snapshot_current() {
    let mut sidebar = Sidebar::new(&ab()).unwrap();
    assert_eq!(docked(&sidebar), keys(&[&["a"], &["b"], &[".unplaced"]]));
    let generation = sidebar.sidebar_arrangement_generation();
    assert_eq!(generation, 1);
    let revision = sidebar.revision();
    let snapshot = sidebar.snapshot();

    // The user reorders and closes the workspace fallback.
    let report = sidebar
        .set_sidebar_arrangement(dock(vec![tabs("1", &["b"]), tabs("2", &["a"])]), 1)
        .unwrap();
    assert!(report.duplicates.is_empty());
    let (view, _) = sidebar.sidebar_arrangement();
    assert_eq!(view.closed, set(&[".unplaced"]));
    assert!(view.owned);
    // A stale commit changes nothing.
    assert_eq!(
        sidebar.set_sidebar_arrangement(dock(vec![tabs("1", &["a", "b"])]), 1),
        Err(ArrangementError::Stale { current: 2 })
    );
    sidebar.restore_sidebar_section(".unplaced", 2).unwrap();
    assert_eq!(docked(&sidebar), keys(&[&["b"], &["a"], &[".unplaced"]]));
    // None of it touched the sidebar's evaluation.
    assert_eq!(sidebar.revision(), revision);
    assert_eq!(sidebar.snapshot(), snapshot);

    // Adding a hinted region places it before its next placed neighbour.
    sidebar
        .configure(&format!("{}{}", ab(), region("c", "order=15")))
        .unwrap();
    assert_eq!(
        docked(&sidebar),
        keys(&[&["c"], &["b"], &["a"], &[".unplaced"]])
    );
    let (_, report) = sidebar.sidebar_arrangement();
    assert_eq!(report.placed, ["c"]);

    // A template that drops b flags it and keeps its place.
    sidebar
        .configure(&config(&[("a", "order=10"), ("c", "order=15")]))
        .unwrap();
    let (view, report) = sidebar.sidebar_arrangement();
    assert_eq!(view.unresolved, set(&["b"]));
    assert_eq!(report.unresolved, ["b"]);
    assert_eq!(
        docked(&sidebar),
        keys(&[&["c"], &["b"], &["a"], &[".unplaced"]])
    );

    // Reset forgets it and places every section by its hints.
    let generation = sidebar.sidebar_arrangement_generation();
    sidebar.reset_sidebar_arrangement(generation).unwrap();
    assert_eq!(docked(&sidebar), keys(&[&["a"], &["c"], &[".unplaced"]]));
}

#[test]
fn the_arrangement_survives_a_restart_in_the_dashboard_record() {
    let mut before = Sidebar::new(&ab()).unwrap();
    before
        .set_sidebar_arrangement(
            dock(vec![
                tabs("1", &["b", "u:notes"]),
                tabs("2", &[".unplaced"]),
            ]),
            1,
        )
        .unwrap();
    let record = before.export_record("dashboard").unwrap();
    assert!(
        record.contains("sidebar generation=2 owned=true"),
        "{record}"
    );
    assert!(record.contains("closed \"a\""), "{record}");

    let mut after = Sidebar::new(&ab()).unwrap();
    after.import_record("dashboard", &record).unwrap();
    assert_eq!(after.export_record("dashboard").unwrap(), record);
    let (view, _) = after.sidebar_arrangement();
    assert_eq!(view.generation, 2);
    assert_eq!(view.closed, set(&["a"]));
    assert_eq!(docked(&after), keys(&[&["b", "u:notes"], &[".unplaced"]]));
    // Closed stays closed after the restart.
    after.configure(&ab()).unwrap();
    assert_eq!(after.sidebar_arrangement().0.closed, set(&["a"]));

    // A stale commit leaves the record byte-identical.
    assert!(after
        .set_sidebar_arrangement(dock(vec![tabs("1", &["a"])]), 1)
        .is_err());
    assert_eq!(after.export_record("dashboard").unwrap(), record);
}

#[test]
fn an_imported_record_with_duplicates_is_repaired_and_older_records_migrate() {
    let mut sidebar = Sidebar::new(&ab()).unwrap();
    let v4 = r#"andamento-record "dashboard" version=4 {
    sidebar generation=5 owned=true {
        dock {
            split "1" axis="column" weight=1.0 {
                tabs "2" weight=1.0 selected="a" {
                    tab "a"
                }
                tabs "3" weight=1.0 selected="a" {
                    tab "a"
                }
            }
        }
        floating {
            tabs "4" weight=1.0 selected="b" {
                tab "b"
                tab "a"
            }
        }
        closed ".unplaced"
    }
}"#;
    sidebar.import_record("dashboard", v4).unwrap();
    let (view, report) = sidebar.sidebar_arrangement();
    assert_eq!(report.duplicates, ["a"]);
    assert_eq!(view.doc.tabs(), ["a", "b"]);
    // Repaired, so it moved past the imported generation.
    assert_eq!(view.generation, 6);

    // A version 3 dashboard has no arrangement: the hints place one.
    let mut migrated = Sidebar::new(&ab()).unwrap();
    migrated
        .import_record(
            "dashboard",
            r#"andamento-record "dashboard" version=3 { display "x" true; }"#,
        )
        .unwrap();
    assert_eq!(docked(&migrated), keys(&[&["a"], &["b"], &[".unplaced"]]));
    assert!(migrated
        .export_record("dashboard")
        .unwrap()
        .starts_with("andamento-record \"dashboard\" version=5"));
}

#[test]
fn local_sections_follow_their_container_and_are_flagged_when_removed() {
    let text = format!(
        "{}{LOCAL}",
        config(&[("tree", "default-host=\"sidebar\" order=10")])
    );
    let mut sidebar = Sidebar::new(&text).unwrap();
    // No local sections yet: the container is a section itself.
    assert_eq!(
        docked(&sidebar),
        keys(&[&["tree"], &["local"], &[".unplaced"]])
    );
    let (entity, facts) = section("pins", "1");
    sidebar.set_local(entity, facts).unwrap();
    let (entity, facts) = section("workspaces", "0");
    sidebar.set_local(entity, facts).unwrap();
    // Each goes in its place among its siblings, by position; the default
    // group's section stays among them, as the container is hinted.
    sidebar
        .set_local(
            EntityRef::local(".group", "g"),
            BTreeMap::from([
                (
                    ".section".into(),
                    MetadataValue::EntityRefs(vec![EntityRef::local(".section", "workspaces")]),
                ),
                (".default".into(), MetadataValue::Bool(true)),
            ]),
        )
        .unwrap();
    assert_eq!(
        docked(&sidebar),
        keys(&[
            &["tree"],
            &["local"],
            &[".section:workspaces"],
            &[".section:pins"],
            &[".unplaced"]
        ])
    );
    let (view, _) = sidebar.sidebar_arrangement();
    assert_eq!(view.unresolved, set(&["local"]));
    // The host drops the stand-in and puts workspaces first.
    let generation = view.generation;
    sidebar
        .set_sidebar_arrangement(
            dock(vec![
                tabs("1", &["tree"]),
                tabs("2", &[".section:workspaces"]),
                tabs("3", &[".section:pins"]),
                tabs("4", &[".unplaced"]),
            ]),
            generation,
        )
        .unwrap();
    // Removing a local section flags its tab; adding a ref changes nothing.
    sidebar.remove_local(&EntityRef::local(".section", "pins"));
    let (view, _) = sidebar.sidebar_arrangement();
    assert_eq!(view.unresolved, set(&[".section:pins"]));
    let generation = view.generation;
    sidebar
        .set_local(
            EntityRef::local(".ref", "r"),
            BTreeMap::from([(
                ".group".into(),
                MetadataValue::EntityRefs(vec![EntityRef::local(".group", "g")]),
            )]),
        )
        .unwrap();
    assert_eq!(sidebar.sidebar_arrangement_generation(), generation);
}
