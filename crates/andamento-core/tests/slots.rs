//! Slots and arrangement documents (#144): a workspace's slots follow its
//! subject's Suggested Layout, the host commits whole arrangements at an
//! expected generation, and neither invalidates the sidebar's snapshot.
use andamento_core::managed::{ContentState, TerminalContent};
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::slots::{ArrangementDoc, ArrangementError, Panel, PanelNode};
use andamento_core::suggested_layout::{
    Axis, CommandLine, Content, LocalRecipe, RebindPolicy, ViewSpec,
};
use andamento_core::{
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, Sidebar,
    WorkspaceId,
};
use std::collections::BTreeSet;

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
}
fn text(s: &str) -> MetadataValue {
    MetadataValue::Text(s.into())
}
fn list(values: &[&str]) -> MetadataValue {
    MetadataValue::StringList(values.iter().map(|v| v.to_string()).collect())
}
fn patch(subject: EntityRef, facts: &[(&str, MetadataValue)], unset: &[&str]) -> MetadataPatch {
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
        unset: unset.iter().map(|key| key.to_string()).collect(),
    }
}

const WORKER: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c";
fn worker() -> WorkspaceId {
    WORKER.parse().unwrap()
}
fn subject() -> EntityRef {
    entity("vessel", "v")
}

/// A local-recipe slot: a shell command.
fn command_slot(key: &str, command: &str) -> Vec<(String, MetadataValue)> {
    vec![
        (format!("layout.slot.{key}.kind"), text("command")),
        (format!("layout.slot.{key}.command"), text(command)),
    ]
}

/// A provider-facet slot resolved to `target`.
fn facet_slot(key: &str, rebind: &str, target: &str) -> Vec<(String, MetadataValue)> {
    vec![
        (
            format!("layout.slot.{key}.entity"),
            MetadataValue::EntityRefs(vec![entity("vessel", key)]),
        ),
        (format!("layout.slot.{key}.facet"), text("terminal")),
        (format!("layout.slot.{key}.rebind"), text(rebind)),
        (format!("layout.slot.{key}.state"), text("ready")),
        (format!("layout.slot.{key}.target"), text(target)),
        (format!("layout.slot.{key}.kind"), text("command")),
        (
            format!("layout.slot.{key}.command"),
            text(&format!("attach {target}")),
        ),
    ]
}

fn layout(version: &str, slots: Vec<Vec<(String, MetadataValue)>>, keys: &[&str]) -> MetadataPatch {
    let mut facts: Vec<(String, MetadataValue)> = vec![
        ("layout.version".into(), text(version)),
        ("layout.slots".into(), list(keys)),
    ];
    facts.extend(slots.into_iter().flatten());
    let facts: Vec<(&str, MetadataValue)> =
        facts.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    patch(subject(), &facts, &[])
}

/// A sidebar with a workspace open for vessel v.
fn session(layout_patch: MetadataPatch) -> Sidebar {
    let mut sidebar = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    sidebar.apply(
        100,
        [
            patch(
                entity("project", "p"),
                &[
                    ("flotilla.project", text("p")),
                    ("display.label", text("P")),
                ],
                &[],
            ),
            patch(
                subject(),
                &[
                    ("flotilla.project", text("p")),
                    ("display.label", text("Vee")),
                    ("action.primary.recipe", text("printf hello")),
                ],
                &[],
            ),
            layout_patch,
        ],
    );
    let effects = sidebar
        .dispatch(Action::Activate { entity: subject() })
        .unwrap();
    let [HostEffect::Materialize { request_id, .. }] = effects[..] else {
        panic!("expected materialize: {effects:?}");
    };
    sidebar.complete(request_id, Ok(Some(worker())));
    sidebar.observe(
        vec![Workspace {
            id: worker(),
            position: 0,
            name: "Worker".into(),
            selected: true,
        }],
        vec![],
    );
    sidebar
}

fn keys(sidebar: &Sidebar) -> Vec<String> {
    sidebar
        .slots(worker())
        .unwrap()
        .into_iter()
        .map(|slot| slot.key)
        .collect()
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
fn row(children: Vec<Panel>) -> ArrangementDoc {
    ArrangementDoc {
        root: Some(Panel {
            id: "root".into(),
            weight: 1.0,
            node: PanelNode::Split {
                axis: Axis::Row,
                children,
            },
        }),
    }
}
fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

#[test]
fn slots_follow_the_layout_and_arrangements_reconcile() {
    let mut first = layout(
        "1",
        vec![command_slot("a", "top"), command_slot("b", "make")],
        &["a", "b"],
    );
    for (key, value) in [
        ("layout.root", text("main")),
        ("layout.panel.main.slots", list(&["a"])),
    ] {
        first.set.insert(
            key.into(),
            MetadataValueUpdate {
                value,
                ttl_ms: None,
                precedence: None,
                ordinal: None,
            },
        );
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
    let commit = sidebar
        .set_arrangement(worker(), committed.clone(), view.generation)
        .unwrap();
    assert!(commit.placed.is_empty() && commit.gone.is_empty());
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.owned);
    assert_eq!(view.generation, commit.generation);
    assert_eq!(view.doc, committed);
    assert!(view.placed.is_empty());

    // A commit against the earlier generation is rejected without effect.
    let record = sidebar
        .export_record(&format!("workspace/{WORKER}"))
        .unwrap();
    let content = sidebar.content_revision();
    let stale = sidebar.set_arrangement(worker(), row(vec![tabs("1", &["b", "a"])]), 1);
    assert_eq!(
        stale,
        Err(ArrangementError::Stale {
            current: commit.generation
        })
    );
    assert_eq!(
        sidebar
            .export_record(&format!("workspace/{WORKER}"))
            .unwrap(),
        record
    );
    assert_eq!(sidebar.content_revision(), content);
    // So is one with a tab for a slot that doesn't exist.
    assert!(matches!(
        sidebar.set_arrangement(
            worker(),
            row(vec![tabs("1", &["a", "zz"]), tabs("2", &["b"])]),
            commit.generation,
        ),
        Err(ArrangementError::Invalid(_))
    ));

    // Baseline 2 drops a and adds c: c is placed in the first tab panel,
    // and a's tab is kept and reported gone.
    let mut second = layout(
        "2",
        vec![command_slot("b", "make"), command_slot("c", "watch")],
        &["b", "c"],
    );
    second.unset = vec![
        "layout.slot.a.kind".into(),
        "layout.slot.a.command".into(),
        "layout.root".into(),
        "layout.panel.main.slots".into(),
    ];
    sidebar.apply(200, [second]);
    assert_eq!(keys(&sidebar), ["b", "c"]);
    assert!(sidebar.content_revision() > content);
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.generation > commit.generation);
    assert_eq!(
        view.doc,
        row(vec![tabs("1", &["a", "c"]), tabs("2", &["b"])])
    );
    assert_eq!(view.placed, set(&["c"]));
    assert_eq!(view.gone, set(&["a"]));
    // The host may commit with a's tab still there; it stays reported.
    let commit = sidebar
        .set_arrangement(worker(), view.doc.clone(), view.generation)
        .unwrap();
    assert_eq!(commit.gone, ["a"]);

    // The user's own slot and an override of b survive in the record.
    let spec = |command: &str| ViewSpec {
        content: Content::Local(LocalRecipe::Command {
            line: CommandLine::Argv(vec![command.into()]),
            cwd: Some("/tmp".into()),
        }),
        presentation: Some("terminal".into()),
    };
    assert!(sidebar
        .set_slot(worker(), "u:1", spec("htop"), RebindPolicy::Ask)
        .unwrap());
    assert!(sidebar
        .set_slot(worker(), "b", spec("btop"), RebindPolicy::Replace)
        .unwrap());
    assert!(sidebar
        .set_slot(worker(), "nope", spec("x"), RebindPolicy::Replace)
        .is_err());
    let slots = sidebar.slots(worker()).unwrap();
    assert!(slots[0].detached);
    assert_eq!(slots[2].key, "u:1");
    // Committing without the new slot's tab places it.
    let generation = sidebar.arrangement(worker()).unwrap().generation;
    let commit = sidebar
        .set_arrangement(
            worker(),
            row(vec![tabs("1", &["c"]), tabs("2", &["b"])]),
            generation,
        )
        .unwrap();
    assert_eq!(commit.placed, ["u:1"]);
    assert!(commit.gone.is_empty());

    let name = format!("workspace/{WORKER}");
    let record = sidebar.export_record(&name).unwrap();
    assert!(
        record.contains(r#"slot "u:1" rebind="ask" presentation="terminal""#),
        "{record}"
    );
    let mut fresh = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    fresh.import_record(&name, &record).unwrap();
    assert_eq!(fresh.slots(worker()), sidebar.slots(worker()));
    assert_eq!(fresh.arrangement(worker()), sidebar.arrangement(worker()));
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
        if let Some(Panel {
            node: PanelNode::Split { children, .. },
            ..
        }) = &mut doc.root
        {
            children[0].weight = weight;
        }
        let generation = sidebar.arrangement(worker()).unwrap().generation;
        sidebar
            .set_arrangement(worker(), doc.clone(), generation)
            .unwrap();
    }
    sidebar
        .set_slot(
            worker(),
            "u:1",
            ViewSpec {
                content: Content::Local(LocalRecipe::Url {
                    url: "https://example.com".into(),
                }),
                presentation: None,
            },
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
    let slots = || {
        vec![
            facet_slot("r", "replace", "r#1"),
            facet_slot("k", "keep-previous", "k#1"),
            facet_slot("q", "ask", "q#1"),
        ]
    };
    let mut sidebar = session(layout("1", slots(), &["r", "k", "q"]));
    // Apply each slot's first resolution.
    let mut applied = std::collections::BTreeMap::new();
    for key in ["r", "k", "q"] {
        let plan = sidebar.plan_slot(worker(), key, "").unwrap();
        assert_eq!(plan.state, ContentState::Updating);
        let update = plan.update.unwrap();
        assert!(sidebar
            .managed
            .complete_slot(worker(), key, update.token, true));
        let plan = sidebar.plan_slot(worker(), key, &update.id).unwrap();
        assert_eq!(plan.state, ContentState::Current);
        // A first instance replaces nothing.
        assert_eq!(plan.previous, None);
        applied.insert(key, update.id);
    }
    assert!(sidebar.plan_slot(worker(), "nope", "").is_err());

    // Every slot resolves to a new instance under the same baseline.
    sidebar.apply(
        200,
        [layout(
            "1",
            vec![
                facet_slot("r", "replace", "r#2"),
                facet_slot("k", "keep-previous", "k#2"),
                facet_slot("q", "ask", "q#2"),
            ],
            &["r", "k", "q"],
        )],
    );
    // replace: the old instance goes.
    let plan = sidebar.plan_slot(worker(), "r", &applied["r"]).unwrap();
    assert_eq!(
        (plan.state, plan.rebind),
        (ContentState::Updating, RebindPolicy::Replace)
    );
    let update = plan.update.unwrap();
    assert!(sidebar
        .managed
        .complete_slot(worker(), "r", update.token, true));
    assert_eq!(
        sidebar
            .plan_slot(worker(), "r", &update.id)
            .unwrap()
            .previous,
        None
    );
    // keep-previous: the old instance stays reachable until released.
    let plan = sidebar.plan_slot(worker(), "k", &applied["k"]).unwrap();
    assert_eq!(plan.rebind, RebindPolicy::KeepPrevious);
    let update = plan.update.unwrap();
    assert!(sidebar
        .managed
        .complete_slot(worker(), "k", update.token, true));
    let plan = sidebar.plan_slot(worker(), "k", &update.id).unwrap();
    assert_eq!(plan.state, ContentState::Current);
    assert_eq!(plan.previous.as_ref(), Some(&applied["k"]));
    assert!(sidebar.managed.release_previous(worker(), "k"));
    assert_eq!(
        sidebar
            .plan_slot(worker(), "k", &update.id)
            .unwrap()
            .previous,
        None
    );
    // ask: the host asks; declining fails the update, which isn't retried
    // until the resolution changes again.
    let plan = sidebar.plan_slot(worker(), "q", &applied["q"]).unwrap();
    assert_eq!(plan.rebind, RebindPolicy::Ask);
    let update = plan.update.unwrap();
    assert!(sidebar
        .managed
        .complete_slot(worker(), "q", update.token, false));
    assert_eq!(
        sidebar
            .plan_slot(worker(), "q", &applied["q"])
            .unwrap()
            .state,
        ContentState::Failed
    );
    sidebar.apply(
        300,
        [layout(
            "1",
            vec![facet_slot("q", "ask", "q#3")],
            &["r", "k", "q"],
        )],
    );
    let plan = sidebar.plan_slot(worker(), "q", &applied["q"]).unwrap();
    assert_eq!(plan.state, ContentState::Updating);
    assert_ne!(plan.update.unwrap().token, update.token);

    // A held resolution suspends the slot without touching its content.
    sidebar.apply(
        400,
        [patch(
            subject(),
            &[("layout.slot.q.state", text("held"))],
            &[],
        )],
    );
    assert_eq!(
        sidebar
            .plan_slot(worker(), "q", &applied["q"])
            .unwrap()
            .state,
        ContentState::Held
    );
}

#[test]
fn primary_content_is_the_primary_slot() {
    let mut sidebar = session(patch(
        subject(),
        &[
            ("workspace.primary.state", text("ready")),
            ("workspace.primary.target", text("attempt-1")),
        ],
        &[],
    ));
    // The primary facts alone are a one-slot baseline.
    assert_eq!(keys(&sidebar), ["primary"]);
    assert_eq!(
        sidebar.arrangement(worker()).unwrap().doc.tabs(),
        ["primary"]
    );
    let applied = TerminalContent {
        target: "attempt-0".into(),
        command: "old".into(),
        cwd: None,
    };
    // The ABI 2 plan and the slot plan share one binding and token.
    let legacy = sidebar
        .managed
        .plan(worker(), subject(), applied.clone())
        .update
        .unwrap();
    let applied_id = andamento_core::managed::Resolved::from(applied).id();
    let slot = sidebar.plan_slot(worker(), "primary", &applied_id).unwrap();
    let update = slot.update.unwrap();
    assert_eq!(update.token, legacy.token);
    assert_eq!(
        update.resolution.as_terminal(),
        Some(("attempt-1", "printf hello", None))
    );
    assert!(sidebar.managed.valid(worker(), legacy.token));
    assert!(sidebar
        .managed
        .complete_slot(worker(), "primary", update.token, true));
    assert_eq!(
        sidebar
            .managed
            .plan(worker(), subject(), legacy.content)
            .state,
        ContentState::Current
    );
}
