//! A workspace's Slots and its arrangement document (Wheelhouse ADR 0012 and
//! 0013; docs/sidebar-design/slots-and-arrangements.md).
//!
//! A workspace's slots are the slots of its subject's Suggested Layout, kept
//! as a cached baseline, plus the slots the host adds in the user namespace
//! (`u:3`). A baseline slot the user overrides is detached: it shows the
//! override and no longer follows the provider. Its arrangement is one typed
//! document the host commits whole, at an expected generation.
use crate::suggested_layout::{
    self, Axis, BaselineVersion, Content, RebindPolicy, SuggestedLayout, ViewSpec,
};
use std::collections::{BTreeMap, BTreeSet};

/// The prefix of slot keys the host adds. Provider keys never contain `:`.
pub const USER_PREFIX: &str = "u:";

/// The cached copy of the Suggested Layout a workspace's slots follow.
#[derive(Clone, Debug, PartialEq)]
pub struct Baseline {
    pub version: BaselineVersion,
    /// In the provider's order.
    pub slots: Vec<SlotDef>,
    /// The provider's arrangement hint, if any.
    pub hint: Option<ArrangementDoc>,
}

impl Baseline {
    pub fn from_layout(layout: &SuggestedLayout) -> Self {
        Self {
            version: layout.version.clone(),
            slots: layout
                .slots
                .iter()
                .map(|slot| SlotDef {
                    key: slot.key.clone(),
                    spec: slot.spec.clone(),
                    rebind: slot.rebind,
                })
                .collect(),
            hint: layout.arrangement.as_ref().map(ArrangementDoc::from_hint),
        }
    }

    pub fn slot(&self, key: &str) -> Option<&SlotDef> {
        self.slots.iter().find(|slot| slot.key == key)
    }
}

/// A slot's definition: its key, View Spec and rebind policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotDef {
    pub key: String,
    pub spec: ViewSpec,
    pub rebind: RebindPolicy,
}

/// The user's override of a baseline slot, which detaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotOverride {
    pub spec: ViewSpec,
    pub rebind: RebindPolicy,
    /// The baseline spec the override was made against.
    pub against: ViewSpec,
}

/// One of a workspace's slots, as the host sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    pub key: String,
    pub spec: ViewSpec,
    pub rebind: RebindPolicy,
    /// The current baseline has this key.
    pub in_baseline: bool,
    /// The user overrode the baseline's content: the slot no longer follows
    /// the provider. A detached slot the baseline has since dropped is kept
    /// as the user's own.
    pub detached: bool,
}

/// A workspace's slot set: the baseline, the user's overrides of it, and
/// the slots the user added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceSlots {
    pub baseline: Option<Baseline>,
    pub overrides: BTreeMap<String, SlotOverride>,
    /// In the order they were added.
    pub user: Vec<SlotDef>,
}

impl WorkspaceSlots {
    pub fn is_empty(&self) -> bool {
        self.baseline.is_none() && self.overrides.is_empty() && self.user.is_empty()
    }

    /// Every slot, in default placement order: the baseline's, then detached
    /// slots the baseline dropped, then the user's.
    pub fn slots(&self) -> Vec<SlotInfo> {
        let mut out = Vec::new();
        let baseline = self.baseline.iter().flat_map(|b| &b.slots);
        for slot in baseline {
            out.push(match self.overrides.get(&slot.key) {
                Some(edit) => SlotInfo {
                    key: slot.key.clone(),
                    spec: edit.spec.clone(),
                    rebind: edit.rebind,
                    in_baseline: true,
                    detached: true,
                },
                None => SlotInfo {
                    key: slot.key.clone(),
                    spec: slot.spec.clone(),
                    rebind: slot.rebind,
                    in_baseline: true,
                    detached: false,
                },
            });
        }
        for (key, edit) in &self.overrides {
            if !self.in_baseline(key) {
                out.push(SlotInfo {
                    key: key.clone(),
                    spec: edit.spec.clone(),
                    rebind: edit.rebind,
                    in_baseline: false,
                    detached: true,
                });
            }
        }
        for slot in &self.user {
            out.push(SlotInfo {
                key: slot.key.clone(),
                spec: slot.spec.clone(),
                rebind: slot.rebind,
                in_baseline: false,
                detached: false,
            });
        }
        out
    }

    pub fn slot(&self, key: &str) -> Option<SlotInfo> {
        self.slots().into_iter().find(|slot| slot.key == key)
    }

    pub fn keys(&self) -> BTreeSet<String> {
        self.slots().into_iter().map(|slot| slot.key).collect()
    }

    fn in_baseline(&self, key: &str) -> bool {
        self.baseline
            .as_ref()
            .is_some_and(|b| b.slot(key).is_some())
    }

    /// Add or replace a slot. A user key (`u:<id>`) adds the user's own
    /// slot; a baseline key overrides it, detaching it, unless the spec and
    /// policy are the baseline's, which reattaches it. Returns whether
    /// anything changed.
    pub fn set(&mut self, key: &str, spec: ViewSpec, rebind: RebindPolicy) -> Result<bool, String> {
        check_spec(&spec)?;
        if let Some(id) = key.strip_prefix(USER_PREFIX) {
            check_key(id).map_err(|_| format!("invalid user slot key {key:?}"))?;
            let slot = SlotDef {
                key: key.to_owned(),
                spec,
                rebind,
            };
            return Ok(match self.user.iter_mut().find(|s| s.key == key) {
                Some(existing) if *existing == slot => false,
                Some(existing) => {
                    *existing = slot;
                    true
                }
                None => {
                    self.user.push(slot);
                    true
                }
            });
        }
        let baseline = self.baseline.as_ref().and_then(|b| b.slot(key)).cloned();
        let Some(against) = baseline
            .as_ref()
            .map(|slot| slot.spec.clone())
            .or_else(|| self.overrides.get(key).map(|edit| edit.against.clone()))
        else {
            return Err(format!(
                "no slot {key:?}: add the user's own slots with keys \"{USER_PREFIX}<id>\""
            ));
        };
        if baseline.is_some_and(|slot| slot.spec == spec && slot.rebind == rebind) {
            return Ok(self.overrides.remove(key).is_some());
        }
        let edit = SlotOverride {
            spec,
            rebind,
            against,
        };
        if self.overrides.get(key) == Some(&edit) {
            return Ok(false);
        }
        self.overrides.insert(key.to_owned(), edit);
        Ok(true)
    }

    /// Remove the user's own slot, or a detached slot the baseline dropped.
    /// A baseline slot can't be removed (the provider removes it); reattach
    /// it instead. Returns whether there was one.
    pub fn remove(&mut self, key: &str) -> Result<bool, String> {
        if let Some(index) = self.user.iter().position(|slot| slot.key == key) {
            self.user.remove(index);
            return Ok(true);
        }
        if self.in_baseline(key) {
            return Err(format!(
                "slot {key:?} is the provider's; reattach it rather than remove it"
            ));
        }
        Ok(self.overrides.remove(key).is_some())
    }

    /// Drop a baseline slot's override, so it follows the provider again.
    /// Returns whether it was detached.
    pub fn reattach(&mut self, key: &str) -> bool {
        self.in_baseline(key) && self.overrides.remove(key).is_some()
    }

    /// Adopt a new baseline. Returns whether it changed.
    pub fn set_baseline(&mut self, baseline: Baseline) -> bool {
        if self.baseline.as_ref() == Some(&baseline) {
            return false;
        }
        self.baseline = Some(baseline);
        true
    }
}

/// Slot keys and panel IDs: what a Suggested Layout allows.
fn check_key(id: &str) -> Result<(), String> {
    let mut chars = id.chars();
    let valid = id.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!("invalid key {id:?}"))
    }
}

fn check_spec(spec: &ViewSpec) -> Result<(), String> {
    use suggested_layout::{CommandLine, LocalRecipe};
    let empty = |what: &str| Err(format!("a view spec needs {what}"));
    match &spec.content {
        Content::ProviderFacet { entity, facet } => {
            if entity.kind.is_empty() || entity.id.is_empty() {
                return empty("an entity kind and ID");
            }
            if facet.is_empty() {
                return empty("a facet");
            }
        }
        Content::Local(LocalRecipe::Command { line, .. }) => match line {
            CommandLine::Shell(shell) if shell.is_empty() => return empty("a command"),
            CommandLine::Argv(argv) if argv.is_empty() => return empty("a command"),
            _ => {}
        },
        Content::Local(LocalRecipe::File { path }) if path.is_empty() => return empty("a path"),
        Content::Local(LocalRecipe::Url { url }) if url.is_empty() => return empty("a URL"),
        Content::Local(LocalRecipe::Jackstay { launcher, endpoint })
            if launcher.is_empty() || endpoint.is_empty() =>
        {
            return empty("a launcher and an endpoint")
        }
        Content::Local(_) => {}
    }
    Ok(())
}

/// A whole arrangement: a tree of panels with stable IDs. Leaves are tab
/// panels holding slot keys; inner panels split along an axis.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrangementDoc {
    /// `None`: no panels.
    pub root: Option<Panel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    /// Stable within its arrangement; nonempty and unique.
    pub id: String,
    /// Relative to its siblings: finite and positive. The root's is kept but
    /// means nothing.
    pub weight: f64,
    pub node: PanelNode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PanelNode {
    Split {
        axis: Axis,
        children: Vec<Panel>,
    },
    Tabs {
        /// Slot keys, in tab order.
        tabs: Vec<String>,
        /// The Selected View: one of `tabs`, or none.
        selected: Option<String>,
    },
}

impl ArrangementDoc {
    /// A Suggested Layout's arrangement hint as a document.
    pub fn from_hint(hint: &suggested_layout::Arrangement) -> Self {
        fn panel(hint: &suggested_layout::Arrangement, id: &str) -> Panel {
            let source = &hint.panels[id];
            Panel {
                id: id.to_owned(),
                weight: f64::from(source.weight),
                node: match &source.node {
                    suggested_layout::PanelNode::Split { axis, children } => PanelNode::Split {
                        axis: *axis,
                        children: children.iter().map(|child| panel(hint, child)).collect(),
                    },
                    suggested_layout::PanelNode::Tabs { slots, selected } => PanelNode::Tabs {
                        tabs: slots.clone(),
                        selected: Some(selected.clone()),
                    },
                },
            }
        }
        Self {
            root: Some(panel(hint, &hint.root)),
        }
    }

    /// Panels in preorder.
    pub fn panels(&self) -> Vec<&Panel> {
        fn walk<'a>(panel: &'a Panel, out: &mut Vec<&'a Panel>) {
            out.push(panel);
            if let PanelNode::Split { children, .. } = &panel.node {
                children.iter().for_each(|child| walk(child, out));
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            walk(root, &mut out);
        }
        out
    }

    /// Every tab's slot key, in preorder.
    pub fn tabs(&self) -> Vec<&str> {
        self.panels()
            .into_iter()
            .flat_map(|panel| match &panel.node {
                PanelNode::Tabs { tabs, .. } => tabs.iter().map(String::as_str).collect(),
                PanelNode::Split { .. } => Vec::new(),
            })
            .collect()
    }

    /// The document's shape rules, independent of any slot set.
    pub fn check(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mut tabs = BTreeSet::new();
        for panel in self.panels() {
            if panel.id.is_empty() {
                return Err("a panel needs an ID".into());
            }
            if !ids.insert(panel.id.as_str()) {
                return Err(format!("panel ID {:?} is used twice", panel.id));
            }
            if !(panel.weight.is_finite() && panel.weight > 0.0) {
                return Err(format!(
                    "panel {:?} has weight {}; weights are positive",
                    panel.id, panel.weight
                ));
            }
            match &panel.node {
                PanelNode::Split { children, .. } if children.is_empty() => {
                    return Err(format!("split {:?} has no panels", panel.id));
                }
                PanelNode::Split { .. } => {}
                PanelNode::Tabs {
                    tabs: panel_tabs,
                    selected,
                } => {
                    for tab in panel_tabs {
                        if tab.is_empty() {
                            return Err(format!("panel {:?} has a tab with no slot", panel.id));
                        }
                        if !tabs.insert(tab.as_str()) {
                            return Err(format!("slot {tab:?} has two tabs"));
                        }
                    }
                    if let Some(selected) = selected {
                        if !panel_tabs.contains(selected) {
                            return Err(format!(
                                "panel {:?} selects {selected:?}, which is not one of its tabs",
                                panel.id
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Place every slot in `order` that has no tab by the default rule:
    /// append it to the first tab panel in preorder, selecting it there if
    /// that panel selects nothing. With no panels, a tab panel is made for
    /// them. Returns the slots placed.
    pub fn place(&mut self, order: &[String]) -> Vec<String> {
        let present: BTreeSet<String> = self.tabs().into_iter().map(str::to_owned).collect();
        let missing: Vec<String> = order
            .iter()
            .filter(|key| !present.contains(*key))
            .cloned()
            .collect();
        if missing.is_empty() {
            return missing;
        }
        if self.root.is_none() {
            self.root = Some(Panel {
                id: "1".into(),
                weight: 1.0,
                node: PanelNode::Tabs {
                    tabs: Vec::new(),
                    selected: None,
                },
            });
        }
        fn first_tabs(panel: &mut Panel) -> Option<(&mut Vec<String>, &mut Option<String>)> {
            match &mut panel.node {
                PanelNode::Tabs { tabs, selected } => Some((tabs, selected)),
                PanelNode::Split { children, .. } => children.iter_mut().find_map(first_tabs),
            }
        }
        let (tabs, selected) =
            first_tabs(self.root.as_mut().expect("made above")).expect("every leaf is a tab panel");
        tabs.extend(missing.iter().cloned());
        if selected.is_none() {
            *selected = missing.first().cloned();
        }
        missing
    }
}

/// A workspace's stored arrangement.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoredArrangement {
    /// Nonzero once stored; it changes whenever the document does.
    pub generation: u64,
    /// The host committed it. Until then it follows the baseline's hint.
    pub owned: bool,
    pub doc: ArrangementDoc,
    /// Tabs Andamento placed since the host last committed.
    pub placed: BTreeSet<String>,
}

/// An arrangement as the host reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrangementView {
    /// 0 when nothing is stored yet.
    pub generation: u64,
    pub owned: bool,
    pub doc: ArrangementDoc,
    /// Tabs Andamento placed since the host last committed.
    pub placed: BTreeSet<String>,
    /// Tabs whose slot has gone. They stay until the host removes them.
    pub gone: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArrangementError {
    /// The expected generation is not the current one; nothing changed.
    Stale {
        current: u64,
    },
    Invalid(String),
}

impl std::fmt::Display for ArrangementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale { current } => write!(
                f,
                "the arrangement changed: its generation is {current}; read it again"
            ),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

/// What a commit did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementCommit {
    pub generation: u64,
    /// Slots that had no tab, which reconciling placed.
    pub placed: Vec<String>,
    /// Tabs whose slot has gone.
    pub gone: Vec<String>,
}

impl StoredArrangement {
    /// Validate `doc` against the slot set and store it if `expected` is the
    /// current generation, then reconcile it. Tabs may name current slots,
    /// or slots already tabbed in the stored document that have since gone.
    pub fn commit(
        &mut self,
        doc: ArrangementDoc,
        expected: u64,
        slots: &[SlotInfo],
        next_generation: &mut impl FnMut() -> u64,
    ) -> Result<ArrangementCommit, ArrangementError> {
        if expected != self.generation {
            return Err(ArrangementError::Stale {
                current: self.generation,
            });
        }
        doc.check().map_err(ArrangementError::Invalid)?;
        let known: BTreeSet<&str> = slots.iter().map(|slot| slot.key.as_str()).collect();
        let stored: BTreeSet<&str> = self.doc.tabs().into_iter().collect();
        if let Some(tab) = doc
            .tabs()
            .into_iter()
            .find(|tab| !known.contains(tab) && !stored.contains(tab))
        {
            return Err(ArrangementError::Invalid(format!(
                "no slot {tab:?}: add a slot before giving it a tab"
            )));
        }
        let mut next = Self {
            generation: self.generation,
            owned: true,
            doc,
            placed: BTreeSet::new(),
        };
        let placed = next.reconcile(slots);
        if next != *self {
            next.generation = next_generation();
            *self = next;
        }
        Ok(ArrangementCommit {
            generation: self.generation,
            placed,
            gone: self.gone(slots).into_iter().collect(),
        })
    }

    /// Place slots with no tab. Returns those placed.
    fn reconcile(&mut self, slots: &[SlotInfo]) -> Vec<String> {
        let order: Vec<String> = slots.iter().map(|slot| slot.key.clone()).collect();
        let placed = self.doc.place(&order);
        self.placed.extend(placed.iter().cloned());
        placed
    }

    /// Follow a changed slot set or baseline: an arrangement the host has
    /// not committed is the baseline's hint again; either way slots with no
    /// tab are placed. Returns whether the document changed.
    pub fn follow(
        &mut self,
        baseline: Option<&Baseline>,
        slots: &[SlotInfo],
        next_generation: &mut impl FnMut() -> u64,
    ) -> bool {
        let mut next = self.clone();
        if !next.owned {
            next.doc = baseline.and_then(|b| b.hint.clone()).unwrap_or_default();
            next.placed.clear();
        }
        next.reconcile(slots);
        // Placed marks only tabs that still exist.
        let tabs: BTreeSet<String> = next.doc.tabs().into_iter().map(str::to_owned).collect();
        next.placed.retain(|tab| tabs.contains(tab));
        if next == *self && self.generation != 0 {
            return false;
        }
        next.generation = next_generation();
        *self = next;
        true
    }

    pub fn gone(&self, slots: &[SlotInfo]) -> BTreeSet<String> {
        let known: BTreeSet<&str> = slots.iter().map(|slot| slot.key.as_str()).collect();
        self.doc
            .tabs()
            .into_iter()
            .filter(|tab| !known.contains(tab))
            .map(str::to_owned)
            .collect()
    }

    pub fn view(&self, slots: &[SlotInfo]) -> ArrangementView {
        ArrangementView {
            generation: self.generation,
            owned: self.owned,
            doc: self.doc.clone(),
            placed: self.placed.clone(),
            gone: self.gone(slots),
        }
    }
}

#[cfg(test)]
mod tests;
