//! Wheelhouse's section-placement scenarios (#191, #207, #221;
//! `uishell_section_placement_diagnostics`), ported to the document model:
//! hints, reorder, restart, add, close, remove, reset, duplicates, closed
//! restore, `.unplaced`, legacy adoption and host hints.
use super::*;

const UNPLACED: &str = ".unplaced";

fn region(name: &str, host: Option<&str>, order: Option<i64>) -> RegionHints {
    RegionHints {
        name: name.into(),
        default_host: host.map(str::to_owned),
        order,
        pinned: false,
        hosts_local_sections: false,
    }
}

fn sidebar(name: &str, order: i64) -> RegionHints {
    region(name, Some("sidebar"), Some(order))
}

fn declared(regions: &[RegionHints]) -> Declared {
    Declared::new(regions, &[])
}

/// `b` at 20 declared before `a` at 10, as Wheelhouse's diagnostics do.
fn ab() -> Declared {
    declared(&[sidebar("b", 20), sidebar("a", 10)])
}

fn counter() -> impl FnMut() -> u64 {
    let mut n = 0;
    move || {
        n += 1;
        n
    }
}

fn tabs(id: &str, weight: f64, keys: &[&str]) -> Panel {
    Panel {
        id: id.into(),
        weight,
        node: PanelNode::Tabs {
            tabs: keys.iter().map(|k| k.to_string()).collect(),
            selected: keys.first().map(|k| k.to_string()),
        },
    }
}

fn split(id: &str, weight: f64, axis: Axis, children: Vec<Panel>) -> Panel {
    Panel {
        id: id.into(),
        weight,
        node: PanelNode::Split { axis, children },
    }
}

fn dock(children: Vec<Panel>) -> SidebarDoc {
    SidebarDoc {
        dock: ArrangementDoc {
            root: Some(split("root", 1.0, Axis::Column, children)),
        },
        floating: Vec::new(),
    }
}

fn children(doc: &SidebarDoc) -> &[Panel] {
    match &doc.dock.root.as_ref().expect("a dock").node {
        PanelNode::Split { children, .. } => children,
        PanelNode::Tabs { .. } => panic!("the dock root is a split"),
    }
}

/// Each dock root child's tabs, in order.
fn docked(arrangement: &SidebarArrangement) -> Vec<Vec<&str>> {
    children(&arrangement.doc).iter().map(Panel::tabs).collect()
}

fn floating(arrangement: &SidebarArrangement) -> Vec<Vec<&str>> {
    arrangement.doc.floating.iter().map(Panel::tabs).collect()
}

fn fresh(declared: &Declared, next: &mut impl FnMut() -> u64) -> SidebarArrangement {
    let mut arrangement = SidebarArrangement::default();
    arrangement.follow(declared, next);
    arrangement
}

fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

#[test]
fn hints_place_sections_and_the_workspace_fallback_goes_last() {
    let mut next = counter();
    let arrangement = fresh(&ab(), &mut next);
    assert_eq!(docked(&arrangement), [vec!["a"], vec!["b"], vec![UNPLACED]]);
    assert_eq!(arrangement.generation, 1);
    assert!(!arrangement.owned);
    assert_eq!(arrangement.placed, set(&["a", "b", UNPLACED]));
    // Reconciling again changes nothing.
    let mut again = arrangement.clone();
    let report = again.follow(&ab(), &mut next);
    assert_eq!(again, arrangement);
    assert_eq!(report, Report::default());
}

#[test]
fn a_reorder_is_kept_and_reconciliation_is_idempotent() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    let mut reordered = arrangement.doc.clone();
    if let Some(PanelNode::Split { children, .. }) =
        reordered.dock.root.as_mut().map(|root| &mut root.node)
    {
        children.swap(0, 1);
    }
    arrangement
        .commit(&ab(), reordered.clone(), 1, &mut next)
        .unwrap();
    assert_eq!(arrangement.generation, 2);
    assert!(arrangement.owned);
    assert!(arrangement.placed.is_empty(), "a commit clears placed");
    assert_eq!(arrangement.doc, reordered);
    let before = arrangement.clone();
    assert_eq!(arrangement.follow(&ab(), &mut next), Report::default());
    assert_eq!(arrangement, before);
    // Committing the stored document again keeps its generation.
    arrangement.commit(&ab(), reordered, 2, &mut next).unwrap();
    assert_eq!(arrangement.generation, 2);
}

#[test]
fn unrelated_panels_and_host_views_are_kept() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    let doc = dock(vec![
        tabs("1", 1.0, &["a"]),
        tabs("reserved", 0.25, &[]),
        tabs("2", 1.0, &["b", "u:notes"]),
        tabs("3", 1.0, &[UNPLACED]),
    ]);
    arrangement
        .commit(&ab(), doc.clone(), 1, &mut next)
        .unwrap();
    arrangement.follow(&ab(), &mut next);
    assert_eq!(arrangement.doc, doc);
    assert!(arrangement.view(&ab()).unresolved.is_empty());
}

#[test]
fn duplicates_keep_the_first_copy_and_drop_panels_they_empty() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    let mut doc = dock(vec![
        tabs("1", 1.0, &["b"]),
        tabs("2", 1.0, &["a"]),
        tabs("3", 1.0, &[UNPLACED]),
        // A copy beside unrelated content: the copy goes, the rest and its
        // weight stay.
        tabs("4", 0.125, &["a", "u:text"]),
        // A copy alone: its panel goes.
        tabs("5", 0.0625, &["a"]),
    ]);
    // Copies in floating panels: the docked originals win.
    doc.floating = vec![
        tabs("6", 0.5, &["a", "u:floating"]),
        tabs("7", 0.25, &["b"]),
    ];
    let report = arrangement.commit(&ab(), doc, 1, &mut next).unwrap();
    assert_eq!(report.duplicates, ["a", "b"]);
    assert_eq!(
        docked(&arrangement),
        [vec!["b"], vec!["a"], vec![UNPLACED], vec!["u:text"]]
    );
    assert_eq!(children(&arrangement.doc)[3].weight, 0.125);
    assert_eq!(floating(&arrangement), [vec!["u:floating"]]);
    assert_eq!(arrangement.doc.floating[0].weight, 0.5);
    // The removed tab was selected; the panel selects what is left.
    assert_eq!(
        children(&arrangement.doc)[3].node,
        PanelNode::Tabs {
            tabs: vec!["u:text".into()],
            selected: Some("u:text".into()),
        }
    );
    assert!(arrangement.doc.check().is_ok());
}

#[test]
fn an_added_section_goes_before_its_next_hinted_neighbour() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    // The user puts b first.
    let doc = dock(vec![
        tabs("1", 1.0, &["b"]),
        tabs("2", 1.0, &["a"]),
        tabs("3", 1.0, &[UNPLACED]),
    ]);
    arrangement.commit(&ab(), doc, 1, &mut next).unwrap();
    let abc = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
    ]);
    let report = arrangement.follow(&abc, &mut next);
    assert_eq!(report.placed, ["c"]);
    assert_eq!(
        docked(&arrangement),
        [vec!["c"], vec!["b"], vec!["a"], vec![UNPLACED]]
    );
    assert_eq!(arrangement.placed, set(&["c"]));
    assert_eq!(arrangement.generation, 3);
}

#[test]
fn closing_is_remembered_and_restore_gives_an_equal_share() {
    let mut next = counter();
    let abc = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
    ]);
    let mut arrangement = fresh(&abc, &mut next);
    assert_eq!(
        docked(&arrangement),
        [vec!["a"], vec!["c"], vec!["b"], vec![UNPLACED]]
    );
    // Closing c: commit without it.
    let doc = dock(vec![
        tabs("1", 0.75, &["a"]),
        tabs("2", 1.0, &["b"]),
        tabs("3", 0.5, &[UNPLACED]),
    ]);
    arrangement.commit(&abc, doc, 1, &mut next).unwrap();
    assert_eq!(arrangement.closed, set(&["c"]));
    // A known, closed section is not an unseen one: it stays closed.
    assert!(arrangement.follow(&abc, &mut next).placed.is_empty());
    assert_eq!(arrangement.generation, 2);

    // Restore: by its hint, an equal share, the others scaled to make room.
    let report = arrangement.restore(&abc, "c", 2, &mut next).unwrap();
    assert_eq!(report.restored, ["c"]);
    assert!(arrangement.closed.is_empty());
    assert_eq!(
        docked(&arrangement),
        [vec!["a"], vec!["c"], vec!["b"], vec![UNPLACED]]
    );
    let weights: Vec<f64> = children(&arrangement.doc)
        .iter()
        .map(|p| p.weight)
        .collect();
    assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    assert_eq!(weights[1], 0.25);
    assert!(
        (weights[0] / weights[2] - 0.75).abs() < 1e-9,
        "relative shares kept"
    );
    assert!(weights.iter().all(|w| *w > 0.0));
    let generation = arrangement.generation;
    assert_eq!(generation, 3);

    // Restoring again, or an open section, changes nothing.
    let before = arrangement.clone();
    let report = arrangement.restore(&abc, "c", 3, &mut next).unwrap();
    assert!(report.restored.is_empty());
    assert_eq!(arrangement, before);
    // An undeclared section can't be restored, and a stale restore is refused.
    assert!(matches!(
        arrangement.restore(&abc, "undeclared", 3, &mut next),
        Err(ArrangementError::Invalid(_))
    ));
    assert_eq!(
        arrangement.restore(&abc, "c", 2, &mut next),
        Err(ArrangementError::Stale { current: 3 })
    );
    assert_eq!(arrangement, before);
}

#[test]
fn a_commit_that_tabs_a_closed_section_restores_it() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    arrangement
        .commit(
            &ab(),
            dock(vec![tabs("1", 1.0, &["a", UNPLACED])]),
            1,
            &mut next,
        )
        .unwrap();
    assert_eq!(arrangement.closed, set(&["b"]));
    let report = arrangement
        .commit(
            &ab(),
            dock(vec![
                tabs("1", 1.0, &["a", UNPLACED]),
                tabs("2", 1.0, &["b"]),
            ]),
            2,
            &mut next,
        )
        .unwrap();
    assert_eq!(report.restored, ["b"]);
    assert!(arrangement.closed.is_empty());
}

#[test]
fn removed_sections_are_flagged_not_dropped() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    let a_panel = children(&arrangement.doc)[0].id.clone();
    let only_a = declared(&[region("a", None, Some(10))]);
    let report = arrangement.follow(&only_a, &mut next);
    // b's tab stays where it was, flagged; a keeps its panel.
    assert_eq!(report.unresolved, ["b"]);
    assert_eq!(docked(&arrangement), [vec!["a"], vec!["b"], vec![UNPLACED]]);
    assert_eq!(children(&arrangement.doc)[0].id, a_panel);
    assert_eq!(arrangement.view(&only_a).unresolved, set(&["b"]));
    // Unchanged document: the generation doesn't move for a flag.
    assert_eq!(arrangement.generation, 1);

    // Declared again, it is where it was: not placed anew.
    let report = arrangement.follow(&ab(), &mut next);
    assert!(report.placed.is_empty() && report.unresolved.is_empty());

    // A commit may keep an unresolved tab; one that drops it forgets it.
    arrangement.follow(&only_a, &mut next);
    let keep = arrangement.doc.clone();
    arrangement.commit(&only_a, keep, 1, &mut next).unwrap();
    assert_eq!(arrangement.view(&only_a).unresolved, set(&["b"]));
    let generation = arrangement.generation;
    arrangement
        .commit(
            &only_a,
            dock(vec![tabs("1", 1.0, &["a"]), tabs("3", 1.0, &[UNPLACED])]),
            generation,
            &mut next,
        )
        .unwrap();
    assert!(arrangement.view(&only_a).unresolved.is_empty());
    assert!(
        arrangement.closed.is_empty(),
        "dropping it doesn't close it"
    );
    // It was never closed, so declared again it is placed.
    assert_eq!(arrangement.follow(&ab(), &mut next).placed, ["b"]);
}

#[test]
fn a_closed_section_that_no_longer_resolves_stays_closed_and_flagged() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    arrangement
        .commit(
            &ab(),
            dock(vec![tabs("1", 1.0, &["a", UNPLACED])]),
            1,
            &mut next,
        )
        .unwrap();
    let only_a = declared(&[sidebar("a", 10)]);
    assert_eq!(arrangement.follow(&only_a, &mut next).unresolved, ["b"]);
    assert_eq!(arrangement.closed, set(&["b"]));
    // When it resolves again it is still closed.
    assert!(arrangement.follow(&ab(), &mut next).placed.is_empty());
    assert_eq!(arrangement.closed, set(&["b"]));
}

#[test]
fn template_drift_flags_every_key_that_no_longer_resolves() {
    // ADR 0013: a template version whose regions are renamed flags the old
    // keys (tabbed or closed) rather than dropping them, and places the new.
    let mut next = counter();
    let v1 = declared(&[
        sidebar("tree", 10),
        sidebar("git", 20),
        sidebar("attention", 30),
    ]);
    let mut arrangement = fresh(&v1, &mut next);
    let doc = dock(vec![
        tabs("1", 1.0, &["git"]),
        tabs("2", 1.0, &["tree"]),
        tabs("3", 1.0, &[UNPLACED]),
    ]);
    arrangement.commit(&v1, doc, 1, &mut next).unwrap();
    assert_eq!(arrangement.closed, set(&["attention"]));
    let v2 = declared(&[
        sidebar("projects", 10),
        sidebar("git", 20),
        sidebar("alerts", 30),
    ]);
    let report = arrangement.follow(&v2, &mut next);
    assert_eq!(report.placed, ["projects", "alerts"]);
    assert_eq!(report.unresolved, ["attention", "tree"]);
    assert_eq!(
        docked(&arrangement),
        [
            vec!["projects"],
            vec!["git"],
            vec!["tree"],
            vec!["alerts"],
            vec![UNPLACED]
        ]
    );
    assert_eq!(
        arrangement.view(&v2).unresolved,
        set(&["attention", "tree"])
    );
}

#[test]
fn reset_forgets_closes_and_positions() {
    let mut next = counter();
    let abc = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
    ]);
    let mut arrangement = fresh(&abc, &mut next);
    arrangement
        .commit(
            &abc,
            dock(vec![tabs("1", 1.0, &["b", "u:x"]), tabs("2", 1.0, &["a"])]),
            1,
            &mut next,
        )
        .unwrap();
    assert_eq!(arrangement.closed, set(&["c", UNPLACED]));
    arrangement.reset(&abc, 2, &mut next).unwrap();
    assert_eq!(
        docked(&arrangement),
        [vec!["a"], vec!["c"], vec!["b"], vec![UNPLACED]]
    );
    assert!(arrangement.closed.is_empty());
    assert!(!arrangement.owned);
    assert_eq!(
        arrangement.reset(&abc, 2, &mut next),
        Err(ArrangementError::Stale { current: 3 })
    );
}

#[test]
fn a_saved_split_stays_whole_when_a_neighbour_arrives_before_it() {
    let mut next = counter();
    let abc = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
    ]);
    let mut arrangement = fresh(&abc, &mut next);
    let nested = split(
        "nested",
        0.5,
        Axis::Row,
        vec![tabs("n1", 1.0, &["u:x"]), tabs("n2", 2.0, &["b"])],
    );
    arrangement
        .commit(
            &abc,
            dock(vec![
                tabs("1", 1.0, &["a"]),
                tabs("2", 1.0, &["c"]),
                nested.clone(),
                tabs("3", 1.0, &[UNPLACED]),
            ]),
            1,
            &mut next,
        )
        .unwrap();
    let abcd = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
        region("d", None, Some(17)),
    ]);
    arrangement.follow(&abcd, &mut next);
    let panels = children(&arrangement.doc);
    assert_eq!(panels[2].tabs(), ["d"]);
    assert_eq!(panels[3], nested, "the split keeps its shape and weight");
}

#[test]
fn legacy_adoption_closes_only_what_is_declared_now() {
    // The host's first commit adopts the layout it saved before Andamento
    // stored one: declared sections it lacks were closed then.
    let mut next = counter();
    let abd = declared(&[sidebar("a", 10), sidebar("b", 20), sidebar("d", 17)]);
    let mut arrangement = fresh(&abd, &mut next);
    let legacy = dock(vec![tabs("legacy", 1.0, &["b", "a"])]);
    arrangement.commit(&abd, legacy, 1, &mut next).unwrap();
    assert_eq!(arrangement.closed, set(&["d", UNPLACED]));
    // A section first declared later is new, and appears.
    let abde = declared(&[
        sidebar("a", 10),
        sidebar("b", 20),
        sidebar("d", 17),
        sidebar("e", 25),
    ]);
    assert_eq!(arrangement.follow(&abde, &mut next).placed, ["e"]);
    assert_eq!(docked(&arrangement), [vec!["b", "a"], vec!["e"]]);
}

#[test]
fn an_empty_declaration_flags_everything_and_drops_nothing() {
    // #221 cleared every position on an authoritative empty declaration;
    // under ADR 0013 the keys are flagged instead, and come back in place.
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    let before = arrangement.doc.clone();
    let none = declared(&[]);
    let report = arrangement.follow(&none, &mut next);
    assert_eq!(report.unresolved, ["a", "b"]);
    assert_eq!(arrangement.doc, before);
    let report = arrangement.follow(&ab(), &mut next);
    assert!(report.placed.is_empty() && report.unresolved.is_empty());
}

#[test]
fn hosts_and_orders() {
    // Absent hints, an unknown host with a negative order, the floating
    // host, and equal orders: x before y in each, in the right host.
    let variants: [(Option<&str>, Option<i64>, Host); 4] = [
        (None, None, Host::Dock),
        (Some("unknown"), Some(-10), Host::Dock),
        (Some("floating"), Some(0), Host::Floating),
        (None, Some(0), Host::Dock),
    ];
    for (host, order, expected) in variants {
        let mut next = counter();
        let xy = declared(&[region("x", host, order), region("y", host, order)]);
        let arrangement = fresh(&xy, &mut next);
        let (x, y) = match expected {
            Host::Dock => (docked(&arrangement), Vec::new()),
            Host::Floating => (floating(&arrangement), docked(&arrangement)),
        };
        assert_eq!(x[..2], [vec!["x"], vec!["y"]], "{host:?} {order:?}");
        if expected == Host::Floating {
            assert_eq!(y, [vec![UNPLACED]]);
        }
        assert!(arrangement.doc.check().is_ok());
    }
}

#[test]
fn a_changed_host_hint_does_not_move_a_placed_section() {
    let mut next = counter();
    let xy = declared(&[region("x", Some("floating"), None), region("y", None, None)]);
    let mut arrangement = fresh(&xy, &mut next);
    assert_eq!(floating(&arrangement), [vec!["x"]]);
    let moved = declared(&[region("x", Some("sidebar"), None), region("y", None, None)]);
    let before = arrangement.clone();
    arrangement.follow(&moved, &mut next);
    assert_eq!(arrangement, before);
    // Restore and reset do use the current hint.
    arrangement.reset(&moved, 1, &mut next).unwrap();
    assert!(floating(&arrangement).is_empty());
    assert_eq!(docked(&arrangement)[0], ["x"]);
}

#[test]
fn closing_every_section_keeps_them_closed_and_restore_fills_an_empty_dock() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    arrangement
        .commit(&ab(), SidebarDoc::default(), 1, &mut next)
        .unwrap();
    assert_eq!(arrangement.closed, set(&["a", "b", UNPLACED]));
    assert!(arrangement.follow(&ab(), &mut next).placed.is_empty());
    assert_eq!(arrangement.doc, SidebarDoc::default());
    arrangement.restore(&ab(), "b", 2, &mut next).unwrap();
    assert_eq!(docked(&arrangement), [vec!["b"]]);
    assert_eq!(children(&arrangement.doc)[0].weight, 1.0);
}

#[test]
fn restore_lifts_a_merged_leaf() {
    // A dock that is one tab panel: its tabs move into a panel of their own
    // beside the restored section, an equal share each.
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    let leaf = SidebarDoc {
        dock: ArrangementDoc {
            root: Some(tabs("leaf", 1.0, &["u:text", "a"])),
        },
        floating: Vec::new(),
    };
    arrangement.commit(&ab(), leaf, 1, &mut next).unwrap();
    arrangement.restore(&ab(), "b", 2, &mut next).unwrap();
    let panels = children(&arrangement.doc);
    assert_eq!(panels.len(), 2);
    assert_eq!(panels[0].id, "leaf");
    assert_eq!(panels[0].tabs(), ["u:text", "a"]);
    assert_eq!(panels[1].tabs(), ["b"]);
    assert_eq!((panels[0].weight, panels[1].weight), (0.5, 0.5));
    let ids: BTreeSet<&str> = arrangement
        .doc
        .panels()
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(ids.len(), 3, "fresh IDs are unique");
}

#[test]
fn local_sections_take_their_containers_place_and_hints() {
    let regions = [
        sidebar("tree", 10),
        RegionHints {
            hosts_local_sections: true,
            ..sidebar("local", 50)
        },
        sidebar("git", 40),
    ];
    // With no local sections, the container is a section itself.
    let plain = Declared::new(&regions, &[]);
    let keys: Vec<&str> = plain.sections.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["tree", "git", "local", UNPLACED]);
    let local = [
        LocalSection {
            id: "pins".into(),
            position: Some(1),
            default: false,
        },
        LocalSection {
            id: "workspaces".into(),
            position: Some(0),
            default: true,
        },
        LocalSection {
            id: "later".into(),
            position: None,
            default: false,
        },
    ];
    let with_local = Declared::new(&regions, &local);
    let keys: Vec<&str> = with_local.sections.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "tree",
            "git",
            ".section:workspaces",
            ".section:pins",
            ".section:later",
            UNPLACED
        ]
    );
    // Adding a local section places it among its siblings; the container
    // that stood in for them is flagged until the host drops it.
    let mut next = counter();
    let mut arrangement = fresh(&plain, &mut next);
    let report = arrangement.follow(&with_local, &mut next);
    assert_eq!(
        report.placed,
        [".section:workspaces", ".section:pins", ".section:later"]
    );
    assert_eq!(report.unresolved, ["local"]);

    // An unhinted container: the default section goes last, like .unplaced.
    let unhinted = [
        region("tree", None, None),
        RegionHints {
            hosts_local_sections: true,
            ..region("local", None, None)
        },
    ];
    let keys: Vec<String> = Declared::new(&unhinted, &local)
        .sections
        .into_iter()
        .map(|s| s.key)
        .collect();
    assert_eq!(
        keys,
        [
            "tree",
            ".section:pins",
            ".section:later",
            ".section:workspaces",
            UNPLACED
        ]
    );
}

#[test]
fn a_local_section_with_no_container_resolves_but_is_not_placed() {
    let local = [LocalSection {
        id: "s1".into(),
        position: None,
        default: false,
    }];
    let declared = Declared::new(&[sidebar("a", 10)], &local);
    assert!(declared.resolves(".section:s1"));
    assert!(!declared.sections.iter().any(|s| s.key == ".section:s1"));
    let mut next = counter();
    let mut arrangement = fresh(&declared, &mut next);
    arrangement
        .commit(
            &declared,
            dock(vec![tabs("1", 1.0, &["a", ".section:s1", UNPLACED])]),
            1,
            &mut next,
        )
        .unwrap();
    assert!(arrangement.view(&declared).unresolved.is_empty());
}

#[test]
fn pinned_regions_sort_first() {
    let declared = declared(&[
        sidebar("a", 10),
        RegionHints {
            pinned: true,
            ..sidebar("b", 20)
        },
    ]);
    let keys: Vec<&str> = declared.sections.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["b", "a", UNPLACED]);
}

#[test]
fn stale_and_invalid_commits_change_nothing() {
    let mut next = counter();
    let mut arrangement = fresh(&ab(), &mut next);
    let before = arrangement.clone();
    let good = dock(vec![tabs("1", 1.0, &["a", "b", UNPLACED])]);
    assert_eq!(
        arrangement.commit(&ab(), good, 0, &mut next),
        Err(ArrangementError::Stale { current: 1 })
    );
    let invalid = [
        // An unknown section, a malformed host view, a repeated panel ID, a
        // weight that isn't positive, an empty split.
        dock(vec![tabs("1", 1.0, &["nope"])]),
        dock(vec![tabs("1", 1.0, &["u:Bad"])]),
        dock(vec![tabs("1", 1.0, &["a"]), tabs("1", 1.0, &["b"])]),
        dock(vec![tabs("1", 0.0, &["a"])]),
        dock(vec![split("s", 1.0, Axis::Row, Vec::new())]),
    ];
    for doc in invalid {
        assert!(matches!(
            arrangement.commit(&ab(), doc, 1, &mut next),
            Err(ArrangementError::Invalid(_))
        ));
    }
    // A floating panel can't reuse a docked panel's ID either.
    let mut clash = dock(vec![tabs("1", 1.0, &["a"])]);
    clash.floating.push(tabs("1", 1.0, &["b"]));
    assert!(arrangement.commit(&ab(), clash, 1, &mut next).is_err());
    assert_eq!(arrangement, before);
}

#[test]
fn unusual_keys_are_ordinary_keys() {
    // Numeric and reserved-looking region names place and close like any.
    let mut next = counter();
    let odd = declared(&[region("123", None, None), region("closed", None, None)]);
    let mut arrangement = fresh(&odd, &mut next);
    assert_eq!(
        docked(&arrangement),
        [vec!["123"], vec!["closed"], vec![UNPLACED]]
    );
    arrangement
        .commit(&odd, dock(vec![tabs("1", 1.0, &["closed"])]), 1, &mut next)
        .unwrap();
    assert_eq!(arrangement.closed, set(&["123", UNPLACED]));
}
