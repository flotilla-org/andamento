//! The Workspace Overlay as an addressed edit set (#145, Wheelhouse ADR
//! 0013). Each test replays a provider's Suggested Layout through its
//! versions against a workspace the user has edited, and checks that the
//! edits survive and that nothing is dropped silently.
mod common;
use andamento_core::managed::ContentState;
use andamento_core::sidebar::{Action, HostEffect, Workspace};
use andamento_core::slots::{flag, ArrangementDoc, Panel, PanelNode, Resolve, SlotInfo};
use andamento_core::suggested_layout::{
    Axis, CommandLine, Content, LocalRecipe, RebindPolicy, ViewSpec,
};
use andamento_core::versioned::CommitError;
use andamento_core::{
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, Sidebar,
    WorkspaceId,
};
use common::PROVIDER;
use std::collections::{BTreeMap, BTreeSet};

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::new(PROVIDER, kind, id)
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
    sidebar.apply(now, PROVIDER, [patch]).unwrap();
}

/// A sidebar with a workspace open for vessel v at `first`.
fn session(first: &Layout) -> Sidebar {
    let mut sidebar = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    sidebar
        .apply(
            100,
            PROVIDER,
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
        )
        .unwrap();
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
    assert!(
        slots(&sidebar).values().all(|slot| slot.flags == 0),
        "nothing the user did"
    );
    let name = format!("workspace/{WORKER}");
    let mut fresh = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    fresh
        .import_record(&name, &sidebar.export_record(&name).unwrap())
        .unwrap();
    assert_eq!(fresh.overlay(worker()).unwrap().departed, overlay.departed);
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
    assert!(
        slots(&sidebar).values().all(|slot| slot.flags == 0),
        "nothing the user did"
    );
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
    assert!(
        slots(&sidebar).values().all(|slot| slot.flags == 0),
        "nothing the user did"
    );
}

#[test]
fn a_reused_provider_key_is_flagged_not_hidden() {
    let mut sidebar = session(&V1);
    // The user closes c: a tombstone, and the unowned arrangement loses it.
    assert!(sidebar.remove_slot(worker(), "c").unwrap());
    assert_eq!(keys(&sidebar), ["a", "b"]);
    let overlay = sidebar.overlay(worker()).unwrap();
    assert_eq!(overlay.tombstoned, set(&["c"]));
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
    assert_eq!(view.soft.len(), 1);

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
    assert!(sidebar.overlay(worker()).unwrap().owned);

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
    let name = format!("workspace/{WORKER}");
    let mut fresh = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    fresh
        .import_record(&name, &sidebar.export_record(&name).unwrap())
        .unwrap();
    assert!(fresh.arrangement(worker()).unwrap().provider_changed);
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
    assert!(!sidebar.overlay(worker()).unwrap().owned);
}

/// The user owns the arrangement: `moved` puts every tab in one panel.
fn owned_session() -> (Sidebar, ArrangementDoc) {
    let mut sidebar = session(&V1);
    let view = sidebar.arrangement(worker()).unwrap();
    let moved = ArrangementDoc {
        root: Some(Panel {
            id: "main".into(),
            weight: 1.0,
            node: PanelNode::Tabs {
                tabs: vec!["a".into(), "c".into(), "b".into()],
                selected: Some("c".into()),
            },
        }),
    };
    assert!(
        sidebar
            .set_arrangement(worker(), moved.clone(), view.generation)
            .unwrap()
            .owned
    );
    (sidebar, moved)
}

#[test]
fn a_provider_change_moves_the_generation_so_a_commit_cannot_erase_it() {
    let (mut sidebar, moved) = owned_session();
    // The host read the document before the provider changed its hint.
    let read = sidebar.arrangement(worker()).unwrap().generation;
    let v2 = Layout {
        version: "2",
        slots: V1.slots,
        hint: &[("top", &["a"]), ("bottom", &["b", "c"])],
    };
    publish(&mut sidebar, 200, Some(&V1), &v2);
    let flagged = sidebar.arrangement(worker()).unwrap();
    assert!(flagged.provider_changed);
    assert!(flagged.generation > read, "raising the flag is a change");
    // A commit made against what the host read is stale, and the flag stays.
    assert_eq!(
        sidebar.set_arrangement(worker(), moved.clone(), read),
        Err(CommitError::Stale {
            current: flagged.generation
        })
    );
    assert!(sidebar.arrangement(worker()).unwrap().provider_changed);
    // Keeping the user's arrangement clears it, which is a change too.
    let kept = sidebar
        .resolve_arrangement(worker(), Resolve::Keep, flagged.generation)
        .unwrap();
    assert!(kept > flagged.generation);
    assert!(sidebar
        .set_arrangement(worker(), moved, flagged.generation)
        .is_err());
}

#[test]
fn an_imported_record_never_reuses_a_generation() {
    // Two sidebars whose arrangements reached the same generation with
    // different documents.
    let (mut sidebar, _) = owned_session();
    let mut other = session(&V1);
    let mut doc = other.arrangement(worker()).unwrap().doc;
    panel_mut(&mut doc, "right").weight = 3.0;
    let generation = other.arrangement(worker()).unwrap().generation;
    other.set_arrangement(worker(), doc, generation).unwrap();
    let live = sidebar.arrangement(worker()).unwrap();
    let theirs = other.arrangement(worker()).unwrap();
    assert_ne!(live.doc, theirs.doc);
    let read = live.generation;
    // This sidebar imports the other's record. A commit the host prepared
    // against the document it read is stale, not applied over the import.
    let name = format!("workspace/{WORKER}");
    sidebar
        .import_record(&name, &other.export_record(&name).unwrap())
        .unwrap();
    let imported = sidebar.arrangement(worker()).unwrap();
    assert_eq!(imported.doc, theirs.doc);
    assert!(imported.generation > read);
    assert!(matches!(
        sidebar.set_arrangement(worker(), live.doc, read),
        Err(CommitError::Stale { .. })
    ));
}

#[test]
fn the_overlay_survives_a_restart_through_the_record() {
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

    // The overlay survives a restart through the record.
    let name = format!("workspace/{WORKER}");
    let record = sidebar.export_record(&name).unwrap();
    let mut fresh = Sidebar::new(include_str!("fixtures/sidebar.kdl")).unwrap();
    fresh.import_record(&name, &record).unwrap();
    assert_eq!(fresh.overlay(worker()), sidebar.overlay(worker()));
    // The same arrangement, at a generation of the fresh sidebar's own.
    let (mut imported, live) = (
        fresh.arrangement(worker()).unwrap(),
        sidebar.arrangement(worker()).unwrap(),
    );
    assert_ne!(imported.generation, 0);
    imported.generation = live.generation;
    assert_eq!(imported, live);
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
    assert_eq!(keys(&sidebar), ["u:1"]);
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
    use andamento_core::records::{DashboardRecord, DashboardState};
    use andamento_core::{DisplayVariableValue, PlacementKey, PlacementLoopKey, PlacementSegment};
    let template_a = template_a();
    let mut sidebar = Sidebar::new(&template_a).unwrap();
    let board = PlacementKey(vec![PlacementSegment {
        loop_name: "board".into(),
        entity: entity("project", "p"),
    }]);
    let order = |region: &str, binding: &str| PlacementLoopKey {
        region: region.into(),
        parent: PlacementKey::default(),
        binding: binding.into(),
    };
    let record = DashboardState {
        display: BTreeMap::from([("show-issues".into(), DisplayVariableValue::Bool(false))]),
        collapsed: BTreeSet::from([board.clone()]),
        orders: BTreeMap::from([
            (order("tree", "board"), vec![entity("project", "q")]),
            (order("attention", "attention"), vec![subject()]),
        ]),
        variables: BTreeMap::from([(
            board,
            BTreeMap::from([("density".into(), "compact".into())]),
        )]),
        ..DashboardState::default()
    };
    let encoded = DashboardRecord {
        state: record.clone(),
        sidebar: None,
    }
    .encode();
    sidebar.import_record("dashboard", &encoded).unwrap();
    let unresolved = |sidebar: &Sidebar| sidebar.sidebar_arrangement().0.unresolved;
    assert!(
        unresolved(&sidebar).is_empty(),
        "{:?}",
        unresolved(&sidebar)
    );

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
    assert_eq!(
        unresolved(&sidebar),
        set(&[
            // The sidebar arrangement's own unresolved section.
            "attention",
            "display:show-issues",
            "collapse:board=project:p@test",
            &format!("order:{}", order("attention", "attention").encode()),
            &format!("order:{}", order("tree", "board").encode()),
            "variable:board=project:p@test",
        ])
    );
    // Nothing is dropped: the keys are still in the record.
    let exported = DashboardRecord::decode(&sidebar.export_record("dashboard").unwrap()).unwrap();
    assert_eq!(
        (
            exported.state.display,
            exported.state.collapsed,
            exported.state.orders,
            exported.state.variables
        ),
        (
            record.display,
            record.collapsed,
            record.orders,
            record.variables
        )
    );
    // Back on template A, they resolve again.
    sidebar.configure(&template_a).unwrap();
    assert!(unresolved(&sidebar).is_empty());
}
