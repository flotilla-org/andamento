use super::*;
use crate::suggested_layout::{CommandLine, LocalRecipe};
use crate::EntityRef;

fn shell(command: &str) -> ViewSpec {
    ViewSpec {
        content: Content::Local(LocalRecipe::Command {
            line: CommandLine::Shell(command.into()),
            cwd: None,
        }),
        presentation: None,
    }
}

fn facet(id: &str) -> ViewSpec {
    ViewSpec {
        content: Content::ProviderFacet {
            entity: EntityRef::new("sub-1", "vessel", id),
            facet: "terminal".into(),
        },
        presentation: Some("terminal".into()),
    }
}

fn def(key: &str, spec: ViewSpec) -> SlotDef {
    SlotDef {
        key: key.into(),
        spec,
        rebind: RebindPolicy::Replace,
    }
}

fn baseline(keys: &[&str]) -> Baseline {
    Baseline {
        version: BaselineVersion::Published(keys.join(",")),
        slots: keys.iter().map(|key| def(key, facet(key))).collect(),
        hint: None,
    }
}

fn tabs(id: &str, keys: &[&str], selected: Option<&str>) -> Panel {
    Panel {
        id: id.into(),
        weight: 1.0,
        node: PanelNode::Tabs {
            tabs: keys.iter().map(|k| k.to_string()).collect(),
            selected: selected.map(str::to_owned),
        },
    }
}

fn split(id: &str, children: Vec<Panel>) -> Panel {
    Panel {
        id: id.into(),
        weight: 1.0,
        node: PanelNode::Split {
            axis: Axis::Row,
            children,
        },
    }
}

fn doc(root: Panel) -> ArrangementDoc {
    ArrangementDoc { root: Some(root) }
}

fn counter() -> impl FnMut() -> u64 {
    let mut n = 0;
    move || {
        n += 1;
        n
    }
}

#[test]
fn user_slots_overrides_and_reattachment() {
    let mut slots = WorkspaceSlots {
        baseline: Some(baseline(&["a", "b"])),
        ..Default::default()
    };
    // Provider keys can't be invented; user keys live in their namespace.
    assert!(slots.set("c", shell("x"), RebindPolicy::Replace).is_err());
    assert!(slots.set("u:", shell("x"), RebindPolicy::Replace).is_err());
    assert!(slots.set("u:A", shell("x"), RebindPolicy::Replace).is_err());
    assert!(slots.set("u:1", shell(""), RebindPolicy::Replace).is_err());
    assert!(slots.set("u:1", shell("x"), RebindPolicy::Ask).unwrap());
    assert!(!slots.set("u:1", shell("x"), RebindPolicy::Ask).unwrap());
    // Overriding a baseline slot detaches it.
    assert!(slots.set("b", shell("y"), RebindPolicy::Replace).unwrap());
    let all = slots.slots();
    let keys: Vec<_> = all.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["a", "b", "u:1"]);
    assert!(all[1].detached && all[1].in_baseline);
    assert_eq!(all[1].spec, shell("y"));
    assert_eq!(
        slots.edits["b"].content.as_ref().unwrap().against,
        facet("b")
    );
    assert!(!all[2].detached && !all[2].in_baseline);
    // Setting the baseline's own spec reattaches it.
    assert!(slots.set("b", facet("b"), RebindPolicy::Replace).unwrap());
    assert!(!slots.slot("b").unwrap().detached);
    assert!(slots.set("b", shell("y"), RebindPolicy::Replace).unwrap());
    assert!(slots.reattach("b"));
    assert!(!slots.reattach("b"));
    // Removing a baseline slot tombstones it; reattaching shows it again.
    assert!(slots.remove("a").unwrap());
    assert!(!slots.remove("a").unwrap());
    assert!(!slots.keys().contains("a"));
    assert_eq!(slots.hidden(), BTreeSet::from(["a".to_owned()]));
    assert!(slots.reattach("a"));
    assert!(slots.keys().contains("a"));
    assert!(slots.remove("u:1").unwrap());
    assert!(!slots.remove("u:1").unwrap());
}

#[test]
fn a_detached_slot_the_baseline_drops_is_kept_as_the_users() {
    let mut slots = WorkspaceSlots {
        baseline: Some(baseline(&["a", "b"])),
        ..Default::default()
    };
    slots.set("b", shell("y"), RebindPolicy::Replace).unwrap();
    assert!(slots.set_baseline(baseline(&["a"])));
    let kept = slots.slot("b").unwrap();
    assert!(kept.detached && !kept.in_baseline);
    // It can still be edited, can't be reattached, and can be removed.
    assert!(slots.set("b", shell("z"), RebindPolicy::Replace).unwrap());
    assert!(!slots.reattach("b"));
    assert!(slots.remove("b").unwrap());
    assert_eq!(slots.keys(), BTreeSet::from(["a".to_owned()]));
}

#[test]
fn documents_are_checked_whole() {
    let ok = doc(split(
        "1",
        vec![tabs("2", &["a"], Some("a")), tabs("3", &[], None)],
    ));
    assert_eq!(ok.check(), Ok(()));
    let mut bad = vec![
        doc(split("1", vec![tabs("1", &["a"], None)])),
        doc(split("1", vec![])),
        doc(split(
            "1",
            vec![tabs("2", &["a"], None), tabs("3", &["a"], None)],
        )),
        doc(tabs("1", &["a"], Some("b"))),
        doc(tabs("", &["a"], None)),
        doc(tabs("1", &[""], None)),
    ];
    for weight in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut panel = tabs("1", &["a"], None);
        panel.weight = weight;
        bad.push(doc(panel));
    }
    for doc in bad {
        assert!(doc.check().is_err(), "{doc:?}");
    }
}

#[test]
fn stale_or_invalid_commits_change_nothing() {
    let slots = WorkspaceSlots {
        baseline: Some(baseline(&["a", "b"])),
        ..Default::default()
    };
    let infos = slots.slots();
    let mut next = counter();
    let mut stored = StoredArrangement::default();
    let first = doc(tabs("1", &["a", "b"], Some("a")));
    let commit = stored
        .commit(first.clone(), 0, slots.baseline.as_ref(), &infos, &mut next)
        .unwrap();
    assert_eq!(commit.generation, 1);
    assert!(commit.placed.is_empty() && commit.gone.is_empty());
    let before = stored.clone();
    // A commit made against generation 0 is now stale.
    let other = doc(tabs("1", &["b", "a"], Some("b")));
    assert_eq!(
        stored.commit(other.clone(), 0, slots.baseline.as_ref(), &infos, &mut next),
        Err(ArrangementError::Stale { current: 1 })
    );
    assert_eq!(stored, before);
    // A tab for a slot that never existed, or a malformed document.
    for invalid in [
        doc(tabs("1", &["a", "b", "nope"], None)),
        doc(tabs("1", &["a", "a"], None)),
    ] {
        assert!(matches!(
            stored.commit(invalid, 1, slots.baseline.as_ref(), &infos, &mut next),
            Err(ArrangementError::Invalid(_))
        ));
        assert_eq!(stored, before);
    }
    // Committing the same document again keeps its generation.
    assert_eq!(
        stored
            .commit(first, 1, slots.baseline.as_ref(), &infos, &mut next)
            .unwrap()
            .generation,
        1
    );
    assert_eq!(
        stored
            .commit(other, 1, slots.baseline.as_ref(), &infos, &mut next)
            .unwrap()
            .generation,
        2
    );
}

#[test]
fn reconciling_places_new_slots_and_reports_gone_ones() {
    let mut slots = WorkspaceSlots {
        baseline: Some(baseline(&["a", "b"])),
        ..Default::default()
    };
    let mut next = counter();
    let mut stored = StoredArrangement::default();
    // The host left b out: it is placed in the first tab panel.
    let host = doc(split(
        "1",
        vec![
            split("2", vec![tabs("3", &[], None)]),
            tabs("4", &["a"], Some("a")),
        ],
    ));
    let commit = stored
        .commit(host, 0, slots.baseline.as_ref(), &slots.slots(), &mut next)
        .unwrap();
    assert_eq!(commit.placed, ["b"]);
    assert_eq!(stored.doc.tabs(), ["b", "a"]);
    assert_eq!(stored.placed, BTreeSet::from(["b".to_owned()]));
    let PanelNode::Tabs { selected, .. } = &stored.doc.panels()[2].node else {
        panic!("expected tabs");
    };
    assert_eq!(selected.as_deref(), Some("b"));

    // The baseline adds c and drops a: c is placed, a's tab stays and is
    // reported gone.
    slots.set_baseline(baseline(&["b", "c"]));
    let infos = slots.slots();
    assert!(stored.follow(slots.baseline.as_ref(), &infos, &mut next));
    assert_eq!(stored.generation, 2);
    assert_eq!(stored.doc.tabs(), ["b", "c", "a"]);
    assert_eq!(stored.gone(&infos), BTreeSet::from(["a".to_owned()]));
    let view = stored.view(&infos);
    assert_eq!(
        view.placed,
        BTreeSet::from(["b".to_owned(), "c".to_owned()])
    );
    // Following an unchanged slot set changes nothing.
    assert!(!stored.follow(slots.baseline.as_ref(), &infos, &mut next));
    // The host may keep the gone tab in its commit; it may not add one.
    let host = doc(tabs("1", &["c", "b", "a"], Some("c")));
    let commit = stored
        .commit(host, 2, slots.baseline.as_ref(), &infos, &mut next)
        .unwrap();
    assert_eq!(commit.gone, ["a"]);
    assert!(commit.placed.is_empty());
    assert!(stored.placed.is_empty(), "the host has seen every tab");
}

#[test]
fn an_uncommitted_arrangement_follows_the_hint() {
    let mut slots = WorkspaceSlots::default();
    let mut hinted = baseline(&["a", "b"]);
    hinted.hint = Some(doc(tabs("main", &["a"], Some("a"))));
    slots.set_baseline(hinted);
    let mut next = counter();
    let mut stored = StoredArrangement::default();
    assert!(stored.follow(slots.baseline.as_ref(), &slots.slots(), &mut next));
    assert!(!stored.owned);
    assert_eq!(stored.doc.tabs(), ["a", "b"]);
    // A new hint replaces it while the host hasn't committed.
    let mut hinted = baseline(&["a", "b"]);
    hinted.hint = Some(doc(split(
        "main",
        vec![tabs("x", &["b"], None), tabs("y", &["a"], None)],
    )));
    slots.set_baseline(hinted);
    assert!(stored.follow(slots.baseline.as_ref(), &slots.slots(), &mut next));
    assert_eq!(stored.doc.tabs(), ["b", "a"]);
    assert!(stored.placed.is_empty());
    // Once committed, the host owns it: a new hint is not applied.
    let generation = stored.generation;
    stored
        .commit(
            doc(tabs("1", &["a", "b"], None)),
            generation,
            slots.baseline.as_ref(),
            &slots.slots(),
            &mut next,
        )
        .unwrap();
    assert!(stored.owned);
    let mut hinted = baseline(&["a", "b"]);
    hinted.hint = Some(doc(tabs("main", &["b", "a"], None)));
    slots.set_baseline(hinted);
    assert!(!stored.follow(slots.baseline.as_ref(), &slots.slots(), &mut next));
    assert_eq!(stored.doc.tabs(), ["a", "b"]);
}

#[test]
fn hints_convert_from_suggested_layouts() {
    let hint = suggested_layout::Arrangement {
        root: "main".into(),
        panels: BTreeMap::from([
            (
                "main".into(),
                suggested_layout::Panel {
                    weight: 1,
                    node: suggested_layout::PanelNode::Split {
                        axis: Axis::Column,
                        children: vec!["x".into()],
                    },
                },
            ),
            (
                "x".into(),
                suggested_layout::Panel {
                    weight: 3,
                    node: suggested_layout::PanelNode::Tabs {
                        slots: vec!["a".into(), "b".into()],
                        selected: "b".into(),
                    },
                },
            ),
        ]),
    };
    let mut expected = tabs("x", &["a", "b"], Some("b"));
    expected.weight = 3.0;
    assert_eq!(
        ArrangementDoc::from_hint(&hint),
        doc(Panel {
            id: "main".into(),
            weight: 1.0,
            node: PanelNode::Split {
                axis: Axis::Column,
                children: vec![expected],
            },
        })
    );
}
