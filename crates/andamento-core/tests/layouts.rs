//! Workspace Layouts (docs/sidebar-design/workspace-layouts.md): a subject
//! kind's template arranges its workspace's Slots by matchers, the default
//! layout follows the subject's facts until the user chooses one, and each
//! layout keeps the user's edits of its arrangement.
mod common;
use andamento_core::sidebar::{Action, HostEffect, LayoutInfo, Workspace};
use andamento_core::slots::{ArrangementDoc, Panel, PanelNode, Resolve};
use andamento_core::suggested_layout::Axis;
use andamento_core::versioned::Clock;
use andamento_core::workspace::WorkspaceRecord;
use andamento_core::{EntityRef, MetadataPatch, MetadataValue, Sidebar};
use common::{entity, list, text, worker, Feed, CONFIG, WORKER};

const TEMPLATE: &str = r#"
workspace kind="vessel" {
  slot "files" role="files" kind="file" path="{git.root}"
  layout "review" {
    when { match "layout.hint" value="review"; }
    split axis="row" {
      tabs id="review" { slots role="review"; }
      tabs id="agents" { slots role="agent"; }
    }
  }
  layout "work" {
    hide { slots role="agent" { match "flotilla.ended" exists=true; }; }
    split axis="row" {
      tabs id="agents" weight=3 { slots role="agent"; }
      tabs id="side" weight=2 selected="last" { slots role="files"; slots role="review"; }
    }
  }
}
"#;

fn patch(subject: EntityRef, facts: &[(&str, MetadataValue)], unset: &[&str]) -> MetadataPatch {
    let mut patch = common::patch(subject, facts);
    patch.unset = unset.iter().map(|key| key.to_string()).collect();
    patch
}

fn subject() -> EntityRef {
    entity("vessel", "v")
}

/// A provider slot `key` with `role`: an agent is a facet of its own vessel.
fn slot(key: &str, role: &str) -> Vec<(String, MetadataValue)> {
    let at = |field: &str| format!("layout.slot.{key}.{field}");
    let mut facts = vec![(at("role"), text(role))];
    match role {
        "agent" => facts.extend([
            (at("entity"), MetadataValue::EntityRefs(vec![entity("vessel", key)])),
            (at("facet"), text("terminal")),
            (at("state"), text("ready")),
            (at("target"), text(key)),
            (at("kind"), text("command")),
            (at("command"), text(&format!("attach {key}"))),
        ]),
        _ => facts.extend([(at("kind"), text("url")), (at("url"), text("https://example.test"))]),
    }
    facts
}

/// The subject's Suggested Layout, version `version`, with `slots`.
fn layout(version: &str, slots: &[(&str, &str)]) -> MetadataPatch {
    let keys: Vec<&str> = slots.iter().map(|(key, _)| *key).collect();
    let mut facts = vec![("layout.version".to_owned(), text(version)), ("layout.slots".to_owned(), list(&keys))];
    facts.extend(slots.iter().flat_map(|(key, role)| slot(key, role)));
    let facts: Vec<(&str, MetadataValue)> = facts.iter().map(|(key, value)| (key.as_str(), value.clone())).collect();
    patch(subject(), &facts, &[])
}

/// A sidebar with vessel v's workspace open: its primary and agents a and b,
/// a review r, and the template's files.
fn session() -> Sidebar {
    let mut sidebar = Sidebar::new(&format!("{CONFIG}\n{TEMPLATE}")).unwrap();
    sidebar.feed(
        100,
        [
            patch(entity("project", "p"), &[("flotilla.project", text("p")), ("display.label", text("P"))], &[]),
            patch(subject(), &[("flotilla.project", text("p")), ("display.label", text("Vee")), ("git.root", text("/src/v"))], &[]),
            layout("1", &[("a", "agent"), ("b", "agent"), ("r", "review")]),
        ],
    );
    let effects = sidebar.dispatch(Action::Activate { entity: subject() }).unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else {
        panic!("expected materialize: {effects:?}");
    };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![Workspace { id: worker(), position: 0, name: "Worker".into(), selected: true }], vec![]);
    sidebar
}

fn tabs(id: &str, weight: f64, keys: &[&str], selected: Option<&str>) -> Panel {
    Panel {
        id: id.into(),
        weight,
        node: PanelNode::Tabs { tabs: keys.iter().map(|key| key.to_string()).collect(), selected: selected.map(str::to_owned) },
    }
}

fn row(children: Vec<Panel>) -> ArrangementDoc {
    ArrangementDoc { root: Some(Panel { id: "1".into(), weight: 1.0, node: PanelNode::Split { axis: Axis::Row, children } }) }
}

fn current(sidebar: &Sidebar) -> Option<String> {
    sidebar.arrangement(worker()).unwrap().layout
}

#[test]
fn a_template_layout_arranges_slots_by_role() {
    let mut sidebar = session();
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.layout.as_deref(), Some("work"));
    assert!(!view.owned);
    // Tabs follow matcher order, then slot order; the side panel selects
    // its last.
    assert_eq!(
        view.doc,
        row(vec![tabs("agents", 3.0, &["primary", "a", "b"], Some("primary")), tabs("side", 2.0, &["t:files", "r"], Some("r")),])
    );
    assert!(view.unplaced.is_empty());

    // The matchers read the facet entity's facts: an ended agent is hidden.
    sidebar.feed(200, [patch(entity("vessel", "a"), &[("flotilla.ended", MetadataValue::Bool(true))], &[])]);
    let hidden = sidebar.arrangement(worker()).unwrap();
    assert!(hidden.generation > view.generation);
    assert_eq!(hidden.doc.panels()[1], &tabs("agents", 3.0, &["primary", "b"], Some("primary")));
    assert_eq!(hidden.unplaced, ["a"]);
    assert_eq!(sidebar.slots(worker()).unwrap().len(), 5, "an unplaced slot is still the workspace's");

    // A slot no matcher takes is unplaced.
    sidebar.feed(300, [layout("2", &[("a", "agent"), ("b", "agent"), ("r", "review"), ("x", "checks")])]);
    assert_eq!(sidebar.arrangement(worker()).unwrap().unplaced, ["a", "x"]);
}

#[test]
fn the_default_layout_follows_the_facts_until_the_user_chooses() {
    let mut sidebar = session();
    let listed = |sidebar: &Sidebar| sidebar.layouts(worker()).unwrap();
    let info = |name: &str, default: bool, current: bool| LayoutInfo { name: name.into(), default, current };
    assert_eq!(listed(&sidebar), [info("review", false, false), info("work", true, true)]);

    sidebar.feed(200, [patch(subject(), &[("layout.hint", text("review"))], &[])]);
    assert_eq!(current(&sidebar).as_deref(), Some("review"));
    assert_eq!(
        sidebar.arrangement(worker()).unwrap().doc,
        row(vec![tabs("review", 1.0, &["r"], Some("r")), tabs("agents", 1.0, &["primary", "a", "b"], Some("primary"))])
    );
    assert_eq!(sidebar.arrangement(worker()).unwrap().unplaced, ["t:files"]);

    // The user's choice wins over the default, and survives its change.
    let before = sidebar.arrangement(worker()).unwrap().generation;
    let content = sidebar.content_revision();
    assert!(sidebar.choose_layout(worker(), Some("work")).unwrap());
    assert!(sidebar.arrangement(worker()).unwrap().generation > before, "a commit against the old layout is stale");
    assert!(sidebar.content_revision() > content);
    assert_eq!(listed(&sidebar), [info("review", true, false), info("work", false, true)]);
    sidebar.feed(300, [patch(subject(), &[], &["layout.hint"])]);
    sidebar.feed(400, [patch(subject(), &[("layout.hint", text("review"))], &[])]);
    assert_eq!(current(&sidebar).as_deref(), Some("work"));

    assert!(sidebar.choose_layout(worker(), Some("nope")).is_err());
    assert!(!sidebar.choose_layout(worker(), Some("work")).unwrap());
    assert!(sidebar.choose_layout(worker(), None).unwrap());
    assert_eq!(current(&sidebar).as_deref(), Some("review"));
}

#[test]
fn an_edited_layout_keeps_its_panels_matchers() {
    let mut sidebar = session();
    let view = sidebar.arrangement(worker()).unwrap();
    // The user moves b beside the files and leaves r out.
    let edited = row(vec![tabs("agents", 3.0, &["primary", "a"], Some("primary")), tabs("side", 2.0, &["t:files", "b"], Some("b"))]);
    let commit = sidebar.set_arrangement(worker(), edited.clone(), view.generation).unwrap();
    assert!(commit.owned && commit.placed.is_empty());
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.doc, edited);
    assert_eq!(view.unplaced, ["r"], "a slot the commit gives no tab is unplaced");

    // A new agent goes to the panel whose matcher takes it; r stays out.
    sidebar.feed(200, [layout("2", &[("a", "agent"), ("b", "agent"), ("r", "review"), ("c", "agent")])]);
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.doc.panels()[1], &tabs("agents", 3.0, &["primary", "a", "c"], Some("primary")));
    assert_eq!(view.placed, common::set(&["c"]));
    assert_eq!(view.unplaced, ["r"]);

    // With the agents panel merged away, a new agent is unplaced, not put
    // somewhere arbitrary.
    let merged = ArrangementDoc { root: Some(tabs("side", 1.0, &["primary", "a", "c", "t:files", "b"], Some("a"))) };
    sidebar.set_arrangement(worker(), merged.clone(), view.generation).unwrap();
    sidebar.feed(300, [layout("3", &[("a", "agent"), ("b", "agent"), ("r", "review"), ("c", "agent"), ("d", "agent")])]);
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.doc, merged);
    assert_eq!(view.unplaced, ["r", "d"]);

    // Placing an unplaced slot is committing it a tab.
    let shown = ArrangementDoc { root: Some(tabs("side", 1.0, &["primary", "a", "c", "t:files", "b", "r"], Some("r"))) };
    sidebar.set_arrangement(worker(), shown.clone(), view.generation).unwrap();
    assert_eq!(sidebar.arrangement(worker()).unwrap().unplaced, ["d"]);

    // Resetting the layout drops the edits.
    let view = sidebar.arrangement(worker()).unwrap();
    sidebar.resolve_arrangement(worker(), Resolve::Follow, view.generation).unwrap();
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(!view.owned);
    assert_eq!(view.doc.panels()[1], &tabs("agents", 3.0, &["primary", "a", "b", "c", "d"], Some("primary")));
}

#[test]
fn each_layout_keeps_its_own_edits() {
    let mut sidebar = session();
    let view = sidebar.arrangement(worker()).unwrap();
    let edited = row(vec![tabs("agents", 1.0, &["primary", "a", "b", "t:files", "r"], Some("b"))]);
    sidebar.set_arrangement(worker(), edited.clone(), view.generation).unwrap();

    sidebar.choose_layout(worker(), Some("review")).unwrap();
    let review = sidebar.arrangement(worker()).unwrap();
    assert!(!review.owned, "review is as its template makes it");
    assert_eq!(review.layout.as_deref(), Some("review"));
    // A soft edit of review is kept too.
    let mut soft = review.doc.clone();
    if let Some(Panel { node: PanelNode::Split { children, .. }, .. }) = &mut soft.root {
        children[0].weight = 4.0;
    }
    assert!(!sidebar.set_arrangement(worker(), soft.clone(), review.generation).unwrap().owned);

    sidebar.choose_layout(worker(), Some("work")).unwrap();
    let work = sidebar.arrangement(worker()).unwrap();
    assert!(work.owned);
    assert_eq!(work.doc, edited);
    sidebar.choose_layout(worker(), Some("review")).unwrap();
    assert_eq!(sidebar.arrangement(worker()).unwrap().doc, soft);

    // The choice and the put-aside edits are the workspace record's.
    let record = WorkspaceRecord::decode(&sidebar.export_record(&format!("workspace/{WORKER}")).unwrap(), &mut Clock::default()).unwrap();
    assert_eq!(record.chosen_layout(), Some("review"));
    assert_eq!(record.layouts.get().stashed["work"].doc, edited);
}

#[test]
fn a_template_change_is_flagged_on_an_edited_layout() {
    let mut sidebar = session();
    let changed = format!("{CONFIG}\n{}", TEMPLATE.replace("weight=3", "weight=5"));
    // An unedited layout follows its template.
    sidebar.configure(&changed).unwrap();
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.doc.panels()[1].weight, 5.0);
    assert!(!view.provider_changed);

    let edited = row(vec![tabs("agents", 1.0, &["primary", "a", "b", "t:files", "r"], Some("b"))]);
    sidebar.set_arrangement(worker(), edited.clone(), view.generation).unwrap();
    sidebar.configure(&format!("{CONFIG}\n{TEMPLATE}")).unwrap();
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.provider_changed);
    assert_eq!(view.doc, edited, "the change is flagged, not applied");
    sidebar.resolve_arrangement(worker(), Resolve::Keep, view.generation).unwrap();
    sidebar.feed(200, [patch(subject(), &[("display.label", text("Renamed"))], &[])]);
    assert!(!sidebar.arrangement(worker()).unwrap().provider_changed, "keeping settles the change");

    // A configuration without the template falls back to the hint's rule.
    sidebar.configure(CONFIG).unwrap();
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.layout, None);
    assert!(sidebar.layouts(worker()).unwrap().is_empty());
}
