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

/// Per-slot flags: [`SlotInfo::flags`], and the C ABI's
/// `ANDAMENTO_SLOT_FLAG_*` bits.
pub mod flag {
    /// The user overrode the slot's content: it no longer follows the
    /// provider. Reattaching drops the override.
    pub const DETACHED: u32 = 1;
    /// The provider's content for this key is not what the user's override
    /// or tombstone was made against. An override still wins; a tombstone no
    /// longer hides the slot. Setting the slot again, removing it again or
    /// reattaching it settles the flag.
    pub const CHANGED: u32 = 1 << 1;
    /// The provider removed this overridden slot; it is kept as the user's
    /// own until the host removes it.
    pub const REMOVED: u32 = 1 << 2;
    /// The user changed the slot's rebind policy. This does not detach it.
    pub const REBIND: u32 = 1 << 3;
}

/// The user's edit of one provider slot, keyed by slot key in the
/// [`EditSet`]. Empty edits are never stored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotEdit {
    /// An override or a tombstone, with the baseline content it was made
    /// against.
    pub content: Option<ContentEdit>,
    /// A rebind policy other than the baseline's.
    pub rebind: Option<RebindPolicy>,
}

impl SlotEdit {
    pub fn is_empty(&self) -> bool {
        self.content.is_none() && self.rebind.is_none()
    }

    fn overridden(&self) -> Option<&ViewSpec> {
        match &self.content {
            Some(ContentEdit {
                change: ContentChange::Override(spec),
                ..
            }) => Some(spec),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentEdit {
    pub change: ContentChange,
    /// The baseline content the edit was made against. When the provider's
    /// content for the key differs, the slot is flagged
    /// [`CHANGED`](flag::CHANGED).
    pub against: ViewSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentChange {
    /// The slot shows this instead, and is detached.
    Override(ViewSpec),
    /// The user removed the slot: it is hidden while the provider's content
    /// is still what it was removed against.
    Tombstone,
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
    /// [`flag`] bits.
    pub flags: u32,
}

/// A workspace's slot set: the cached baseline, the user's edits of it, and
/// the slots the user added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceSlots {
    pub baseline: Option<Baseline>,
    /// Edits of provider slots, by slot key: normalised, so an edit that
    /// equals the baseline is not kept.
    pub edits: BTreeMap<String, SlotEdit>,
    /// The user's own slots (`u:<id>`), in the order they were added.
    pub user: Vec<SlotDef>,
    /// The user's name for the workspace, over the provider's.
    pub name: Option<String>,
    /// The user's mood for the workspace, over the provider's.
    pub mood: Option<String>,
    /// Untouched slots the provider removed, with the rebind policy their
    /// live instance follows, until the host releases the instance.
    pub departed: BTreeMap<String, RebindPolicy>,
}

impl WorkspaceSlots {
    pub fn is_empty(&self) -> bool {
        self.baseline.is_none()
            && self.edits.is_empty()
            && self.user.is_empty()
            && self.name.is_none()
            && self.mood.is_none()
            && self.departed.is_empty()
    }

    /// Every live slot, in default placement order: the baseline's (less
    /// those the user removed), then overridden slots the baseline dropped,
    /// then the user's.
    pub fn slots(&self) -> Vec<SlotInfo> {
        let mut out = Vec::new();
        let baseline = self.baseline.iter().flat_map(|b| &b.slots);
        for slot in baseline {
            let edit = self.edits.get(&slot.key);
            let rebind_edit = edit.and_then(|edit| edit.rebind);
            let mut flags = if rebind_edit.is_some() {
                flag::REBIND
            } else {
                0
            };
            let mut spec = slot.spec.clone();
            if let Some(content) = edit.and_then(|edit| edit.content.as_ref()) {
                let changed = content.against != slot.spec;
                if changed {
                    flags |= flag::CHANGED;
                }
                match &content.change {
                    ContentChange::Tombstone if !changed => continue,
                    ContentChange::Tombstone => {}
                    ContentChange::Override(user) => {
                        flags |= flag::DETACHED;
                        spec = user.clone();
                    }
                }
            }
            out.push(SlotInfo {
                key: slot.key.clone(),
                spec,
                rebind: rebind_edit.unwrap_or(slot.rebind),
                in_baseline: true,
                detached: flags & flag::DETACHED != 0,
                flags,
            });
        }
        for (key, edit) in &self.edits {
            match edit.overridden() {
                Some(spec) if !self.in_baseline(key) => out.push(SlotInfo {
                    key: key.clone(),
                    spec: spec.clone(),
                    rebind: edit.rebind.unwrap_or_default(),
                    in_baseline: false,
                    detached: true,
                    flags: flag::DETACHED | flag::REMOVED,
                }),
                _ => {}
            }
        }
        for slot in &self.user {
            out.push(SlotInfo {
                key: slot.key.clone(),
                spec: slot.spec.clone(),
                rebind: slot.rebind,
                in_baseline: false,
                detached: false,
                flags: 0,
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

    /// Baseline slots the user removed, which are hidden.
    pub fn hidden(&self) -> BTreeSet<String> {
        let live = self.keys();
        self.baseline
            .iter()
            .flat_map(|b| &b.slots)
            .map(|slot| &slot.key)
            .filter(|key| !live.contains(*key))
            .cloned()
            .collect()
    }

    fn in_baseline(&self, key: &str) -> bool {
        self.baseline
            .as_ref()
            .is_some_and(|b| b.slot(key).is_some())
    }

    /// Store `edit` for `key`, or drop it when empty. Returns whether
    /// anything changed.
    fn put(&mut self, key: &str, edit: SlotEdit) -> bool {
        if edit.is_empty() {
            return self.edits.remove(key).is_some();
        }
        if self.edits.get(key) == Some(&edit) {
            return false;
        }
        self.edits.insert(key.to_owned(), edit);
        true
    }

    /// Add or replace a slot. A user key (`u:<id>`) adds the user's own
    /// slot. A provider key edits that slot: content other than the
    /// baseline's overrides it, detaching it, and is recorded against the
    /// baseline's current content; a policy other than the baseline's is a
    /// rebind edit, which doesn't detach it. Setting the baseline's own
    /// spec and policy drops both, restoring a removed slot. Returns whether
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
        if let Some(baseline) = self.baseline.as_ref().and_then(|b| b.slot(key)).cloned() {
            let edit = SlotEdit {
                content: (spec != baseline.spec).then(|| ContentEdit {
                    change: ContentChange::Override(spec),
                    against: baseline.spec.clone(),
                }),
                rebind: (rebind != baseline.rebind).then_some(rebind),
            };
            return Ok(self.put(key, edit));
        }
        // An overridden slot the baseline dropped: it keeps what it was
        // made against.
        let Some(against) = self
            .edits
            .get(key)
            .filter(|edit| edit.overridden().is_some())
            .and_then(|edit| edit.content.as_ref())
            .map(|content| content.against.clone())
        else {
            return Err(format!(
                "no slot {key:?}: add the user's own slots with keys \"{USER_PREFIX}<id>\""
            ));
        };
        let edit = SlotEdit {
            content: Some(ContentEdit {
                change: ContentChange::Override(spec),
                against,
            }),
            rebind: Some(rebind),
        };
        Ok(self.put(key, edit))
    }

    /// Remove a slot. The user's own slot, or an overridden slot the
    /// baseline dropped, goes. A baseline slot gets a tombstone recording
    /// the content it was removed against: it stays hidden while the
    /// provider's content is the same, and reappears flagged
    /// [`CHANGED`](flag::CHANGED) if the provider reuses the key for
    /// different content. Returns whether anything changed.
    pub fn remove(&mut self, key: &str) -> Result<bool, String> {
        if let Some(index) = self.user.iter().position(|slot| slot.key == key) {
            self.user.remove(index);
            return Ok(true);
        }
        if let Some(baseline) = self.baseline.as_ref().and_then(|b| b.slot(key)).cloned() {
            let edit = SlotEdit {
                content: Some(ContentEdit {
                    change: ContentChange::Tombstone,
                    against: baseline.spec,
                }),
                rebind: None,
            };
            return Ok(self.put(key, edit));
        }
        Ok(self.edits.remove(key).is_some())
    }

    /// Drop a baseline slot's override or tombstone, so it follows the
    /// provider again. A rebind edit stays. Returns whether there was one.
    pub fn reattach(&mut self, key: &str) -> bool {
        if !self.in_baseline(key) {
            return false;
        }
        let Some(mut edit) = self.edits.get(key).cloned() else {
            return false;
        };
        if edit.content.take().is_none() {
            return false;
        }
        self.put(key, edit)
    }

    /// Adopt a new baseline, applying the baseline-change rules (ADR 0013):
    /// a removed overridden slot is kept as the user's own (its rebind
    /// policy frozen); a removed untouched slot departs, keeping its rebind
    /// policy for its live instance; a removed tombstoned slot's tombstone
    /// is spent. Edits the new baseline equals drop out. Returns whether
    /// anything changed.
    pub fn set_baseline(&mut self, baseline: Baseline) -> bool {
        if self.baseline.as_ref() == Some(&baseline) {
            return false;
        }
        let old = self.baseline.replace(baseline);
        let new = self.baseline.as_ref().expect("set above");
        for slot in old.iter().flat_map(|b| &b.slots) {
            if new.slot(&slot.key).is_some() {
                continue;
            }
            let edit = self.edits.get(&slot.key).cloned().unwrap_or_default();
            if edit.overridden().is_some() {
                let mut kept = edit;
                kept.rebind = Some(kept.rebind.unwrap_or(slot.rebind));
                self.edits.insert(slot.key.clone(), kept);
                continue;
            }
            if edit.content.is_none() {
                self.departed
                    .insert(slot.key.clone(), edit.rebind.unwrap_or(slot.rebind));
            }
            self.edits.remove(&slot.key);
        }
        self.departed.retain(|key, _| new.slot(key).is_none());
        for (key, edit) in self.edits.iter_mut() {
            let Some(slot) = new.slot(key) else {
                continue;
            };
            if edit.overridden() == Some(&slot.spec) {
                edit.content = None;
            }
            if edit.rebind == Some(slot.rebind) {
                edit.rebind = None;
            }
        }
        let keep: Vec<String> = self
            .edits
            .iter()
            .filter(|(key, edit)| {
                !edit.is_empty() && (new.slot(key).is_some() || edit.overridden().is_some())
            })
            .map(|(key, _)| key.clone())
            .collect();
        self.edits.retain(|key, _| keep.contains(key));
        true
    }

    /// Forget a departed slot once the host has dealt with its live
    /// instance. Returns whether it had departed.
    pub fn release_departed(&mut self, key: &str) -> bool {
        self.departed.remove(key).is_some()
    }
}

/// Slot keys and panel IDs: what a Suggested Layout allows.
pub(crate) fn check_key(id: &str) -> Result<(), String> {
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

impl Panel {
    /// This panel and its descendants, in preorder.
    pub fn preorder(&self) -> Vec<&Panel> {
        fn walk<'a>(panel: &'a Panel, out: &mut Vec<&'a Panel>) {
            out.push(panel);
            if let PanelNode::Split { children, .. } = &panel.node {
                children.iter().for_each(|child| walk(child, out));
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out
    }

    /// Slot (or section) keys of this panel's tabs and its descendants', in
    /// preorder.
    pub fn tabs(&self) -> Vec<&str> {
        self.preorder()
            .into_iter()
            .flat_map(|panel| match &panel.node {
                PanelNode::Tabs { tabs, .. } => tabs.iter().map(String::as_str).collect(),
                PanelNode::Split { .. } => Vec::new(),
            })
            .collect()
    }
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
        self.root.iter().flat_map(Panel::preorder).collect()
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
        check_panels(self.panels(), false)
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

/// Shape rules for panels in preorder: IDs nonempty and unique, weights
/// finite and positive, splits nonempty, tabs nonempty, a selected tab one of
/// its panel's, and, unless `duplicate_tabs`, each key tabbed at most once.
pub(crate) fn check_panels(panels: Vec<&Panel>, duplicate_tabs: bool) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    let mut tabs = BTreeSet::new();
    for panel in panels {
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
                    if !tabs.insert(tab.as_str()) && !duplicate_tabs {
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

/// A soft override of one panel, keyed by panel ID: a split weight or the
/// Selected View. Soft overrides never take ownership of the arrangement,
/// so the provider's structure keeps flowing under them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoftOverride {
    pub weight: Option<f64>,
    pub selected: Option<String>,
}

impl SoftOverride {
    pub fn is_empty(&self) -> bool {
        self.weight.is_none() && self.selected.is_none()
    }
}

/// A workspace's stored arrangement.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoredArrangement {
    /// Nonzero once stored; it changes whenever the document does.
    pub generation: u64,
    /// The user made a structural edit (split, move, close, reorder, add), so
    /// the overlay owns the whole document. Until then it is derived from the
    /// baseline's hint, with the soft overrides applied.
    pub owned: bool,
    pub doc: ArrangementDoc,
    /// Tabs Andamento placed since the host last committed.
    pub placed: BTreeSet<String>,
    /// Soft overrides by panel ID, while not owned.
    pub soft: BTreeMap<String, SoftOverride>,
    /// The provider changed its arrangement hint while the user owned the
    /// document. It is not applied; the host keeps the user's arrangement or
    /// follows the provider's ([`StoredArrangement::resolve`]).
    pub provider_changed: bool,
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
    pub soft: BTreeMap<String, SoftOverride>,
    pub provider_changed: bool,
    /// Panel IDs of soft overrides that no longer resolve: the panel has
    /// gone, or the tab it selects is not in it. They are kept.
    pub unresolved: BTreeSet<String>,
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
    /// The commit was structural and the overlay owns the arrangement.
    pub owned: bool,
}

/// How to settle a provider change to an owned arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolve {
    /// Keep the user's arrangement; the flag clears.
    Keep,
    /// Drop the user's arrangement and soft overrides, and follow the
    /// provider's hint again.
    Follow,
}

/// The document with weights and selections cleared: what a structural edit
/// changes.
fn shape(doc: &ArrangementDoc) -> ArrangementDoc {
    fn walk(panel: &Panel) -> Panel {
        Panel {
            id: panel.id.clone(),
            weight: 1.0,
            node: match &panel.node {
                PanelNode::Split { axis, children } => PanelNode::Split {
                    axis: *axis,
                    children: children.iter().map(walk).collect(),
                },
                PanelNode::Tabs { tabs, .. } => PanelNode::Tabs {
                    tabs: tabs.clone(),
                    selected: None,
                },
            },
        }
    }
    ArrangementDoc {
        root: doc.root.as_ref().map(walk),
    }
}

impl ArrangementDoc {
    /// Remove the tabs of `keys`, then tab panels and splits the removal
    /// emptied. A selection of a removed tab moves to the panel's first.
    pub fn strip(&mut self, keys: &BTreeSet<String>) {
        fn walk(panel: &mut Panel, keys: &BTreeSet<String>) -> bool {
            match &mut panel.node {
                PanelNode::Tabs { tabs, selected } => {
                    let had = !tabs.is_empty();
                    tabs.retain(|tab| !keys.contains(tab));
                    if selected.as_ref().is_some_and(|tab| keys.contains(tab)) {
                        *selected = tabs.first().cloned();
                    }
                    !had || !tabs.is_empty()
                }
                PanelNode::Split { children, .. } => {
                    let had = !children.is_empty();
                    children.retain_mut(|child| walk(child, keys));
                    !had || !children.is_empty()
                }
            }
        }
        if keys.is_empty() {
            return;
        }
        if let Some(root) = &mut self.root {
            if !walk(root, keys) {
                self.root = None;
            }
        }
    }

    fn panel_mut(&mut self, id: &str) -> Option<&mut Panel> {
        fn walk<'a>(panel: &'a mut Panel, id: &str) -> Option<&'a mut Panel> {
            if panel.id == id {
                return Some(panel);
            }
            match &mut panel.node {
                PanelNode::Split { children, .. } => {
                    children.iter_mut().find_map(|child| walk(child, id))
                }
                PanelNode::Tabs { .. } => None,
            }
        }
        self.root.as_mut().and_then(|root| walk(root, id))
    }

    /// Apply soft overrides whose panel (and selected tab) resolve.
    fn apply_soft(&mut self, soft: &BTreeMap<String, SoftOverride>) {
        for (id, edit) in soft {
            let Some(panel) = self.panel_mut(id) else {
                continue;
            };
            if let Some(weight) = edit.weight {
                panel.weight = weight;
            }
            if let (Some(tab), PanelNode::Tabs { tabs, selected }) =
                (&edit.selected, &mut panel.node)
            {
                if tabs.contains(tab) {
                    *selected = Some(tab.clone());
                }
            }
        }
    }
}

impl StoredArrangement {
    /// The document the provider's baseline gives, before soft overrides:
    /// its hint less the slots the user removed, with every slot placed.
    fn derived(baseline: Option<&Baseline>, slots: &[SlotInfo]) -> (ArrangementDoc, Vec<String>) {
        let mut doc = baseline.and_then(|b| b.hint.clone()).unwrap_or_default();
        let live: BTreeSet<&str> = slots.iter().map(|slot| slot.key.as_str()).collect();
        let hidden: BTreeSet<String> = baseline
            .iter()
            .flat_map(|b| &b.slots)
            .map(|slot| slot.key.clone())
            .filter(|key| !live.contains(key.as_str()))
            .collect();
        doc.strip(&hidden);
        let order: Vec<String> = slots.iter().map(|slot| slot.key.clone()).collect();
        let placed = doc.place(&order);
        (doc, placed)
    }

    /// Validate `doc` against the slot set and store it if `expected` is the
    /// current generation, then reconcile it. Tabs may name current slots,
    /// or slots already tabbed in the stored document that have since gone.
    ///
    /// The commit is diffed against the stored document. If only weights
    /// and selections differ and the user doesn't own the arrangement yet,
    /// they become soft overrides (against the provider's document) and the
    /// provider's structure keeps flowing. Any structural difference (a
    /// split, move, close, reorder or added tab) makes the overlay own the
    /// whole document from now on.
    pub fn commit(
        &mut self,
        doc: ArrangementDoc,
        expected: u64,
        baseline: Option<&Baseline>,
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
        let soft_only = !self.owned && self.generation != 0 && shape(&doc) == shape(&self.doc);
        let mut next = Self {
            generation: self.generation,
            owned: true,
            doc,
            placed: BTreeSet::new(),
            soft: BTreeMap::new(),
            provider_changed: false,
        };
        if soft_only {
            next.owned = false;
            next.soft = self.soft.clone();
            let (base, _) = Self::derived(baseline, slots);
            for panel in next.doc.panels() {
                let Some(from) = base.panels().into_iter().find(|p| p.id == panel.id) else {
                    continue;
                };
                let mut edit = SoftOverride {
                    weight: (panel.weight != from.weight).then_some(panel.weight),
                    selected: None,
                };
                if let (
                    PanelNode::Tabs { selected, .. },
                    PanelNode::Tabs {
                        selected: provider, ..
                    },
                ) = (&panel.node, &from.node)
                {
                    if selected != provider {
                        edit.selected = selected.clone();
                    }
                }
                if edit.is_empty() {
                    next.soft.remove(&panel.id);
                } else {
                    next.soft.insert(panel.id.clone(), edit);
                }
            }
        }
        let placed = next.reconcile(slots);
        if next != *self {
            next.generation = next_generation();
            *self = next;
        }
        Ok(ArrangementCommit {
            generation: self.generation,
            placed,
            gone: self.gone(slots).into_iter().collect(),
            owned: self.owned,
        })
    }

    /// Place slots with no tab. Returns those placed.
    fn reconcile(&mut self, slots: &[SlotInfo]) -> Vec<String> {
        let order: Vec<String> = slots.iter().map(|slot| slot.key.clone()).collect();
        let placed = self.doc.place(&order);
        self.placed.extend(placed.iter().cloned());
        placed
    }

    /// Follow a changed slot set or baseline. An arrangement the user
    /// doesn't own is derived again: the baseline's hint less removed slots,
    /// every slot placed, soft overrides reapplied. An owned one only gets
    /// new slots placed. Returns whether the document changed.
    pub fn follow(
        &mut self,
        baseline: Option<&Baseline>,
        slots: &[SlotInfo],
        next_generation: &mut impl FnMut() -> u64,
    ) -> bool {
        let mut next = self.clone();
        if !next.owned {
            let (mut doc, placed) = Self::derived(baseline, slots);
            doc.apply_soft(&next.soft);
            next.doc = doc;
            next.placed = placed.into_iter().collect();
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

    /// The provider's arrangement hint changed. An owned arrangement is
    /// flagged rather than changed; returns whether the flag was raised.
    pub fn note_hint_change(&mut self) -> bool {
        if self.owned && !self.provider_changed {
            self.provider_changed = true;
            return true;
        }
        false
    }

    /// Keep the user's arrangement over a provider change, or follow the
    /// provider again, dropping ownership and soft overrides.
    pub fn resolve(
        &mut self,
        choice: Resolve,
        expected: u64,
        baseline: Option<&Baseline>,
        slots: &[SlotInfo],
        next_generation: &mut impl FnMut() -> u64,
    ) -> Result<u64, ArrangementError> {
        if expected != self.generation {
            return Err(ArrangementError::Stale {
                current: self.generation,
            });
        }
        match choice {
            Resolve::Keep => self.provider_changed = false,
            Resolve::Follow => {
                let mut next = Self {
                    generation: self.generation,
                    doc: self.doc.clone(),
                    ..Self::default()
                };
                next.follow(baseline, slots, &mut || 0);
                next.generation = self.generation;
                if next != *self {
                    next.generation = next_generation();
                    *self = next;
                }
            }
        }
        Ok(self.generation)
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

    /// Soft overrides whose panel, or selected tab, is no longer in the
    /// document.
    pub fn unresolved(&self) -> BTreeSet<String> {
        let panels = self.doc.panels();
        self.soft
            .iter()
            .filter(
                |(id, edit)| match panels.iter().find(|panel| panel.id == **id) {
                    None => true,
                    Some(panel) => match (&edit.selected, &panel.node) {
                        (Some(tab), PanelNode::Tabs { tabs, .. }) => !tabs.contains(tab),
                        (Some(_), PanelNode::Split { .. }) => true,
                        (None, _) => false,
                    },
                },
            )
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn view(&self, slots: &[SlotInfo]) -> ArrangementView {
        ArrangementView {
            generation: self.generation,
            owned: self.owned,
            doc: self.doc.clone(),
            placed: self.placed.clone(),
            gone: self.gone(slots),
            soft: self.soft.clone(),
            provider_changed: self.provider_changed,
            unresolved: self.unresolved(),
        }
    }
}

/// A workspace's edit set in the Overlay Sync proposal shape: the user's
/// edits, keyed by slot key and panel ID, and the baseline version they
/// apply to. It is normalised: an edit that equals the baseline is not in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditSet {
    /// The Suggested Layout version the edits were made against; `None`
    /// for a workspace with no Suggested Layout (an empty baseline).
    pub baseline: Option<BaselineVersion>,
    pub name: Option<String>,
    pub mood: Option<String>,
    /// Edits of provider slots: overrides, tombstones and rebind policies.
    pub slots: BTreeMap<String, SlotEdit>,
    /// The user's own slots, in the order they were added.
    pub added: Vec<SlotDef>,
    /// Soft overrides by panel ID.
    pub panels: BTreeMap<String, SoftOverride>,
    /// The whole arrangement, once a structural edit made the overlay own it.
    pub arrangement: Option<ArrangementDoc>,
}

impl EditSet {
    pub fn new(slots: &WorkspaceSlots, arrangement: Option<&StoredArrangement>) -> Self {
        Self {
            baseline: slots.baseline.as_ref().map(|b| b.version.clone()),
            name: slots.name.clone(),
            mood: slots.mood.clone(),
            slots: slots.edits.clone(),
            added: slots.user.clone(),
            panels: arrangement.map(|a| a.soft.clone()).unwrap_or_default(),
            arrangement: arrangement.filter(|a| a.owned).map(|a| a.doc.clone()),
        }
    }
}

/// A workspace's overlay as the host reads it: the edit set and what it
/// should know about it beyond each slot's [`flag`]s.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayView {
    pub edits: EditSet,
    /// The overlay owns the arrangement.
    pub owned: bool,
    /// Baseline slots the user removed, hidden while the provider's content
    /// is what they were removed against. Reattach one to show it again.
    pub tombstoned: BTreeSet<String>,
    /// Untouched slots the provider removed, with the rebind policy their
    /// live instance follows, until the host releases the instance.
    pub departed: BTreeMap<String, RebindPolicy>,
}

impl OverlayView {
    pub fn new(slots: &WorkspaceSlots, arrangement: Option<&StoredArrangement>) -> Self {
        Self {
            edits: EditSet::new(slots, arrangement),
            owned: arrangement.is_some_and(|a| a.owned),
            tombstoned: slots.hidden(),
            departed: slots.departed.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
