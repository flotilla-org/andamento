//! Semantic placement snapshots. No terminal coordinates, ANSI, host SDK types,
//! or GroupPath rows cross this interface. The legacy model is an internal input
//! until the placement cutover removes it; consumers use these types instead.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::template_config::{
    ChromeSpec, SurfaceRegionSource, TemplateConfigFieldClass, TemplateConfigMatchContext,
    TemplateConfigNodeKind, TemplateConfigRenderedField, TemplateConfigSlot, TemplateControlSpec,
    TemplateVariableDefinition,
};
use crate::{
    ControllerViewModel, DisplayEntity, DisplayVariableValue, EffectiveNodeVariables, EntityRef,
    MetadataValue, NodeKey, PlacementKey, ResolvedTemplateSlot, PLACEMENT_LOOP_BINDING_KEY,
    PLACEMENT_LOOP_TIER_KEY,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSnapshot {
    pub sections: Vec<Section>,
    pub display_variables: Vec<TemplateVariableDefinition>,
    pub display_values: BTreeMap<String, DisplayVariableValue>,
    /// Legacy tree regions are deliberately not exported as a native contract.
    /// Configure placements for them; these diagnostics prevent silent omission.
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub pinned: bool,
    pub content: Content,
    pub nodes: Vec<PlacementNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementNode {
    pub key: PlacementKey,
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

impl SurfaceSnapshot {
    pub fn node(&self, key: &PlacementKey) -> Option<&PlacementNode> {
        fn find<'a>(nodes: &'a [PlacementNode], key: &PlacementKey) -> Option<&'a PlacementNode> {
            nodes.iter().find_map(|node| {
                if &node.key == key {
                    Some(node)
                } else {
                    find(&node.children, key)
                }
            })
        }
        self.sections
            .iter()
            .find_map(|section| find(&section.nodes, key))
    }

    pub(crate) fn resolve(
        model: &ControllerViewModel,
        layouts: &BTreeMap<PlacementKey, Option<String>>,
        states: &BTreeMap<EntityRef, PresentationState>,
    ) -> Self {
        let mut snapshot = Self {
            display_variables: model.display_variables.clone(),
            display_values: model.display_variable_values.clone(),
            ..Self::default()
        };
        for region in &model.surface_regions {
            let definition = &region.definition;
            if definition.placement.is_none()
                && matches!(
                    definition.source,
                    SurfaceRegionSource::Tree | SurfaceRegionSource::Attention
                )
            {
                snapshot.diagnostics.push(format!(
                    "region {} needs a placement declaration for native rendering",
                    definition.name
                ));
                continue;
            }
            let mut metadata = BTreeMap::new();
            effective_metadata(model, &NodeKey::Root, &mut metadata);
            let content = resolve_content(region.root.as_ref(), &metadata, false, false);
            snapshot.sections.push(Section {
                name: definition.name.clone(),
                pinned: definition.pinned,
                content,
                nodes: region
                    .entities
                    .iter()
                    .filter_map(|entity| resolve_node(model, entity, definition, layouts, states))
                    .collect(),
            });
        }
        snapshot
    }
}

/// Resolve the declared tier without measuring or shortening producer text.
/// Returns true when a shorter tier had to fall back to the full label; a
/// frontend may apply its own measured elision to that fallback.
pub fn apply_declared_abbreviation(metadata: &mut BTreeMap<String, MetadataValue>) -> bool {
    let Some(binding) = metadata_text(metadata, PLACEMENT_LOOP_BINDING_KEY).map(str::to_owned)
    else {
        return false;
    };
    let tier = metadata_text(metadata, PLACEMENT_LOOP_TIER_KEY)
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

fn resolve_node(
    model: &ControllerViewModel,
    entity: &DisplayEntity,
    region: &crate::template_config::SurfaceRegionDefinition,
    layouts: &BTreeMap<PlacementKey, Option<String>>,
    states: &BTreeMap<EntityRef, PresentationState>,
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
    apply_declared_abbreviation(&mut display_facts);
    Some(PlacementNode {
        content: resolve_content(slot, &display_facts, collapsed, !entity.children.is_empty()),
        detail: resolve_content(
            entity.templates.detail.as_ref(),
            &facts,
            collapsed,
            !entity.children.is_empty(),
        ),
        layout: layouts.get(&key).cloned().flatten(),
        state: states.get(&entity.entity).cloned().unwrap_or_default(),
        key,
        entity: entity.entity.clone(),
        label: entity.label.clone(),
        form,
        facts,
        variables,
        collapsed,
        children: entity
            .children
            .iter()
            .filter_map(|child| resolve_node(model, child, region, layouts, states))
            .collect(),
    })
}

fn resolve_content(
    slot: Option<&ResolvedTemplateSlot>,
    metadata: &BTreeMap<String, MetadataValue>,
    collapsed: bool,
    collapsible: bool,
) -> Content {
    let Some(slot) = slot else {
        return Content::default();
    };
    let fields = if let Some(ready) = &slot.render_ready {
        ready.render_fields(TemplateConfigMatchContext {
            slot: TemplateConfigSlot::Compact,
            node_kind: TemplateConfigNodeKind::Entity,
            metadata,
            collapsed,
            collapsible,
            active_tab_name: None,
        })
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
