//! Semantic placement snapshots shared by all frontends.
use crate::FastHashMap;
use std::{collections::BTreeMap, ops::ControlFlow, rc::Rc};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::census::Census;
#[cfg(test)]
use crate::state::variables_census;
use crate::state::{ControllerTab, EvaluatedPlacement, EvaluatedSection, NodeVariables};
use crate::template_config::{
    AbbreviationTier, ChromeSpec, SurfaceRegionDefinition, TemplateConfigFieldClass, TemplateConfigMatchContext, TemplateConfigNodeKind,
    TemplateConfigRenderedField, TemplateConfigSlot, TemplateControlSpec, TemplateVariableDefinition,
};
use crate::{
    ControllerViewModel, DisplayVariableValue, EffectiveKdl, EffectiveNodeVariables, EntityRef, MetadataValue, NodeKey, PlacementKey,
    PlacementLoopKey, PlacementSegment, ResolvedTemplateSlot, WorkspaceId, DISPLAY_FORM_COMPACT,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSnapshot {
    pub sections: Vec<Section>,
    pub display_variables: Vec<TemplateVariableDefinition>,
    pub display_values: BTreeMap<String, DisplayVariableValue>,
    /// Diagnostics attached to a semantic snapshot.
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    pub pinned: bool,
    pub content: Content,
    pub nodes: Vec<Rc<PlacementNode>>,
}

/// Andamento's own system kinds and facts start with `.`; conventions that
/// producers share (`display.label`, `flotilla.project`) keep their names.
pub mod system {
    /// Whether a kind or fact name is one of Andamento's own.
    pub fn is_system(name: &str) -> bool {
        name.starts_with('.')
    }

    /// A host's workspace: a host entity, or the synthetic entity covering a
    /// tab nothing places.
    pub const WORKSPACE: &str = ".workspace";
    /// A section someone made; placed by a `layout="section"` loop.
    pub const SECTION: &str = ".section";
    /// A group inside a section (fact `.section`), holding workspaces and
    /// references (fact `.group`).
    pub const GROUP: &str = ".group";
    /// A reference (ghost): presents its `.target` entity.
    pub const REF: &str = ".ref";
    pub const TARGET: &str = ".target";
    /// Marks the group that covers tabs nothing else places.
    pub const DEFAULT: &str = ".default";
    /// Tab metadata naming the host entity published for that workspace.
    pub const HOST_KIND: &str = ".host.kind";
    pub const HOST_ID: &str = ".host.id";
    /// The host entity's provider; stamped by the core, never set by a patch.
    pub const HOST_PROVIDER: &str = ".host.provider";
    /// The section covering tabs nothing places, when no default group does.
    pub const UNPLACED: &str = ".unplaced";
}

/// The synthetic section covering workspaces with no normal placement. It is
/// also the loop name under a default group that covers them instead.
pub const UNPLACED_WORKSPACES_SECTION: &str = system::UNPLACED;

impl Section {
    /// The workspace fallback is emitted even when empty so hosts can anchor
    /// workspace creation to its header. Frontends without that affordance
    /// should not render it while it has no rows.
    pub fn is_empty_workspace_fallback(&self) -> bool {
        self.name == UNPLACED_WORKSPACES_SECTION && self.nodes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementNode {
    pub key: PlacementKey,
    /// Loop invocation shared by sibling items, independent of their entity IDs.
    pub loop_key: PlacementLoopKey,
    pub entity: EntityRef,
    pub label: String,
    /// Logical intent declared on the loop, interpreted by each frontend.
    pub layout: Option<String>,
    pub form: String,
    pub state: PresentationState,
    pub content: Content,
    pub detail: Content,
    pub facts: Rc<BTreeMap<String, MetadataValue>>,
    pub variables: Option<EffectiveNodeVariables>,
    pub collapsed: bool,
    /// The provider of the entity this node presents (a ref's target) is
    /// stale: its facts are kept from when its connection dropped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    /// Retained even when collapsed, for inspection and native input handling.
    pub children: Vec<Rc<PlacementNode>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum PresentationState {
    #[default]
    Catalog,
    Latent {
        openable: bool,
    },
    Opening,
    Live {
        workspace_id: WorkspaceId,
        selected: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub fields: Vec<TemplateConfigRenderedField>,
    pub controls: Vec<TemplateControlSpec>,
    pub chrome: ChromeSpec,
    pub template_name: Option<String>,
    pub effective_kdl: EffectiveKdl,
    pub error: Option<String>,
}

impl Content {
    /// Plain content for a frontend to measure. No terminal elision or styling.
    pub fn text(&self) -> String {
        self.fields.iter().map(|f| f.value.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// The first placement of `entity`, depth first. A default group placed more
/// than once covers leftover tabs under its first placement only.
/// The node presenting `entity`, made exclusively this snapshot's (copying
/// the nodes on the way to it that are shared) so it can be changed.
fn find_node_mut<'a>(nodes: &'a mut [Rc<PlacementNode>], entity: &EntityRef) -> Option<&'a mut PlacementNode> {
    fn path(nodes: &[Rc<PlacementNode>], entity: &EntityRef, out: &mut Vec<usize>) -> bool {
        for (index, node) in nodes.iter().enumerate() {
            out.push(index);
            if &node.entity == entity || path(&node.children, entity, out) {
                return true;
            }
            out.pop();
        }
        false
    }
    let mut indices = vec![];
    if !path(nodes, entity, &mut indices) {
        return None;
    }
    let (&first, rest) = indices.split_first()?;
    let mut node = Rc::make_mut(&mut nodes[first]);
    for &index in rest {
        node = Rc::make_mut(&mut node.children[index]);
    }
    Some(node)
}

// Inspection and coverage both traverse collapsed descendants. The callback can
// stop early for lookup, without allocating an intermediate list of nodes.
fn visit_nodes<'a, B>(nodes: &'a [Rc<PlacementNode>], visit: &mut impl FnMut(&'a PlacementNode) -> ControlFlow<B>) -> ControlFlow<B> {
    for node in nodes {
        visit(node)?;
        visit_nodes(&node.children, visit)?;
    }
    ControlFlow::Continue(())
}

impl SurfaceSnapshot {
    /// Sections a frontend without a workspace-creation affordance renders:
    /// everything except the empty workspace fallback.
    pub fn visible_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|section| !section.is_empty_workspace_fallback())
    }

    pub fn node(&self, key: &PlacementKey) -> Option<&PlacementNode> {
        self.sections.iter().find_map(|section| {
            visit_nodes(&section.nodes, &mut |node| {
                if &node.key == key {
                    ControlFlow::Break(node)
                } else {
                    ControlFlow::Continue(())
                }
            })
            .break_value()
        })
    }

    /// Cover host inventory even when catalog filtering or provider loss removes
    /// its normal placement. Collapsed descendants still have a reachable path.
    /// The section is emitted even when empty: hosts anchor workspace creation
    /// to its header, which must stay reachable with no unplaced workspaces.
    /// A workspace with a host entity is covered under that entity, so its
    /// details and placements resolve; others use a synthetic workspace entity,
    /// as does every tab after the first sharing one host entity, keeping each
    /// covered tab's key distinct.
    ///
    /// When a default group (`.default`) is placed, uncovered tabs become its
    /// children instead, in a `.unplaced` loop after its own items, and the
    /// fallback section is emitted empty. With several default groups, the
    /// first placed one (in catalog order) covers them.
    pub(crate) fn cover_workspaces(
        &mut self,
        workspaces: &[ControllerTab],
        host_entities: &BTreeMap<WorkspaceId, EntityRef>,
        default_groups: &[EntityRef],
        stale: &dyn Fn(&EntityRef) -> bool,
    ) {
        let mut covered = std::collections::BTreeSet::new();
        for section in &self.sections {
            let _: ControlFlow<()> = visit_nodes(&section.nodes, &mut |node| {
                if let PresentationState::Live { workspace_id, .. } = node.state {
                    covered.insert(workspace_id);
                }
                ControlFlow::Continue(())
            });
        }
        let mut nodes = Vec::new();
        let mut hosts_used = std::collections::BTreeSet::new();
        for workspace in workspaces {
            if !covered.insert(workspace.tab_id) {
                continue;
            }
            let entity = host_entities
                .get(&workspace.tab_id)
                .filter(|host| hosts_used.insert((*host).clone()))
                .cloned()
                .unwrap_or_else(|| EntityRef::local(system::WORKSPACE, workspace.tab_id.to_string()));
            let key = PlacementKey(vec![PlacementSegment { loop_name: UNPLACED_WORKSPACES_SECTION.into(), entity: entity.clone() }]);
            let is_stale = stale(&entity);
            nodes.push(Rc::new(PlacementNode {
                loop_key: key.loop_key(UNPLACED_WORKSPACES_SECTION).expect("a cover key has its one segment"),
                key,
                entity,
                label: workspace.name.clone(),
                layout: None,
                form: "compact".into(),
                state: PresentationState::Live { workspace_id: workspace.tab_id, selected: workspace.active },
                content: Content {
                    fields: vec![TemplateConfigRenderedField {
                        class: TemplateConfigFieldClass::Required,
                        priority: None,
                        value: workspace.name.clone(),
                        source: None,
                    }],
                    ..Content::default()
                },
                detail: Content::default(),
                facts: Default::default(),
                variables: None,
                collapsed: false,
                stale: is_stale,
                children: Vec::new(),
            }));
        }
        // The first default group (in catalog order) that is placed covers
        // them; an unplaced default doesn't hide a placed one.
        'groups: for group in default_groups {
            for section in &mut self.sections {
                if let Some(parent) = find_node_mut(&mut section.nodes, group) {
                    for mut node in std::mem::take(&mut nodes) {
                        let node_mut = Rc::make_mut(&mut node);
                        let mut segments = parent.key.0.clone();
                        segments.push(PlacementSegment { loop_name: UNPLACED_WORKSPACES_SECTION.into(), entity: node_mut.entity.clone() });
                        node_mut.key = PlacementKey(segments);
                        node_mut.loop_key =
                            node_mut.key.loop_key(&section.name).expect("a cover key has the default group's segment and its own");
                        parent.children.push(node);
                    }
                    break 'groups;
                }
            }
        }
        self.sections.push(Section {
            name: UNPLACED_WORKSPACES_SECTION.into(),
            default_host: None,
            order: None,
            pinned: false,
            content: Content {
                fields: vec![TemplateConfigRenderedField {
                    class: TemplateConfigFieldClass::Required,
                    priority: None,
                    value: "Other workspaces".into(),
                    source: None,
                }],
                ..Content::default()
            },
            nodes,
        });
    }

    /// Resolve each section's placed entities into nodes. A node whose
    /// placement, variables, state, collapse, staleness and children are
    /// what they were last time is the same node, shared with the snapshot
    /// before; `kept` holds those, with what they were resolved under.
    pub(crate) fn resolve(model: &ControllerViewModel, sections: &[EvaluatedSection], cx: SurfaceContext, kept: &mut SurfaceKept) -> Self {
        let mut snapshot = Self {
            display_variables: model.display_variables.clone(),
            display_values: model.display_variable_values.clone(),
            ..Self::default()
        };
        let inputs = (cx.generation, model.sibling_orders.clone());
        let mut prior = std::mem::take(&mut kept.nodes);
        if kept.inputs.as_ref() != Some(&inputs) {
            prior.clear();
            kept.inputs = Some(inputs);
        }
        let mut resolver = Resolver {
            model,
            cx,
            orders: model.sibling_orders.iter().cloned().collect(),
            next: FastHashMap::with_capacity_and_hasher(prior.len(), Default::default()),
            prior,
        };
        let root = model.template_config.effective_variables.iter().find(|variables| variables.node == NodeKey::Root);
        for region in sections {
            let definition = &region.definition;
            let mut metadata = BTreeMap::new();
            effective_metadata(root, &mut metadata);
            let content = resolve_content(region.root.as_ref(), &metadata, false, false, false);
            if let Some(error) = &content.error {
                snapshot.diagnostics.push(format!("region {}: {error}", definition.name));
            }
            let nodes = region.entities.iter().filter_map(|entity| resolver.node(entity, definition)).collect();
            snapshot.sections.push(Section {
                name: definition.name.clone(),
                default_host: definition.default_host.clone(),
                order: definition.order,
                pinned: definition.pinned,
                content,
                nodes: apply_sibling_orders(nodes, &resolver.orders),
            });
        }
        kept.nodes = resolver.next;
        snapshot
    }
}

/// What resolving nodes reads besides their placements.
pub(crate) struct SurfaceContext<'a> {
    pub(crate) generation: u64,
    pub(crate) variables: &'a NodeVariables,
    /// How a node presenting an entity with these facts shows it.
    pub(crate) state: &'a dyn Fn(&EntityRef, &BTreeMap<String, MetadataValue>) -> PresentationState,
    /// Whether the entity a node presents has a stale provider.
    pub(crate) stale: &'a dyn Fn(&EntityRef) -> bool,
}

/// A node's resolution, kept while what it was resolved from is the same.
#[derive(Debug)]
pub(crate) struct SurfaceMemo {
    placement: Rc<EvaluatedPlacement>,
    variables: Option<Rc<EffectiveNodeVariables>>,
    state: PresentationState,
    collapsed: bool,
    stale: bool,
    node: Rc<PlacementNode>,
}

type SiblingOrders = Vec<(PlacementLoopKey, Vec<EntityRef>)>;

/// Resolved nodes, by the address of their placements, with what they were
/// resolved under: the template catalog's generation and the sibling orders.
#[derive(Debug, Default)]
pub(crate) struct SurfaceKept {
    inputs: Option<(u64, SiblingOrders)>,
    nodes: FastHashMap<*const EvaluatedPlacement, SurfaceMemo>,
}

struct Resolver<'a> {
    model: &'a ControllerViewModel,
    cx: SurfaceContext<'a>,
    orders: BTreeMap<PlacementLoopKey, Vec<EntityRef>>,
    prior: FastHashMap<*const EvaluatedPlacement, SurfaceMemo>,
    next: FastHashMap<*const EvaluatedPlacement, SurfaceMemo>,
}

impl Resolver<'_> {
    fn node(&mut self, entity: &Rc<EvaluatedPlacement>, region: &SurfaceRegionDefinition) -> Option<Rc<PlacementNode>> {
        let key = entity.placement.as_ref()?;
        let children: Vec<_> = entity.children.iter().filter_map(|child| self.node(child, region)).collect();
        let children = apply_sibling_orders(children, &self.orders);
        let address = Rc::as_ptr(entity);
        let variables = self.cx.variables.get(&address).map(|memo| memo.resolved.clone());
        let state = (self.cx.state)(&entity.entity, &entity.metadata);
        let collapsed = self.model.collapsed_placements.contains(key);
        let stale = (self.cx.stale)(&entity.entity);
        if let Some(memo) = self.prior.remove(&address) {
            let same = Rc::ptr_eq(&memo.placement, entity)
                && match (&memo.variables, &variables) {
                    (Some(before), Some(now)) => Rc::ptr_eq(before, now),
                    (None, None) => true,
                    _ => false,
                }
                && memo.state == state
                && memo.collapsed == collapsed
                && memo.stale == stale
                && memo.node.children.len() == children.len()
                && memo.node.children.iter().zip(&children).all(|(before, now)| Rc::ptr_eq(before, now));
            if same {
                let node = memo.node.clone();
                self.next.insert(address, memo);
                return Some(node);
            }
        }
        let node = Rc::new(resolve_node(entity, variables.as_deref(), region, state.clone(), collapsed, stale, children)?);
        self.next.insert(address, SurfaceMemo { placement: entity.clone(), variables, state, collapsed, stale, node: node.clone() });
        Some(node)
    }
}

/// Resolve the declared tier without measuring or shortening producer text.
/// Returns true when a shorter tier had to fall back to the full label; a
/// frontend may apply its own measured elision to that fallback.
pub fn apply_declared_abbreviation(
    metadata: &mut BTreeMap<String, MetadataValue>,
    binding: &str,
    explicit: Option<AbbreviationTier>,
) -> bool {
    let Some((selected, producer_supplied)) = declared_abbreviation(metadata, binding, explicit) else {
        return false;
    };
    let selected = selected.to_owned();
    metadata.insert("display.label".to_owned(), MetadataValue::Text(selected.into()));
    !producer_supplied
}

/// The label `binding`'s tier selects, and whether its producer supplied it.
fn declared_abbreviation<'a>(
    metadata: &'a BTreeMap<String, MetadataValue>,
    binding: &str,
    explicit: Option<AbbreviationTier>,
) -> Option<(&'a str, bool)> {
    let tier = explicit.map(|tier| tier.as_str()).or_else(|| metadata_text(metadata, &format!("var.{binding}.tier"))).unwrap_or("full");
    let full = metadata_text(metadata, "display.label");
    let medium = metadata_text(metadata, "display.label.medium");
    let short = metadata_text(metadata, "display.label.short");
    match tier {
        "short" => short.or(medium).map(|label| (label, true)).or_else(|| full.map(|label| (label, false))),
        "medium" => medium.map(|label| (label, true)).or_else(|| full.map(|label| (label, false))),
        _ => full.map(|label| (label, true)),
    }
}

fn metadata_text<'a>(metadata: &'a BTreeMap<String, MetadataValue>, key: &str) -> Option<&'a str> {
    match metadata.get(key) {
        Some(MetadataValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn effective_metadata(
    variables: Option<&EffectiveNodeVariables>,
    metadata: &mut BTreeMap<String, MetadataValue>,
) -> Option<EffectiveNodeVariables> {
    let variables = variables?;
    for (name, value) in &variables.values {
        metadata.insert(format!("var.{name}"), MetadataValue::Text(value.value.as_str().into()));
    }
    Some(variables.clone())
}

/// Apply host-owned sibling orders to each run of siblings sharing a loop
/// invocation. Named entities come first in the saved order; an entity the
/// order doesn't name goes after its nearest data-order predecessor already
/// placed, or first; names that no longer match are skipped.
fn apply_sibling_orders(nodes: Vec<Rc<PlacementNode>>, orders: &BTreeMap<PlacementLoopKey, Vec<EntityRef>>) -> Vec<Rc<PlacementNode>> {
    if orders.is_empty() {
        return nodes;
    }
    let mut out = Vec::with_capacity(nodes.len());
    let mut nodes = nodes.into_iter().peekable();
    while let Some(first) = nodes.next() {
        let mut run = vec![first];
        while nodes.peek().is_some_and(|n| n.loop_key == run[0].loop_key) {
            run.push(nodes.next().unwrap());
        }
        match orders.get(&run[0].loop_key) {
            Some(order) => out.extend(order_run(run, order)),
            None => out.extend(run),
        }
    }
    out
}

fn order_run(data: Vec<Rc<PlacementNode>>, order: &[EntityRef]) -> Vec<Rc<PlacementNode>> {
    // Data indices in display order: saved names first, once each, then each
    // unnamed index after its nearest data-order predecessor already placed.
    let index_of = |entity: &EntityRef| data.iter().position(|n| &n.entity == entity);
    let mut placed: Vec<usize> = Vec::with_capacity(data.len());
    for index in order.iter().filter_map(index_of) {
        if !placed.contains(&index) {
            placed.push(index);
        }
    }
    for index in 0..data.len() {
        if placed.contains(&index) {
            continue;
        }
        let at = (0..index).rev().find_map(|before| placed.iter().position(|&p| p == before)).map_or(0, |position| position + 1);
        placed.insert(at, index);
    }
    let mut data: Vec<Option<Rc<PlacementNode>>> = data.into_iter().map(Some).collect();
    placed.into_iter().filter_map(|index| data[index].take()).collect()
}

fn resolve_node(
    entity: &EvaluatedPlacement,
    variables: Option<&EffectiveNodeVariables>,
    region: &SurfaceRegionDefinition,
    state: PresentationState,
    collapsed: bool,
    stale: bool,
    children: Vec<Rc<PlacementNode>>,
) -> Option<PlacementNode> {
    let key = entity.placement.clone()?;
    let mut facts = entity.metadata.clone();
    let variables = variables.and_then(|variables| effective_metadata(Some(variables), Rc::make_mut(&mut facts)));
    let form = region
        .promotions
        .iter()
        .find(|p| facts.get(&p.when) == Some(&MetadataValue::Bool(true)))
        .map(|p| &p.form)
        .unwrap_or(&region.form)
        .clone();
    let slot = if form == DISPLAY_FORM_COMPACT { entity.templates.compact.as_deref() } else { entity.templates.detail.as_deref() };
    let mut display_facts = facts.clone();
    let declared = declared_abbreviation(&display_facts, &key.0.last()?.loop_name, entity.tier)
        .map(|(label, _)| label)
        .filter(|label| metadata_text(&display_facts, "display.label") != Some(label))
        .map(str::to_owned);
    if let Some(label) = declared {
        Rc::make_mut(&mut display_facts).insert("display.label".to_owned(), MetadataValue::Text(label.into()));
    }
    Some(PlacementNode {
        content: resolve_content(slot, &display_facts, collapsed, !entity.children.is_empty(), true),
        detail: {
            let mut detail = resolve_content(entity.templates.detail.as_deref(), &facts, collapsed, !entity.children.is_empty(), false);
            detail.fields.extend(entity.annotation.related_detail.iter().cloned());
            detail
        },
        layout: entity.annotation.layout.clone(),
        state,
        loop_key: key.loop_key(&region.name)?,
        key,
        entity: entity.entity.clone(),
        label: entity.label.clone(),
        form,
        facts,
        variables,
        collapsed,
        stale,
        children,
    })
}

fn resolve_content(
    slot: Option<&ResolvedTemplateSlot>,
    metadata: &BTreeMap<String, MetadataValue>,
    collapsed: bool,
    collapsible: bool,
    reserve_columns: bool,
) -> Content {
    let Some(slot) = slot else {
        return Content::default();
    };
    let fields = if let Some(ready) = &slot.render_ready {
        let context = TemplateConfigMatchContext {
            slot: TemplateConfigSlot::Compact,
            node_kind: TemplateConfigNodeKind::Entity,
            metadata,
            collapsed,
            collapsible,
            active_tab_name: None,
        };
        ready
            .fields
            .iter()
            .filter(|spec| !spec.structured_only)
            .filter_map(|spec| {
                spec.render(context).or_else(|| {
                    reserve_columns.then(|| TemplateConfigRenderedField {
                        class: spec.class,
                        priority: spec.priority,
                        value: String::new(),
                        source: None,
                    })
                })
            })
            .collect()
    } else {
        slot.fields
            .iter()
            .map(|field| TemplateConfigRenderedField {
                class: TemplateConfigFieldClass::Priority,
                priority: Some(field.priority),
                value: field.text.clone(),
                source: field.source.clone(),
            })
            .collect()
    };
    Content {
        fields,
        controls: slot.render_ready.as_ref().map(|r| r.controls.clone()).unwrap_or_default(),
        chrome: slot.render_ready.as_ref().map(|r| r.chrome.clone()).unwrap_or_default(),
        template_name: Some(slot.template_name.clone()),
        effective_kdl: slot.effective_kdl.clone(),
        error: slot.resolve_error.clone(),
    }
}

#[cfg(test)]
impl SurfaceKept {
    pub(crate) fn nodes_census(&self) -> Vec<&Rc<PlacementNode>> {
        self.nodes.values().map(|memo| &memo.node).collect()
    }
}

#[cfg(test)]
impl PlacementNode {
    /// Count what a node holds, and its children not counted yet.
    pub(crate) fn census(&self, census: &mut Census, prefix: &str) {
        census.key(&format!("{prefix}: keys"), &self.key);
        census.string(&format!("{prefix}: loop keys"), &self.loop_key.region);
        census.key(&format!("{prefix}: loop keys"), &self.loop_key.parent);
        census.string(&format!("{prefix}: loop keys"), &self.loop_key.binding);
        census.entity(&format!("{prefix}: entity refs"), &self.entity);
        census.string(&format!("{prefix}: labels"), &self.label);
        census.string(&format!("{prefix}: forms"), &self.form);
        if let Some(layout) = &self.layout {
            census.string(&format!("{prefix}: layouts"), layout);
        }
        for (what, content) in [("content", &self.content), ("detail", &self.detail)] {
            for field in &content.fields {
                census.string(&format!("{prefix}: {what} field values"), &field.value);
                if let Some(source) = &field.source {
                    census.string(&format!("{prefix}: {what} field sources"), &source.key);
                    census.value(&format!("{prefix}: {what} field sources"), &source.value);
                }
            }
            if let Some(kdl) = content.effective_kdl.written().filter(|kdl| census.first(kdl)) {
                census.string(&format!("{prefix}: {what} effective KDL"), kdl);
            }
            if let Some(name) = &content.template_name {
                census.string(&format!("{prefix}: {what} template names"), name);
            }
        }
        if census.first(&self.facts) {
            census.map(&format!("{prefix}: facts"), &format!("{prefix}: fact keys"), self.facts.keys());
            self.facts.values().for_each(|value| census.value(&format!("{prefix}: fact values"), value));
        }
        if let Some(variables) = &self.variables {
            variables_census(census, &format!("{prefix}: variables"), variables);
        }
        for child in &self.children {
            if census.first(child) {
                child.census(census, prefix);
            }
        }
    }
}
