//! Semantic placement snapshots shared by all frontends.
use std::{collections::BTreeMap, ops::ControlFlow};

use serde::{Deserialize, Serialize};

use crate::template_config::{
    ChromeSpec, TemplateConfigFieldClass, TemplateConfigMatchContext, TemplateConfigNodeKind,
    TemplateConfigRenderedField, TemplateConfigSlot, TemplateControlSpec,
    TemplateVariableDefinition,
};
use crate::{
    ControllerViewModel, DisplayVariableValue, EffectiveNodeVariables, EntityRef, MetadataValue,
    NodeKey, PlacementKey, ResolvedTemplateSlot,
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
    pub nodes: Vec<PlacementNode>,
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
    pub loop_key: crate::PlacementLoopKey,
    pub entity: EntityRef,
    pub label: String,
    /// Logical intent declared on the loop, interpreted by each frontend.
    pub layout: Option<String>,
    pub form: String,
    pub state: PresentationState,
    pub content: Content,
    pub detail: Content,
    pub facts: BTreeMap<String, MetadataValue>,
    pub variables: Option<EffectiveNodeVariables>,
    pub collapsed: bool,
    /// Retained even when collapsed, for inspection and native input handling.
    pub children: Vec<PlacementNode>,
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
        workspace_id: u64,
        selected: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub fields: Vec<TemplateConfigRenderedField>,
    pub controls: Vec<TemplateControlSpec>,
    pub chrome: ChromeSpec,
    pub template_name: Option<String>,
    pub effective_kdl: String,
    pub error: Option<String>,
}

impl Content {
    /// Plain content for a frontend to measure. No terminal elision or styling.
    pub fn text(&self) -> String {
        self.fields
            .iter()
            .map(|f| f.value.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The first placement of `entity`, depth first. A default group placed more
/// than once covers leftover tabs under its first placement only.
fn find_node_mut<'a>(
    nodes: &'a mut [PlacementNode],
    entity: &EntityRef,
) -> Option<&'a mut PlacementNode> {
    for node in nodes {
        if &node.entity == entity {
            return Some(node);
        }
        if let Some(found) = find_node_mut(&mut node.children, entity) {
            return Some(found);
        }
    }
    None
}

// Inspection and coverage both traverse collapsed descendants. The callback can
// stop early for lookup, without allocating an intermediate list of nodes.
fn visit_nodes<'a, B>(
    nodes: &'a [PlacementNode],
    visit: &mut impl FnMut(&'a PlacementNode) -> ControlFlow<B>,
) -> ControlFlow<B> {
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
        self.sections
            .iter()
            .filter(|section| !section.is_empty_workspace_fallback())
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
        workspaces: &[crate::state::ControllerTab],
        host_entities: &BTreeMap<u64, EntityRef>,
        default_groups: &[EntityRef],
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
                .unwrap_or_else(|| EntityRef {
                    kind: system::WORKSPACE.into(),
                    id: workspace.tab_id.to_string(),
                });
            let key = PlacementKey(vec![crate::PlacementSegment {
                loop_name: UNPLACED_WORKSPACES_SECTION.into(),
                entity: entity.clone(),
            }]);
            nodes.push(PlacementNode {
                loop_key: key
                    .loop_key(UNPLACED_WORKSPACES_SECTION)
                    .expect("a cover key has its one segment"),
                key,
                entity,
                label: workspace.name.clone(),
                layout: None,
                form: "compact".into(),
                state: PresentationState::Live {
                    workspace_id: workspace.tab_id,
                    selected: workspace.active,
                },
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
                facts: BTreeMap::new(),
                variables: None,
                collapsed: false,
                children: Vec::new(),
            });
        }
        // The first default group (in catalog order) that is placed covers
        // them; an unplaced default doesn't hide a placed one.
        'groups: for group in default_groups {
            for section in &mut self.sections {
                if let Some(parent) = find_node_mut(&mut section.nodes, group) {
                    for mut node in std::mem::take(&mut nodes) {
                        let mut segments = parent.key.0.clone();
                        segments.push(crate::PlacementSegment {
                            loop_name: UNPLACED_WORKSPACES_SECTION.into(),
                            entity: node.entity.clone(),
                        });
                        node.key = PlacementKey(segments);
                        node.loop_key = node
                            .key
                            .loop_key(&section.name)
                            .expect("a cover key has the default group's segment and its own");
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

    pub(crate) fn resolve(
        model: &ControllerViewModel,
        sections: &[crate::state::EvaluatedSection],
        layouts: &BTreeMap<PlacementKey, crate::state::PlacementAnnotation>,
        states: &BTreeMap<EntityRef, PresentationState>,
    ) -> Self {
        let mut snapshot = Self {
            display_variables: model.display_variables.clone(),
            display_values: model.display_variable_values.clone(),
            ..Self::default()
        };
        let orders: BTreeMap<_, _> = model.sibling_orders.iter().cloned().collect();
        for region in sections {
            let definition = &region.definition;
            let mut metadata = BTreeMap::new();
            effective_metadata(model, &NodeKey::Root, &mut metadata);
            let content = resolve_content(region.root.as_ref(), &metadata, false, false, false);
            if let Some(error) = &content.error {
                snapshot
                    .diagnostics
                    .push(format!("region {}: {error}", definition.name));
            }
            snapshot.sections.push(Section {
                name: definition.name.clone(),
                default_host: definition.default_host.clone(),
                order: definition.order,
                pinned: definition.pinned,
                content,
                nodes: apply_sibling_orders(
                    region
                        .entities
                        .iter()
                        .filter_map(|entity| {
                            resolve_node(model, entity, definition, layouts, states, &orders)
                        })
                        .collect(),
                    &orders,
                ),
            });
        }
        snapshot
    }
}

/// Resolve the declared tier without measuring or shortening producer text.
/// Returns true when a shorter tier had to fall back to the full label; a
/// frontend may apply its own measured elision to that fallback.
pub fn apply_declared_abbreviation(
    metadata: &mut BTreeMap<String, MetadataValue>,
    binding: &str,
    explicit: Option<crate::template_config::AbbreviationTier>,
) -> bool {
    let tier = explicit
        .map(|tier| tier.as_str())
        .or_else(|| metadata_text(metadata, &format!("var.{binding}.tier")))
        .unwrap_or("full");
    let full = metadata_text(metadata, "display.label").map(str::to_owned);
    let medium = metadata_text(metadata, "display.label.medium").map(str::to_owned);
    let short = metadata_text(metadata, "display.label.short").map(str::to_owned);
    let Some((selected, producer_supplied)) = (match tier {
        "short" => short
            .map(|label| (label, true))
            .or_else(|| medium.map(|label| (label, true)))
            .or_else(|| full.map(|label| (label, false))),
        "medium" => medium
            .map(|label| (label, true))
            .or_else(|| full.map(|label| (label, false))),
        _ => full.map(|label| (label, true)),
    }) else {
        return false;
    };
    metadata.insert("display.label".to_owned(), MetadataValue::Text(selected));
    !producer_supplied
}

fn metadata_text<'a>(metadata: &'a BTreeMap<String, MetadataValue>, key: &str) -> Option<&'a str> {
    match metadata.get(key) {
        Some(MetadataValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn effective_metadata(
    model: &ControllerViewModel,
    target: &NodeKey,
    metadata: &mut BTreeMap<String, MetadataValue>,
) -> Option<EffectiveNodeVariables> {
    let variables = model
        .template_config
        .effective_variables
        .iter()
        .find(|v| &v.node == target)?;
    for (name, value) in &variables.values {
        metadata.insert(
            format!("var.{name}"),
            MetadataValue::Text(value.value.clone()),
        );
    }
    Some(variables.clone())
}

/// Apply host-owned sibling orders to each run of siblings sharing a loop
/// invocation. Named entities come first in the saved order; an entity the
/// order doesn't name goes after its nearest data-order predecessor already
/// placed, or first; names that no longer match are skipped.
fn apply_sibling_orders(
    nodes: Vec<PlacementNode>,
    orders: &BTreeMap<crate::PlacementLoopKey, Vec<EntityRef>>,
) -> Vec<PlacementNode> {
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

fn order_run(data: Vec<PlacementNode>, order: &[EntityRef]) -> Vec<PlacementNode> {
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
        let at = (0..index)
            .rev()
            .find_map(|before| placed.iter().position(|&p| p == before))
            .map_or(0, |position| position + 1);
        placed.insert(at, index);
    }
    let mut data: Vec<Option<PlacementNode>> = data.into_iter().map(Some).collect();
    placed
        .into_iter()
        .filter_map(|index| data[index].take())
        .collect()
}

fn resolve_node(
    model: &ControllerViewModel,
    entity: &crate::state::EvaluatedPlacement,
    region: &crate::template_config::SurfaceRegionDefinition,
    layouts: &BTreeMap<PlacementKey, crate::state::PlacementAnnotation>,
    states: &BTreeMap<EntityRef, PresentationState>,
    orders: &BTreeMap<crate::PlacementLoopKey, Vec<EntityRef>>,
) -> Option<PlacementNode> {
    let key = entity.placement.clone()?;
    let mut facts = entity.metadata.clone();
    let variables = effective_metadata(model, &NodeKey::Placement(key.clone()), &mut facts);
    let form = region
        .promotions
        .iter()
        .find(|p| facts.get(&p.when) == Some(&MetadataValue::Bool(true)))
        .map(|p| &p.form)
        .unwrap_or(&region.form)
        .clone();
    let collapsed = model.collapsed_placements.contains(&key);
    let slot = if form == crate::DISPLAY_FORM_COMPACT {
        entity.templates.compact.as_ref()
    } else {
        entity.templates.detail.as_ref()
    };
    let mut display_facts = facts.clone();
    apply_declared_abbreviation(&mut display_facts, &key.0.last()?.loop_name, entity.tier);
    Some(PlacementNode {
        content: resolve_content(
            slot,
            &display_facts,
            collapsed,
            !entity.children.is_empty(),
            true,
        ),
        detail: {
            let mut detail = resolve_content(
                entity.templates.detail.as_ref(),
                &facts,
                collapsed,
                !entity.children.is_empty(),
                false,
            );
            if let Some(annotation) = layouts.get(&key) {
                detail
                    .fields
                    .extend(annotation.related_detail.iter().cloned());
            }
            detail
        },
        layout: layouts
            .get(&key)
            .and_then(|annotation| annotation.layout.clone()),
        state: states.get(&entity.entity).cloned().unwrap_or_default(),
        loop_key: key.loop_key(&region.name)?,
        key,
        entity: entity.entity.clone(),
        label: entity.label.clone(),
        form,
        facts,
        variables,
        collapsed,
        children: apply_sibling_orders(
            entity
                .children
                .iter()
                .filter_map(|child| resolve_node(model, child, region, layouts, states, orders))
                .collect(),
            orders,
        ),
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
        controls: slot
            .render_ready
            .as_ref()
            .map(|r| r.controls.clone())
            .unwrap_or_default(),
        chrome: slot
            .render_ready
            .as_ref()
            .map(|r| r.chrome.clone())
            .unwrap_or_default(),
        template_name: Some(slot.template_name.clone()),
        effective_kdl: slot.effective_kdl.clone(),
        error: slot.resolve_error.clone(),
    }
}
