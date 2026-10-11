//! Wheelhouse's section-placement scenarios (#191, #207, #221;
//! `uishell_section_placement_diagnostics`), ported to the document model:
//! hints, reorder, restart, add, close, remove, reset, duplicates, closed
//! restore, `.unplaced`, legacy adoption and host hints.
use super::*;
use crate::versioned::{Clock, CommitError, Versioned};

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

/// A stored arrangement as the sidebar keeps it: versioned on a clock.
#[derive(Clone, Debug)]
struct Stored {
    clock: Clock,
    value: Versioned<SidebarArrangement>,
}

impl PartialEq for Stored {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl std::ops::Deref for Stored {
    type Target = SidebarArrangement;
    fn deref(&self) -> &SidebarArrangement {
        self.value.get()
    }
}

impl Stored {
    fn generation(&self) -> u64 {
        self.value.generation()
    }

    fn follow(&mut self, declared: &Declared) -> Report {
        let mut report = Report::default();
        self.value
            .follow(&mut self.clock, |stored| report = stored.follow(declared));
        report
    }

    fn edit(
        &mut self,
        expected: u64,
        edit: impl FnOnce(&mut SidebarArrangement) -> Result<Report, String>,
    ) -> Result<Report, CommitError> {
        let (_, report) = self
            .value
            .update(Some(expected), &mut self.clock, |stored| Ok(edit(stored)?))?;
        Ok(report)
    }

    fn commit(
        &mut self,
        declared: &Declared,
        doc: SidebarDoc,
        expected: u64,
    ) -> Result<Report, CommitError> {
        self.edit(expected, |stored| stored.commit(declared, doc))
    }

    fn restore(
        &mut self,
        declared: &Declared,
        key: &str,
        expected: u64,
    ) -> Result<Report, CommitError> {
        self.edit(expected, |stored| stored.restore(declared, key))
    }

    fn reset(&mut self, declared: &Declared, expected: u64) -> Result<Report, CommitError> {
        self.edit(expected, |stored| Ok(stored.reset(declared)))
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

fn fresh(declared: &Declared) -> Stored {
    let mut arrangement = Stored {
        clock: Clock::default(),
        value: Versioned::default(),
    };
    arrangement.follow(declared);
    arrangement
}

fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

#[test]
fn unrelated_panels_and_host_views_are_kept() {
    let mut arrangement = fresh(&ab());
    let doc = dock(vec![
        tabs("1", 1.0, &["a"]),
        tabs("reserved", 0.25, &[]),
        tabs("2", 1.0, &["b", "u:notes"]),
        tabs("3", 1.0, &[UNPLACED]),
    ]);
    arrangement.commit(&ab(), doc.clone(), 1).unwrap();
    arrangement.follow(&ab());
    assert_eq!(arrangement.doc, doc);
    assert!(arrangement.view(0, &ab()).unresolved.is_empty());
}

#[test]
fn duplicates_keep_the_first_copy_and_drop_panels_they_empty() {
    let mut arrangement = fresh(&ab());
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
    let report = arrangement.commit(&ab(), doc, 1).unwrap();
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
fn closing_is_remembered_and_restore_gives_an_equal_share() {
    let abc = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
    ]);
    let mut arrangement = fresh(&abc);
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
    arrangement.commit(&abc, doc, 1).unwrap();
    assert_eq!(arrangement.closed, set(&["c"]));
    // A known, closed section is not an unseen one: it stays closed.
    assert!(arrangement.follow(&abc).placed.is_empty());
    assert_eq!(arrangement.generation(), 2);

    // Restore: by its hint, an equal share, the others scaled to make room.
    let report = arrangement.restore(&abc, "c", 2).unwrap();
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
    let generation = arrangement.generation();
    assert_eq!(generation, 3);

    // Restoring again, or an open section, changes nothing.
    let before = arrangement.clone();
    let report = arrangement.restore(&abc, "c", 3).unwrap();
    assert!(report.restored.is_empty());
    assert_eq!(arrangement, before);
    // An undeclared section can't be restored, and a stale restore is refused.
    assert!(matches!(
        arrangement.restore(&abc, "undeclared", 3),
        Err(CommitError::Invalid(_))
    ));
    assert_eq!(
        arrangement.restore(&abc, "c", 2),
        Err(CommitError::Stale { current: 3 })
    );
    assert_eq!(arrangement, before);
}

#[test]
fn a_saved_split_stays_whole_when_a_neighbour_arrives_before_it() {
    let abc = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
    ]);
    let mut arrangement = fresh(&abc);
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
        )
        .unwrap();
    let abcd = declared(&[
        sidebar("b", 20),
        sidebar("a", 10),
        region("c", None, Some(15)),
        region("d", None, Some(17)),
    ]);
    arrangement.follow(&abcd);
    let panels = children(&arrangement.doc);
    assert_eq!(panels[2].tabs(), ["d"]);
    assert_eq!(panels[3], nested, "the split keeps its shape and weight");
}

#[test]
fn restore_lifts_a_merged_leaf() {
    // A dock that is one tab panel: its tabs move into a panel of their own
    // beside the restored section, an equal share each.
    let mut arrangement = fresh(&ab());
    let leaf = SidebarDoc {
        dock: ArrangementDoc {
            root: Some(tabs("leaf", 1.0, &["u:text", "a"])),
        },
        floating: Vec::new(),
    };
    arrangement.commit(&ab(), leaf, 1).unwrap();
    arrangement.restore(&ab(), "b", 2).unwrap();
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
    let mut arrangement = fresh(&plain);
    let report = arrangement.follow(&with_local);
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
    let mut arrangement = fresh(&declared);
    arrangement
        .commit(
            &declared,
            dock(vec![tabs("1", 1.0, &["a", ".section:s1", UNPLACED])]),
            1,
        )
        .unwrap();
    assert!(arrangement.view(0, &declared).unresolved.is_empty());
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
    let mut arrangement = fresh(&ab());
    let before = arrangement.clone();
    let good = dock(vec![tabs("1", 1.0, &["a", "b", UNPLACED])]);
    assert_eq!(
        arrangement.commit(&ab(), good, 0),
        Err(CommitError::Stale { current: 1 })
    );
    let mut clash = dock(vec![tabs("1", 1.0, &["a"])]);
    clash.floating.push(tabs("1", 1.0, &["b"]));
    let invalid = [
        (dock(vec![tabs("1", 1.0, &["nope"])]), "no section \"nope\""),
        (
            dock(vec![tabs("1", 1.0, &["u:Bad"])]),
            "no section \"u:Bad\"",
        ),
        (
            dock(vec![tabs("1", 1.0, &["a"]), tabs("1", 1.0, &["b"])]),
            "panel ID \"1\" is used twice",
        ),
        (dock(vec![tabs("1", 0.0, &["a"])]), "weights are positive"),
        (
            dock(vec![split("s", 1.0, Axis::Row, Vec::new())]),
            "split \"s\" has no panels",
        ),
        // A floating panel can't reuse a docked panel's ID either.
        (clash, "panel ID \"1\" is used twice"),
    ];
    for (doc, reason) in invalid {
        match arrangement.commit(&ab(), doc, 1) {
            Err(CommitError::Invalid(error)) => assert!(error.contains(reason), "{error}"),
            other => panic!("{reason}: {other:?}"),
        }
    }
    assert_eq!(arrangement, before);
}

/// One region a scenario declares: name, default host and order.
type Region = (&'static str, Option<&'static str>, Option<i64>);
const S: Option<&str> = Some("sidebar");
const F: Option<&str> = Some("floating");

/// A step of a scenario, against what is declared now.
enum Step {
    /// Declare these regions (a template change).
    Declare(&'static [Region]),
    /// The host commits a dock of tab panels (each a list of keys), at the
    /// current generation.
    Commit(&'static [&'static [&'static str]]),
    Restore(&'static str),
    Reset,
}

const AB: &[Region] = &[("b", S, Some(20)), ("a", S, Some(10))];
const ABC: &[Region] = &[
    ("b", S, Some(20)),
    ("a", S, Some(10)),
    ("c", None, Some(15)),
];
const ONLY_A: &[Region] = &[("a", S, Some(10))];
const U: &str = UNPLACED;

/// What an arrangement holds and what the last step reported, as one line:
/// the dock's tab panels, the floating ones, closed and placed keys, and the
/// report's placed, restored and unresolved keys.
fn summary(arrangement: &SidebarArrangement, report: &Report) -> String {
    let panels = |panels: Vec<Vec<&str>>| {
        panels
            .iter()
            .map(|keys| format!("[{}]", keys.join(" ")))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let keys = |keys: Vec<&String>| keys.into_iter().cloned().collect::<Vec<_>>().join(" ");
    let docked = match arrangement.doc.dock.root {
        Some(_) => panels(docked(arrangement)),
        None => String::new(),
    };
    format!(
        "dock {docked}; float {}; closed {}; placed {}; report {} / {} / {}{}",
        panels(floating(arrangement)),
        keys(arrangement.closed.iter().collect()),
        keys(arrangement.placed.iter().collect()),
        keys(report.placed.iter().collect()),
        keys(report.restored.iter().collect()),
        keys(report.unresolved.iter().collect()),
        if arrangement.owned { "; owned" } else { "" },
    )
}

/// Wheelhouse's section-placement scenarios: what is declared, the steps,
/// and the outcome as [`summary`] gives it. Reports read placed /
/// restored / unresolved.
#[test]
fn placement_scenarios() {
    use Step::*;
    #[rustfmt::skip]
    let cases: &[(&str, &[Region], &[Step], &str)] = &[
        ("hints place sections, the workspace fallback last", AB, &[],
         "dock [a] [b] [.unplaced]; float ; closed ; placed .unplaced a b; report a b .unplaced /  / "),
        ("a reorder is kept and reconciling it changes nothing", AB,
         &[Commit(&[&["b"], &["a"], &[U]]), Declare(AB)],
         "dock [b] [a] [.unplaced]; float ; closed ; placed ; report  /  / ; owned"),
        ("an added section goes before its next hinted neighbour", AB,
         &[Commit(&[&["b"], &["a"], &[U]]), Declare(ABC)],
         "dock [c] [b] [a] [.unplaced]; float ; closed ; placed c; report c /  / ; owned"),
        ("leaving a section out closes it; tabbing it again restores it", AB,
         &[Commit(&[&["a", U]]), Commit(&[&["a", U], &["b"]])],
         "dock [a .unplaced] [b]; float ; closed ; placed ; report  / b / ; owned"),
        ("a closed section stays closed when declarations change", ABC,
         &[Commit(&[&["a"], &["b"], &[U]]), Declare(ABC)],
         "dock [a] [b] [.unplaced]; float ; closed c; placed ; report  /  / ; owned"),
        ("a removed section is flagged in place, and there when declared again", AB,
         &[Declare(ONLY_A)],
         "dock [a] [b] [.unplaced]; float ; closed ; placed .unplaced a b; report  /  / b"),
        ("... and comes back where it was", AB, &[Declare(ONLY_A), Declare(AB)],
         "dock [a] [b] [.unplaced]; float ; closed ; placed .unplaced a b; report  /  / "),
        ("a commit dropping an unresolved tab forgets it, unclosed", AB,
         &[Declare(ONLY_A), Commit(&[&["a"], &[U]]), Declare(AB)],
         "dock [a] [b] [.unplaced]; float ; closed ; placed b; report b /  / ; owned"),
        ("a closed section that no longer resolves stays closed, flagged", AB,
         &[Commit(&[&["a", U]]), Declare(ONLY_A)],
         "dock [a .unplaced]; float ; closed b; placed ; report  /  / b; owned"),
        ("template drift flags every key that no longer resolves, places the new",
         &[("tree", S, Some(10)), ("git", S, Some(20)), ("attention", S, Some(30))],
         &[Commit(&[&["git"], &["tree"], &[U]]),
           Declare(&[("projects", S, Some(10)), ("git", S, Some(20)), ("alerts", S, Some(30))])],
         "dock [projects] [git] [tree] [alerts] [.unplaced]; float ; closed attention; \
          placed alerts projects; report projects alerts /  / attention tree; owned"),
        ("an empty declaration flags everything and drops nothing", AB, &[Declare(&[])],
         "dock [a] [b] [.unplaced]; float ; closed ; placed .unplaced a b; report  /  / a b"),
        ("reset forgets closes and positions", ABC, &[Commit(&[&["b", "u:x"], &["a"]]), Reset],
         "dock [a] [c] [b] [.unplaced]; float ; closed ; placed .unplaced a b c; report a c b .unplaced /  / "),
        ("legacy adoption closes only what is declared now",
         &[("a", S, Some(10)), ("b", S, Some(20)), ("d", S, Some(17))],
         &[Commit(&[&["b", "a"]]),
           Declare(&[("a", S, Some(10)), ("b", S, Some(20)), ("d", S, Some(17)), ("e", S, Some(25))])],
         "dock [b a] [e]; float ; closed .unplaced d; placed e; report e /  / ; owned"),
        ("hosts: floating, unknown, absent", &[("x", F, None), ("y", Some("unknown"), Some(-10))], &[],
         "dock [y] [.unplaced]; float [x]; closed ; placed .unplaced x y; report y x .unplaced /  / "),
        ("a changed host hint doesn't move a placed section; reset uses it",
         &[("x", F, None), ("y", None, None)],
         &[Declare(&[("x", S, None), ("y", None, None)]), Reset],
         "dock [x] [y] [.unplaced]; float ; closed ; placed .unplaced x y; report x y .unplaced /  / "),
        ("closing every section keeps them closed; restore fills the empty dock", AB,
         &[Commit(&[]), Declare(AB), Restore("b")],
         "dock [b]; float ; closed .unplaced a; placed b; report  / b / ; owned"),
        ("numeric and reserved-looking names are ordinary keys",
         &[("123", None, None), ("closed", None, None)], &[Commit(&[&["closed"]])],
         "dock [closed]; float ; closed .unplaced 123; placed ; report  /  / ; owned"),
    ];
    let regions = |regions: &[Region]| {
        declared(
            &regions
                .iter()
                .map(|(name, host, order)| region(name, *host, *order))
                .collect::<Vec<_>>(),
        )
    };
    for (name, initial, steps, expected) in cases {
        let mut now = regions(initial);
        let mut arrangement = Stored {
            clock: Clock::default(),
            value: Versioned::default(),
        };
        let mut report = arrangement.follow(&now);
        for step in *steps {
            let generation = arrangement.generation();
            report = match step {
                Declare(next) => {
                    now = regions(next);
                    arrangement.follow(&now)
                }
                Commit(panels) => {
                    let mut doc = SidebarDoc::default();
                    if !panels.is_empty() {
                        doc = dock(
                            panels
                                .iter()
                                .enumerate()
                                .map(|(i, keys)| tabs(&(i + 1).to_string(), 1.0, keys))
                                .collect(),
                        );
                    }
                    arrangement.commit(&now, doc, generation).unwrap()
                }
                Restore(key) => arrangement.restore(&now, key, generation).unwrap(),
                Reset => arrangement.reset(&now, generation).unwrap(),
            };
        }
        assert_eq!(summary(&arrangement, &report), *expected, "{name}");
        assert!(arrangement.doc.check().is_ok(), "{name}");
    }
}
