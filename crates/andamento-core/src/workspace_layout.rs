//! Workspace Layouts as the sidebar configuration declares them: a subject
//! kind's template slots and its named layouts, each a panel tree whose tab
//! panels hold Slots by first-match matchers
//! (docs/sidebar-design/workspace-layouts.md).
//!
//! This module reads and validates the definitions, routes a workspace's
//! Slots through a layout's matchers and evaluates the layout into an
//! arrangement document. Keeping that arrangement, with the user's edits, is
//! [`crate::slots`]' business.
use std::collections::{BTreeMap, BTreeSet};

use kdl::KdlNode;
use serde::{Deserialize, Serialize};

use crate::slots::{ArrangementDoc, Panel, PanelNode};
use crate::suggested_layout::{
    invalid_cleat_field, Axis, CleatTransport, CommandLine, Content, LocalRecipe, RebindPolicy, Slot, SlotResolution, ViewSpec,
};
use crate::template_config::{TemplateConfigError, VisibilityPredicate, WorkspaceDefinition};
use crate::{EntityRef, FactMap, MetadataValue};

/// The prefix of a template slot's key (`t:files`). Provider keys never
/// contain `:`, and the user's start `u:`.
pub const TEMPLATE_PREFIX: &str = "t:";

/// A slot the template declares, built from the subject's facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateSlot {
    /// 1 to 64 of `[a-z0-9_-]`; its key is `t:<name>`.
    pub name: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub presentation: Option<String>,
    pub recipe: RecipeTemplate,
}

impl TemplateSlot {
    pub fn key(&self) -> String {
        format!("{TEMPLATE_PREFIX}{}", self.name)
    }
}

/// A local recipe whose text values may name subject facts in braces,
/// `{git.root}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum RecipeTemplate {
    Command {
        #[serde(default)]
        argv: Vec<String>,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
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
    /// `transport` is `local` or `ssh` once substituted.
    CleatSession {
        transport: String,
        host: String,
        runtime_root: String,
        daemon: String,
        session: String,
    },
}

/// One named layout of a subject kind's Slots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LayoutDefinition {
    pub name: String,
    /// Predicates over the subject's facts, all of which must hold; none
    /// always matches.
    #[serde(default)]
    pub when: Vec<VisibilityPredicate>,
    /// Slots these take are unplaced in this layout.
    #[serde(default)]
    pub hide: Vec<SlotMatcher>,
    pub root: PanelTemplate,
}

impl LayoutDefinition {
    /// The layout's panels in preorder, which is also matcher order.
    pub fn panels(&self) -> Vec<&PanelTemplate> {
        fn walk<'a>(panel: &'a PanelTemplate, out: &mut Vec<&'a PanelTemplate>) {
            out.push(panel);
            if let PanelTemplateNode::Split { children, .. } = &panel.node {
                children.iter().for_each(|child| walk(child, out));
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// The tab panel with ID `id`, and its matchers.
    pub fn matchers(&self, id: &str) -> Option<&[SlotMatcher]> {
        self.panels().into_iter().find(|panel| panel.id == id).and_then(|panel| match &panel.node {
            PanelTemplateNode::Tabs { matchers, .. } => Some(matchers.as_slice()),
            PanelTemplateNode::Split { .. } => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PanelTemplate {
    /// Given, or from the panel's position when the template names none.
    pub id: String,
    #[serde(default = "one")]
    pub weight: u32,
    pub node: PanelTemplateNode,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum PanelTemplateNode {
    Split {
        axis: Axis,
        children: Vec<PanelTemplate>,
    },
    Tabs {
        #[serde(default)]
        matchers: Vec<SlotMatcher>,
        #[serde(default)]
        selected: SelectedTab,
    },
}

/// Which of a template tab panel's tabs is selected when it is made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectedTab {
    #[default]
    First,
    Last,
}

/// What a tab panel (or `hide`) takes: Slots whose fields equal every given
/// attribute and whose facet entity's facts satisfy every predicate, or,
/// with `rest`, every Slot. A Slot goes to the first matcher in template
/// order that takes it (`LayoutDefinition::route`), so `rest` gets only the
/// Slots that `hide` and the matchers before it didn't take, and a matcher
/// after it gets a Slot only once the user has removed `rest`'s panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SlotMatcher {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub facet: Option<String>,
    #[serde(default)]
    pub presentation: Option<String>,
    #[serde(default)]
    pub entity_kind: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub rest: bool,
    #[serde(default)]
    pub predicates: Vec<VisibilityPredicate>,
}

/// What a matcher is offered: a Slot's fields, and the facts its predicates
/// read (the facet entity's, or the subject's for a Slot with none).
#[derive(Debug, Clone, Copy)]
pub struct SlotFields<'a> {
    pub key: &'a str,
    pub role: Option<&'a str>,
    pub facet: Option<&'a str>,
    pub presentation: Option<&'a str>,
    pub entity_kind: Option<&'a str>,
    pub facts: Option<&'a FactMap>,
}

impl SlotMatcher {
    pub fn accepts(&self, slot: &SlotFields<'_>) -> bool {
        if self.rest {
            return true;
        }
        let equal = |want: &Option<String>, have: Option<&str>| want.as_deref().is_none_or(|want| Some(want) == have);
        equal(&self.role, slot.role)
            && equal(&self.facet, slot.facet)
            && equal(&self.presentation, slot.presentation)
            && equal(&self.entity_kind, slot.entity_kind)
            && equal(&self.key, Some(slot.key))
            && predicates_hold(&self.predicates, slot.facts)
    }
}

/// Whether `facts` satisfy every predicate: `exists` tests presence, `value`
/// compares text, booleans and integers by their text. No facts satisfy only
/// `exists=false`.
pub fn predicates_hold(predicates: &[VisibilityPredicate], facts: Option<&FactMap>) -> bool {
    predicates.iter().all(|predicate| {
        let fact = facts.and_then(|facts| facts.get(predicate.key.as_str()));
        if let Some(exists) = predicate.exists {
            return exists == fact.is_some();
        }
        let expected = predicate.value.as_deref();
        match fact {
            Some(MetadataValue::Text(text)) => Some(&**text) == expected,
            Some(MetadataValue::Bool(value)) => Some(if *value { "true" } else { "false" }) == expected,
            Some(MetadataValue::Integer(value)) => Some(value.to_string().as_str()) == expected,
            _ => false,
        }
    })
}

/// A piece of a template text: literal text, or the fact a `{name}` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece<'a> {
    Literal(&'a str),
    Fact(&'a str),
}

/// `text`'s pieces: `{name}` names a fact, and `{{` and `}}` are literal
/// braces. `None` for a `{` that doesn't close, a `{}`, or a lone `}`.
fn pieces(text: &str) -> Option<Vec<Piece<'_>>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        if at > 0 {
            out.push(Piece::Literal(&rest[..at]));
        }
        let brace = &rest[at..at + 1];
        if rest[at + 1..].starts_with(brace) {
            out.push(Piece::Literal(brace));
            rest = &rest[at + 2..];
            continue;
        }
        if brace == "}" {
            return None;
        }
        let end = at + 1 + rest[at + 1..].find(['{', '}'])?;
        if &rest[end..end + 1] != "}" || end == at + 1 {
            return None;
        }
        out.push(Piece::Fact(&rest[at + 1..end]));
        rest = &rest[end + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Literal(rest));
    }
    Some(out)
}

/// How a substituted fact is written into its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    /// As it is: a path, a URL, an argument of its own.
    None,
    /// As one POSIX shell word, so a fact's text is never shell syntax.
    Shell,
}

/// Replace each `{fact}` in `text` with the fact's text (a boolean or
/// integer by its text, as matchers compare them), written as `quoting`
/// says; `{{` and `}}` are literal braces. `None` when a named fact is
/// absent or has no text, or `text` is malformed.
pub fn substitute(text: &str, facts: &FactMap, quoting: Quoting) -> Option<String> {
    let mut out = String::new();
    for piece in pieces(text)? {
        match piece {
            Piece::Literal(literal) => out.push_str(literal),
            Piece::Fact(name) => {
                let value = match facts.get(name)? {
                    MetadataValue::Text(text) => text.to_string(),
                    MetadataValue::Bool(value) => value.to_string(),
                    MetadataValue::Integer(value) => value.to_string(),
                    _ => return None,
                };
                match quoting {
                    Quoting::None => out.push_str(&value),
                    Quoting::Shell => {
                        out.push('\'');
                        out.push_str(&value.replace('\'', r"'\''"));
                        out.push('\'');
                    }
                }
            }
        }
    }
    Some(out)
}

/// Where a layout puts one Slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A `hide` matcher took it: unplaced.
    Hidden,
    /// The tab panels whose matchers accept it, in matcher order: it goes to
    /// the first that exists, and is unplaced when none does or the list is
    /// empty.
    To(Vec<RouteTarget>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTarget {
    pub panel: String,
    /// The matcher's index within its panel, which orders the panel's tabs.
    pub matcher: usize,
}

impl LayoutDefinition {
    /// Where this layout puts `slot`: offered to `hide`, then to every tab
    /// panel's matchers in template order.
    pub fn route(&self, slot: &SlotFields<'_>) -> Route {
        if self.hide.iter().any(|matcher| matcher.accepts(slot)) {
            return Route::Hidden;
        }
        let mut targets = Vec::new();
        for panel in self.panels() {
            if let PanelTemplateNode::Tabs { matchers, .. } = &panel.node {
                if let Some(matcher) = matchers.iter().position(|matcher| matcher.accepts(slot)) {
                    targets.push(RouteTarget { panel: panel.id.clone(), matcher });
                }
            }
        }
        Route::To(targets)
    }
}

impl WorkspaceDefinition {
    /// The workspace's default layout: the first whose `when` holds over the
    /// subject's facts.
    pub fn default_layout(&self, facts: &FactMap) -> Option<&LayoutDefinition> {
        self.layouts.iter().find(|layout| predicates_hold(&layout.when, Some(facts)))
    }

    /// The layout a workspace shows: the one the user chose while the
    /// template has it, else the default.
    pub fn current_layout(&self, chosen: Option<&str>, facts: &FactMap) -> Option<&LayoutDefinition> {
        chosen.and_then(|name| self.layouts.iter().find(|layout| layout.name == name)).or_else(|| self.default_layout(facts))
    }
}

/// Plans the Workspace Layout a workspace shows over its slots, if it shows
/// one.
pub type Planner<'a> = dyn Fn(&[crate::slots::SlotInfo]) -> Option<LayoutPlan> + 'a;

/// A layout chosen for a workspace, with every Slot routed.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutPlan {
    pub definition: LayoutDefinition,
    /// By slot key.
    pub routes: BTreeMap<String, Route>,
}

impl LayoutPlan {
    pub fn new<'a>(definition: &LayoutDefinition, slots: impl IntoIterator<Item = SlotFields<'a>>) -> Self {
        let routes = slots.into_iter().map(|slot| (slot.key.to_owned(), definition.route(&slot))).collect();
        Self { definition: definition.clone(), routes }
    }

    pub fn name(&self) -> &str {
        &self.definition.name
    }

    /// The layout's document over the Slots in `order`, and the Slots it
    /// leaves unplaced, in that order. A panel's tabs are in matcher order,
    /// then in `order`.
    pub fn evaluate(&self, order: &[String]) -> (ArrangementDoc, Vec<String>) {
        let mut held: BTreeMap<&str, Vec<(usize, usize, &String)>> = BTreeMap::new();
        let mut unplaced = Vec::new();
        for (position, key) in order.iter().enumerate() {
            match self.routes.get(key) {
                Some(Route::To(targets)) if !targets.is_empty() => {
                    held.entry(&targets[0].panel).or_default().push((targets[0].matcher, position, key));
                }
                _ => unplaced.push(key.clone()),
            }
        }
        fn build(panel: &PanelTemplate, held: &mut BTreeMap<&str, Vec<(usize, usize, &String)>>) -> Panel {
            let node = match &panel.node {
                PanelTemplateNode::Split { axis, children } => {
                    PanelNode::Split { axis: *axis, children: children.iter().map(|child| build(child, held)).collect() }
                }
                PanelTemplateNode::Tabs { selected, .. } => {
                    let mut tabs = held.remove(panel.id.as_str()).unwrap_or_default();
                    tabs.sort();
                    let tabs: Vec<String> = tabs.into_iter().map(|(_, _, key)| key.clone()).collect();
                    let selected = match selected {
                        SelectedTab::First => tabs.first(),
                        SelectedTab::Last => tabs.last(),
                    }
                    .cloned();
                    PanelNode::Tabs { tabs, selected }
                }
            };
            Panel { id: panel.id.clone(), weight: f64::from(panel.weight), node }
        }
        (ArrangementDoc { root: Some(build(&self.definition.root, &mut held)) }, unplaced)
    }
}

/// `slots` as Suggested Layout slots of `subject`: each a facet `t:<name>`
/// of the subject, ready with its recipe built from the subject's `facts`,
/// or unavailable while a fact it names is missing.
pub fn template_slots(subject: &EntityRef, slots: &[TemplateSlot], facts: &FactMap) -> Vec<Slot> {
    slots
        .iter()
        .map(|slot| {
            let resolution = match slot.recipe.build(facts) {
                Some(recipe) => SlotResolution::Ready { target: None, recipe },
                None => SlotResolution::Unavailable,
            };
            Slot {
                key: slot.key(),
                spec: ViewSpec {
                    content: Content::ProviderFacet { entity: subject.clone(), facet: slot.key() },
                    presentation: slot.presentation.clone(),
                    role: slot.role.clone(),
                },
                rebind: RebindPolicy::Replace,
                resolution,
            }
        })
        .collect()
}

impl RecipeTemplate {
    /// The recipe with `facts` substituted; `None` while one is missing.
    pub fn build(&self, facts: &FactMap) -> Option<LocalRecipe> {
        let text = |text: &String| substitute(text, facts, Quoting::None);
        Some(match self {
            RecipeTemplate::Command { argv, command, cwd } => {
                let line = match (argv.is_empty(), command) {
                    (false, _) => CommandLine::Argv(argv.iter().map(text).collect::<Option<_>>()?),
                    // A shell parses the line: each fact is one word of it.
                    (true, Some(command)) => CommandLine::Shell(substitute(command, facts, Quoting::Shell)?),
                    (true, None) => return None,
                };
                let cwd = match cwd {
                    Some(cwd) => Some(text(cwd)?),
                    None => None,
                };
                LocalRecipe::Command { line, cwd }
            }
            RecipeTemplate::File { path } => LocalRecipe::File { path: text(path)? },
            RecipeTemplate::Url { url } => LocalRecipe::Url { url: text(url)? },
            RecipeTemplate::Jackstay { launcher, endpoint } => {
                LocalRecipe::Jackstay { launcher: text(launcher)?, endpoint: text(endpoint)? }
            }
            // An unknown transport or an unusable field is as unresolved as a
            // missing fact.
            RecipeTemplate::CleatSession { transport, host, runtime_root, daemon, session } => {
                let (host, runtime_root, daemon, session) = (text(host)?, text(runtime_root)?, text(daemon)?, text(session)?);
                if invalid_cleat_field(&host, &runtime_root, &daemon, &session).is_some() {
                    return None;
                }
                LocalRecipe::CleatSession { transport: CleatTransport::parse(&text(transport)?)?, host, runtime_root, daemon, session }
            }
        })
    }
}

/// Read a `workspace kind="…"` node's children: its template slots and
/// layouts.
pub(crate) fn parse_kdl_workspace(node: &KdlNode) -> Result<(Vec<TemplateSlot>, Vec<LayoutDefinition>), TemplateConfigError> {
    let mut slots = Vec::new();
    let mut layouts = Vec::new();
    for child in node.children().map(|children| children.nodes()).unwrap_or_default() {
        match child.name().value() {
            "slot" => slots.push(parse_slot(child)?),
            "layout" => layouts.push(parse_layout(child)?),
            other => return Err(invalid(format!("workspace only accepts slot and layout, not {other}"))),
        }
    }
    Ok((slots, layouts))
}

fn parse_slot(node: &KdlNode) -> Result<TemplateSlot, TemplateConfigError> {
    let name = arg(node, "template slot name")?;
    only(
        node,
        &[
            "kind",
            "role",
            "presentation",
            "command",
            "cwd",
            "path",
            "url",
            "launcher",
            "endpoint",
            "transport",
            "host",
            "runtime_root",
            "daemon",
            "session",
        ],
        &format!("template slot {name}"),
    )?;
    let text = |key: &str| string(node, key);
    let required = |key: &str| text(key)?.ok_or_else(|| invalid(format!("template slot {name} needs {key}")));
    let recipe = match required("kind")?.as_str() {
        "command" => {
            let mut argv = Vec::new();
            for child in node.children().map(|children| children.nodes()).unwrap_or_default() {
                if child.name().value() != "argv" {
                    return Err(invalid(format!("template slot {name} only accepts argv")));
                }
                for entry in child.entries() {
                    if entry.name().is_some() {
                        return Err(invalid(format!("template slot {name} argv takes arguments only")));
                    }
                    let arg = entry.value().as_string().ok_or_else(|| invalid(format!("template slot {name} argv takes strings")))?;
                    argv.push(arg.to_owned());
                }
            }
            RecipeTemplate::Command { argv, command: text("command")?, cwd: text("cwd")? }
        }
        "file" => RecipeTemplate::File { path: required("path")? },
        "url" => RecipeTemplate::Url { url: required("url")? },
        "jackstay" => RecipeTemplate::Jackstay { launcher: required("launcher")?, endpoint: required("endpoint")? },
        "cleat-session" => RecipeTemplate::CleatSession {
            transport: required("transport")?,
            host: required("host")?,
            runtime_root: required("runtime_root")?,
            daemon: required("daemon")?,
            session: required("session")?,
        },
        other => return Err(invalid(format!("template slot {name} has unknown kind {other}"))),
    };
    if !matches!(recipe, RecipeTemplate::Command { .. }) && node.children().is_some() {
        return Err(invalid(format!("template slot {name} takes argv only with kind command")));
    }
    Ok(TemplateSlot { name, role: text("role")?, presentation: text("presentation")?, recipe })
}

fn parse_layout(node: &KdlNode) -> Result<LayoutDefinition, TemplateConfigError> {
    let name = arg(node, "layout name")?;
    only(node, &[], &format!("layout {name}"))?;
    let mut when = Vec::new();
    let mut hide = Vec::new();
    let mut root = None;
    for child in node.children().map(|children| children.nodes()).unwrap_or_default() {
        match child.name().value() {
            "when" => {
                only(child, &[], &format!("layout {name}"))?;
                when.extend(parse_predicates(child, &name)?)
            }
            "hide" => {
                only(child, &[], &format!("layout {name}"))?;
                for matcher in child.children().map(|children| children.nodes()).unwrap_or_default() {
                    hide.push(parse_matcher(matcher, &name)?);
                }
            }
            "split" | "tabs" if root.is_none() => root = Some(parse_panel(child, &name, "1")?),
            "split" | "tabs" => return Err(invalid(format!("layout {name} has more than one root panel"))),
            other => return Err(invalid(format!("layout {name} only accepts when, hide, split and tabs, not {other}"))),
        }
    }
    let root = root.ok_or_else(|| invalid(format!("layout {name} needs a split or tabs panel")))?;
    Ok(LayoutDefinition { name, when, hide, root })
}

/// `position` names a panel without an `id`: `1` for the root, `1.2` for
/// its second child.
fn parse_panel(node: &KdlNode, layout: &str, position: &str) -> Result<PanelTemplate, TemplateConfigError> {
    let id = string(node, "id")?.unwrap_or_else(|| position.to_owned());
    let properties: &[&str] = if node.name().value() == "split" { &["id", "weight", "axis"] } else { &["id", "weight", "selected"] };
    only(node, properties, &format!("layout {layout} panel {id}"))?;
    let weight = match node.get("weight") {
        None => 1,
        Some(entry) => entry
            .value()
            .as_i64()
            .and_then(|weight| u32::try_from(weight).ok())
            .filter(|weight| *weight > 0)
            .ok_or_else(|| invalid(format!("layout {layout} panel {id} weight must be a positive integer")))?,
    };
    let children = node.children().map(|children| children.nodes()).unwrap_or_default();
    let node = match node.name().value() {
        "split" => {
            let axis = match string(node, "axis")?.as_deref() {
                Some("row") => Axis::Row,
                Some("column") => Axis::Column,
                _ => return Err(invalid(format!("layout {layout} split {id} needs axis row or column"))),
            };
            let children = children
                .iter()
                .enumerate()
                .map(|(index, child)| match child.name().value() {
                    "split" | "tabs" => parse_panel(child, layout, &format!("{position}.{}", index + 1)),
                    other => Err(invalid(format!("layout {layout} split {id} holds panels, not {other}"))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            PanelTemplateNode::Split { axis, children }
        }
        _ => {
            let selected = match string(node, "selected")?.as_deref() {
                None | Some("first") => SelectedTab::First,
                Some("last") => SelectedTab::Last,
                Some(other) => return Err(invalid(format!("layout {layout} tabs {id} selected must be first or last, not {other}"))),
            };
            let matchers = children.iter().map(|child| parse_matcher(child, layout)).collect::<Result<Vec<_>, _>>()?;
            PanelTemplateNode::Tabs { matchers, selected }
        }
    };
    Ok(PanelTemplate { id, weight, node })
}

fn parse_matcher(node: &KdlNode, layout: &str) -> Result<SlotMatcher, TemplateConfigError> {
    if node.name().value() != "slots" {
        return Err(invalid(format!("layout {layout} matches slots with slots, not {}", node.name().value())));
    }
    only(node, &["role", "facet", "presentation", "entity-kind", "key", "rest"], &format!("layout {layout}"))?;
    let rest = match node.get("rest") {
        None => false,
        Some(entry) => entry.value().as_bool().ok_or_else(|| invalid(format!("layout {layout} rest must be boolean")))?,
    };
    Ok(SlotMatcher {
        role: string(node, "role")?,
        facet: string(node, "facet")?,
        presentation: string(node, "presentation")?,
        entity_kind: string(node, "entity-kind")?,
        key: string(node, "key")?,
        rest,
        predicates: parse_predicates(node, layout)?,
    })
}

/// A node's `match` children, as the visibility rules write them.
fn parse_predicates(node: &KdlNode, layout: &str) -> Result<Vec<VisibilityPredicate>, TemplateConfigError> {
    let mut predicates = Vec::new();
    for child in node.children().map(|children| children.nodes()).unwrap_or_default() {
        if child.name().value() != "match" || child.get("of").is_some() {
            return Err(invalid(format!("layout {layout} only accepts constant fact matches")));
        }
        only(child, &["value", "exists"], &format!("layout {layout}"))?;
        predicates.push(VisibilityPredicate {
            key: arg(child, "match fact key")?,
            value: string(child, "value")?,
            exists: child
                .get("exists")
                .map(|entry| entry.value().as_bool().ok_or_else(|| invalid(format!("layout {layout} exists must be boolean"))))
                .transpose()?,
        });
    }
    Ok(predicates)
}

/// The rules a kind's template slots and layouts follow, beyond their
/// syntax.
pub(crate) fn validate(kind: &str, slots: &[TemplateSlot], layouts: &[LayoutDefinition]) -> Result<(), TemplateConfigError> {
    let mut names = BTreeSet::new();
    for slot in slots {
        check_name(&slot.name).map_err(|_| invalid(format!("workspace {kind} template slot name {:?} is invalid", slot.name)))?;
        if !names.insert(&slot.name) {
            return Err(invalid(format!("workspace {kind} declares template slot {} twice", slot.name)));
        }
        let texts: Vec<&String> = match &slot.recipe {
            RecipeTemplate::Command { argv, command, cwd } => {
                if argv.is_empty() && command.as_deref().is_none_or(str::is_empty) {
                    return Err(invalid(format!("workspace {kind} template slot {} needs argv or a command", slot.name)));
                }
                argv.iter().chain(command).chain(cwd).collect()
            }
            RecipeTemplate::File { path } => vec![path],
            RecipeTemplate::Url { url } => vec![url],
            RecipeTemplate::Jackstay { launcher, endpoint } => vec![launcher, endpoint],
            RecipeTemplate::CleatSession { transport, host, runtime_root, daemon, session } => {
                // A literal transport is checked now; one naming a fact, when built.
                if !transport.contains('{') && CleatTransport::parse(transport).is_none() {
                    return Err(invalid(format!("workspace {kind} template slot {} has unknown transport {transport:?}", slot.name)));
                }
                vec![transport, host, runtime_root, daemon, session]
            }
        };
        if let Some(text) = texts.into_iter().find(|text| !well_formed(text)) {
            return Err(invalid(format!("workspace {kind} template slot {} has unbalanced braces in {text:?}", slot.name)));
        }
    }
    let mut names = BTreeSet::new();
    for layout in layouts {
        let at = format!("workspace {kind} layout {}", layout.name);
        check_name(&layout.name).map_err(|_| invalid(format!("{at}: the name is invalid")))?;
        if !names.insert(&layout.name) {
            return Err(invalid(format!("workspace {kind} declares layout {} twice", layout.name)));
        }
        let mut ids = BTreeSet::new();
        let mut tabs = 0;
        for panel in layout.panels() {
            if panel.id.is_empty() || panel.id.len() > 64 || panel.id.contains(':') {
                return Err(invalid(format!("{at}: panel ID {:?} is invalid", panel.id)));
            }
            if !ids.insert(&panel.id) {
                return Err(invalid(format!("{at}: panel ID {} is used twice", panel.id)));
            }
            match &panel.node {
                PanelTemplateNode::Split { children, .. } if children.is_empty() => {
                    return Err(invalid(format!("{at}: split {} has no panels", panel.id)));
                }
                PanelTemplateNode::Split { .. } => {}
                PanelTemplateNode::Tabs { matchers, .. } => {
                    tabs += 1;
                    matchers.iter().try_for_each(|matcher| check_matcher(matcher, &at))?;
                }
            }
        }
        if tabs == 0 {
            return Err(invalid(format!("{at}: a layout needs a tabs panel")));
        }
        layout.hide.iter().try_for_each(|matcher| check_matcher(matcher, &at))?;
        check_predicates(&layout.when, &at)?;
    }
    Ok(())
}

fn check_matcher(matcher: &SlotMatcher, at: &str) -> Result<(), TemplateConfigError> {
    let narrowed = matcher.role.is_some()
        || matcher.facet.is_some()
        || matcher.presentation.is_some()
        || matcher.entity_kind.is_some()
        || matcher.key.is_some()
        || !matcher.predicates.is_empty();
    match (matcher.rest, narrowed) {
        (true, true) => Err(invalid(format!("{at}: slots rest=true takes no other conditions"))),
        (false, false) => Err(invalid(format!("{at}: slots needs a condition, or rest=true"))),
        _ => check_predicates(&matcher.predicates, at),
    }
}

fn check_predicates(predicates: &[VisibilityPredicate], at: &str) -> Result<(), TemplateConfigError> {
    if predicates.iter().any(|predicate| predicate.key.is_empty() || predicate.value.is_some() == predicate.exists.is_some()) {
        return Err(invalid(format!("{at}: a match needs a fact key and exactly one of value or exists")));
    }
    Ok(())
}

/// Every `{` closes before the next opens and names something, and every
/// other brace is doubled.
fn well_formed(text: &str) -> bool {
    pieces(text).is_some()
}

fn check_name(name: &str) -> Result<(), String> {
    crate::slots::check_key(name)
}

fn arg(node: &KdlNode, what: &str) -> Result<String, TemplateConfigError> {
    node.get(0)
        .filter(|entry| entry.name().is_none())
        .and_then(|entry| entry.value().as_string())
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("{} needs a {what}", node.name().value())))
}

/// Rejects a property `node` doesn't take, which would otherwise change what
/// it means silently: `slots entity_kind="vessel"` would match every Slot.
fn only(node: &KdlNode, properties: &[&str], at: &str) -> Result<(), TemplateConfigError> {
    for entry in node.entries() {
        if let Some(name) = entry.name().filter(|name| !properties.contains(&name.value())) {
            return Err(invalid(format!("{at}: {} takes no property {}", node.name().value(), name.value())));
        }
    }
    Ok(())
}

fn string(node: &KdlNode, key: &str) -> Result<Option<String>, TemplateConfigError> {
    node.get(key)
        .map(|entry| entry.value().as_string().map(str::to_owned).ok_or_else(|| invalid(format!("{key} must be a string"))))
        .transpose()
}

fn invalid(message: String) -> TemplateConfigError {
    TemplateConfigError::Validation(message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::template_config::{parse_template_config_kdl, TemplateConfigCatalog, TemplateConfigLayer};

    fn workspace(kdl: &str) -> Result<crate::template_config::WorkspaceDefinition, TemplateConfigError> {
        parse_template_config_kdl(kdl).map(|config| config.workspaces.into_iter().next().expect("one workspace"))
    }

    fn facts(pairs: &[(&str, &str)]) -> FactMap {
        pairs.iter().map(|(key, value)| ((*key).into(), MetadataValue::Text((*value).into()))).collect()
    }

    const VESSEL: &str = r#"
        workspace kind="vessel" {
          slot "files" role="files" kind="file" path="{git.root}"
          slot "shell" role="shell" kind="command" cwd="{git.root}" { argv "zsh" "-l"; }
          layout "live-only" {
            when { match "layout.hint" value="live"; }
            hide { slots role="agent" { match "flotilla.ended" exists=true; }; }
            split axis="row" {
              tabs id="agents" weight=3 { slots role="agent"; }
              tabs weight=2 selected="last" { slots role="files"; slots rest=true; }
            }
          }
          layout "single" { tabs; }
        }
    "#;

    #[test]
    fn a_workspace_declares_template_slots_and_layouts() {
        let vessel = workspace(VESSEL).expect("vessel parses");
        assert_eq!(vessel.slots.len(), 2);
        assert_eq!(vessel.slots[0].key(), "t:files");
        assert_eq!(vessel.slots[0].recipe, RecipeTemplate::File { path: "{git.root}".into() });
        assert_eq!(
            vessel.slots[1].recipe,
            RecipeTemplate::Command { argv: vec!["zsh".into(), "-l".into()], command: None, cwd: Some("{git.root}".into()) }
        );
        let live = &vessel.layouts[0];
        assert_eq!(live.when, vec![VisibilityPredicate { key: "layout.hint".into(), value: Some("live".into()), exists: None }]);
        assert_eq!(live.hide[0].role.as_deref(), Some("agent"));
        assert_eq!(live.hide[0].predicates[0].exists, Some(true));
        let ids: Vec<&str> = live.panels().iter().map(|panel| panel.id.as_str()).collect();
        assert_eq!(ids, ["1", "agents", "1.2"], "a panel without an id is named by its position");
        assert_eq!(live.panels()[1].weight, 3);
        let PanelTemplateNode::Tabs { matchers, selected } = &live.panels()[2].node else { panic!("tabs") };
        assert_eq!(*selected, SelectedTab::Last);
        assert!(matchers[1].rest);
        assert_eq!(live.matchers("agents").map(<[_]>::len), Some(1));
        assert_eq!(live.matchers("1"), None, "a split holds no matchers");
        assert_eq!(vessel.layouts[1].matchers("1"), Some(&[][..]), "a matcher-less tab panel is allowed");
    }

    // A transport named by a fact is checked when the recipe is built: one
    // that is neither local nor ssh leaves the slot unresolved.
    #[test]
    fn a_cleat_session_template_builds_from_facts() {
        let crew = workspace(
            r#"workspace kind="vessel" {
              slot "crew" kind="cleat-session" transport="{t}" host="{h}" runtime_root="/run/cleat" daemon="d" session="{s}"
            }"#,
        )
        .expect("parses");
        let build = |pairs: &[(&str, &str)]| crew.slots[0].recipe.build(&facts(pairs));
        assert_eq!(
            build(&[("t", "ssh"), ("h", "robert@feta"), ("s", "s1")]),
            Some(LocalRecipe::CleatSession {
                transport: CleatTransport::Ssh,
                host: "robert@feta".into(),
                runtime_root: "/run/cleat".into(),
                daemon: "d".into(),
                session: "s1".into(),
            })
        );
        assert_eq!(build(&[("t", "mosh"), ("h", "robert@feta"), ("s", "s1")]), None);
        assert_eq!(build(&[("t", "local"), ("h", "feta")]), None);
        // An empty fact, or one a host would read as an option, is unusable.
        assert_eq!(build(&[("t", "ssh"), ("h", ""), ("s", "s1")]), None);
        assert_eq!(build(&[("t", "ssh"), ("h", "-oProxyCommand=x"), ("s", "s1")]), None);
    }

    #[test]
    fn a_workspace_without_children_has_no_template() {
        assert!(!workspace(r#"workspace kind="vessel""#).expect("parses").has_template());
        assert!(workspace(VESSEL).expect("parses").has_template());
    }

    #[test]
    fn invalid_templates_are_rejected() {
        // kdl 4 needs `;` after a block that ends a block: `a { b { c; }; }`.
        let wrap = |body: &str| format!("workspace kind=\"vessel\" {{\n{body}\n}}");
        for (body, why) in [
            (r#"slot "Files" kind="file" path="x""#, "slot name"),
            (r#"slot "a" kind="file" path="x"; slot "a" kind="file" path="y""#, "duplicate slot"),
            (r#"slot "a" kind="file""#, "missing path"),
            (r#"slot "a" kind="pipe" path="x""#, "unknown kind"),
            (r#"slot "a" kind="command""#, "command without argv"),
            (r#"slot "a" kind="file" path="x" { argv "y"; }"#, "argv off command"),
            (r#"slot "a" kind="file" path="{git.root""#, "unclosed brace"),
            (r#"slot "a" kind="file" path="{}""#, "empty brace"),
            (r#"slot "a" kind="url" url="a}b""#, "stray brace"),
            (r#"layout "a" { tabs; }; layout "a" { tabs; }"#, "duplicate layout"),
            (r#"layout "A" { tabs; }"#, "layout name"),
            (r#"layout "a" { }"#, "no panel"),
            (r#"layout "a" { tabs; tabs; }"#, "two roots"),
            (r#"layout "a" { split axis="row"; }"#, "empty split"),
            (r#"layout "a" { split { tabs; }; }"#, "no axis"),
            (r#"layout "a" { split axis="row" { tabs id="x"; tabs id="x"; }; }"#, "duplicate panel"),
            (r#"layout "a" { tabs id="a:b"; }"#, "panel id"),
            (r#"layout "a" { tabs weight=0; }"#, "zero weight"),
            (r#"layout "a" { tabs selected="middle"; }"#, "selected"),
            (r#"layout "a" { tabs { slots; }; }"#, "unconditional matcher"),
            (r#"layout "a" { tabs { slots rest=true role="agent"; }; }"#, "narrowed rest"),
            (r#"layout "a" { tabs { panel; }; }"#, "not a matcher"),
            (r#"layout "a" { tabs { slots { match "k"; }; }; }"#, "predicate without a test"),
            (r#"layout "a" { tabs { slots { match "k" value="v" exists=true; }; }; }"#, "predicate with two tests"),
            (r#"layout "a" { when { match "k" of="x" value="v"; }; tabs; }"#, "dynamic predicate"),
            (r#"panel"#, "unknown child"),
            (r#"slot "a" kind="file" path="x" paht="y""#, "unknown slot property"),
            (r#"layout "a" { tabs { slots entity_kind="vessel"; }; }"#, "unknown matcher property"),
            (r#"layout "a" { tabs widht=2; }"#, "unknown tabs property"),
            (r#"layout "a" { split axis="row" selected="last" { tabs; }; }"#, "a tabs property on split"),
            (r#"layout "a" default=true { tabs; }"#, "unknown layout property"),
            (r#"layout "a" { hide role="agent"; tabs; }"#, "a property on hide"),
            (r#"layout "a" { tabs { slots { match "k" valeu="v"; }; }; }"#, "unknown match property"),
            (r#"slot "a" kind="cleat-session" transport="ssh" host="h" runtime_root="r" daemon="d""#, "cleat without session"),
            (r#"slot "a" kind="cleat-session" transport="mosh" host="h" runtime_root="r" daemon="d" session="s""#, "transport"),
        ] {
            let error = workspace(&wrap(body)).expect_err(&format!("{why}: {body} parses"));
            assert!(matches!(error, TemplateConfigError::Validation(_)), "{why}: {body} fails to parse: {error}");
        }
        let twice = parse_template_config_kdl(r#"workspace kind="vessel"; workspace kind="vessel""#);
        assert!(matches!(twice, Err(TemplateConfigError::Validation(_))), "a kind declared twice");
    }

    #[test]
    fn the_highest_ranked_applying_layer_with_a_template_wins() {
        let bundled = parse_template_config_kdl(r#"workspace kind="vessel" { layout "bundled" { tabs; }; }"#).expect("parses");
        let user = parse_template_config_kdl(r#"workspace kind="vessel""#).expect("parses");
        let repo = parse_template_config_kdl(r#"workspace kind="vessel" { layout "repo" { tabs; }; }"#).expect("parses");
        let catalog = TemplateConfigCatalog::from_layers(vec![
            TemplateConfigLayer::bundled("bundled.kdl", bundled),
            TemplateConfigLayer::user("user.kdl", user),
            TemplateConfigLayer::repo("acme/app", "repo.kdl", repo),
        ]);
        let name = |facts: &FactMap| catalog.workspace_template("vessel", facts).map(|template| template.layouts[0].name.clone());
        assert_eq!(name(&facts(&[])).as_deref(), Some("bundled"), "a childless declaration doesn't shadow a template");
        assert_eq!(name(&facts(&[("vcs.repo", "acme/app")])).as_deref(), Some("repo"));
        assert_eq!(catalog.workspace_template("convoy", &facts(&[])), None);
    }

    #[test]
    fn matchers_take_slots_whose_fields_and_facts_match() {
        let ended = facts(&[("flotilla.ended", "yes"), ("flotilla.phase", "landed")]);
        let slot = SlotFields {
            key: "crew-1",
            role: Some("agent"),
            facet: Some("terminal"),
            presentation: None,
            entity_kind: Some("vessel"),
            facts: Some(&ended),
        };
        let matcher = |kdl: &str| {
            let vessel = workspace(&format!("workspace kind=\"vessel\" {{\nlayout \"a\" {{\ntabs {{\n{kdl}\n}}\n}}\n}}")).expect("parses");
            vessel.layouts[0].matchers("1").expect("tabs")[0].clone()
        };
        for (kdl, accepts) in [
            (r#"slots role="agent""#, true),
            (r#"slots role="files""#, false),
            (r#"slots role="agent" entity-kind="vessel" facet="terminal" key="crew-1""#, true),
            (r#"slots role="agent" key="crew-2""#, false),
            (r#"slots presentation="table""#, false),
            (r#"slots { match "flotilla.ended" exists=true; }"#, true),
            (r#"slots { match "flotilla.ended" exists=false; }"#, false),
            (r#"slots { match "flotilla.phase" value="landed"; }"#, true),
            (r#"slots { match "flotilla.phase" value="running"; }"#, false),
            (r#"slots role="agent" { match "flotilla.phase" value="running"; }"#, false),
            (r#"slots rest=true"#, true),
        ] {
            assert_eq!(matcher(kdl).accepts(&slot), accepts, "{kdl}");
        }
        let bare = SlotFields { facts: None, ..slot };
        assert!(matcher(r#"slots { match "flotilla.ended" exists=false; }"#).accepts(&bare));
        assert!(!matcher(r#"slots { match "flotilla.phase" value="landed"; }"#).accepts(&bare));
    }

    #[test]
    fn predicates_compare_booleans_and_integers_by_their_text() {
        let facts: FactMap = BTreeMap::from([("ready".into(), MetadataValue::Bool(true)), ("count".into(), MetadataValue::Integer(3))]);
        let test = |key: &str, value: &str| {
            predicates_hold(&[VisibilityPredicate { key: key.into(), value: Some(value.into()), exists: None }], Some(&facts))
        };
        assert!(test("ready", "true") && !test("ready", "false"));
        assert!(test("count", "3") && !test("count", "4"));
        assert!(predicates_hold(&[], None), "no predicates always hold");
    }

    #[test]
    fn substitution_replaces_facts_and_fails_on_a_missing_one() {
        let facts = facts(&[("git.root", "/src/app"), ("branch", "main")]);
        let plain = |text: &str| substitute(text, &facts, Quoting::None);
        assert_eq!(plain("{git.root}/docs").as_deref(), Some("/src/app/docs"));
        assert_eq!(plain("{git.root}@{branch}").as_deref(), Some("/src/app@main"));
        assert_eq!(plain("plain").as_deref(), Some("plain"));
        assert_eq!(plain("{missing}"), None);
        let typed: FactMap = BTreeMap::from([("port".into(), MetadataValue::Integer(80)), ("ready".into(), MetadataValue::Bool(true))]);
        assert_eq!(substitute("{port}:{ready}", &typed, Quoting::None).as_deref(), Some("80:true"), "integers and booleans by their text");
    }

    #[test]
    fn doubled_braces_are_literal_and_malformed_text_substitutes_nothing() {
        let facts = facts(&[("a", "x")]);
        let plain = |text: &str| substitute(text, &facts, Quoting::None);
        assert_eq!(plain("awk '{{print $1}}'").as_deref(), Some("awk '{print $1}'"));
        assert_eq!(plain("${{SHELL}}").as_deref(), Some("${SHELL}"));
        assert_eq!(plain("{{{a}}}").as_deref(), Some("{x}"));
        for malformed in ["{a", "{}", "a}", "{a}}", "{{a}", "{a{b}}", "}{"] {
            assert_eq!(plain(malformed), None, "{malformed:?}");
            assert!(!well_formed(malformed), "{malformed:?} is rejected at validation");
        }
        assert!(well_formed("{{}}") && well_formed("{a}/{b}") && well_formed(""));
    }

    /// A fact in a shell `command` is one shell word, whatever its text; in
    /// `argv` and `cwd` it is the text as it is.
    #[cfg(unix)]
    #[test]
    fn a_fact_in_a_shell_command_is_one_word() {
        for value in ["main", "a b", "x; echo injected", "$(id)", "`id`", "it's", "'", "\"q\"", "a\nb", "*", "-n", ""] {
            let facts = facts(&[("branch", value)]);
            let line = substitute("printf %s {branch}", &facts, Quoting::Shell).expect("substitutes");
            let output = std::process::Command::new("sh").arg("-c").arg(&line).output().expect("sh runs");
            assert_eq!(String::from_utf8_lossy(&output.stdout), value, "{line}");
        }
        let facts = facts(&[("branch", "a b; c")]);
        let shell =
            RecipeTemplate::Command { argv: Vec::new(), command: Some("git switch {branch}".into()), cwd: Some("/w/{branch}".into()) };
        assert_eq!(
            shell.build(&facts),
            Some(LocalRecipe::Command { line: CommandLine::Shell("git switch 'a b; c'".into()), cwd: Some("/w/a b; c".into()) })
        );
        let argv = RecipeTemplate::Command { argv: vec!["git".into(), "{branch}".into()], command: None, cwd: None };
        assert_eq!(
            argv.build(&facts),
            Some(LocalRecipe::Command { line: CommandLine::Argv(vec!["git".into(), "a b; c".into()]), cwd: None })
        );
    }

    #[test]
    fn rest_takes_only_what_earlier_matchers_left() {
        let layout = workspace(
            r#"workspace kind="vessel" {
              layout "a" {
                split axis="row" {
                  tabs id="agents" { slots role="agent"; slots rest=true; slots role="files"; };
                  tabs id="files" { slots role="files"; };
                };
              }
            }"#,
        )
        .expect("parses")
        .layouts
        .remove(0);
        let slot = |role| SlotFields { key: "k", role: Some(role), facet: None, presentation: None, entity_kind: None, facts: None };
        let route = |role| match layout.route(&slot(role)) {
            Route::To(targets) => targets.into_iter().map(|target| (target.panel, target.matcher)).collect::<Vec<_>>(),
            Route::Hidden => panic!("nothing is hidden"),
        };
        assert_eq!(route("agent"), [("agents".into(), 0)], "the first matcher that takes it");
        assert_eq!(
            route("files"),
            [("agents".into(), 1), ("files".into(), 0)],
            "rest, earlier in template order, takes it; the later panel is where it goes once rest's panel is gone"
        );
    }
}
