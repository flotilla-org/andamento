//! Slots and arrangement documents (#144): a workspace's slots follow its
//! subject's Suggested Layout, the host commits whole arrangements at an
//! expected generation, and neither invalidates the sidebar's snapshot.
//! Portable Target Resolutions (#158) are saved per slot in the record.
mod common;
use andamento_core::managed::{ContentState, Resolved, SlotPlan, TerminalContent};
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::slots::{ArrangementDoc, Panel, PanelNode};
use andamento_core::suggested_layout::{Axis, CommandLine, Content, LocalRecipe, RebindPolicy, ViewSpec};
use andamento_core::target_resolution::PortableResolution;
use andamento_core::versioned::CommitError;
use andamento_core::workspace::WorkspaceRecord;
use andamento_core::{EntityRef, MetadataPatch, MetadataValue, MetadataValueUpdate, Sidebar};
use common::{entity, list, set, text, worker, Feed, CONFIG, WORKER};
use std::collections::BTreeMap;

/// A patch setting `facts` on `subject` and unsetting `unset`.
fn patch(subject: EntityRef, facts: &[(&str, MetadataValue)], unset: &[&str]) -> MetadataPatch {
    let mut patch = common::patch(subject, facts);
    patch.unset = unset.iter().map(|key| key.to_string()).collect();
    patch
}

fn subject() -> EntityRef {
    entity("vessel", "v")
}

/// A local-recipe slot: a shell command.
fn command_slot(key: &str, command: &str) -> Vec<(String, MetadataValue)> {
    vec![(format!("layout.slot.{key}.kind"), text("command")), (format!("layout.slot.{key}.command"), text(command))]
}

/// A provider-facet slot resolved to `target`.
fn facet_slot(key: &str, rebind: &str, target: &str) -> Vec<(String, MetadataValue)> {
    vec![
        (format!("layout.slot.{key}.entity"), MetadataValue::EntityRefs(vec![entity("vessel", key)])),
        (format!("layout.slot.{key}.facet"), text("terminal")),
        (format!("layout.slot.{key}.rebind"), text(rebind)),
        (format!("layout.slot.{key}.state"), text("ready")),
        (format!("layout.slot.{key}.target"), text(target)),
        (format!("layout.slot.{key}.kind"), text("command")),
        (format!("layout.slot.{key}.command"), text(&format!("attach {target}"))),
    ]
}

fn layout(version: &str, slots: Vec<Vec<(String, MetadataValue)>>, keys: &[&str]) -> MetadataPatch {
    let mut facts: Vec<(String, MetadataValue)> = vec![("layout.version".into(), text(version)), ("layout.slots".into(), list(keys))];
    facts.extend(slots.into_iter().flatten());
    let facts: Vec<(&str, MetadataValue)> = facts.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    patch(subject(), &facts, &[])
}

/// A sidebar with a workspace open for vessel v.
fn session(layout_patch: MetadataPatch) -> Sidebar {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    sidebar.feed(
        100,
        [
            patch(entity("project", "p"), &[("flotilla.project", text("p")), ("display.label", text("P"))], &[]),
            patch(
                subject(),
                &[("flotilla.project", text("p")), ("display.label", text("Vee")), ("action.primary.recipe", text("printf hello"))],
                &[],
            ),
            layout_patch,
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

fn keys(sidebar: &Sidebar) -> Vec<String> {
    sidebar.slots(worker()).unwrap().into_iter().map(|slot| slot.key).collect()
}
fn tabs(id: &str, keys: &[&str]) -> Panel {
    Panel {
        id: id.into(),
        weight: 1.0,
        node: PanelNode::Tabs { tabs: keys.iter().map(|k| k.to_string()).collect(), selected: keys.first().map(|k| k.to_string()) },
    }
}
fn row(children: Vec<Panel>) -> ArrangementDoc {
    ArrangementDoc { root: Some(Panel { id: "root".into(), weight: 1.0, node: PanelNode::Split { axis: Axis::Row, children } }) }
}

#[test]
fn slots_follow_the_layout_and_arrangements_reconcile() {
    let mut first = layout("1", vec![command_slot("a", "top"), command_slot("b", "make")], &["a", "b"]);
    for (key, value) in [("layout.root", text("main")), ("layout.panel.main.slots", list(&["a"]))] {
        first.set.insert(key.into(), MetadataValueUpdate { value, ttl_ms: None, precedence: None, ordinal: None });
    }
    let mut sidebar = session(first);
    assert_eq!(keys(&sidebar), ["a", "b"]);
    // Uncommitted, the arrangement is the provider's hint, with b placed.
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(!view.owned);
    assert_eq!(view.doc.tabs(), ["a", "b"]);
    assert_eq!(view.placed, set(&["b"]));

    // The host commits its own arrangement.
    let committed = row(vec![tabs("1", &["a"]), tabs("2", &["b"])]);
    let commit = sidebar.set_arrangement(worker(), committed.clone(), view.generation).unwrap();
    assert!(commit.placed.is_empty() && commit.gone.is_empty());
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.owned);
    assert_eq!(view.generation, commit.generation);
    assert_eq!(view.doc, committed);
    assert!(view.placed.is_empty());

    // A commit against the earlier generation is rejected without effect.
    let record = sidebar.export_record(&format!("workspace/{WORKER}")).unwrap();
    let content = sidebar.content_revision();
    let stale = sidebar.set_arrangement(worker(), row(vec![tabs("1", &["b", "a"])]), 1);
    assert_eq!(stale, Err(CommitError::Stale { current: commit.generation }));
    assert_eq!(sidebar.export_record(&format!("workspace/{WORKER}")).unwrap(), record);
    assert_eq!(sidebar.content_revision(), content);
    // So is one with a tab for a slot that doesn't exist.
    assert!(matches!(
        sidebar.set_arrangement(worker(), row(vec![tabs("1", &["a", "zz"]), tabs("2", &["b"])]), commit.generation,),
        Err(CommitError::Invalid(_))
    ));

    // Baseline 2 drops a and adds c: c is placed in the first tab panel,
    // and a's tab is kept and reported gone.
    let mut second = layout("2", vec![command_slot("b", "make"), command_slot("c", "watch")], &["b", "c"]);
    second.unset =
        vec!["layout.slot.a.kind".into(), "layout.slot.a.command".into(), "layout.root".into(), "layout.panel.main.slots".into()];
    sidebar.feed(200, [second]);
    assert_eq!(keys(&sidebar), ["b", "c"]);
    assert!(sidebar.content_revision() > content);
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.generation > commit.generation);
    assert_eq!(view.doc, row(vec![tabs("1", &["a", "c"]), tabs("2", &["b"])]));
    assert_eq!(view.placed, set(&["c"]));
    assert_eq!(view.gone, set(&["a"]));
    // The host may commit with a's tab still there; it stays reported.
    let commit = sidebar.set_arrangement(worker(), view.doc.clone(), view.generation).unwrap();
    assert_eq!(commit.gone, ["a"]);

    // The user's own slot and an override of b survive in the record.
    let spec = |command: &str| ViewSpec {
        content: Content::Local(LocalRecipe::Command { line: CommandLine::Argv(vec![command.into()]), cwd: Some("/tmp".into()) }),
        presentation: Some("terminal".into()),
    };
    assert!(sidebar.set_slot(worker(), "u:1", spec("htop"), RebindPolicy::Ask).unwrap());
    assert!(sidebar.set_slot(worker(), "b", spec("btop"), RebindPolicy::Replace).unwrap());
    assert!(sidebar.set_slot(worker(), "nope", spec("x"), RebindPolicy::Replace).is_err());
    let slots = sidebar.slots(worker()).unwrap();
    assert!(slots[0].detached);
    assert_eq!(slots[2].key, "u:1");
    // Committing without the new slot's tab places it.
    let generation = sidebar.arrangement(worker()).unwrap().generation;
    let commit = sidebar.set_arrangement(worker(), row(vec![tabs("1", &["c"]), tabs("2", &["b"])]), generation).unwrap();
    assert_eq!(commit.placed, ["u:1"]);
    assert!(commit.gone.is_empty());

    let name = format!("workspace/{WORKER}");
    let record = sidebar.export_record(&name).unwrap();
    let mut fresh = Sidebar::new(CONFIG).unwrap();
    fresh.import_record(&name, &record).unwrap();
    assert_eq!(fresh.slots(worker()), sidebar.slots(worker()));
    let (mut imported, live) = (fresh.arrangement(worker()).unwrap(), sidebar.arrangement(worker()).unwrap());
    imported.generation = live.generation;
    assert_eq!(imported, live);
    assert_eq!(fresh.export_record(&name).unwrap(), record);
}

#[test]
fn arrangement_commits_and_slot_edits_leave_the_snapshot_current() {
    let mut sidebar = session(layout("1", vec![command_slot("a", "top")], &["a"]));
    let snapshot = sidebar.snapshot();
    let revision = sidebar.revision();
    let content = sidebar.content_revision();
    let generation = sidebar.arrangement(worker()).unwrap().generation;
    let mut doc = row(vec![tabs("1", &["a"]), tabs("2", &[])]);
    for weight in [0.5, 0.25, 0.75] {
        if let Some(Panel { node: PanelNode::Split { children, .. }, .. }) = &mut doc.root {
            children[0].weight = weight;
        }
        let generation = sidebar.arrangement(worker()).unwrap().generation;
        sidebar.set_arrangement(worker(), doc.clone(), generation).unwrap();
    }
    sidebar
        .set_slot(
            worker(),
            "u:1",
            ViewSpec { content: Content::Local(LocalRecipe::Url { url: "https://example.com".into() }), presentation: None },
            RebindPolicy::Replace,
        )
        .unwrap();
    assert!(sidebar.content_revision() > content);
    assert!(sidebar.arrangement(worker()).unwrap().generation > generation);
    // The sidebar's revision, and so its snapshot and evaluation, stand.
    assert_eq!(sidebar.revision(), revision);
    assert_eq!(sidebar.snapshot_shared().revision, snapshot.revision);
    assert_eq!(sidebar.snapshot(), snapshot);
}

#[test]
fn rebind_policies_decide_what_happens_to_the_replaced_instance() {
    let slots = || vec![facet_slot("r", "replace", "r#1"), facet_slot("k", "keep-previous", "k#1"), facet_slot("q", "ask", "q#1")];
    let mut sidebar = session(layout("1", slots(), &["r", "k", "q"]));
    // Apply each slot's first resolution.
    let mut applied = BTreeMap::new();
    for key in ["r", "k", "q"] {
        let plan = sidebar.plan_slot(worker(), key, "").unwrap();
        assert_eq!(plan.state, ContentState::Updating);
        let update = plan.update.unwrap();
        assert!(sidebar.managed.complete_slot(worker(), key, update.token, true));
        let plan = sidebar.plan_slot(worker(), key, &update.id).unwrap();
        assert_eq!(plan.state, ContentState::Current);
        // A first instance replaces nothing.
        assert_eq!(plan.previous, None);
        applied.insert(key, update.id);
    }
    assert!(sidebar.plan_slot(worker(), "nope", "").is_err());

    // Every slot resolves to a new instance under the same baseline.
    sidebar.feed(
        200,
        [layout(
            "1",
            vec![facet_slot("r", "replace", "r#2"), facet_slot("k", "keep-previous", "k#2"), facet_slot("q", "ask", "q#2")],
            &["r", "k", "q"],
        )],
    );
    // replace: the old instance goes.
    let plan = sidebar.plan_slot(worker(), "r", &applied["r"]).unwrap();
    assert_eq!((plan.state, plan.rebind), (ContentState::Updating, RebindPolicy::Replace));
    let update = plan.update.unwrap();
    assert!(sidebar.managed.complete_slot(worker(), "r", update.token, true));
    assert_eq!(sidebar.plan_slot(worker(), "r", &update.id).unwrap().previous, None);
    // keep-previous: the old instance stays reachable until released.
    let plan = sidebar.plan_slot(worker(), "k", &applied["k"]).unwrap();
    assert_eq!(plan.rebind, RebindPolicy::KeepPrevious);
    let update = plan.update.unwrap();
    assert!(sidebar.managed.complete_slot(worker(), "k", update.token, true));
    let plan = sidebar.plan_slot(worker(), "k", &update.id).unwrap();
    assert_eq!(plan.state, ContentState::Current);
    assert_eq!(plan.previous.as_ref(), Some(&applied["k"]));
    assert!(sidebar.managed.release_previous(worker(), "k"));
    assert_eq!(sidebar.plan_slot(worker(), "k", &update.id).unwrap().previous, None);
    // ask: the host asks; declining fails the update, which isn't retried
    // until the resolution changes again.
    let plan = sidebar.plan_slot(worker(), "q", &applied["q"]).unwrap();
    assert_eq!(plan.rebind, RebindPolicy::Ask);
    let update = plan.update.unwrap();
    assert!(sidebar.managed.complete_slot(worker(), "q", update.token, false));
    assert_eq!(sidebar.plan_slot(worker(), "q", &applied["q"]).unwrap().state, ContentState::Failed);
    sidebar.feed(300, [layout("1", vec![facet_slot("q", "ask", "q#3")], &["r", "k", "q"])]);
    let plan = sidebar.plan_slot(worker(), "q", &applied["q"]).unwrap();
    assert_eq!(plan.state, ContentState::Updating);
    assert_ne!(plan.update.unwrap().token, update.token);

    // A held resolution suspends the slot without touching its content.
    sidebar.feed(400, [patch(subject(), &[("layout.slot.q.state", text("held"))], &[])]);
    assert_eq!(sidebar.plan_slot(worker(), "q", &applied["q"]).unwrap().state, ContentState::Held);
}

#[test]
fn primary_content_is_the_primary_slot() {
    let mut sidebar =
        session(patch(subject(), &[("workspace.primary.state", text("ready")), ("workspace.primary.target", text("attempt-1"))], &[]));
    // The primary facts alone are a one-slot baseline.
    assert_eq!(keys(&sidebar), ["primary"]);
    assert_eq!(sidebar.arrangement(worker()).unwrap().doc.tabs(), ["primary"]);
    let applied = Resolved::from(TerminalContent { target: "attempt-0".into(), command: "old".into(), cwd: None });
    let slot = sidebar.plan_slot(worker(), "primary", &applied.id()).unwrap();
    let update = slot.update.unwrap();
    assert_eq!(update.resolution.as_terminal(), Some(("attempt-1", "printf hello", None)));
    assert!(sidebar.managed.slot_valid(worker(), "primary", update.token));
    assert!(sidebar.managed.complete_slot(worker(), "primary", update.token, true));
    assert_eq!(sidebar.plan_slot(worker(), "primary", &update.resolution.id()).unwrap().state, ContentState::Current);
}

fn cleat(session: &str) -> PortableResolution {
    PortableResolution {
        kind: "cleat-session".into(),
        fields: [("host", "feta"), ("session", session), ("daemon", "D")].into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
    }
}

#[test]
fn portable_resolutions_are_saved_planned_and_invalidated_by_a_new_target() {
    let slots = |r: &str, k: &str| vec![facet_slot("r", "keep-previous", r), facet_slot("k", "replace", k)];
    let mut sidebar = session(layout("1", slots("r#1", "k#1"), &["r", "k"]));
    let plan = sidebar.plan_slot(worker(), "r", "").unwrap();
    assert_eq!(plan.saved, None);
    let first = plan.update.unwrap();
    assert!(sidebar.managed.complete_slot(worker(), "r", first.token, true));

    // The host saves what it resolved r#1 to, for the resolution it applied.
    let revision = sidebar.content_revision();
    assert!(matches!(sidebar.set_slot_resolution(worker(), "r", cleat("S1"), "other", 0), Err(CommitError::Invalid(_))));
    assert_eq!(sidebar.set_slot_resolution(worker(), "r", cleat("S1"), &first.id, 1), Err(CommitError::Stale { current: 0 }));
    assert!(sidebar.set_slot_resolution(worker(), "nope", cleat("S1"), &first.id, 0).is_err());
    assert_eq!(sidebar.content_revision(), revision);
    let g1 = sidebar.set_slot_resolution(worker(), "r", cleat("S1"), &first.id, 0).unwrap();
    assert!(g1 > 0 && sidebar.content_revision() > revision);
    let saved = sidebar.slot_resolution(worker(), "r").unwrap().unwrap();
    assert_eq!((saved.resolution.clone(), saved.against.as_str(), saved.generation), (cleat("S1"), first.id.as_str(), g1));
    // Saving it again changes nothing.
    let revision = sidebar.content_revision();
    assert_eq!(sidebar.set_slot_resolution(worker(), "r", cleat("S1"), &first.id, g1), Ok(g1));
    assert_eq!(sidebar.content_revision(), revision);
    // The plan reports it whatever the state.
    let plan = sidebar.plan_slot(worker(), "r", &first.id).unwrap();
    assert_eq!(plan.state, ContentState::Current);
    assert_eq!(plan.saved.as_ref(), Some(&saved));

    // It round-trips through the record. Another device with no instance
    // plans Updating to the same resolution, with the saved one to try first.
    let name = format!("workspace/{WORKER}");
    let record = sidebar.export_record(&name).unwrap();
    let mut other = Sidebar::new(CONFIG).unwrap();
    other.import_record(&name, &record).unwrap();
    assert_eq!(other.export_record(&name).unwrap(), record);
    // No facts yet: Unavailable, and the saved resolution is kept, at a
    // generation of the other sidebar's own.
    let same = |plan: &SlotPlan| plan.saved.as_ref().is_some_and(|s| (&s.resolution, &s.against) == (&saved.resolution, &saved.against));
    let plan = other.plan_slot(worker(), "r", "").unwrap();
    assert_eq!(plan.state, ContentState::Unavailable);
    assert!(same(&plan));
    other.feed(100, [layout("1", slots("r#1", "k#1"), &["r", "k"])]);
    let plan = other.plan_slot(worker(), "r", "").unwrap();
    assert_eq!(plan.state, ContentState::Updating);
    assert!(same(&plan));
    assert_eq!(plan.update.unwrap().id, saved.against);

    // The provider resolves r to a new target: the saved resolution is
    // cleared, and the slot rebinds by its policy as for any change.
    let revision = sidebar.content_revision();
    sidebar.feed(200, [layout("1", slots("r#2", "k#1"), &["r", "k"])]);
    assert!(sidebar.content_revision() > revision);
    assert_eq!(sidebar.slot_resolution(worker(), "r").unwrap(), None);
    let record = sidebar.export_record(&name).unwrap();
    let resolutions = WorkspaceRecord::decode(&record, &mut Default::default()).unwrap().resolutions;
    assert!(resolutions.get().get("r").is_none());
    let plan = sidebar.plan_slot(worker(), "r", &first.id).unwrap();
    assert_eq!((plan.state, plan.rebind, plan.saved), (ContentState::Updating, RebindPolicy::KeepPrevious, None));
    let second = plan.update.unwrap();
    assert!(sidebar.managed.complete_slot(worker(), "r", second.token, true));
    let plan = sidebar.plan_slot(worker(), "r", &second.id).unwrap();
    assert_eq!(plan.previous.as_ref(), Some(&first.id));
    // A generation is never given out twice.
    let g2 = sidebar.set_slot_resolution(worker(), "r", cleat("S2"), &second.id, 0).unwrap();
    assert!(g2 > g1);
    // The same happens to a device that imports the old record later.
    other.import_record(&name, &record).unwrap();
    other.feed(300, [layout("1", slots("r#2", "k#1"), &["r", "k"])]);
    assert_eq!(other.slot_resolution(worker(), "r").unwrap(), None);

    // Held (or expired) facts are silence, not a new target: kept.
    let k = sidebar.plan_slot(worker(), "k", "").unwrap().update.unwrap();
    assert!(sidebar.set_slot_resolution(worker(), "k", cleat("K1"), &k.id, 0).unwrap() > g2);
    sidebar.feed(400, [patch(subject(), &[("layout.slot.k.state", text("held"))], &[])]);
    assert_eq!(sidebar.plan_slot(worker(), "k", &k.id).unwrap().state, ContentState::Held);
    assert!(sidebar.slot_resolution(worker(), "k").unwrap().is_some());
    // The user overriding the slot's content clears it too.
    let spec = ViewSpec {
        content: Content::Local(LocalRecipe::Command { line: CommandLine::Shell("htop".into()), cwd: None }),
        presentation: None,
    };
    assert!(sidebar.set_slot(worker(), "k", spec, RebindPolicy::Replace).unwrap());
    assert_eq!(sidebar.slot_resolution(worker(), "k").unwrap(), None);

    // Clearing compares generations.
    assert_eq!(sidebar.clear_slot_resolution(worker(), "r", g1), Err(CommitError::Stale { current: g2 }));
    assert_eq!(sidebar.clear_slot_resolution(worker(), "r", g2), Ok(true));
    assert_eq!(sidebar.clear_slot_resolution(worker(), "r", 0), Ok(false));
    // Removing a slot drops what was saved for it.
    let r = sidebar.plan_slot(worker(), "r", "").unwrap();
    let id = r.update.map_or(second.id.clone(), |u| u.id);
    sidebar.set_slot_resolution(worker(), "r", cleat("S3"), &id, 0).unwrap();
    assert!(sidebar.remove_slot(worker(), "r").unwrap());
    assert!(sidebar.slot_resolution(worker(), "r").is_err());
    let record = WorkspaceRecord::decode(&sidebar.export_record(&name).unwrap(), &mut Default::default()).unwrap();
    assert!(record.resolutions.get().get("r").is_none());
}

/// Regression: an imported record's saved resolutions take generations no
/// earlier state had, so a save prepared against the resolution an import
/// replaced is stale (as for an arrangement), not applied over the import.
#[test]
fn an_imported_resolution_never_reuses_a_generation() {
    let url = |url: &str| ViewSpec { content: Content::Local(LocalRecipe::Url { url: url.into() }), presentation: None };
    let name = format!("workspace/{WORKER}");
    let saved_at = |sidebar: &mut Sidebar, session: &str| {
        sidebar.register_workspace(worker());
        sidebar.set_slot(worker(), "u:1", url("https://a"), RebindPolicy::Replace).unwrap();
        let applied = sidebar.plan_slot(worker(), "u:1", "").unwrap().update.unwrap().id;
        let generation = sidebar.set_slot_resolution(worker(), "u:1", cleat(session), &applied, 0).unwrap();
        (generation, applied)
    };
    let mut here = Sidebar::new(CONFIG).unwrap();
    let mut there = Sidebar::new(CONFIG).unwrap();
    let (read, applied) = saved_at(&mut here, "S1");
    saved_at(&mut there, "S2");
    here.import_record(&name, &there.export_record(&name).unwrap()).unwrap();
    let imported = here.slot_resolution(worker(), "u:1").unwrap().unwrap();
    assert_eq!(imported.resolution, cleat("S2"));
    assert!(imported.generation > read);
    assert_eq!(
        here.set_slot_resolution(worker(), "u:1", cleat("S3"), &applied, read),
        Err(CommitError::Stale { current: imported.generation })
    );
}

/// A sidebar whose config makes vessels workspace subjects, with vessel v
/// published without an opening recipe.
fn subject_session() -> Sidebar {
    let mut sidebar = Sidebar::new(&format!("{CONFIG}\nworkspace kind=\"vessel\"\n")).unwrap();
    sidebar.feed(
        100,
        [
            patch(entity("project", "p"), &[("flotilla.project", text("p")), ("display.label", text("P"))], &[]),
            patch(subject(), &[("flotilla.project", text("p")), ("display.label", text("Vee"))], &[]),
        ],
    );
    sidebar
}

#[test]
fn a_workspace_subject_opens_without_a_recipe_and_its_primary_fills_in() {
    let mut sidebar = subject_session();
    let effects = sidebar.dispatch(Action::Activate { entity: subject() }).unwrap();
    let [HostEffect::Materialize { request_id, ref recipe, ref primary_target, .. }] = effects[..] else {
        panic!("expected materialize: {effects:?}");
    };
    assert_eq!((recipe, primary_target), (&None, &None));
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![Workspace { id: worker(), position: 0, name: "Worker".into(), selected: true }], vec![]);
    assert_eq!(keys(&sidebar), ["primary"]);
    let plan = sidebar.plan_slot(worker(), "primary", "").unwrap();
    assert_eq!((plan.state, plan.update), (ContentState::Unavailable, None));

    // The recipe arrives: the primary resolves to it, at the subject's action target.
    sidebar.feed(200, [patch(subject(), &[("action.primary.recipe", text("attach v"))], &[])]);
    let plan = sidebar.plan_slot(worker(), "primary", "").unwrap();
    assert_eq!(plan.state, ContentState::Updating);
    let update = plan.update.unwrap();
    let expected = Resolved::from(TerminalContent { target: subject().action_target(), command: "attach v".into(), cwd: None });
    assert_eq!(update.resolution, expected);
    assert!(sidebar.managed.complete_slot(worker(), "primary", update.token, true));
    assert_eq!(sidebar.plan_slot(worker(), "primary", &update.id).unwrap().state, ContentState::Current);

    // Withdrawn again, the slot is unavailable; the workspace stays.
    sidebar.feed(300, [patch(subject(), &[], &["action.primary.recipe"])]);
    assert_eq!(sidebar.plan_slot(worker(), "primary", &update.id).unwrap().state, ContentState::Unavailable);
    assert_eq!(keys(&sidebar), ["primary"]);
}

// A workspace opened before its kind was a subject keeps its terminal as
// the user's slot, so it gains no second, provider primary.
#[test]
fn a_workspace_with_its_own_primary_gains_no_provider_primary() {
    let mut sidebar = session(patch(subject(), &[], &[]));
    let terminal = ViewSpec {
        content: Content::Local(LocalRecipe::Command { line: CommandLine::Shell("printf hello".into()), cwd: None }),
        presentation: None,
    };
    sidebar.set_slot(worker(), "u:primary", terminal, RebindPolicy::Replace).unwrap();
    sidebar.configure(&format!("{CONFIG}\nworkspace kind=\"vessel\"\n")).unwrap();
    sidebar.feed(200, [patch(subject(), &[("display.label", text("Renamed"))], &[])]);
    assert_eq!(keys(&sidebar), ["u:primary"]);
}

// A subject's layout may place its primary without publishing
// `workspace.primary.*`: its other slots still resolve.
#[test]
fn a_subject_layout_may_list_its_primary_without_primary_facts() {
    let mut sidebar = subject_session();
    let mut listed = layout("1", vec![command_slot("shell", "make")], &["shell", "primary"]);
    listed.set.insert(
        "action.primary.recipe".into(),
        MetadataValueUpdate { value: text("attach v"), ttl_ms: None, precedence: None, ordinal: None },
    );
    sidebar.feed(150, [listed]);
    let effects = sidebar.dispatch(Action::Activate { entity: subject() }).unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else {
        panic!("expected materialize: {effects:?}");
    };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(vec![Workspace { id: worker(), position: 0, name: "Worker".into(), selected: true }], vec![]);
    assert_eq!(keys(&sidebar), ["shell", "primary"]);
    for key in ["shell", "primary"] {
        assert_eq!(sidebar.plan_slot(worker(), key, "").unwrap().state, ContentState::Updating, "{key}");
    }
}

// An empty opening recipe is none: it opens nothing on its own, and a
// subject with one opens as a subject without a recipe does.
#[test]
fn an_empty_recipe_is_no_recipe() {
    let empty = patch(subject(), &[("action.primary.recipe", text(""))], &[]);
    let mut plain = Sidebar::new(CONFIG).unwrap();
    plain.feed(
        100,
        [
            patch(entity("project", "p"), &[("flotilla.project", text("p")), ("display.label", text("P"))], &[]),
            patch(subject(), &[("flotilla.project", text("p")), ("display.label", text("Vee"))], &[]),
            empty.clone(),
        ],
    );
    let effects = plain.dispatch(Action::Activate { entity: subject() }).unwrap();
    assert!(!effects.iter().any(|effect| matches!(effect, HostEffect::Materialize { .. })), "{effects:?}");
    let mut sidebar = subject_session();
    sidebar.feed(150, [empty]);
    let effects = sidebar.dispatch(Action::Activate { entity: subject() }).unwrap();
    assert!(matches!(&effects[..], [HostEffect::Materialize { recipe: None, .. }]), "{effects:?}");
}
