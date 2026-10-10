//! Typed reading of a provider's Suggested Layout from an entity's flat facts.
//! The schema is docs/sidebar-design/suggested-layouts.md. Parsing has no
//! effect on managed content, placement or snapshots yet.
//!
//! Two kinds of problem are distinguished. A malformed *spec* (slot list,
//! view specs, arrangement) is a [`LayoutError`] for the whole layout. An
//! incomplete *resolution* of a provider-facet slot is that slot's
//! [`SlotResolution::Unavailable`], matching managed primary content.
use crate::{EntityRef, MetadataValue};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The reserved slot key for the `workspace.primary.*` facts.
pub const PRIMARY_SLOT: &str = "primary";
const PRIMARY_FACET: &str = "primary";
const KEY_VERSION: &str = "layout.version";
const KEY_SLOTS: &str = "layout.slots";
const KEY_ROOT: &str = "layout.root";
const SLOT_PREFIX: &str = "layout.slot.";
const PANEL_PREFIX: &str = "layout.panel.";
const KEY_PRIMARY_STATE: &str = "workspace.primary.state";
const KEY_PRIMARY_TARGET: &str = "workspace.primary.target";
const KEY_PRIMARY_RECIPE: &str = "action.primary.recipe";
const KEY_PRIMARY_CWD: &str = "git.root";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestedLayout {
    pub version: BaselineVersion,
    /// In the provider's order, which is also the default placement order.
    pub slots: Vec<Slot>,
    /// `None` when the provider gave no arrangement hint.
    pub arrangement: Option<Arrangement>,
}

impl SuggestedLayout {
    pub fn slot(&self, key: &str) -> Option<&Slot> {
        self.slots.iter().find(|slot| slot.key == key)
    }
}

/// What a Workspace Overlay records its edits against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BaselineVersion {
    /// The opaque `layout.version` fact; compared only for equality.
    Published(String),
    /// Only today's primary-slot facts: one slot whose spec never changes.
    PrimaryOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub key: String,
    pub spec: ViewSpec,
    pub rebind: RebindPolicy,
    pub resolution: SlotResolution,
}

/// The logical definition of a View: its content and preferred presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewSpec {
    pub content: Content,
    /// Open vocabulary (`terminal`, `web`, `markdown`, ...); frontends pick a
    /// renderer they support and otherwise show a placeholder.
    pub presentation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// An aspect of a provider entity; the provider publishes its resolution.
    ProviderFacet { entity: EntityRef, facet: String },
    /// Belongs to the `local` provider and is resolved by the frontend.
    Local(LocalRecipe),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalRecipe {
    Command {
        line: CommandLine,
        cwd: Option<String>,
    },
    File {
        path: String,
    },
    Url {
        url: String,
    },
    Jackstay {
        launcher: String,
        endpoint: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandLine {
    /// Structured `argv.<n>` facts (andamento#134). Preferred when present.
    Argv(Vec<String>),
    /// A shell command line, the shape of `action.primary.recipe`.
    Shell(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RebindPolicy {
    #[default]
    Replace,
    KeepPrevious,
    Ask,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotResolution {
    /// What a frontend runs now. `target` identifies the backing instance and
    /// changes whenever it is replaced, even if the recipe text does not.
    Ready {
        target: Option<String>,
        recipe: LocalRecipe,
    },
    /// The provider explicitly suspends replacement.
    Held,
    /// Missing, malformed or incomplete resolution facts. Not deletion.
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrangement {
    pub root: String,
    pub panels: BTreeMap<String, Panel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    /// Relative to siblings in the parent split; ignored for the root.
    pub weight: u32,
    pub node: PanelNode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelNode {
    Split {
        axis: Axis,
        children: Vec<String>,
    },
    Tabs {
        slots: Vec<String>,
        selected: String,
    },
}

/// `Row` lays children left to right, `Column` top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Row,
    Column,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    MissingFact {
        key: String,
    },
    WrongType {
        key: String,
        expected: &'static str,
    },
    InvalidValue {
        key: String,
        value: String,
    },
    InvalidId {
        key: String,
        id: String,
    },
    DuplicateSlot {
        slot: String,
    },
    /// `layout.slot.primary.*`: the primary slot comes from `workspace.primary.*`.
    ReservedPrimary {
        key: String,
    },
    /// `layout.slots` names `primary` but `workspace.primary.state` is absent.
    MissingPrimary,
    /// A slot fact for a key `layout.slots` does not list.
    UnlistedSlot {
        key: String,
    },
    /// Only one of `entity` and `facet`.
    IncompleteFacet {
        slot: String,
    },
    /// `argv.<n>` keys that are not exactly `0..len`.
    ArgvGap {
        slot: String,
    },
    PanelShape {
        panel: String,
    },
    UnknownPanel {
        panel: String,
    },
    PanelReused {
        panel: String,
    },
    UnreachablePanel {
        panel: String,
    },
    UnknownSlot {
        panel: String,
        slot: String,
    },
    SlotInTwoPanels {
        slot: String,
    },
    SelectedNotInPanel {
        panel: String,
        slot: String,
    },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use LayoutError::*;
        match self {
            MissingFact { key } => write!(f, "missing {key}"),
            WrongType { key, expected } => write!(f, "{key} must be {expected}"),
            InvalidValue { key, value } => write!(f, "{key} has invalid value {value:?}"),
            InvalidId { key, id } => write!(f, "{key} has invalid id {id:?}"),
            DuplicateSlot { slot } => write!(f, "slot {slot} is listed twice"),
            ReservedPrimary { key } => {
                write!(f, "{key}: the primary slot comes from workspace.primary.*")
            }
            MissingPrimary => write!(
                f,
                "layout.slots lists primary without workspace.primary.state"
            ),
            UnlistedSlot { key } => write!(f, "{key} names a slot layout.slots does not list"),
            IncompleteFacet { slot } => write!(f, "slot {slot} needs both entity and facet"),
            ArgvGap { slot } => write!(f, "slot {slot} argv indices are not 0..n"),
            PanelShape { panel } => {
                write!(f, "panel {panel} needs either axis and children, or slots")
            }
            UnknownPanel { panel } => write!(f, "panel {panel} has no facts"),
            PanelReused { panel } => write!(f, "panel {panel} appears twice in the arrangement"),
            UnreachablePanel { panel } => {
                write!(f, "panel {panel} is not reachable from layout.root")
            }
            UnknownSlot { panel, slot } => write!(f, "panel {panel} holds unknown slot {slot}"),
            SlotInTwoPanels { slot } => write!(f, "slot {slot} is held by two panels"),
            SelectedNotInPanel { panel, slot } => {
                write!(f, "panel {panel} selects {slot}, which it does not hold")
            }
        }
    }
}

impl std::error::Error for LayoutError {}

/// Reads the Suggested Layout `entity` publishes, if any. `facts` are its
/// resolved, live facts. Returns `Ok(None)` when it publishes neither
/// `layout.*` nor `workspace.primary.state`.
pub fn parse(
    entity: &EntityRef,
    facts: &BTreeMap<String, MetadataValue>,
) -> Result<Option<SuggestedLayout>, LayoutError> {
    let primary = facts
        .contains_key(KEY_PRIMARY_STATE)
        .then(|| primary_slot(entity, facts));
    if !facts.keys().any(|key| key.starts_with("layout.")) {
        return Ok(primary.map(|slot| SuggestedLayout {
            version: BaselineVersion::PrimaryOnly,
            slots: vec![slot],
            arrangement: None,
        }));
    }
    let f = Facts(facts);
    let version = f.text(KEY_VERSION)?.filter(|v| !v.is_empty());
    let version = version.ok_or_else(|| missing(KEY_VERSION))?;
    let keys = f.list(KEY_SLOTS)?.ok_or_else(|| missing(KEY_SLOTS))?;
    let mut listed = BTreeSet::new();
    for key in keys {
        check_id(KEY_SLOTS, key)?;
        if !listed.insert(key.as_str()) {
            return Err(LayoutError::DuplicateSlot { slot: key.clone() });
        }
    }
    for fact in facts.keys() {
        if let Some((slot, _)) = fact
            .strip_prefix(SLOT_PREFIX)
            .and_then(|rest| rest.split_once('.'))
        {
            if slot == PRIMARY_SLOT {
                return Err(LayoutError::ReservedPrimary { key: fact.clone() });
            }
            if !listed.contains(slot) {
                return Err(LayoutError::UnlistedSlot { key: fact.clone() });
            }
        }
    }
    let mut primary = primary;
    if listed.contains(PRIMARY_SLOT) && primary.is_none() {
        return Err(LayoutError::MissingPrimary);
    }
    // An unlisted primary slot leads the slot order.
    let mut slots: Vec<Slot> = match primary.as_ref() {
        Some(_) if !listed.contains(PRIMARY_SLOT) => primary.take().into_iter().collect(),
        _ => Vec::new(),
    };
    for key in keys {
        slots.push(match key.as_str() {
            PRIMARY_SLOT => primary.take().expect("checked above"),
            _ => provider_slot(&f, key)?,
        });
    }
    let known: BTreeSet<&str> = slots.iter().map(|s| s.key.as_str()).collect();
    let arrangement = arrangement(&f, &known)?;
    Ok(Some(SuggestedLayout {
        version: BaselineVersion::Published(version.to_owned()),
        slots,
        arrangement,
    }))
}

/// The single-slot special case, read exactly as managed primary content does.
fn primary_slot(entity: &EntityRef, facts: &BTreeMap<String, MetadataValue>) -> Slot {
    let text = |key| match facts.get(key) {
        Some(MetadataValue::Text(value)) => Some(value.clone()),
        _ => None,
    };
    let resolution = match text(KEY_PRIMARY_STATE).as_deref() {
        Some("held") => SlotResolution::Held,
        Some("ready") => match (
            text(KEY_PRIMARY_TARGET).filter(|s| !s.is_empty()),
            text(KEY_PRIMARY_RECIPE).filter(|s| !s.is_empty()),
        ) {
            (Some(target), Some(command)) => SlotResolution::Ready {
                target: Some(target),
                recipe: LocalRecipe::Command {
                    line: CommandLine::Shell(command),
                    cwd: text(KEY_PRIMARY_CWD),
                },
            },
            _ => SlotResolution::Unavailable,
        },
        _ => SlotResolution::Unavailable,
    };
    Slot {
        key: PRIMARY_SLOT.to_owned(),
        spec: ViewSpec {
            content: Content::ProviderFacet {
                entity: entity.clone(),
                facet: PRIMARY_FACET.to_owned(),
            },
            presentation: None,
        },
        rebind: RebindPolicy::Replace,
        resolution,
    }
}

fn provider_slot(f: &Facts, key: &str) -> Result<Slot, LayoutError> {
    let at = |field: &str| format!("{SLOT_PREFIX}{key}.{field}");
    let presentation = f.text(&at("presentation"))?.map(str::to_owned);
    let rebind = match f.text(&at("rebind"))? {
        None | Some("replace") => RebindPolicy::Replace,
        Some("keep-previous") => RebindPolicy::KeepPrevious,
        Some("ask") => RebindPolicy::Ask,
        Some(other) => return Err(invalid(&at("rebind"), other)),
    };
    let entity = match f.0.get(&at("entity")) {
        None => None,
        Some(MetadataValue::EntityRefs(refs)) if refs.len() == 1 => Some(refs[0].clone()),
        Some(_) => return Err(wrong(&at("entity"), "exactly one entity ref")),
    };
    let facet = f.text(&at("facet"))?.filter(|s| !s.is_empty());
    let state = f.text(&at("state"));
    let (content, resolution) = match (entity, facet) {
        (Some(entity), Some(facet)) => {
            // Resolution facts never invalidate the layout; they make the slot
            // unavailable, as incomplete workspace.primary.* facts do.
            let resolution = match state.clone().ok().flatten() {
                Some("held") => SlotResolution::Held,
                Some("ready") => {
                    let target = f.text(&at("target")).ok().flatten();
                    match (target.filter(|s| !s.is_empty()), recipe(f, key)) {
                        (Some(target), Ok(recipe)) => SlotResolution::Ready {
                            target: Some(target.to_owned()),
                            recipe,
                        },
                        _ => SlotResolution::Unavailable,
                    }
                }
                _ => SlotResolution::Unavailable,
            };
            let facet = facet.to_owned();
            (Content::ProviderFacet { entity, facet }, resolution)
        }
        (None, None) => {
            let recipe = recipe(f, key)?;
            let resolution = match state? {
                None | Some("ready") => SlotResolution::Ready {
                    target: f.text(&at("target"))?.map(str::to_owned),
                    recipe: recipe.clone(),
                },
                Some("held") => SlotResolution::Held,
                Some(other) => return Err(invalid(&at("state"), other)),
            };
            (Content::Local(recipe), resolution)
        }
        _ => {
            return Err(LayoutError::IncompleteFacet {
                slot: key.to_owned(),
            })
        }
    };
    Ok(Slot {
        key: key.to_owned(),
        spec: ViewSpec {
            content,
            presentation,
        },
        rebind,
        resolution,
    })
}

fn recipe(f: &Facts, key: &str) -> Result<LocalRecipe, LayoutError> {
    let at = |field: &str| format!("{SLOT_PREFIX}{key}.{field}");
    let required = |field: &str| -> Result<String, LayoutError> {
        let key = at(field);
        match f.text(&key)? {
            Some(value) if !value.is_empty() => Ok(value.to_owned()),
            _ => Err(missing(&key)),
        }
    };
    let kind = required("kind")?;
    Ok(match kind.as_str() {
        "command" => {
            let argv = argv(f, key)?;
            let line = match (argv.is_empty(), f.text(&at("command"))?) {
                (false, _) => CommandLine::Argv(argv),
                (true, Some(shell)) if !shell.is_empty() => CommandLine::Shell(shell.to_owned()),
                _ => return Err(missing(&at("argv.0"))),
            };
            LocalRecipe::Command {
                line,
                cwd: f.text(&at("cwd"))?.map(str::to_owned),
            }
        }
        "file" => LocalRecipe::File {
            path: required("path")?,
        },
        "url" => LocalRecipe::Url {
            url: required("url")?,
        },
        "jackstay" => LocalRecipe::Jackstay {
            launcher: required("launcher")?,
            endpoint: required("endpoint")?,
        },
        other => return Err(invalid(&at("kind"), other)),
    })
}

fn argv(f: &Facts, key: &str) -> Result<Vec<String>, LayoutError> {
    let prefix = format!("{SLOT_PREFIX}{key}.argv.");
    let mut indexed = BTreeMap::new();
    for (fact, value) in f.0.range(prefix.clone()..) {
        let Some(index) = fact.strip_prefix(&prefix) else {
            break;
        };
        let n: usize = index
            .parse()
            .ok()
            .filter(|n: &usize| n.to_string() == index)
            .ok_or_else(|| LayoutError::ArgvGap {
                slot: key.to_owned(),
            })?;
        let MetadataValue::Text(arg) = value else {
            return Err(wrong(fact, "text"));
        };
        indexed.insert(n, arg.clone());
    }
    if indexed.keys().copied().ne(0..indexed.len()) {
        return Err(LayoutError::ArgvGap {
            slot: key.to_owned(),
        });
    }
    Ok(indexed.into_values().collect())
}

fn arrangement(f: &Facts, known: &BTreeSet<&str>) -> Result<Option<Arrangement>, LayoutError> {
    let mut declared = BTreeSet::new();
    for fact in f.0.keys() {
        if let Some((panel, _)) = fact
            .strip_prefix(PANEL_PREFIX)
            .and_then(|rest| rest.split_once('.'))
        {
            check_id(fact, panel)?;
            declared.insert(panel.to_owned());
        }
    }
    let Some(root) = f.text(KEY_ROOT)? else {
        return match declared.into_iter().next() {
            Some(_) => Err(missing(KEY_ROOT)),
            None => Ok(None),
        };
    };
    check_id(KEY_ROOT, root)?;
    let mut panels = BTreeMap::new();
    let mut placed = BTreeSet::new();
    let mut pending = vec![root.to_owned()];
    while let Some(id) = pending.pop() {
        if panels.contains_key(&id) {
            return Err(LayoutError::PanelReused { panel: id });
        }
        if !declared.contains(&id) {
            return Err(LayoutError::UnknownPanel { panel: id });
        }
        let at = |field: &str| format!("{PANEL_PREFIX}{id}.{field}");
        let weight = match f.0.get(&at("weight")) {
            None => 1,
            Some(MetadataValue::Integer(w)) => u32::try_from(*w)
                .ok()
                .filter(|w| *w > 0)
                .ok_or_else(|| invalid(&at("weight"), &w.to_string()))?,
            Some(_) => return Err(wrong(&at("weight"), "a positive integer")),
        };
        let shape = (
            f.text(&at("axis"))?,
            f.list(&at("children"))?,
            f.list(&at("slots"))?,
        );
        let node = match shape {
            (Some(axis), Some(children), None) if !children.is_empty() => {
                let axis = match axis {
                    "row" => Axis::Row,
                    "column" => Axis::Column,
                    other => return Err(invalid(&at("axis"), other)),
                };
                for child in children {
                    check_id(&at("children"), child)?;
                }
                pending.extend(children.iter().rev().cloned());
                if f.0.contains_key(&at("selected")) {
                    return Err(LayoutError::PanelShape { panel: id });
                }
                PanelNode::Split {
                    axis,
                    children: children.clone(),
                }
            }
            (None, None, Some(slots)) if !slots.is_empty() => {
                for slot in slots {
                    if !known.contains(slot.as_str()) {
                        return Err(LayoutError::UnknownSlot {
                            panel: id,
                            slot: slot.clone(),
                        });
                    }
                    if !placed.insert(slot.clone()) {
                        return Err(LayoutError::SlotInTwoPanels { slot: slot.clone() });
                    }
                }
                let selected = f.text(&at("selected"))?.unwrap_or(&slots[0]).to_owned();
                if !slots.contains(&selected) {
                    return Err(LayoutError::SelectedNotInPanel {
                        panel: id,
                        slot: selected,
                    });
                }
                PanelNode::Tabs {
                    slots: slots.clone(),
                    selected,
                }
            }
            _ => return Err(LayoutError::PanelShape { panel: id }),
        };
        panels.insert(id, Panel { weight, node });
    }
    if let Some(panel) = declared.into_iter().find(|id| !panels.contains_key(id)) {
        return Err(LayoutError::UnreachablePanel { panel });
    }
    Ok(Some(Arrangement {
        root: root.to_owned(),
        panels,
    }))
}

struct Facts<'a>(&'a BTreeMap<String, MetadataValue>);

impl Facts<'_> {
    fn text(&self, key: &str) -> Result<Option<&str>, LayoutError> {
        match self.0.get(key) {
            None => Ok(None),
            Some(MetadataValue::Text(value)) => Ok(Some(value)),
            Some(_) => Err(wrong(key, "text")),
        }
    }
    fn list(&self, key: &str) -> Result<Option<&Vec<String>>, LayoutError> {
        match self.0.get(key) {
            None => Ok(None),
            Some(MetadataValue::StringList(values)) => Ok(Some(values)),
            Some(_) => Err(wrong(key, "a string list")),
        }
    }
}

/// Slot keys and panel IDs are embedded in fact keys, so they exclude `.`;
/// user-added slots (`u:3`) can never collide because they contain `:`.
fn check_id(key: &str, id: &str) -> Result<(), LayoutError> {
    let mut chars = id.chars();
    let valid = id.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(LayoutError::InvalidId {
            key: key.to_owned(),
            id: id.to_owned(),
        })
    }
}

fn missing(key: &str) -> LayoutError {
    LayoutError::MissingFact {
        key: key.to_owned(),
    }
}
fn wrong(key: &str, expected: &'static str) -> LayoutError {
    LayoutError::WrongType {
        key: key.to_owned(),
        expected,
    }
}
fn invalid(key: &str, value: &str) -> LayoutError {
    LayoutError::InvalidValue {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests;
