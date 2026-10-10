//! The Dashboard's sidebar arrangement (Wheelhouse ADR 0012 and 0013;
//! docs/sidebar-design/sidebar-arrangement.md).
//!
//! Which sections are docked where is one document: the dock's panel tree,
//! the floating panels, and the sections closed on purpose. The host owns the
//! live copy and commits it whole at an expected generation, as it does a
//! workspace's arrangement. Andamento reconciles it against what the template
//! and the local sections declare: new sections are placed by their region's
//! `default-host` and `order` hints, duplicate tabs are removed, and keys that
//! no longer resolve are flagged rather than dropped. Pixel geometry and
//! section collapse are the host's Presentation State.
use std::collections::BTreeSet;

use crate::slots::{self, ArrangementDoc, ArrangementError, Panel, PanelNode};
use crate::suggested_layout::Axis;

/// The key of a local section (`.section` entity `<id>`) among sections.
pub const LOCAL_SECTION_PREFIX: &str = ".section:";
/// The prefix of keys of the host's own views, which Andamento stores but
/// never places or flags (`u:<id>`, as for a workspace's user slots).
pub const HOST_VIEW_PREFIX: &str = slots::USER_PREFIX;
/// The `default-host` hint for a floating panel; any other (or none) docks.
pub const FLOATING_HOST: &str = "floating";

/// A template region's placement hints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionHints {
    pub name: String,
    pub default_host: Option<String>,
    pub order: Option<i64>,
    pub pinned: bool,
    /// Its placement has a `layout="section"` loop: while any local section
    /// exists, each is a section of its own in its place, with its hints.
    pub hosts_local_sections: bool,
}

/// A section someone made: a local `.section` entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSection {
    pub id: String,
    /// Its `.position` fact.
    pub position: Option<i64>,
    /// It holds the default group (`.default`), which hosts the workspaces
    /// nothing else places.
    pub default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Dock,
    Floating,
}

/// A declared section and where it goes by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub key: String,
    pub host: Host,
    pub pinned: bool,
    /// The explicit order, or the declaration index; an unhinted workspace
    /// fallback is `i64::MAX`.
    pub order: i64,
}

/// What the template and local sections declare: sections in default
/// placement order, and the keys of local sections that exist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    pub sections: Vec<Section>,
    pub local: BTreeSet<String>,
}

impl Declared {
    /// Declarations from template regions, in declaration order, and the
    /// local sections. The rules are Wheelhouse's (section-placement.md):
    ///
    /// - A region is a section keyed by its name. The first region that
    ///   hosts local sections is, while any exists, replaced by them
    ///   (`.section:<id>`, by `.position` then ID), each with its hints.
    /// - Andamento always emits the workspace fallback, `.unplaced`, after
    ///   the regions.
    /// - An explicit order sorts lower first; an omitted one is the
    ///   section's declaration index; ties keep declaration order. An
    ///   unhinted workspace fallback (`.unplaced`, or the local section
    ///   holding the default group) sorts after every other section.
    /// - Pinned regions sort before unpinned ones.
    /// - `default-host="floating"` places a section in a floating panel; any
    ///   other host, or none, docks it.
    pub fn new(regions: &[RegionHints], local: &[LocalSection]) -> Self {
        let mut local_sorted: Vec<&LocalSection> = local.iter().collect();
        local_sorted.sort_by(|a, b| {
            (a.position.is_none(), a.position, &a.id).cmp(&(
                b.position.is_none(),
                b.position,
                &b.id,
            ))
        });
        let container = regions
            .iter()
            .position(|region| region.hosts_local_sections)
            .filter(|_| !local.is_empty());
        let mut sections: Vec<Section> = Vec::new();
        let mut push = |key: String, region: Option<&RegionHints>, fallback: bool| {
            if sections.iter().any(|section| section.key == key) {
                return;
            }
            let index = i64::try_from(sections.len()).unwrap_or(i64::MAX);
            let order = region.and_then(|r| r.order);
            sections.push(Section {
                key,
                host: match region.and_then(|r| r.default_host.as_deref()) {
                    Some(FLOATING_HOST) => Host::Floating,
                    _ => Host::Dock,
                },
                pinned: region.is_some_and(|r| r.pinned),
                order: order.unwrap_or(if fallback { i64::MAX } else { index }),
            });
        };
        for (index, region) in regions.iter().enumerate() {
            if Some(index) == container {
                for section in &local_sorted {
                    push(local_key(&section.id), Some(region), section.default);
                }
            } else {
                push(region.name.clone(), Some(region), false);
            }
        }
        push(
            crate::presentation::UNPLACED_WORKSPACES_SECTION.to_owned(),
            None,
            true,
        );
        // Stable: equal keys keep declaration order.
        sections.sort_by_key(|section| (!section.pinned, section.order));
        Self {
            sections,
            local: local.iter().map(|section| local_key(&section.id)).collect(),
        }
    }

    fn index(&self, key: &str) -> Option<usize> {
        self.sections.iter().position(|section| section.key == key)
    }

    /// Whether a key names something that exists: a declared section, a
    /// local section (declared or not), or one of the host's own views.
    pub fn resolves(&self, key: &str) -> bool {
        self.index(key).is_some() || self.local.contains(key) || is_host_view(key)
    }
}

/// The section key of local section `id`.
pub fn local_key(id: &str) -> String {
    format!("{LOCAL_SECTION_PREFIX}{id}")
}

fn is_host_view(key: &str) -> bool {
    key.strip_prefix(HOST_VIEW_PREFIX)
        .is_some_and(|id| slots::check_key(id).is_ok())
}

/// The sidebar's panels: the dock's tree, and the floating panels, each a
/// tree of its own. Tabs hold section keys (or the host's `u:<id>` views).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarDoc {
    pub dock: ArrangementDoc,
    pub floating: Vec<Panel>,
}

impl SidebarDoc {
    /// Every panel in preorder: the dock's, then each floating panel's.
    pub fn panels(&self) -> Vec<&Panel> {
        let mut out = self.dock.panels();
        out.extend(self.floating.iter().flat_map(Panel::preorder));
        out
    }

    /// Every tab's key, in the same order.
    pub fn tabs(&self) -> Vec<&str> {
        let mut out = self.dock.tabs();
        out.extend(self.floating.iter().flat_map(Panel::tabs));
        out
    }

    fn contains(&self, key: &str) -> bool {
        self.tabs().contains(&key)
    }

    /// The shape rules of an arrangement document, across all panels. A key
    /// may have two tabs here: reconciling removes the later ones.
    pub fn check(&self) -> Result<(), String> {
        slots::check_panels(self.panels(), true)
    }
}

/// The Dashboard's stored sidebar arrangement.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarArrangement {
    /// Nonzero once stored; it changes whenever anything here does.
    pub generation: u64,
    /// The host has committed it since it was last reset.
    pub owned: bool,
    pub doc: SidebarDoc,
    /// Sections closed on purpose: declared, with no tab. A closed key that
    /// no longer resolves is kept, and flagged.
    pub closed: BTreeSet<String>,
    /// Tabs Andamento placed since the host last committed.
    pub placed: BTreeSet<String>,
}

/// What a reconciliation did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// New sections placed by their hints.
    pub placed: Vec<String>,
    /// Closed sections that have a tab again.
    pub restored: Vec<String>,
    /// Keys whose later tabs were removed, keeping the first.
    pub duplicates: Vec<String>,
    /// Tabs and closed keys that no longer resolve. They are kept until the
    /// host drops them, and come back as they were if they resolve again.
    pub unresolved: Vec<String>,
}

/// Reconcile a stored arrangement with what is declared: remove duplicate
/// tabs (keeping the first, dock before floating, in preorder), and place
/// each declared section that has neither a tab nor a closed record by its
/// hints. Nothing that no longer resolves is dropped; it is reported. The
/// generation is the caller's.
pub fn reconcile(declared: &Declared, stored: &SidebarArrangement) -> (SidebarArrangement, Report) {
    let mut next = stored.clone();
    let mut report = Report {
        duplicates: dedupe(&mut next.doc),
        ..Report::default()
    };
    // A merged record can both tab and close a section: the tab wins.
    let tabs: BTreeSet<String> = next.doc.tabs().into_iter().map(str::to_owned).collect();
    next.closed.retain(|key| !tabs.contains(key));
    for (index, section) in declared.sections.iter().enumerate() {
        if tabs.contains(&section.key) || next.closed.contains(&section.key) {
            continue;
        }
        place(&mut next.doc, declared, index, Share::Arrival);
        next.placed.insert(section.key.clone());
        report.placed.push(section.key.clone());
    }
    let tabs: BTreeSet<String> = next.doc.tabs().into_iter().map(str::to_owned).collect();
    next.placed.retain(|key| tabs.contains(key));
    report.unresolved = unresolved(declared, &next).into_iter().collect();
    (next, report)
}

/// Tabs and closed keys that don't resolve.
pub fn unresolved(declared: &Declared, arrangement: &SidebarArrangement) -> BTreeSet<String> {
    arrangement
        .doc
        .tabs()
        .into_iter()
        .chain(arrangement.closed.iter().map(String::as_str))
        .filter(|key| !declared.resolves(key))
        .map(str::to_owned)
        .collect()
}

/// An arrangement as the host reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarView {
    /// 0 when nothing is stored yet.
    pub generation: u64,
    pub owned: bool,
    pub doc: SidebarDoc,
    pub closed: BTreeSet<String>,
    pub placed: BTreeSet<String>,
    pub unresolved: BTreeSet<String>,
}

impl SidebarArrangement {
    pub fn view(&self, declared: &Declared) -> SidebarView {
        SidebarView {
            generation: self.generation,
            owned: self.owned,
            doc: self.doc.clone(),
            closed: self.closed.clone(),
            placed: self.placed.clone(),
            unresolved: unresolved(declared, self),
        }
    }

    /// Store `next` if it differs, at a new generation. Returns whether it did.
    fn adopt(&mut self, mut next: Self, next_generation: &mut impl FnMut() -> u64) -> bool {
        next.generation = self.generation;
        if next == *self {
            return false;
        }
        next.generation = next_generation();
        *self = next;
        true
    }

    fn expect(&self, expected: u64) -> Result<(), ArrangementError> {
        if expected == self.generation {
            Ok(())
        } else {
            Err(ArrangementError::Stale {
                current: self.generation,
            })
        }
    }

    /// Follow what is declared: on configure, a template change, a local
    /// section change or an import. Returns the reconciliation's report.
    pub fn follow(
        &mut self,
        declared: &Declared,
        next_generation: &mut impl FnMut() -> u64,
    ) -> Report {
        let (next, report) = reconcile(declared, self);
        self.adopt(next, next_generation);
        report
    }

    /// Commit the host's whole document, if `expected` is the current
    /// generation (0 before any). A tab names something that resolves, or a
    /// key the stored arrangement already holds. Declared sections it leaves
    /// out are closed; closed sections it tabs are restored. A stale or
    /// invalid commit changes nothing.
    pub fn commit(
        &mut self,
        declared: &Declared,
        doc: SidebarDoc,
        expected: u64,
        next_generation: &mut impl FnMut() -> u64,
    ) -> Result<Report, ArrangementError> {
        self.expect(expected)?;
        doc.check().map_err(ArrangementError::Invalid)?;
        if let Some(key) = doc.tabs().into_iter().find(|key| {
            !declared.resolves(key) && !self.doc.contains(key) && !self.closed.contains(*key)
        }) {
            return Err(ArrangementError::Invalid(format!(
                "no section {key:?}: a tab names a declared or local section, or a host view \"{HOST_VIEW_PREFIX}<id>\""
            )));
        }
        let mut next = Self {
            generation: self.generation,
            owned: true,
            doc,
            closed: self.closed.clone(),
            placed: BTreeSet::new(),
        };
        let duplicates = dedupe(&mut next.doc);
        let tabs: BTreeSet<String> = next.doc.tabs().into_iter().map(str::to_owned).collect();
        let restored: Vec<String> = next
            .closed
            .iter()
            .filter(|key| tabs.contains(*key))
            .cloned()
            .collect();
        next.closed.retain(|key| !tabs.contains(key));
        for section in &declared.sections {
            if !tabs.contains(&section.key) {
                next.closed.insert(section.key.clone());
            }
        }
        let (next, mut report) = reconcile(declared, &next);
        report.duplicates = duplicates;
        report.restored = restored;
        self.adopt(next, next_generation);
        Ok(report)
    }

    /// Reopen a declared section the user closed, by its hints, with an
    /// equal share of its host. A section that has a tab is left where it is.
    pub fn restore(
        &mut self,
        declared: &Declared,
        key: &str,
        expected: u64,
        next_generation: &mut impl FnMut() -> u64,
    ) -> Result<Report, ArrangementError> {
        self.expect(expected)?;
        let index = declared.index(key).ok_or_else(|| {
            ArrangementError::Invalid(format!("no section {key:?} is declared to restore"))
        })?;
        let mut next = self.clone();
        let mut report = Report::default();
        next.closed.remove(key);
        if !next.doc.contains(key) {
            place(&mut next.doc, declared, index, Share::Equal);
            next.placed.insert(key.to_owned());
            next.owned = true;
            report.restored.push(key.to_owned());
        }
        report.unresolved = unresolved(declared, &next).into_iter().collect();
        self.adopt(next, next_generation);
        Ok(report)
    }

    /// Forget the user's arrangement and closed sections, and every key that
    /// no longer resolves, placing every declared section by its hints.
    pub fn reset(
        &mut self,
        declared: &Declared,
        expected: u64,
        next_generation: &mut impl FnMut() -> u64,
    ) -> Result<Report, ArrangementError> {
        self.expect(expected)?;
        let (next, report) = reconcile(declared, &Self::default());
        self.adopt(next, next_generation);
        Ok(report)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Share {
    /// A declared section arriving: weight 1 (the host sizes sections).
    Arrival,
    /// A restored section: an equal share of its host, the others scaled to
    /// make room.
    Equal,
}

/// Remove every tab whose key an earlier tab has (dock before floating,
/// preorder), and the panels that removal empties. Panels that were already
/// empty stay. Returns the keys removed, once each.
fn dedupe(doc: &mut SidebarDoc) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut removed = Vec::new();
    if doc
        .dock
        .root
        .as_mut()
        .is_some_and(|root| dedupe_panel(root, &mut seen, &mut removed))
    {
        doc.dock.root = None;
    }
    doc.floating
        .retain_mut(|root| !dedupe_panel(root, &mut seen, &mut removed));
    removed
}

/// Dedupe one panel; returns whether removal emptied it, so it goes.
fn dedupe_panel(panel: &mut Panel, seen: &mut BTreeSet<String>, removed: &mut Vec<String>) -> bool {
    match &mut panel.node {
        PanelNode::Tabs { tabs, selected } => {
            let before = tabs.len();
            tabs.retain(|tab| {
                if seen.insert(tab.clone()) {
                    return true;
                }
                if !removed.contains(tab) {
                    removed.push(tab.clone());
                }
                false
            });
            if selected.as_ref().is_some_and(|s| !tabs.contains(s)) {
                *selected = tabs.first().cloned();
            }
            before != tabs.len() && tabs.is_empty()
        }
        PanelNode::Split { children, .. } => {
            children.retain_mut(|child| !dedupe_panel(child, seen, removed));
            children.is_empty()
        }
    }
}

/// `count` panel IDs the document doesn't use: the smallest positive
/// integers free.
fn fresh_ids(doc: &SidebarDoc, count: usize) -> Vec<String> {
    let used: BTreeSet<&str> = doc.panels().into_iter().map(|p| p.id.as_str()).collect();
    (1..)
        .map(|n: u64| n.to_string())
        .filter(|id| !used.contains(id.as_str()))
        .take(count)
        .collect()
}

/// Place declared section `index` in a new tab panel of its host, before the
/// whole subtree holding the next declared section placed there; else last.
/// Saved panels keep their structure, selection and weights (an equal share
/// scales the dock's).
fn place(doc: &mut SidebarDoc, declared: &Declared, index: usize, share: Share) {
    let section = &declared.sections[index];
    let later: Vec<&str> = declared.sections[index + 1..]
        .iter()
        .map(|s| s.key.as_str())
        .collect();
    let [panel_id, split_id]: [String; 2] = fresh_ids(doc, 2).try_into().expect("two IDs");
    let mut panel = Panel {
        id: panel_id,
        weight: 1.0,
        node: PanelNode::Tabs {
            tabs: vec![section.key.clone()],
            selected: Some(section.key.clone()),
        },
    };
    let anchor = |panels: &[Panel]| {
        later
            .iter()
            .find_map(|key| panels.iter().position(|p| p.tabs().contains(key)))
            .unwrap_or(panels.len())
    };
    match section.host {
        Host::Floating => {
            let at = anchor(&doc.floating);
            doc.floating.insert(at, panel);
        }
        Host::Dock => {
            let children = dock_children(&mut doc.dock, split_id);
            let at = anchor(children);
            if share == Share::Equal && !children.is_empty() {
                let total: f64 = children.iter().map(|p| p.weight).sum();
                let fraction = 1.0 / (children.len() + 1) as f64;
                for child in children.iter_mut() {
                    child.weight = (1.0 - fraction) * child.weight / total;
                }
                panel.weight = fraction;
            }
            children.insert(at, panel);
        }
    }
}

/// The dock root's panels, making the root a column split first: an empty
/// dock gets one; an empty tab panel becomes one; a tab panel with tabs is
/// lifted into a new one.
fn dock_children(dock: &mut ArrangementDoc, split_id: String) -> &mut Vec<Panel> {
    let column = |id, weight, children| Panel {
        id,
        weight,
        node: PanelNode::Split {
            axis: Axis::Column,
            children,
        },
    };
    let root = match dock.root.take() {
        None => column(split_id, 1.0, Vec::new()),
        Some(Panel {
            id,
            weight,
            node: PanelNode::Tabs { tabs, .. },
        }) if tabs.is_empty() => column(id, weight, Vec::new()),
        Some(
            leaf @ Panel {
                node: PanelNode::Tabs { .. },
                ..
            },
        ) => {
            let weight = leaf.weight;
            column(
                split_id,
                weight,
                vec![Panel {
                    weight: 1.0,
                    ..leaf
                }],
            )
        }
        Some(split) => split,
    };
    match &mut dock.root.insert(root).node {
        PanelNode::Split { children, .. } => children,
        PanelNode::Tabs { .. } => unreachable!("the root is a split"),
    }
}

#[cfg(test)]
mod tests;
