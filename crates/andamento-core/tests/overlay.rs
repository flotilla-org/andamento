//! The Workspace Overlay as an addressed edit set (#145, Wheelhouse ADR
//! 0013). Each test replays a provider's Suggested Layout through its
//! versions against a workspace the user has edited, and checks that the
//! edits survive and that nothing is dropped silently.
use andamento_core::dashboard_overlay::KeyKind;
use andamento_core::managed::ContentState;
use andamento_core::records::decode_proposal;
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::slots::{
    flag, ArrangementDoc, ContentChange, Panel, PanelNode, Resolve, SlotInfo,
};
use andamento_core::suggested_layout::{
    Axis, BaselineVersion, CommandLine, Content, LocalRecipe, RebindPolicy, ViewSpec,
};
use andamento_core::{
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, Sidebar,
    WorkspaceId,
};
use std::collections::{BTreeMap, BTreeSet};

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
}
fn text(s: &str) -> MetadataValue {
    MetadataValue::Text(s.into())
}
fn list(values: &[&str]) -> MetadataValue {
    MetadataValue::StringList(values.iter().map(|v| v.to_string()).collect())
}
fn update(value: MetadataValue) -> MetadataValueUpdate {
    MetadataValueUpdate {
        value,
        ttl_ms: None,
        precedence: None,
        ordinal: None,
    }
}
fn patch(subject: EntityRef, facts: Vec<(String, MetadataValue)>) -> MetadataPatch {
    MetadataPatch {
        target: MetadataTarget::Entity(subject),
        source_id: "fixture".into(),
        set: facts.into_iter().map(|(k, v)| (k, update(v))).collect(),
        unset: vec![],
    }
}

const WORKER: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c";
fn worker() -> WorkspaceId {
    WORKER.parse().unwrap()
}
fn subject() -> EntityRef {
    entity("vessel", "v")
}

/// One version of the provider's Suggested Layout: slots as `(key, shell
/// command, rebind)`, and an optional hint of tab panels in a row split
/// (`(panel, [slots])`). Facts a version leaves out are unset, so each patch
/// replaces the layout whole.
struct Layout<'a> {
    version: &'a str,
    slots: &'a [(&'a str, &'a str, &'a str)],
    hint: &'a [(&'a str, &'a [&'a str])],
}

impl Layout<'_> {
    fn facts(&self) -> Vec<(String, MetadataValue)> {
        let mut facts = vec![
            ("layout.version".to_owned(), text(self.version)),
            (
                "layout.slots".to_owned(),
                list(&self.slots.iter().map(|s| s.0).collect::<Vec<_>>()),
            ),
        ];
        for (key, command, rebind) in self.slots {
            facts.push((format!("layout.slot.{key}.kind"), text("command")));
            facts.push((format!("layout.slot.{key}.command"), text(command)));
            facts.push((format!("layout.slot.{key}.rebind"), text(rebind)));
        }
        if !self.hint.is_empty() {
            facts.push(("layout.root".into(), text("main")));
            facts.push((
                "layout.panel.main.children".into(),
                list(&self.hint.iter().map(|p| p.0).collect::<Vec<_>>()),
            ));
            facts.push(("layout.panel.main.axis".into(), text("row")));
            for (panel, slots) in self.hint {
                facts.push((format!("layout.panel.{panel}.slots"), list(slots)));
            }
        }
        facts
    }
}

/// Replay one layout version: set its facts and unset the previous one's
/// that it leaves out.
fn publish(sidebar: &mut Sidebar, now: u64, previous: Option<&Layout>, next: &Layout) {
    let facts = next.facts();
    let keys: BTreeSet<&String> = facts.iter().map(|(k, _)| k).collect();
    let unset = previous
        .map(|p| p.facts())
        .unwrap_or_default()
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| !keys.contains(k))
        .collect();
    let mut patch = patch(subject(), facts);
    patch.unset = unset;
    sidebar.apply(now, [patch]);
}

/// A sidebar with a workspace open for vessel v at `first`.
fn session(first: &Layout) -> Sidebar {
    let mut sidebar = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    sidebar.apply(
        100,
        [
            patch(
                entity("project", "p"),
                vec![
                    ("flotilla.project".into(), text("p")),
                    ("display.label".into(), text("P")),
                ],
            ),
            patch(
                subject(),
                vec![
                    ("flotilla.project".into(), text("p")),
                    ("display.label".into(), text("Vee")),
                    ("action.primary.recipe".into(), text("printf hello")),
                ],
            ),
        ],
    );
    publish(&mut sidebar, 100, None, first);
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

fn slots(sidebar: &Sidebar) -> BTreeMap<String, SlotInfo> {
    sidebar
        .slots(worker())
        .unwrap()
        .into_iter()
        .map(|slot| (slot.key.clone(), slot))
        .collect()
}
fn keys(sidebar: &Sidebar) -> Vec<String> {
    sidebar
        .slots(worker())
        .unwrap()
        .into_iter()
        .map(|slot| slot.key)
        .collect()
}
fn shell(command: &str) -> ViewSpec {
    ViewSpec {
        content: Content::Local(LocalRecipe::Command {
            line: CommandLine::Shell(command.into()),
            cwd: None,
        }),
        presentation: None,
    }
}
fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}
fn panel_mut<'a>(doc: &'a mut ArrangementDoc, id: &str) -> &'a mut Panel {
    fn walk<'a>(panel: &'a mut Panel, id: &str) -> Option<&'a mut Panel> {
        if panel.id == id {
            return Some(panel);
        }
        match &mut panel.node {
            PanelNode::Split { children, .. } => children.iter_mut().find_map(|c| walk(c, id)),
            PanelNode::Tabs { .. } => None,
        }
    }
    walk(doc.root.as_mut().unwrap(), id).unwrap()
}
fn panel_tabs(doc: &ArrangementDoc, id: &str) -> Vec<String> {
    let panel = doc.panels().into_iter().find(|p| p.id == id).unwrap();
    match &panel.node {
        PanelNode::Tabs { tabs, .. } => tabs.clone(),
        PanelNode::Split { .. } => panic!("{id} is a split"),
    }
}
fn weight(doc: &ArrangementDoc, id: &str) -> f64 {
    doc.panels()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap()
        .weight
}

const V1: Layout = Layout {
    version: "1",
    slots: &[
        ("a", "top", "replace"),
        ("b", "make", "keep-previous"),
        ("c", "tail -f log", "ask"),
    ],
    hint: &[("left", &["a", "b"]), ("right", &["c"])],
};

#[test]
fn a_new_baseline_slot_appears_placed_by_the_default_rule() {
    let mut sidebar = session(&V1);
    let v2 = Layout {
        version: "2",
        slots: &[
            ("a", "top", "replace"),
            ("b", "make", "keep-previous"),
            ("c", "tail -f log", "ask"),
            ("d", "htop", "replace"),
        ],
        hint: &[("left", &["a", "b"]), ("right", &["c", "d"])],
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    assert_eq!(keys(&sidebar), ["a", "b", "c", "d"]);
    // Not owned: the hint places it.
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(panel_tabs(&view.doc, "right"), ["c", "d"]);

    // Owned: the default rule (the first tab panel) places it, flagged.
    let mut doc = view.doc.clone();
    if let PanelNode::Tabs { tabs, .. } = &mut panel_mut(&mut doc, "left").node {
        tabs.reverse();
    }
    sidebar
        .set_arrangement(worker(), doc, view.generation)
        .unwrap();
    let v3 = Layout {
        version: "3",
        slots: &[
            ("a", "top", "replace"),
            ("b", "make", "keep-previous"),
            ("c", "tail -f log", "ask"),
            ("d", "htop", "replace"),
            ("e", "btop", "replace"),
        ],
        hint: &[("left", &["a", "b"]), ("right", &["c", "d", "e"])],
    };
    publish(&mut sidebar, 300, Some(&v2), &v3);
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(panel_tabs(&view.doc, "left"), ["b", "a", "e"]);
    assert_eq!(view.placed, set(&["e"]));
}

#[test]
fn an_untouched_slot_removed_goes_and_its_instance_follows_its_rebind_policy() {
    let mut sidebar = session(&V1);
    let v2 = Layout {
        version: "2",
        slots: &[("a", "top", "replace"), ("c", "tail -f log", "ask")],
        hint: &[("left", &["a"]), ("right", &["c"])],
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    assert_eq!(keys(&sidebar), ["a", "c"]);
    // b departs: the host learns its policy, closes or keeps its live
    // instance, and releases it.
    let overlay = sidebar.overlay(worker()).unwrap();
    assert_eq!(
        overlay.departed,
        BTreeMap::from([("b".to_owned(), RebindPolicy::KeepPrevious)])
    );
    assert!(overlay.edits.slots.is_empty(), "nothing the user did");
    let record = sidebar
        .export_record(&format!("workspace/{WORKER}"))
        .unwrap();
    assert!(
        record.contains(r#"departed "b" rebind="keep-previous""#),
        "{record}"
    );
    assert!(sidebar.release_slot_previous(worker(), "b").unwrap());
    assert!(!sidebar.release_slot_previous(worker(), "b").unwrap());
    assert!(sidebar.overlay(worker()).unwrap().departed.is_empty());

    // A slot only the rebind policy was edited on is still untouched content.
    sidebar
        .set_slot(worker(), "c", shell("tail -f log"), RebindPolicy::Replace)
        .unwrap();
    assert_eq!(slots(&sidebar)["c"].flags, flag::REBIND);
    assert!(!slots(&sidebar)["c"].detached);
    let v3 = Layout {
        version: "3",
        slots: &[("a", "top", "replace")],
        hint: &[],
    };
    publish(&mut sidebar, 300, Some(&v2), &v3);
    let overlay = sidebar.overlay(worker()).unwrap();
    assert_eq!(
        overlay.departed,
        BTreeMap::from([("c".to_owned(), RebindPolicy::Replace)])
    );
    assert!(overlay.edits.slots.is_empty());
}

#[test]
fn an_overridden_slot_removed_is_flagged_and_kept_as_the_users() {
    let mut sidebar = session(&V1);
    assert!(sidebar
        .set_slot(
            worker(),
            "b",
            shell("make check"),
            RebindPolicy::KeepPrevious
        )
        .unwrap());
    assert_eq!(slots(&sidebar)["b"].flags, flag::DETACHED);
    let v2 = Layout {
        version: "2",
        slots: &[("a", "top", "replace"), ("c", "tail -f log", "ask")],
        hint: &[("left", &["a"]), ("right", &["c"])],
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    let kept = &slots(&sidebar)["b"];
    assert_eq!(kept.flags, flag::DETACHED | flag::REMOVED);
    assert!(!kept.in_baseline);
    assert_eq!(kept.spec, shell("make check"));
    // Its policy was the baseline's; it is frozen with the slot.
    assert_eq!(kept.rebind, RebindPolicy::KeepPrevious);
    assert!(sidebar.overlay(worker()).unwrap().departed.is_empty());
    // It keeps a tab: the unowned arrangement places it.
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.doc.tabs().contains(&"b"));
    assert!(view.gone.is_empty());
    // The user can still remove it.
    assert!(sidebar.remove_slot(worker(), "b").unwrap());
    assert_eq!(keys(&sidebar), ["a", "c"]);
}

#[test]
fn an_untouched_slot_whose_content_changes_follows_through_an_updating_rebind() {
    let mut sidebar = session(&V1);
    let plan = sidebar.plan_slot(worker(), "a", "").unwrap();
    let first = plan.update.unwrap();
    assert!(sidebar
        .managed
        .complete_slot(worker(), "a", first.token, true));
    let v2 = Layout {
        version: "2",
        slots: &[
            ("a", "top -d 1", "replace"),
            ("b", "make", "keep-previous"),
            ("c", "tail -f log", "ask"),
        ],
        hint: V1.hint,
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    assert_eq!(slots(&sidebar)["a"].spec, shell("top -d 1"));
    assert_eq!(slots(&sidebar)["a"].flags, 0);
    let plan = sidebar.plan_slot(worker(), "a", &first.id).unwrap();
    assert_eq!(plan.state, ContentState::Updating);
    assert_ne!(plan.update.unwrap().id, first.id);
}

#[test]
fn an_overridden_slot_whose_content_changes_keeps_the_override_flagged() {
    let mut sidebar = session(&V1);
    sidebar
        .set_slot(worker(), "c", shell("less log"), RebindPolicy::Ask)
        .unwrap();
    let v2 = Layout {
        version: "2",
        slots: &[
            ("a", "top", "replace"),
            ("b", "make", "keep-previous"),
            ("c", "tail -F log", "ask"),
        ],
        hint: V1.hint,
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    let c = &slots(&sidebar)["c"];
    assert_eq!(c.spec, shell("less log"), "the override wins");
    assert_eq!(c.flags, flag::DETACHED | flag::CHANGED);
    // Setting it again records it against the new content: settled.
    sidebar
        .set_slot(worker(), "c", shell("less log"), RebindPolicy::Ask)
        .unwrap();
    assert_eq!(slots(&sidebar)["c"].flags, flag::DETACHED);
    // When the provider adopts the user's content, the edit drops out.
    let v3 = Layout {
        version: "3",
        slots: &[
            ("a", "top", "replace"),
            ("b", "make", "keep-previous"),
            ("c", "less log", "ask"),
        ],
        hint: V1.hint,
    };
    publish(&mut sidebar, 300, Some(&v2), &v3);
    assert_eq!(slots(&sidebar)["c"].flags, 0);
    assert!(sidebar.overlay(worker()).unwrap().edits.slots.is_empty());
}

#[test]
fn a_reused_provider_key_is_flagged_not_hidden() {
    let mut sidebar = session(&V1);
    // The user closes c: a tombstone, and the unowned arrangement loses it.
    assert!(sidebar.remove_slot(worker(), "c").unwrap());
    assert_eq!(keys(&sidebar), ["a", "b"]);
    let overlay = sidebar.overlay(worker()).unwrap();
    assert_eq!(overlay.tombstoned, set(&["c"]));
    assert!(matches!(
        overlay.edits.slots["c"].content.as_ref().unwrap().change,
        ContentChange::Tombstone
    ));
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(!view.owned);
    assert_eq!(view.doc.tabs(), ["a", "b"], "the emptied panel goes");
    // A new version with c unchanged keeps it hidden.
    let v2 = Layout {
        version: "2",
        slots: V1.slots,
        hint: &[("left", &["a"]), ("right", &["b", "c"])],
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    assert_eq!(keys(&sidebar), ["a", "b"]);
    // The provider reuses c for something else: it shows, flagged.
    let v3 = Layout {
        version: "3",
        slots: &[
            ("a", "top", "replace"),
            ("b", "make", "keep-previous"),
            ("c", "journalctl -f", "ask"),
        ],
        hint: v2.hint,
    };
    publish(&mut sidebar, 300, Some(&v2), &v3);
    let c = &slots(&sidebar)["c"];
    assert_eq!(c.flags, flag::CHANGED);
    assert_eq!(c.spec, shell("journalctl -f"));
    assert!(sidebar.overlay(worker()).unwrap().tombstoned.is_empty());
    assert_eq!(
        panel_tabs(&sidebar.arrangement(worker()).unwrap().doc, "right"),
        ["b", "c"]
    );
    // Removing it again hides it against the new content.
    assert!(sidebar.remove_slot(worker(), "c").unwrap());
    assert_eq!(keys(&sidebar), ["a", "b"]);
    // Reattaching shows it again, unflagged.
    assert!(sidebar.reattach_slot(worker(), "c").unwrap());
    assert_eq!(slots(&sidebar)["c"].flags, 0);

    // An override of a key the provider dropped, then reused, wins and is
    // flagged.
    sidebar
        .set_slot(
            worker(),
            "b",
            shell("make check"),
            RebindPolicy::KeepPrevious,
        )
        .unwrap();
    let v4 = Layout {
        version: "4",
        slots: &[("a", "top", "replace"), ("c", "journalctl -f", "ask")],
        hint: &[],
    };
    publish(&mut sidebar, 400, Some(&v3), &v4);
    assert_eq!(slots(&sidebar)["b"].flags, flag::DETACHED | flag::REMOVED);
    let v5 = Layout {
        version: "5",
        slots: &[
            ("a", "top", "replace"),
            ("b", "cargo build", "replace"),
            ("c", "journalctl -f", "ask"),
        ],
        hint: &[],
    };
    publish(&mut sidebar, 500, Some(&v4), &v5);
    // Its frozen policy differs from the reused key's, so that is an edit
    // too.
    let b = &slots(&sidebar)["b"];
    assert_eq!(b.flags, flag::DETACHED | flag::CHANGED | flag::REBIND);
    assert_eq!(b.rebind, RebindPolicy::KeepPrevious);
    assert_eq!(b.spec, shell("make check"));
}

#[test]
fn a_divider_resize_does_not_stop_provider_structure_flowing() {
    let mut sidebar = session(&V1);
    let view = sidebar.arrangement(worker()).unwrap();
    // Resize the divider and select another tab: soft overrides only.
    let mut doc = view.doc.clone();
    panel_mut(&mut doc, "left").weight = 3.0;
    if let PanelNode::Tabs { selected, .. } = &mut panel_mut(&mut doc, "left").node {
        *selected = Some("b".into());
    }
    let commit = sidebar
        .set_arrangement(worker(), doc.clone(), view.generation)
        .unwrap();
    assert!(!commit.owned);
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(!view.owned);
    assert_eq!(view.doc, doc);
    assert_eq!(view.soft["left"].weight, Some(3.0));
    assert_eq!(view.soft["left"].selected.as_deref(), Some("b"));
    let edits = sidebar.overlay(worker()).unwrap().edits;
    assert_eq!(edits.arrangement, None);
    assert_eq!(edits.panels.len(), 1);

    // The provider restructures: a new panel splits c off, and d arrives.
    let v2 = Layout {
        version: "2",
        slots: &[
            ("a", "top", "replace"),
            ("b", "make", "keep-previous"),
            ("c", "tail -f log", "ask"),
            ("d", "htop", "replace"),
        ],
        hint: &[("left", &["a", "b"]), ("mid", &["d"]), ("right", &["c"])],
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(!view.owned);
    assert_eq!(view.doc.tabs(), ["a", "b", "d", "c"], "the new structure");
    assert_eq!(weight(&view.doc, "left"), 3.0, "the resize is reapplied");
    let PanelNode::Tabs { selected, .. } = &view
        .doc
        .panels()
        .into_iter()
        .find(|p| p.id == "left")
        .unwrap()
        .node
    else {
        panic!()
    };
    assert_eq!(selected.as_deref(), Some("b"));
    assert!(view.unresolved.is_empty());

    // A panel the provider drops keeps its override, flagged.
    let v3 = Layout {
        version: "3",
        slots: v2.slots,
        hint: &[("all", &["a", "b", "c", "d"])],
    };
    publish(&mut sidebar, 300, Some(&v2), &v3);
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.unresolved, set(&["left"]));
    assert_eq!(view.soft["left"].weight, Some(3.0));
}

#[test]
fn a_structural_edit_takes_ownership_and_a_provider_change_is_flagged() {
    let mut sidebar = session(&V1);
    let view = sidebar.arrangement(worker()).unwrap();
    // Soft first, then a move: c joins the left panel.
    let mut doc = view.doc.clone();
    panel_mut(&mut doc, "right").weight = 2.0;
    sidebar
        .set_arrangement(worker(), doc, view.generation)
        .unwrap();
    let moved = ArrangementDoc {
        root: Some(Panel {
            id: "main".into(),
            weight: 1.0,
            node: PanelNode::Split {
                axis: Axis::Row,
                children: vec![Panel {
                    id: "left".into(),
                    weight: 1.0,
                    node: PanelNode::Tabs {
                        tabs: vec!["a".into(), "c".into(), "b".into()],
                        selected: Some("c".into()),
                    },
                }],
            },
        }),
    };
    let generation = sidebar.arrangement(worker()).unwrap().generation;
    let commit = sidebar
        .set_arrangement(worker(), moved.clone(), generation)
        .unwrap();
    assert!(commit.owned);
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.owned && view.soft.is_empty() && !view.provider_changed);
    assert_eq!(
        sidebar.overlay(worker()).unwrap().edits.arrangement,
        Some(moved.clone())
    );

    // The provider changes its hint: flagged, neither applied nor ignored.
    let content = sidebar.content_revision();
    let v2 = Layout {
        version: "2",
        slots: V1.slots,
        hint: &[("top", &["a"]), ("bottom", &["b", "c"])],
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    let view = sidebar.arrangement(worker()).unwrap();
    assert_eq!(view.doc, moved);
    assert!(view.provider_changed);
    assert!(sidebar.content_revision() > content);
    assert!(sidebar
        .export_record(&format!("workspace/{WORKER}"))
        .unwrap()
        .contains("provider-changed=true"));
    // Keeping the user's arrangement settles it.
    sidebar
        .resolve_arrangement(worker(), Resolve::Keep, view.generation)
        .unwrap();
    assert!(!sidebar.arrangement(worker()).unwrap().provider_changed);
    // A further change flags it again; following the provider drops the
    // user's arrangement.
    let v3 = Layout {
        version: "3",
        slots: V1.slots,
        hint: &[("top", &["a", "b"]), ("bottom", &["c"])],
    };
    publish(&mut sidebar, 300, Some(&v2), &v3);
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(view.provider_changed);
    assert!(sidebar
        .resolve_arrangement(worker(), Resolve::Follow, view.generation - 1)
        .is_err());
    let generation = sidebar
        .resolve_arrangement(worker(), Resolve::Follow, view.generation)
        .unwrap();
    let view = sidebar.arrangement(worker()).unwrap();
    assert!(generation > 0 && view.generation == generation);
    assert!(!view.owned && !view.provider_changed);
    assert_eq!(panel_tabs(&view.doc, "top"), ["a", "b"]);
    assert_eq!(sidebar.overlay(worker()).unwrap().edits.arrangement, None);
}

#[test]
fn the_edit_set_exports_as_an_overlay_sync_proposal() {
    let mut sidebar = session(&V1);
    sidebar
        .set_slot(worker(), "a", shell("btop"), RebindPolicy::Replace)
        .unwrap();
    sidebar.remove_slot(worker(), "c").unwrap();
    sidebar
        .set_slot(worker(), "b", shell("make"), RebindPolicy::Ask)
        .unwrap();
    sidebar
        .set_slot(worker(), "u:1", shell("lazygit"), RebindPolicy::Replace)
        .unwrap();
    sidebar
        .set_workspace_name(worker(), Some("Build".into()))
        .unwrap();
    sidebar
        .set_workspace_mood(worker(), Some("calm".into()))
        .unwrap();
    let view = sidebar.arrangement(worker()).unwrap();
    let mut doc = view.doc.clone();
    panel_mut(&mut doc, "left").weight = 0.4;
    sidebar
        .set_arrangement(worker(), doc, view.generation)
        .unwrap();

    let text = sidebar.export_overlay(worker()).unwrap();
    assert!(
        text.starts_with(&format!(
            r#"overlay-proposal workspace="{WORKER}" baseline="1" {{"#
        )),
        "{text}"
    );
    for expected in [
        r#"subject "vessel" "v" provider="local""#,
        r#"name "Build""#,
        r#"mood "calm""#,
        r#"edit "a" {"#,
        r#"edit "b" rebind="ask""#,
        r#"edit "c" {"#,
        "tombstone",
        r#"slot "u:1" {"#,
        r#"panel "left" weight=0.4"#,
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    let (workspace, edits) = decode_proposal(&text).unwrap();
    assert_eq!(workspace, worker());
    assert_eq!(edits, sidebar.overlay(worker()).unwrap().edits);
    assert_eq!(edits.baseline, Some(BaselineVersion::Published("1".into())));
    // Exporting changes nothing.
    assert_eq!(sidebar.export_overlay(worker()).unwrap(), text);

    // The provider accepts a's override and b's policy: those edits drop
    // out of the next proposal, which applies to version 2.
    let v2 = Layout {
        version: "2",
        slots: &[
            ("a", "btop", "replace"),
            ("b", "make", "ask"),
            ("c", "tail -f log", "ask"),
        ],
        hint: V1.hint,
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    let (_, edits) = decode_proposal(&sidebar.export_overlay(worker()).unwrap()).unwrap();
    assert_eq!(edits.baseline, Some(BaselineVersion::Published("2".into())));
    assert_eq!(edits.slots.keys().collect::<Vec<_>>(), ["c"]);

    // The overlay survives a restart through the record.
    let name = format!("workspace/{WORKER}");
    let record = sidebar.export_record(&name).unwrap();
    let mut fresh = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    fresh.import_record(&name, &record).unwrap();
    assert_eq!(fresh.overlay(worker()), sidebar.overlay(worker()));
    assert_eq!(fresh.arrangement(worker()), sidebar.arrangement(worker()));
    assert_eq!(fresh.export_record(&name).unwrap(), record);
}

#[test]
fn a_workspace_with_no_suggested_layout_has_an_empty_baseline() {
    let mut sidebar = session(&Layout {
        version: "1",
        slots: &[],
        hint: &[],
    });
    // The empty layout is a baseline with no slots; edits are the user's.
    sidebar
        .set_slot(worker(), "u:1", shell("bash"), RebindPolicy::Replace)
        .unwrap();
    let edits = sidebar.overlay(worker()).unwrap().edits;
    assert_eq!(edits.added.len(), 1);
    assert!(sidebar
        .remove_slot(worker(), "nope")
        .is_ok_and(|removed| !removed));
}

/// A template's region and loop names, display variables and sections:
/// the fixture, with a project loop named "board".
fn template_a() -> String {
    include_str!("fixtures/sidebar.kdl").replace(
        r#"for "project" kind="project""#,
        r#"for "board" kind="project""#,
    )
}

#[test]
fn dashboard_keys_that_no_longer_resolve_are_flagged_and_kept() {
    let template_a = template_a();
    let mut sidebar = Sidebar::new(&template_a).unwrap();
    let version_a = sidebar.dashboard_overlay().template;
    let record = format!(
        r#"andamento-record "dashboard" version=5 {{
    template version="{version_a}"
    display "show-issues" false
    collapsed {{
        at "board" "project" "p" provider="local"
    }}
    order "tree" "board" {{
        entity "project" "q" provider="local"
        entity "project" "p" provider="local"
    }}
    order "attention" "attention" {{
        entity "vessel" "v" provider="local"
    }}
    variable "density" "compact" {{
        at "board" "project" "p" provider="local"
    }}
}}"#
    );
    sidebar.import_record("dashboard", &record).unwrap();
    let overlay = sidebar.dashboard_overlay();
    assert_eq!(overlay.recorded.as_deref(), Some(version_a.as_str()));
    assert!(overlay.unresolved.is_empty(), "{:?}", overlay.unresolved);

    // Template B renames the project loop, drops the attention region and
    // the show-issues variable.
    let template_b = template_a
        .replace(r#"for "board" kind="project""#, r#"for "proj" kind="project""#)
        .replace(
            r#"region "attention" root-template="attention/title" form="compact" placement="attention""#,
            "",
        )
        .replace(
            r#"display-variable "show-issues" type="bool" default=true label="Issues" icon="I""#,
            "",
        )
        .replace(
            r#"  control "display-variable" variable="show-issues""#,
            r#"  field "label" source="literal" value="Controls""#,
        );
    sidebar.configure(&template_b).unwrap();
    let overlay = sidebar.dashboard_overlay();
    assert_ne!(overlay.template, version_a);
    let kinds: Vec<(KeyKind, &str)> = overlay
        .unresolved
        .iter()
        .map(|(kind, key)| (*kind, key.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (KeyKind::Display, "show-issues"),
            (KeyKind::Collapse, "board=local:project:p"),
            (KeyKind::Order, "attention/attention"),
            (KeyKind::Order, "tree/board"),
            (KeyKind::Variable, "board=local:project:p"),
            // The sidebar arrangement's own unresolved key.
            (KeyKind::Section, "attention"),
        ]
    );
    // Nothing is dropped: the keys are still in the record, which now
    // names template B.
    let exported = sidebar.export_record("dashboard").unwrap();
    for kept in [
        r#"display "show-issues" false"#,
        r#"order "attention" "attention""#,
        r#"variable "density" "compact""#,
        r#"at "board" "project" "p" provider="local""#,
    ] {
        assert!(exported.contains(kept), "{kept}\n{exported}");
    }
    assert!(exported.contains(&format!(r#"template version="{}""#, overlay.template)));
    // Back on template A, they resolve again.
    sidebar.configure(&template_a).unwrap();
    assert!(sidebar.dashboard_overlay().unresolved.is_empty());
}

#[test]
fn a_pinned_view_that_goes_away_is_flagged_and_kept() {
    let mut sidebar = session(&V1);
    let pin = entity(".ref", "pin");
    let facts = |view: &str| {
        BTreeMap::from([(
            andamento_core::dashboard_overlay::VIEW.to_owned(),
            text(view),
        )])
    };
    // A malformed pin, or a pin on something other than a ref, is refused.
    assert!(sidebar.set_local(pin.clone(), facts("nope")).is_err());
    assert!(sidebar
        .set_local(entity(".group", "g"), facts(&format!("{WORKER}/a")))
        .is_err());
    sidebar
        .set_local(pin.clone(), facts(&format!("{WORKER}/c")))
        .unwrap();
    assert!(sidebar.dashboard_overlay().unresolved.is_empty());
    // The slot is removed: the pin stays, flagged.
    sidebar.remove_slot(worker(), "c").unwrap();
    assert_eq!(
        sidebar.dashboard_overlay().unresolved,
        [(KeyKind::Pin, "pin".to_owned())]
    );
    assert!(sidebar.local_entities().contains_key(&pin));
    // It resolves again when the slot comes back.
    sidebar.reattach_slot(worker(), "c").unwrap();
    assert!(sidebar.dashboard_overlay().unresolved.is_empty());
    // The workspace is forgotten: flagged again; the host offers removal.
    sidebar.forget_workspace(worker());
    assert_eq!(
        sidebar.dashboard_overlay().unresolved,
        [(KeyKind::Pin, "pin".to_owned())]
    );
    assert!(sidebar.remove_local(&pin));
    assert!(sidebar.dashboard_overlay().unresolved.is_empty());
}
