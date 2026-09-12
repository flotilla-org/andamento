use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::{Deserialize, Serialize};

use crate::{MetadataValue, ResolvedTemplateFieldSource};

pub fn parse_template_config_json(
    input: &str,
) -> Result<ExternalTemplateConfig, TemplateConfigError> {
    let config = serde_json::from_str::<ExternalTemplateConfig>(input)
        .map_err(|source| TemplateConfigError::Parse(source.to_string()))?;
    config.validate()?;
    Ok(config)
}

pub fn parse_template_config_kdl(
    input: &str,
) -> Result<ExternalTemplateConfig, TemplateConfigError> {
    let document = input
        .parse::<KdlDocument>()
        .map_err(|source| TemplateConfigError::Parse(source.to_string()))?;
    let version = document
        .get_arg("version")
        .map(kdl_u32)
        .transpose()?
        .unwrap_or_else(default_template_config_version);
    let templates = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "template")
        .map(parse_kdl_template)
        .collect::<Result<Vec<_>, _>>()?;
    let fragments = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "fragment")
        .map(parse_kdl_fragment)
        .collect::<Result<Vec<_>, _>>()?;
    let variables = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "variable")
        .map(parse_kdl_node_variable)
        .collect::<Result<Vec<_>, _>>()?;
    let display_variables = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "display-variable")
        .map(parse_kdl_display_variable)
        .collect::<Result<Vec<_>, _>>()?;
    let sets = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "set")
        .map(parse_kdl_variable_setter)
        .collect::<Result<Vec<_>, _>>()?;
    let regions = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "region")
        .map(parse_kdl_region)
        .collect::<Result<Vec<_>, _>>()?;
    let placements = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "placement")
        .map(parse_kdl_placement)
        .collect::<Result<Vec<_>, _>>()?;
    let config = ExternalTemplateConfig {
        version,
        templates,
        fragments,
        variables,
        display_variables,
        sets,
        regions,
        placements,
    };
    config.validate()?;
    Ok(config)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExternalTemplateConfig {
    #[serde(default = "default_template_config_version")]
    pub version: u32,
    #[serde(default)]
    pub templates: Vec<TemplateConfigDefinition>,
    #[serde(default)]
    pub fragments: Vec<TemplateConfigFragmentDefinition>,
    #[serde(default)]
    pub variables: Vec<NodeVariableDefinition>,
    #[serde(default)]
    pub display_variables: Vec<TemplateVariableDefinition>,
    #[serde(default)]
    pub sets: Vec<TemplateVariableSetter>,
    #[serde(default)]
    pub regions: Vec<SurfaceRegionDefinition>,
    #[serde(default)]
    pub placements: Vec<PlacementDefinition>,
}

impl ExternalTemplateConfig {
    fn validate(&self) -> Result<(), TemplateConfigError> {
        if self.version != 1 {
            return Err(TemplateConfigError::Validation(format!(
                "unsupported template config version: {}",
                self.version
            )));
        }
        let mut names = BTreeSet::new();
        for template in &self.templates {
            if template.name.trim().is_empty() {
                return Err(TemplateConfigError::Validation(
                    "template name cannot be empty".to_owned(),
                ));
            }
            if !names.insert(template.name.clone()) {
                return Err(TemplateConfigError::Validation(format!(
                    "duplicate template name: {}",
                    template.name
                )));
            }
            if template.operations.is_empty()
                && template.sets.is_empty()
                && template.loops.is_empty()
                && template.apply_template.is_none()
                && template.extends.is_none()
            {
                return Err(TemplateConfigError::Validation(format!(
                    "template {} must define at least one field, set, or extends",
                    template.name
                )));
            }
            let mut field_names = BTreeSet::new();
            for operation in &template.operations {
                if let TemplateConfigFieldOperation::Set { field } = operation {
                    if !field_names.insert(field.name.clone()) {
                        return Err(TemplateConfigError::Validation(format!(
                            "template {} has duplicate field name: {}",
                            template.name, field.name
                        )));
                    }
                    if field.sources.is_empty() {
                        return Err(TemplateConfigError::Validation(format!(
                            "template {} has a field with no sources",
                            template.name
                        )));
                    }
                }
            }
            let mut setter_names = BTreeSet::new();
            for setter in &template.sets {
                setter.validate()?;
                if !setter_names.insert(setter.name.clone()) {
                    return Err(TemplateConfigError::Validation(format!(
                        "template {} has duplicate variable setter: {}",
                        template.name, setter.name
                    )));
                }
            }
            let template_binding = template
                .name
                .split('/')
                .next()
                .unwrap_or(&template.name)
                .to_owned();
            validate_placement_loops(&template.loops, &BTreeSet::from([template_binding]))?;
        }
        let mut fragment_names = BTreeSet::new();
        for fragment in &self.fragments {
            if fragment.name.trim().is_empty() {
                return Err(TemplateConfigError::Validation(
                    "fragment name cannot be empty".to_owned(),
                ));
            }
            if !fragment_names.insert(fragment.name.clone()) {
                return Err(TemplateConfigError::Validation(format!(
                    "duplicate fragment name: {}",
                    fragment.name
                )));
            }
            let mut field_names = BTreeSet::new();
            for operation in &fragment.operations {
                if let TemplateConfigFieldOperation::Set { field } = operation {
                    if !field_names.insert(field.name.clone()) {
                        return Err(TemplateConfigError::Validation(format!(
                            "fragment {} has duplicate field name: {}",
                            fragment.name, field.name
                        )));
                    }
                    if field.sources.is_empty() {
                        return Err(TemplateConfigError::Validation(format!(
                            "fragment {} has a field with no sources",
                            fragment.name
                        )));
                    }
                }
            }
        }
        for placement in &self.placements {
            validate_placement_loops(&placement.loops, &BTreeSet::new())?;
        }
        let mut variable_names = BTreeSet::new();
        for variable in &self.variables {
            if variable.name.trim().is_empty() {
                return Err(TemplateConfigError::Validation(
                    "variable name cannot be empty".to_owned(),
                ));
            }
            if !variable_names.insert(variable.name.clone()) {
                return Err(TemplateConfigError::Validation(format!(
                    "duplicate variable name: {}",
                    variable.name
                )));
            }
            variable.validate()?;
        }
        let local_variables = self
            .variables
            .iter()
            .map(|variable| (variable.name.as_str(), variable))
            .collect::<BTreeMap<_, _>>();
        for (scope, setter) in
            self.sets
                .iter()
                .map(|setter| ("config", setter))
                .chain(self.templates.iter().flat_map(|template| {
                    template
                        .sets
                        .iter()
                        .map(move |setter| (template.name.as_str(), setter))
                }))
        {
            if local_variables
                .get(setter.name.as_str())
                .is_some_and(|variable| !variable.accepts(&setter.value))
            {
                return Err(TemplateConfigError::Validation(format!(
                    "{scope} sets variable {} to disallowed value {}",
                    setter.name, setter.value
                )));
            }
        }
        let mut display_variable_names = BTreeSet::new();
        for variable in &self.display_variables {
            if variable.name.trim().is_empty() {
                return Err(TemplateConfigError::Validation(
                    "display variable name cannot be empty".to_owned(),
                ));
            }
            if !display_variable_names.insert(variable.name.clone()) {
                return Err(TemplateConfigError::Validation(format!(
                    "duplicate display variable name: {}",
                    variable.name
                )));
            }
            variable.validate()?;
        }
        let mut setter_names = BTreeSet::new();
        for setter in &self.sets {
            setter.validate()?;
            if !setter_names.insert(setter.name.clone()) {
                return Err(TemplateConfigError::Validation(format!(
                    "duplicate config variable setter: {}",
                    setter.name
                )));
            }
        }
        let mut region_names = BTreeSet::new();
        for region in &self.regions {
            if region.name.trim().is_empty() {
                return Err(TemplateConfigError::Validation(
                    "region name cannot be empty".to_owned(),
                ));
            }
            if !region_names.insert(region.name.clone()) {
                return Err(TemplateConfigError::Validation(format!(
                    "duplicate region name: {}",
                    region.name
                )));
            }
            if region.form.trim().is_empty() {
                return Err(TemplateConfigError::Validation(format!(
                    "region {} form cannot be empty",
                    region.name
                )));
            }
            // A named placement must resolve. Falling through silently to the
            // legacy path (or to nothing) on a typo is exactly the failure mode
            // this slice refuses everywhere else.
            //
            // Checked within the document: regions all come from one layer, and
            // a config that declares a region names its placements beside it. A
            // region referencing a placement from another layer would need this
            // moved to catalog construction, where the whole stack is visible.
            if let Some(placement) = region.placement.as_deref() {
                if !self
                    .placements
                    .iter()
                    .any(|declared| declared.name == placement)
                {
                    return Err(TemplateConfigError::Validation(format!(
                        "region {} names placement {placement}, which is not declared",
                        region.name
                    )));
                }
            }
            // A placement selects the region's entities itself, so the legacy
            // single-key attention filter is not needed alongside it.
            if region.source == SurfaceRegionSource::Attention
                && region.placement.is_none()
                && region.attention_key.as_deref().is_none_or(str::is_empty)
            {
                return Err(TemplateConfigError::Validation(format!(
                    "attention region {} must declare attention-key",
                    region.name
                )));
            }
            for promotion in &region.promotions {
                if promotion.when.trim().is_empty() || promotion.form.trim().is_empty() {
                    return Err(TemplateConfigError::Validation(format!(
                        "region {} promotion must declare non-empty when and form",
                        region.name
                    )));
                }
            }
        }
        Ok(())
    }
}

fn validate_placement_loops(
    loops: &[PlacementLoop],
    enclosing: &BTreeSet<String>,
) -> Result<(), TemplateConfigError> {
    let mut bindings = BTreeSet::new();
    for loop_definition in loops {
        if !bindings.insert(loop_definition.binding.clone()) {
            return Err(TemplateConfigError::Validation(format!(
                "duplicate sibling loop binding {}",
                loop_definition.binding
            )));
        }
        if loop_definition.binding.ends_with('s') {
            return Err(TemplateConfigError::Validation(format!(
                "loop binding {} must be singular",
                loop_definition.binding
            )));
        }
        if enclosing.contains(&loop_definition.binding) {
            return Err(TemplateConfigError::Validation(format!(
                "loop binding {} shadows an enclosing binding",
                loop_definition.binding
            )));
        }
        for predicate in &loop_definition.predicates {
            if predicate.value.is_some() == predicate.of.is_some() {
                return Err(TemplateConfigError::Validation(format!(
                    "loop {} predicate {} must declare exactly one of value or of",
                    loop_definition.binding, predicate.key
                )));
            }
            if let Some(of) = predicate.of.as_ref() {
                if !enclosing.contains(of) {
                    return Err(TemplateConfigError::Validation(format!(
                        "loop {} refers to unbound enclosing loop {of}",
                        loop_definition.binding
                    )));
                }
            }
        }
        let mut scope = enclosing.clone();
        scope.insert(loop_definition.binding.clone());
        validate_placement_loops(&loop_definition.loops, &scope)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigLayerKind {
    User,
    Repo,
    Project,
    Fleet,
    Bundled,
}

impl TemplateConfigLayerKind {
    fn rank(self) -> u8 {
        match self {
            Self::User => 0,
            Self::Repo => 1,
            Self::Project => 2,
            Self::Fleet => 3,
            Self::Bundled => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Repo => "repo",
            Self::Project => "project",
            Self::Fleet => "fleet",
            Self::Bundled => "bundled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateConfigOrigin {
    pub layer: TemplateConfigLayerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership: Option<String>,
    pub source: String,
}

impl TemplateConfigOrigin {
    pub fn label(&self) -> String {
        match self.membership.as_deref() {
            Some(membership) => format!("{}:{} ({})", self.layer.as_str(), membership, self.source),
            None => format!("{} ({})", self.layer.as_str(), self.source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigLayer {
    origin: TemplateConfigOrigin,
    config: ExternalTemplateConfig,
}

impl TemplateConfigLayer {
    pub fn user(source: impl Into<String>, config: ExternalTemplateConfig) -> Self {
        Self::new(TemplateConfigLayerKind::User, None, source, config)
    }

    pub fn repo(
        membership: impl Into<String>,
        source: impl Into<String>,
        config: ExternalTemplateConfig,
    ) -> Self {
        Self::new(
            TemplateConfigLayerKind::Repo,
            Some(membership.into()),
            source,
            config,
        )
    }

    pub fn project(
        membership: impl Into<String>,
        source: impl Into<String>,
        config: ExternalTemplateConfig,
    ) -> Self {
        Self::new(
            TemplateConfigLayerKind::Project,
            Some(membership.into()),
            source,
            config,
        )
    }

    pub fn fleet(source: impl Into<String>, config: ExternalTemplateConfig) -> Self {
        Self::new(TemplateConfigLayerKind::Fleet, None, source, config)
    }

    pub fn bundled(source: impl Into<String>, config: ExternalTemplateConfig) -> Self {
        Self::new(TemplateConfigLayerKind::Bundled, None, source, config)
    }

    fn new(
        layer: TemplateConfigLayerKind,
        membership: Option<String>,
        source: impl Into<String>,
        config: ExternalTemplateConfig,
    ) -> Self {
        Self {
            origin: TemplateConfigOrigin {
                layer,
                membership,
                source: source.into(),
            },
            config,
        }
    }

    fn applies_to(&self, metadata: &BTreeMap<String, MetadataValue>) -> bool {
        let membership_key = match self.origin.layer {
            TemplateConfigLayerKind::Repo => Some("vcs.repo"),
            TemplateConfigLayerKind::Project => Some("flotilla.project"),
            TemplateConfigLayerKind::User
            | TemplateConfigLayerKind::Fleet
            | TemplateConfigLayerKind::Bundled => None,
        };
        membership_key
            .is_none_or(|key| self.origin.membership.as_deref() == metadata_text(metadata, key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigCatalog {
    layers: Vec<TemplateConfigLayer>,
    display_variables: Vec<TemplateVariableDefinition>,
    regions: Vec<SurfaceRegionDefinition>,
    placements: Vec<PlacementDefinition>,
}

impl Default for TemplateConfigCatalog {
    fn default() -> Self {
        Self::from_layers(vec![TemplateConfigLayer::bundled(
            "templates/flotilla-default.kdl",
            bundled_default_config(),
        )])
    }
}

impl TemplateConfigCatalog {
    pub fn from_config(config: ExternalTemplateConfig) -> Self {
        Self::from_layers(vec![TemplateConfigLayer::user("<memory>", config)])
    }

    pub fn with_bundled_defaults(config: ExternalTemplateConfig) -> Self {
        Self::from_layers(vec![
            TemplateConfigLayer::user("<configured>", config),
            TemplateConfigLayer::bundled(
                "templates/flotilla-default.kdl",
                bundled_default_config(),
            ),
        ])
    }

    pub fn from_layers(mut layers: Vec<TemplateConfigLayer>) -> Self {
        layers.sort_by_key(|layer| layer.origin.layer.rank());
        let mut seen_display_variables = BTreeSet::new();
        let display_variables = layers
            .iter()
            .flat_map(|layer| layer.config.display_variables.iter())
            .filter(|variable| seen_display_variables.insert(variable.name.clone()))
            .cloned()
            .collect();
        let regions = layers
            .iter()
            .find_map(|layer| {
                (!layer.config.regions.is_empty()).then(|| layer.config.regions.clone())
            })
            .unwrap_or_default();
        let mut seen_placements = BTreeSet::new();
        let placements = layers
            .iter()
            .flat_map(|layer| layer.config.placements.iter())
            .filter(|placement| seen_placements.insert(placement.name.clone()))
            .cloned()
            .collect();
        Self {
            layers,
            display_variables,
            regions,
            placements,
        }
    }

    pub fn resolve(
        &self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Result<Option<TemplateConfigResolved>, TemplateConfigResolveError> {
        let Some((requested_name, declared)) = template_name_for(context) else {
            return Ok(None);
        };
        let stack = self.layer_stack(context.metadata);
        let found = find_template(&stack, &requested_name).or_else(|| {
            (!declared).then(|| {
                let bundled_name = format!("flotilla/{requested_name}");
                find_bundled_template(&stack, &bundled_name).or_else(|| {
                    (context.node_kind == TemplateConfigNodeKind::Entity)
                        .then(|| {
                            let fallback_name =
                                format!("flotilla/entity/{}", context.slot.convention_form()?);
                            find_bundled_template(&stack, &fallback_name)
                        })
                        .flatten()
                })
            })?
        });
        let Some((layer, template)) = found else {
            if declared {
                return Err(TemplateConfigResolveError::UnknownDeclaredTemplate(
                    requested_name,
                ));
            }
            return Ok(None);
        };
        let mut template_stack = vec![];
        let mut fragment_stack = vec![];
        let mut resolved = flatten_template(
            &stack,
            layer,
            template,
            &mut template_stack,
            &mut fragment_stack,
        )?;
        resolved.name = requested_name;
        if resolved.slot != context.slot || resolved.node_kind != context.node_kind {
            return Err(TemplateConfigResolveError::BindingMismatch {
                name: resolved.name,
                expected_slot: context.slot,
                actual_slot: resolved.slot,
                expected_node_kind: context.node_kind,
                actual_node_kind: resolved.node_kind,
            });
        }
        Ok(Some(resolved))
    }

    pub fn len(&self) -> usize {
        self.layers
            .iter()
            .map(|layer| layer.config.templates.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn template_names(&self) -> Vec<String> {
        self.layers
            .iter()
            .flat_map(|layer| layer.config.templates.iter())
            .map(|template| template.name.clone())
            .collect()
    }

    pub fn display_variables(&self) -> &[TemplateVariableDefinition] {
        &self.display_variables
    }

    pub fn variables(
        &self,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> Vec<ResolvedNodeVariableDefinition> {
        self.variable_scope(metadata).declarations
    }

    pub fn variable_scope(
        &self,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> ResolvedVariableScope {
        let stack = self.layer_stack(metadata);
        let mut seen = BTreeSet::new();
        let declarations = stack
            .iter()
            .copied()
            .flat_map(|layer| {
                layer.config.variables.iter().map(move |definition| {
                    ResolvedNodeVariableDefinition {
                        definition: definition.clone(),
                        origin: layer.origin.clone(),
                    }
                })
            })
            .filter(|variable| seen.insert(variable.definition.name.clone()))
            .collect::<Vec<_>>();
        let declaration_by_name = declarations
            .iter()
            .map(|resolved| (resolved.definition.name.as_str(), &resolved.definition))
            .collect::<BTreeMap<_, _>>();
        let mut setters = vec![];
        let mut warnings = vec![];
        let mut seen = BTreeSet::new();
        for resolved in stack.iter().copied().flat_map(|layer| {
            layer
                .config
                .sets
                .iter()
                .map(move |setter| ResolvedVariableSetter {
                    setter: setter.clone(),
                    origin: layer.origin.clone(),
                })
        }) {
            if !seen.insert(resolved.setter.name.clone()) {
                continue;
            }
            let Some(declaration) = declaration_by_name.get(resolved.setter.name.as_str()) else {
                warnings.push(format!(
                    "{} sets undeclared node variable {}",
                    resolved.origin.label(),
                    resolved.setter.name,
                ));
                continue;
            };
            if !declaration.accepts(&resolved.setter.value) {
                warnings.push(format!(
                    "{} sets node variable {} to disallowed value {}",
                    resolved.origin.label(),
                    resolved.setter.name,
                    resolved.setter.value,
                ));
                continue;
            }
            setters.push(resolved);
        }
        ResolvedVariableScope {
            declarations,
            setters,
            warnings,
        }
    }

    pub fn config_variable_setters(
        &self,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> Vec<ResolvedVariableSetter> {
        self.variable_scope(metadata).setters
    }

    pub fn regions(&self) -> &[SurfaceRegionDefinition] {
        &self.regions
    }

    pub fn placement(&self, name: &str) -> Option<&PlacementDefinition> {
        self.placements
            .iter()
            .find(|placement| placement.name == name)
    }

    pub fn placement_template<'a>(
        &'a self,
        name: &str,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> Option<&'a TemplateConfigDefinition> {
        self.layer_stack(metadata).into_iter().find_map(|layer| {
            layer
                .config
                .templates
                .iter()
                .find(|template| template.name == name)
        })
    }

    /// Resolve the render surface of a template reached through
    /// `apply-template`. Placement-only templates may omit the normal slot and
    /// node-kind declarations, for which compact/entity is the rendering
    /// convention, but otherwise use the same inheritance and operation
    /// pipeline as every other template consumer.
    pub fn resolve_placement_template(
        &self,
        name: &str,
        metadata: &BTreeMap<String, MetadataValue>,
    ) -> Result<Option<TemplateConfigResolved>, TemplateConfigResolveError> {
        let stack = self.layer_stack(metadata);
        let Some((layer, template)) = find_template(&stack, name) else {
            return Ok(None);
        };
        let mut inherited_slot = None;
        let mut inherited_node_kind = None;
        let mut cursor = Some(template);
        let mut seen = BTreeSet::new();
        while let Some(candidate) = cursor {
            if !seen.insert(candidate.name.as_str()) {
                break;
            }
            inherited_slot = inherited_slot.or(candidate.slot);
            inherited_node_kind = inherited_node_kind.or(candidate.node_kind);
            if inherited_slot.is_some() && inherited_node_kind.is_some() {
                break;
            }
            cursor = candidate
                .extends
                .as_deref()
                .and_then(|parent| find_template(&stack, parent).map(|(_, template)| template));
        }
        let mut template = template.clone();
        template.slot = inherited_slot.or(Some(TemplateConfigSlot::Compact));
        template.node_kind = inherited_node_kind.or(Some(TemplateConfigNodeKind::Entity));
        let mut resolved = flatten_template(&stack, layer, &template, &mut vec![], &mut vec![])?;
        resolved.name = name.to_owned();
        Ok(Some(resolved))
    }

    fn layer_stack(&self, metadata: &BTreeMap<String, MetadataValue>) -> Vec<&TemplateConfigLayer> {
        self.layers
            .iter()
            .filter(|layer| layer.applies_to(metadata))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceRegionSource {
    Header,
    Attention,
    Tree,
    Controls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SurfaceRegionDefinition {
    pub name: String,
    pub source: SurfaceRegionSource,
    pub root_template: String,
    pub form: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_key: Option<String>,
    /// Names a `placement` that supplies this region's contents. When absent
    /// the region keeps the legacy pipeline, which stays the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub promotions: Vec<SurfaceFormPromotion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SurfaceFormPromotion {
    pub when: String,
    pub form: String,
}

/// A named placement: a section built by pulling entities in, rather than by
/// entities pushing themselves into a grouping path.
///
/// This slice carries exactly one flat loop. Nesting, bindings and
/// `apply-template` are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PlacementDefinition {
    pub name: String,
    pub loops: Vec<PlacementLoop>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PlacementLoop {
    /// The name this loop binds its current entity to. Unused until nesting
    /// arrives, but part of the syntax from the start so configs do not churn.
    pub binding: String,
    pub predicates: Vec<PlacementPredicate>,
    /// Fact keys are compared in declaration order. Entity identity is always
    /// appended as the final tie-break, so both declared and undeclared order
    /// are total and stable across model rebuilds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<PlacementOrder>,
    #[serde(default)]
    pub fields: Vec<TemplateConfigFieldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loops: Vec<PlacementLoop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PlacementOrder {
    pub key: String,
    #[serde(default)]
    pub direction: PlacementOrderDirection,
    #[serde(default)]
    pub absent: PlacementOrderAbsent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlacementOrderDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlacementOrderAbsent {
    First,
    #[default]
    Last,
}

/// Fact equality against a constant.
///
/// Every predicate must be answerable by the `(key, value)` index the
/// controller builds once per model. That is a hard constraint, not a
/// stylistic one: an unindexable predicate turns a nested loop into a scan,
/// and nested scans are quadratic in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PlacementPredicate {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
}

/// The text a value is indexed under. Predicates compare against this, so a
/// config writes `value="true"` for a bool and `value="3"` for an integer.
///
/// Values with no single indexable text — lists, group paths — return `None`
/// and simply never match, rather than matching something surprising.
pub fn placement_index_text(value: &MetadataValue) -> Option<String> {
    match value {
        MetadataValue::Text(text) => Some(text.clone()),
        MetadataValue::Bool(flag) => Some(flag.to_string()),
        MetadataValue::Integer(number) => Some(number.to_string()),
        MetadataValue::StringList(_) | MetadataValue::GroupPath(_) => None,
    }
}

fn bundled_default_config() -> ExternalTemplateConfig {
    parse_template_config_kdl(include_str!("../../../templates/flotilla-default.kdl"))
        .expect("bundled template config must remain valid")
}

#[derive(Debug, Clone, Copy)]
pub struct TemplateConfigMatchContext<'a> {
    pub slot: TemplateConfigSlot,
    pub node_kind: TemplateConfigNodeKind,
    pub metadata: &'a BTreeMap<String, MetadataValue>,
    pub collapsed: bool,
    pub collapsible: bool,
    pub active_tab_name: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigResolved {
    pub name: String,
    pub slot: TemplateConfigSlot,
    pub node_kind: TemplateConfigNodeKind,
    pub fields: Vec<TemplateConfigResolvedField>,
    pub controls: Vec<TemplateControlSpec>,
    pub chrome: ChromeSpec,
    pub setters: Vec<ResolvedVariableSetter>,
    pub chain: Vec<TemplateConfigChainEntry>,
    pub is_bundled: bool,
}

impl TemplateConfigResolved {
    pub fn render_fields(
        &self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Vec<TemplateConfigRenderedField> {
        self.fields
            .iter()
            .filter_map(|field| field.spec.render(context))
            .collect()
    }

    pub fn render_ready(&self) -> TemplateConfigRenderReady {
        TemplateConfigRenderReady {
            fields: self.fields.iter().map(|field| field.spec.clone()).collect(),
            controls: self.controls.clone(),
            chrome: self.chrome.clone(),
        }
    }

    pub fn dump_kdl(&self) -> String {
        let chain = self
            .chain
            .iter()
            .map(|entry| format!("{} [{}]", entry.name, entry.origin.label()))
            .collect::<Vec<_>>()
            .join(" -> ");
        let mut output = format!("// chain: {chain}\n");
        output.push_str(&format!(
            "template {} slot={} node-kind={} {{\n",
            quote_kdl(&self.name),
            quote_kdl(self.slot.as_str()),
            quote_kdl(self.node_kind.as_str()),
        ));
        for setter in &self.setters {
            output.push_str(&format!(
                "  // origin: {}\n  set {} {}\n",
                setter.origin.label(),
                quote_kdl(&setter.setter.name),
                quote_kdl(&setter.setter.value),
            ));
        }
        for primitive in &self.chrome.primitives {
            output.push_str(&primitive.to_kdl(2));
        }
        for field in &self.fields {
            output.push_str(&format!("  // origin: {}\n", field.origin.label()));
            output.push_str(&field.spec.to_kdl(2));
        }
        for control in &self.controls {
            output.push_str(&control.to_kdl(2));
        }
        output.push_str("}\n");
        output
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigResolvedField {
    pub spec: TemplateConfigFieldSpec,
    pub origin: TemplateConfigOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigChainEntry {
    pub name: String,
    pub origin: TemplateConfigOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateConfigResolveError {
    UnknownDeclaredTemplate(String),
    UnknownParent {
        template: String,
        parent: String,
    },
    UnknownFragment {
        template: String,
        fragment: String,
    },
    TemplateCycle(Vec<String>),
    FragmentCycle(Vec<String>),
    MissingProperty {
        template: String,
        property: &'static str,
    },
    RemoveUnknownField {
        template: String,
        field: String,
    },
    BindingMismatch {
        name: String,
        expected_slot: TemplateConfigSlot,
        actual_slot: TemplateConfigSlot,
        expected_node_kind: TemplateConfigNodeKind,
        actual_node_kind: TemplateConfigNodeKind,
    },
}

impl fmt::Display for TemplateConfigResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownDeclaredTemplate(name) => {
                write!(formatter, "declared template not found: {name}")
            }
            Self::UnknownParent { template, parent } => {
                write!(formatter, "template {template} extends unknown template {parent}")
            }
            Self::UnknownFragment { template, fragment } => {
                write!(formatter, "template {template} uses unknown fragment {fragment}")
            }
            Self::TemplateCycle(chain) => {
                write!(formatter, "template inheritance cycle: {}", chain.join(" -> "))
            }
            Self::FragmentCycle(chain) => {
                write!(formatter, "fragment use cycle: {}", chain.join(" -> "))
            }
            Self::MissingProperty { template, property } => {
                write!(formatter, "template {template} has no inherited or declared {property}")
            }
            Self::RemoveUnknownField { template, field } => {
                write!(formatter, "template {template} removes unknown field {field}")
            }
            Self::BindingMismatch {
                name,
                expected_slot,
                actual_slot,
                expected_node_kind,
                actual_node_kind,
            } => write!(
                formatter,
                "template {name} resolves as {actual_node_kind:?}/{actual_slot:?}, expected {expected_node_kind:?}/{expected_slot:?}"
            ),
        }
    }
}

impl Error for TemplateConfigResolveError {}

fn template_name_for(context: TemplateConfigMatchContext<'_>) -> Option<(String, bool)> {
    if let Some(name) = metadata_text(context.metadata, "presentation.template") {
        return Some((name.to_owned(), true));
    }
    let name = match context.node_kind {
        TemplateConfigNodeKind::Entity => {
            let kind = metadata_text(context.metadata, "entity.kind")?;
            format!("{kind}/{}", context.slot.convention_form()?)
        }
        TemplateConfigNodeKind::Tab => match context.slot {
            TemplateConfigSlot::TabTitle => "tab/title".to_owned(),
            TemplateConfigSlot::TabStatus => "tab/status".to_owned(),
            _ => return None,
        },
        TemplateConfigNodeKind::Group => {
            let kind = metadata_text(context.metadata, "presentation.kind")
                .or_else(|| metadata_text(context.metadata, "group.key").map(group_kind))
                .unwrap_or("group");
            format!("{kind}/full")
        }
    };
    Some((name, false))
}

fn group_kind(key: &str) -> &str {
    match key {
        "flotilla.project" | "andamento.project" => "project",
        "vcs.repo" | "git.repo" => "repo",
        "flotilla.convoy" => "convoy",
        "flotilla.vessel" => "vessel",
        "flotilla.session" | "session" => "session",
        "flotilla.independent" => "session",
        "flotilla.issue" | "issue" => "issue",
        "flotilla.checkout" => "checkout",
        "project" | "repo" | "worktree" | "git.branch" | "branch" | "zellij.pane.cwd" => "group",
        _ => key,
    }
}

fn find_template<'a>(
    stack: &'a [&'a TemplateConfigLayer],
    name: &str,
) -> Option<(&'a TemplateConfigLayer, &'a TemplateConfigDefinition)> {
    stack.iter().find_map(|layer| {
        layer
            .config
            .templates
            .iter()
            .find(|template| template.name == name)
            .map(|template| (*layer, template))
    })
}

fn find_bundled_template<'a>(
    stack: &'a [&'a TemplateConfigLayer],
    name: &str,
) -> Option<(&'a TemplateConfigLayer, &'a TemplateConfigDefinition)> {
    stack
        .iter()
        .filter(|layer| layer.origin.layer == TemplateConfigLayerKind::Bundled)
        .find_map(|layer| {
            layer
                .config
                .templates
                .iter()
                .find(|template| template.name == name)
                .map(|template| (*layer, template))
        })
}

fn find_fragment<'a>(
    stack: &'a [&'a TemplateConfigLayer],
    name: &str,
) -> Option<(
    &'a TemplateConfigLayer,
    &'a TemplateConfigFragmentDefinition,
)> {
    stack.iter().find_map(|layer| {
        layer
            .config
            .fragments
            .iter()
            .find(|fragment| fragment.name == name)
            .map(|fragment| (*layer, fragment))
    })
}

fn flatten_template(
    stack: &[&TemplateConfigLayer],
    layer: &TemplateConfigLayer,
    template: &TemplateConfigDefinition,
    template_stack: &mut Vec<String>,
    fragment_stack: &mut Vec<String>,
) -> Result<TemplateConfigResolved, TemplateConfigResolveError> {
    if let Some(position) = template_stack
        .iter()
        .position(|name| name == &template.name)
    {
        let mut cycle = template_stack[position..].to_vec();
        cycle.push(template.name.clone());
        return Err(TemplateConfigResolveError::TemplateCycle(cycle));
    }
    template_stack.push(template.name.clone());

    let parent = if let Some(parent_name) = template.extends.as_deref() {
        let Some((parent_layer, parent)) = find_template(stack, parent_name) else {
            return Err(TemplateConfigResolveError::UnknownParent {
                template: template.name.clone(),
                parent: parent_name.to_owned(),
            });
        };
        Some(flatten_template(
            stack,
            parent_layer,
            parent,
            template_stack,
            fragment_stack,
        )?)
    } else {
        None
    };

    let slot = template
        .slot
        .or_else(|| parent.as_ref().map(|parent| parent.slot))
        .ok_or_else(|| TemplateConfigResolveError::MissingProperty {
            template: template.name.clone(),
            property: "slot",
        })?;
    let node_kind = template
        .node_kind
        .or_else(|| parent.as_ref().map(|parent| parent.node_kind))
        .ok_or_else(|| TemplateConfigResolveError::MissingProperty {
            template: template.name.clone(),
            property: "node-kind",
        })?;
    let interleave_new_fields = parent.is_some();
    let mut fields = parent
        .as_ref()
        .map(|parent| parent.fields.clone())
        .unwrap_or_default();
    let mut controls = parent
        .as_ref()
        .map(|parent| parent.controls.clone())
        .unwrap_or_default();
    let mut chrome = parent
        .as_ref()
        .map(|parent| parent.chrome.clone())
        .unwrap_or_default();
    let mut setters = parent
        .as_ref()
        .map(|parent| parent.setters.clone())
        .unwrap_or_default();
    for setter in &template.sets {
        let resolved = ResolvedVariableSetter {
            setter: setter.clone(),
            origin: layer.origin.clone(),
        };
        if let Some(index) = setters
            .iter()
            .position(|existing| existing.setter.name == setter.name)
        {
            setters[index] = resolved;
        } else {
            setters.push(resolved);
        }
    }
    apply_operations(
        stack,
        &template.name,
        &template.operations,
        &layer.origin,
        &mut fields,
        &mut controls,
        &mut chrome,
        fragment_stack,
        interleave_new_fields,
    )?;
    let mut chain = vec![TemplateConfigChainEntry {
        name: template.name.clone(),
        origin: layer.origin.clone(),
    }];
    if let Some(parent) = parent {
        chain.extend(parent.chain);
    }
    template_stack.pop();
    Ok(TemplateConfigResolved {
        name: template.name.clone(),
        slot,
        node_kind,
        fields,
        controls,
        chrome,
        setters,
        chain,
        is_bundled: layer.origin.layer == TemplateConfigLayerKind::Bundled,
    })
}

#[allow(clippy::too_many_arguments)]
fn apply_operations(
    stack: &[&TemplateConfigLayer],
    template_name: &str,
    operations: &[TemplateConfigFieldOperation],
    origin: &TemplateConfigOrigin,
    fields: &mut Vec<TemplateConfigResolvedField>,
    controls: &mut Vec<TemplateControlSpec>,
    chrome: &mut ChromeSpec,
    fragment_stack: &mut Vec<String>,
    interleave_new_fields: bool,
) -> Result<(), TemplateConfigResolveError> {
    for operation in operations {
        match operation {
            TemplateConfigFieldOperation::Set { field } => {
                let resolved = TemplateConfigResolvedField {
                    spec: field.clone(),
                    origin: origin.clone(),
                };
                if let Some(index) = fields
                    .iter()
                    .position(|existing| existing.spec.name == field.name)
                {
                    fields[index] = resolved;
                    continue;
                }
                let insert_at = if interleave_new_fields {
                    fields
                        .iter()
                        .position(|existing| {
                            existing.spec.order_priority() < field.order_priority()
                        })
                        .unwrap_or(fields.len())
                } else {
                    fields.len()
                };
                fields.insert(insert_at, resolved);
            }
            TemplateConfigFieldOperation::Remove { name } => {
                let Some(index) = fields
                    .iter()
                    .position(|existing| existing.spec.name == *name)
                else {
                    return Err(TemplateConfigResolveError::RemoveUnknownField {
                        template: template_name.to_owned(),
                        field: name.clone(),
                    });
                };
                fields.remove(index);
            }
            TemplateConfigFieldOperation::Use { name } => {
                if let Some(position) = fragment_stack.iter().position(|entry| entry == name) {
                    let mut cycle = fragment_stack[position..].to_vec();
                    cycle.push(name.clone());
                    return Err(TemplateConfigResolveError::FragmentCycle(cycle));
                }
                let Some((fragment_layer, fragment)) = find_fragment(stack, name) else {
                    return Err(TemplateConfigResolveError::UnknownFragment {
                        template: template_name.to_owned(),
                        fragment: name.clone(),
                    });
                };
                fragment_stack.push(name.clone());
                apply_operations(
                    stack,
                    template_name,
                    &fragment.operations,
                    &fragment_layer.origin,
                    fields,
                    controls,
                    chrome,
                    fragment_stack,
                    interleave_new_fields,
                )?;
                fragment_stack.pop();
            }
            TemplateConfigFieldOperation::Chrome { primitive } => {
                chrome.set(primitive.clone());
            }
            TemplateConfigFieldOperation::Control { control } => {
                controls.push(control.clone());
            }
        }
    }
    Ok(())
}

fn quote_kdl(value: &str) -> String {
    serde_json::to_string(value).expect("KDL strings share JSON string escaping")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateConfigRenderedField {
    pub class: TemplateConfigFieldClass,
    pub priority: Option<i64>,
    pub value: String,
    pub source: Option<ResolvedTemplateFieldSource>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromeSpec {
    #[serde(default)]
    pub primitives: Vec<ChromePrimitive>,
}

impl ChromeSpec {
    fn set(&mut self, primitive: ChromePrimitive) {
        let kind = primitive.kind();
        if let Some(existing) = self
            .primitives
            .iter_mut()
            .find(|existing| existing.kind() == kind)
        {
            *existing = primitive;
        } else {
            self.primitives.push(primitive);
        }
    }

    pub fn boxed(&self) -> bool {
        self.primitives
            .iter()
            .any(|primitive| matches!(primitive, ChromePrimitive::Box { .. }))
    }

    pub fn toggle(&self) -> Option<(&str, &str)> {
        self.primitives
            .iter()
            .find_map(|primitive| match primitive {
                ChromePrimitive::Toggle {
                    collapsed,
                    expanded,
                } => Some((collapsed.as_str(), expanded.as_str())),
                _ => None,
            })
    }

    pub fn fill(&self) -> Option<char> {
        self.primitives
            .iter()
            .find_map(|primitive| match primitive {
                ChromePrimitive::Fill { glyph } => Some(*glyph),
                _ => None,
            })
    }

    pub fn dimmed(&self, metadata: &BTreeMap<String, MetadataValue>) -> bool {
        self.primitives.iter().any(|primitive| match primitive {
            ChromePrimitive::Dim { when } => when
                .as_deref()
                .is_none_or(|key| matches!(metadata.get(key), Some(MetadataValue::Bool(true)))),
            _ => false,
        })
    }

    pub fn indent_width(&self) -> Option<usize> {
        self.primitives
            .iter()
            .find_map(|primitive| match primitive {
                ChromePrimitive::Indent { level } => Some(*level),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ChromePrimitive {
    Box {
        #[serde(default)]
        border: BoxBorderStyle,
    },
    Toggle {
        collapsed: String,
        expanded: String,
    },
    Fill {
        glyph: char,
    },
    Dim {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
    },
    Indent {
        level: usize,
    },
}

impl ChromePrimitive {
    fn kind(&self) -> &'static str {
        match self {
            Self::Box { .. } => "box",
            Self::Toggle { .. } => "toggle",
            Self::Fill { .. } => "fill",
            Self::Dim { .. } => "dim",
            Self::Indent { .. } => "indent",
        }
    }

    fn to_kdl(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        match self {
            Self::Box { border } => {
                format!("{pad}box border={}\n", quote_kdl(border.as_str()))
            }
            Self::Toggle {
                collapsed,
                expanded,
            } => format!(
                "{pad}toggle collapsed={} expanded={}\n",
                quote_kdl(collapsed),
                quote_kdl(expanded)
            ),
            Self::Fill { glyph } => {
                format!("{pad}fill glyph={}\n", quote_kdl(&glyph.to_string()))
            }
            Self::Dim { when } => match when {
                Some(when) => format!("{pad}dim when={}\n", quote_kdl(when)),
                None => format!("{pad}dim\n"),
            },
            Self::Indent { level } => format!("{pad}indent level={level}\n"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoxBorderStyle {
    #[default]
    Single,
}

impl BoxBorderStyle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateConfigRenderReady {
    #[serde(default)]
    pub fields: Vec<TemplateConfigFieldSpec>,
    #[serde(default)]
    pub controls: Vec<TemplateControlSpec>,
    #[serde(default)]
    pub chrome: ChromeSpec,
}

impl TemplateConfigRenderReady {
    pub fn render_fields(
        &self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Vec<TemplateConfigRenderedField> {
        self.fields
            .iter()
            .filter_map(|field| field.render(context))
            .collect()
    }
}

fn default_template_config_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateConfigDefinition {
    pub name: String,
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub slot: Option<TemplateConfigSlot>,
    #[serde(default)]
    pub node_kind: Option<TemplateConfigNodeKind>,
    #[serde(default)]
    pub operations: Vec<TemplateConfigFieldOperation>,
    #[serde(default)]
    pub sets: Vec<TemplateVariableSetter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loops: Vec<PlacementLoop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateConfigFragmentDefinition {
    pub name: String,
    #[serde(default)]
    pub operations: Vec<TemplateConfigFieldOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TemplateConfigFieldOperation {
    Set { field: TemplateConfigFieldSpec },
    Remove { name: String },
    Use { name: String },
    Chrome { primitive: ChromePrimitive },
    Control { control: TemplateControlSpec },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateControlSpec {
    pub kind: TemplateControlKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyph: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateControlKind {
    OpenConfig,
    DisplayVariable,
    ScrollDown,
    ScrollUp,
    InspectRoot,
}

impl TemplateControlSpec {
    fn to_kdl(&self, indent: usize) -> String {
        let kind = match self.kind {
            TemplateControlKind::OpenConfig => "open-config",
            TemplateControlKind::DisplayVariable => "display-variable",
            TemplateControlKind::ScrollDown => "scroll-down",
            TemplateControlKind::ScrollUp => "scroll-up",
            TemplateControlKind::InspectRoot => "inspect-root",
        };
        let mut output = format!("{}control {}", " ".repeat(indent), quote_kdl(kind));
        if let Some(variable) = &self.variable {
            output.push_str(&format!(" variable={}", quote_kdl(variable)));
        }
        if let Some(glyph) = &self.glyph {
            output.push_str(&format!(" glyph={}", quote_kdl(glyph)));
        }
        output.push('\n');
        output
    }
}

impl TemplateConfigFieldOperation {
    fn set(field: TemplateConfigFieldSpec) -> Self {
        Self::Set { field }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigSlot {
    GroupHeader,
    TabTitle,
    TabStatus,
    Compact,
    Detail,
}

impl TemplateConfigSlot {
    pub fn is_rail_local(self) -> bool {
        matches!(self, Self::GroupHeader | Self::TabTitle | Self::TabStatus)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GroupHeader => "group-header",
            Self::TabTitle => "tab-title",
            Self::TabStatus => "tab-status",
            Self::Compact => "compact",
            Self::Detail => "detail",
        }
    }

    fn convention_form(self) -> Option<&'static str> {
        match self {
            Self::Compact => Some("compact"),
            Self::Detail => Some("detail"),
            Self::GroupHeader | Self::TabTitle | Self::TabStatus => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigNodeKind {
    Group,
    Tab,
    Entity,
}

impl TemplateConfigNodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Tab => "tab",
            Self::Entity => "entity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NodeVariableDefinition {
    pub name: String,
    pub default: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

impl NodeVariableDefinition {
    fn validate(&self) -> Result<(), TemplateConfigError> {
        if self.name.trim().is_empty() {
            return Err(TemplateConfigError::Validation(
                "variable name cannot be empty".to_owned(),
            ));
        }
        if self.default.is_empty() {
            return Err(TemplateConfigError::Validation(format!(
                "variable {} default cannot be empty",
                self.name
            )));
        }
        if !self.values.is_empty() {
            let unique = self.values.iter().collect::<BTreeSet<_>>();
            if unique.len() != self.values.len() {
                return Err(TemplateConfigError::Validation(format!(
                    "variable {} has duplicate allowed values",
                    self.name
                )));
            }
            if !self.values.contains(&self.default) {
                return Err(TemplateConfigError::Validation(format!(
                    "variable {} default is not in its allowed values",
                    self.name
                )));
            }
        }
        Ok(())
    }

    pub fn accepts(&self, value: &str) -> bool {
        self.values.is_empty() || self.values.iter().any(|allowed| allowed == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateVariableSetter {
    pub name: String,
    pub value: String,
}

impl TemplateVariableSetter {
    fn validate(&self) -> Result<(), TemplateConfigError> {
        if self.name.trim().is_empty() || self.value.is_empty() {
            return Err(TemplateConfigError::Validation(
                "set must declare a non-empty variable name and value".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedNodeVariableDefinition {
    pub definition: NodeVariableDefinition,
    pub origin: TemplateConfigOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedVariableSetter {
    pub setter: TemplateVariableSetter,
    pub origin: TemplateConfigOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVariableScope {
    pub declarations: Vec<ResolvedNodeVariableDefinition>,
    pub setters: Vec<ResolvedVariableSetter>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateVariableDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub variable_type: TemplateVariableType,
    pub default: crate::DisplayVariableValue,
    pub label: String,
    pub icon: String,
    #[serde(default = "default_true")]
    pub persist: bool,
}

impl TemplateVariableDefinition {
    fn validate(&self) -> Result<(), TemplateConfigError> {
        let valid_default = match (&self.variable_type, &self.default) {
            (TemplateVariableType::Bool, crate::DisplayVariableValue::Bool(_)) => true,
            (TemplateVariableType::Enum { values }, crate::DisplayVariableValue::Enum(value)) => {
                !values.is_empty() && values.contains(value)
            }
            _ => false,
        };
        if !valid_default {
            return Err(TemplateConfigError::Validation(format!(
                "variable {} has a default incompatible with its type",
                self.name
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TemplateVariableType {
    Bool,
    Enum { values: Vec<String> },
}

fn default_true() -> bool {
    true
}

fn metadata_text<'a>(metadata: &'a BTreeMap<String, MetadataValue>, key: &str) -> Option<&'a str> {
    match metadata.get(key) {
        Some(MetadataValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn format_metadata_value(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Text(value) => value.clone(),
        MetadataValue::Bool(value) => value.to_string(),
        MetadataValue::Integer(value) => value.to_string(),
        MetadataValue::StringList(values) => values.join(", "),
        MetadataValue::GroupPath(segments) => segments
            .iter()
            .map(|segment| {
                segment
                    .label
                    .clone()
                    .unwrap_or_else(|| segment.value.display())
            })
            .collect::<Vec<_>>()
            .join(" / "),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateConfigFieldSpec {
    pub name: String,
    pub class: TemplateConfigFieldClass,
    #[serde(default)]
    pub priority: Option<i64>,
    pub sources: Vec<TemplateConfigValueSource>,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub condition: TemplateConfigFieldCondition,
}

impl TemplateConfigFieldSpec {
    pub fn render(
        &self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Option<TemplateConfigRenderedField> {
        if !self.condition.matches(context) {
            return None;
        }
        let resolved = self
            .sources
            .iter()
            .find_map(|source| source.resolve(context))?;
        let value = format!("{}{}{}", self.prefix, resolved.value, self.suffix);
        Some(TemplateConfigRenderedField {
            class: self.class,
            priority: self.priority,
            value,
            source: resolved.source,
        })
    }

    fn order_priority(&self) -> i64 {
        self.priority.unwrap_or(match self.class {
            TemplateConfigFieldClass::Optional => 0,
            TemplateConfigFieldClass::Required | TemplateConfigFieldClass::Priority => 100,
        })
    }

    fn to_kdl(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let mut properties = format!("class={}", quote_kdl(self.class.as_str()));
        if let Some(priority) = self.priority {
            properties.push_str(&format!(" priority={priority}"));
        }
        if !self.prefix.is_empty() {
            properties.push_str(&format!(" prefix={}", quote_kdl(&self.prefix)));
        }
        if !self.suffix.is_empty() {
            properties.push_str(&format!(" suffix={}", quote_kdl(&self.suffix)));
        }
        if self.condition != TemplateConfigFieldCondition::Always {
            properties.push_str(&format!(
                " condition={}",
                quote_kdl(self.condition.as_str())
            ));
        }
        if self.sources.len() == 1 {
            properties.push(' ');
            properties.push_str(&self.sources[0].to_kdl_properties());
            return format!("{pad}field {} {properties}\n", quote_kdl(&self.name));
        }
        let mut output = format!("{pad}field {} {properties} {{\n", quote_kdl(&self.name));
        for source in &self.sources {
            output.push_str(&format!("{pad}  value {}\n", source.to_kdl_properties()));
        }
        output.push_str(&format!("{pad}}}\n"));
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigFieldClass {
    Required,
    Optional,
    Priority,
}

impl TemplateConfigFieldClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Optional => "optional",
            Self::Priority => "priority",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigFieldCondition {
    #[default]
    Always,
    Collapsed,
}

impl TemplateConfigFieldCondition {
    fn matches(&self, context: TemplateConfigMatchContext<'_>) -> bool {
        match self {
            TemplateConfigFieldCondition::Always => true,
            TemplateConfigFieldCondition::Collapsed => context.collapsed,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Collapsed => "collapsed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TemplateConfigValueSource {
    Literal { value: String },
    MetadataText { key: String },
    MetadataTextBasename { key: String },
    MetadataDisplay { key: String },
    MetadataFirstToken { key: String },
    TabNumberFromPosition,
    ActiveTabName,
    CollapsedToggle { collapsed: String, expanded: String },
}

impl TemplateConfigValueSource {
    fn resolve(
        &self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Option<TemplateConfigResolvedValue> {
        let value = match self {
            TemplateConfigValueSource::Literal { value } => value.clone(),
            TemplateConfigValueSource::MetadataText { key } => {
                let value = MetadataValue::Text(metadata_text(context.metadata, key)?.to_owned());
                return Some(TemplateConfigResolvedValue {
                    value: format_metadata_value(&value),
                    source: Some(ResolvedTemplateFieldSource {
                        key: key.clone(),
                        value,
                    }),
                });
            }
            TemplateConfigValueSource::MetadataTextBasename { key } => {
                let raw = metadata_text(context.metadata, key)?;
                let value = raw.trim_end_matches('/').rsplit('/').next()?.to_owned();
                return (!value.is_empty()).then(|| TemplateConfigResolvedValue {
                    value,
                    source: context.metadata.get(key).cloned().map(|value| {
                        ResolvedTemplateFieldSource {
                            key: key.clone(),
                            value,
                        }
                    }),
                });
            }
            TemplateConfigValueSource::MetadataDisplay { key } => {
                let value = context.metadata.get(key)?.clone();
                return Some(TemplateConfigResolvedValue {
                    value: format_metadata_value(&value),
                    source: Some(ResolvedTemplateFieldSource {
                        key: key.clone(),
                        value,
                    }),
                });
            }
            TemplateConfigValueSource::MetadataFirstToken { key } => {
                let value = metadata_text(context.metadata, key)?
                    .split_whitespace()
                    .next()?
                    .to_owned();
                return Some(TemplateConfigResolvedValue {
                    value,
                    source: context.metadata.get(key).cloned().map(|value| {
                        ResolvedTemplateFieldSource {
                            key: key.clone(),
                            value,
                        }
                    }),
                });
            }
            TemplateConfigValueSource::TabNumberFromPosition => {
                let MetadataValue::Integer(position) =
                    context.metadata.get("zellij.tab.position")?
                else {
                    return None;
                };
                format!("Tab {}", position + 1)
            }
            TemplateConfigValueSource::ActiveTabName => context.active_tab_name?.to_owned(),
            TemplateConfigValueSource::CollapsedToggle {
                collapsed,
                expanded,
            } => {
                if !context.collapsible {
                    return None;
                }
                if context.collapsed {
                    collapsed.clone()
                } else {
                    expanded.clone()
                }
            }
        };
        (!value.is_empty()).then_some(TemplateConfigResolvedValue {
            value,
            source: None,
        })
    }

    fn to_kdl_properties(&self) -> String {
        match self {
            Self::Literal { value } => {
                format!("source=\"literal\" value={}", quote_kdl(value))
            }
            Self::MetadataText { key } => {
                format!("source=\"metadata-text\" key={}", quote_kdl(key))
            }
            Self::MetadataTextBasename { key } => {
                format!("source=\"metadata-text-basename\" key={}", quote_kdl(key))
            }
            Self::MetadataDisplay { key } => {
                format!("source=\"metadata-display\" key={}", quote_kdl(key))
            }
            Self::MetadataFirstToken { key } => {
                format!("source=\"metadata-first-token\" key={}", quote_kdl(key))
            }
            Self::TabNumberFromPosition => "source=\"tab-number\"".to_owned(),
            Self::ActiveTabName => "source=\"active-tab-name\"".to_owned(),
            Self::CollapsedToggle {
                collapsed,
                expanded,
            } => format!(
                "source=\"collapsed-toggle\" collapsed={} expanded={}",
                quote_kdl(collapsed),
                quote_kdl(expanded)
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TemplateConfigResolvedValue {
    value: String,
    source: Option<ResolvedTemplateFieldSource>,
}

fn parse_kdl_template(node: &KdlNode) -> Result<TemplateConfigDefinition, TemplateConfigError> {
    let children = node.children().ok_or_else(|| {
        TemplateConfigError::Validation(format!(
            "template {} must define a child block",
            kdl_node_arg_string(node, 0).unwrap_or("<missing>")
        ))
    })?;
    let mut operations = vec![];
    let mut sets = vec![];
    let mut loops = vec![];
    let mut apply_template = None;
    let template_name = kdl_required_arg_string(node, 0, "template name")?;
    let template_binding = template_name
        .split('/')
        .next()
        .unwrap_or(&template_name)
        .to_owned();
    let template_scope = BTreeSet::from([template_binding]);
    for child in children.nodes() {
        match child.name().value() {
            "when" | "predicate" => {
                return Err(TemplateConfigError::Validation(
                    "template-level when/predicate selection has been removed; use convention binding or a declaration override"
                        .to_owned(),
                ));
            }
            "field" | "remove" | "use" | "box" | "toggle" | "fill" | "dim" | "indent"
            | "control" => operations.push(parse_kdl_field_operation(child)?),
            "set" => sets.push(parse_kdl_variable_setter(child)?),
            "for" => loops.push(parse_kdl_placement_loop(child, &template_scope)?),
            "apply-template" => {
                if apply_template.is_some() {
                    return Err(TemplateConfigError::Validation(
                        "template applies more than one template".to_owned(),
                    ));
                }
                apply_template = Some(kdl_node_arg_string(child, 0).unwrap_or("").to_owned());
            }
            other => {
                return Err(TemplateConfigError::Validation(format!(
                    "template {} has unsupported child node: {other}",
                    kdl_required_arg_string(node, 0, "template name")?
                )));
            }
        }
    }
    Ok(TemplateConfigDefinition {
        name: template_name,
        extends: kdl_prop_string(node, "extends"),
        slot: kdl_prop_string(node, "slot")
            .map(|value| parse_kdl_slot(&value))
            .transpose()?,
        node_kind: kdl_prop_string(node, "node-kind")
            .map(|value| parse_kdl_node_kind(&value))
            .transpose()?,
        operations,
        sets,
        loops,
        apply_template,
    })
}

fn parse_kdl_placement(node: &KdlNode) -> Result<PlacementDefinition, TemplateConfigError> {
    let name = kdl_required_arg_string(node, 0, "placement name")?;
    let loops = node
        .children()
        .map(|children| {
            children
                .nodes()
                .iter()
                .filter(|child| child.name().value() == "for")
                .map(|node| parse_kdl_placement_loop(node, &BTreeSet::new()))
                .collect::<Result<Vec<_>, TemplateConfigError>>()
        })
        .transpose()?
        .unwrap_or_default();
    if loops.is_empty() {
        return Err(TemplateConfigError::Validation(format!(
            "placement {name} declares no loops"
        )));
    }
    if loops.len() > 1 {
        return Err(TemplateConfigError::Validation(format!(
            "placement {name} declares {} loops; this build renders one",
            loops.len()
        )));
    }
    Ok(PlacementDefinition { name, loops })
}

fn parse_kdl_placement_loop(
    node: &KdlNode,
    enclosing: &BTreeSet<String>,
) -> Result<PlacementLoop, TemplateConfigError> {
    let binding = kdl_required_arg_string(node, 0, "loop binding")?;
    if binding.ends_with('s') {
        return Err(TemplateConfigError::Validation(format!(
            "loop binding {binding} must be singular"
        )));
    }
    if enclosing.contains(&binding) {
        return Err(TemplateConfigError::Validation(format!(
            "loop binding {binding} shadows an enclosing binding"
        )));
    }
    let mut scope = enclosing.clone();
    scope.insert(binding.clone());
    let mut predicates = vec![];
    // `kind=` is sugar for the overwhelmingly common entity.kind equality.
    if let Some(kind) = kdl_prop_string(node, "kind") {
        predicates.push(PlacementPredicate {
            key: "entity.kind".to_owned(),
            value: Some(kind),
            of: None,
        });
    }
    let mut fields = vec![];
    let mut order = vec![];
    let mut loops = vec![];
    let mut apply_template = kdl_prop_string(node, "apply-template");
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "match" => {
                    predicates.push(parse_kdl_placement_predicate(child, &binding, enclosing)?)
                }
                "field" => fields.push(parse_kdl_field(child)?),
                "order" => order.push(parse_kdl_placement_order(child)?),
                "for" => loops.push(parse_kdl_placement_loop(child, &scope)?),
                "apply-template" => {
                    if apply_template.is_some() {
                        return Err(TemplateConfigError::Validation(format!(
                            "loop {binding} applies more than one template"
                        )));
                    }
                    apply_template = Some(kdl_node_arg_string(child, 0).unwrap_or("").to_owned());
                }
                other => {
                    return Err(TemplateConfigError::Validation(format!(
                        "loop {binding} has unsupported child {other}"
                    )))
                }
            }
        }
    }
    if predicates.is_empty() {
        return Err(TemplateConfigError::Validation(format!(
            "loop {binding} selects every entity; declare kind= or a match"
        )));
    }
    Ok(PlacementLoop {
        binding,
        predicates,
        order,
        fields,
        layout: kdl_prop_string(node, "layout"),
        loops,
        apply_template,
    })
}

fn parse_kdl_placement_order(node: &KdlNode) -> Result<PlacementOrder, TemplateConfigError> {
    let key = kdl_required_arg_string(node, 0, "order fact key")?;
    let direction = match kdl_prop_string(node, "direction").as_deref() {
        None | Some("ascending") => PlacementOrderDirection::Ascending,
        Some("descending") => PlacementOrderDirection::Descending,
        Some(other) => {
            return Err(TemplateConfigError::Validation(format!(
                "unsupported order direction: {other}"
            )))
        }
    };
    let absent = match kdl_prop_string(node, "absent").as_deref() {
        None | Some("last") => PlacementOrderAbsent::Last,
        Some("first") => PlacementOrderAbsent::First,
        Some(other) => {
            return Err(TemplateConfigError::Validation(format!(
                "unsupported absent placement: {other}"
            )))
        }
    };
    Ok(PlacementOrder {
        key,
        direction,
        absent,
    })
}

fn parse_kdl_placement_predicate(
    node: &KdlNode,
    binding: &str,
    enclosing: &BTreeSet<String>,
) -> Result<PlacementPredicate, TemplateConfigError> {
    let key = kdl_required_arg_string(node, 0, "match key")?;
    // The index is keyed by (key, value). Anything else — existence tests,
    // comparisons, matches against another loop's binding — cannot be served
    // by a lookup, so it is refused here rather than silently scanned.
    if node.get("exists").is_some() {
        return Err(TemplateConfigError::Validation(format!(
            "loop {binding} matches {key} with exists=; the placement index is keyed by (key, value) and cannot serve an existence test"
        )));
    }
    if let Some(of) = kdl_prop_string(node, "of") {
        if !enclosing.contains(&of) {
            return Err(TemplateConfigError::Validation(format!(
                "loop {binding} refers to unbound enclosing loop {of}"
            )));
        }
        return Ok(PlacementPredicate {
            key,
            value: None,
            of: Some(of),
        });
    }
    let Some(value) = kdl_prop_string(node, "value") else {
        return Err(TemplateConfigError::Validation(format!(
            "loop {binding} matches {key} without value=; the placement index can only answer equality against a constant"
        )));
    };
    Ok(PlacementPredicate {
        key,
        value: Some(value),
        of: None,
    })
}

fn parse_kdl_region(node: &KdlNode) -> Result<SurfaceRegionDefinition, TemplateConfigError> {
    let source = match kdl_required_prop_string(node, "source")?.as_str() {
        "header" => SurfaceRegionSource::Header,
        "attention" => SurfaceRegionSource::Attention,
        "tree" => SurfaceRegionSource::Tree,
        "controls" => SurfaceRegionSource::Controls,
        other => {
            return Err(TemplateConfigError::Validation(format!(
                "unsupported region source: {other}"
            )))
        }
    };
    Ok(SurfaceRegionDefinition {
        name: kdl_required_arg_string(node, 0, "region name")?,
        source,
        root_template: kdl_required_prop_string(node, "root-template")?,
        form: kdl_prop_string(node, "form").unwrap_or_else(|| "full".to_owned()),
        attention_key: kdl_prop_string(node, "attention-key"),
        placement: kdl_prop_string(node, "placement"),
        pinned: node
            .get("pinned")
            .and_then(|entry| entry.value().as_bool())
            .unwrap_or(false),
        promotions: node
            .children()
            .map(|children| {
                children
                    .nodes()
                    .iter()
                    .filter(|child| child.name().value() == "promote")
                    .map(|child| {
                        Ok(SurfaceFormPromotion {
                            when: kdl_required_prop_string(child, "when")?,
                            form: kdl_required_prop_string(child, "form")?,
                        })
                    })
                    .collect::<Result<Vec<_>, TemplateConfigError>>()
            })
            .transpose()?
            .unwrap_or_default(),
    })
}

fn parse_kdl_fragment(
    node: &KdlNode,
) -> Result<TemplateConfigFragmentDefinition, TemplateConfigError> {
    let name = kdl_required_arg_string(node, 0, "fragment name")?;
    let children = node.children().ok_or_else(|| {
        TemplateConfigError::Validation(format!("fragment {name} must define a child block"))
    })?;
    let operations = children
        .nodes()
        .iter()
        .map(parse_kdl_field_operation)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TemplateConfigFragmentDefinition { name, operations })
}

fn parse_kdl_field_operation(
    node: &KdlNode,
) -> Result<TemplateConfigFieldOperation, TemplateConfigError> {
    match node.name().value() {
        "field" => Ok(TemplateConfigFieldOperation::set(parse_kdl_field(node)?)),
        "remove" => Ok(TemplateConfigFieldOperation::Remove {
            name: kdl_required_arg_string(node, 0, "field name")?,
        }),
        "use" => Ok(TemplateConfigFieldOperation::Use {
            name: kdl_required_arg_string(node, 0, "fragment name")?,
        }),
        "control" => Ok(TemplateConfigFieldOperation::Control {
            control: parse_kdl_control(node)?,
        }),
        "box" | "toggle" | "fill" | "dim" | "indent" => Ok(TemplateConfigFieldOperation::Chrome {
            primitive: parse_kdl_chrome(node)?,
        }),
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported fragment/template operation: {other}"
        ))),
    }
}

fn parse_kdl_control(node: &KdlNode) -> Result<TemplateControlSpec, TemplateConfigError> {
    let kind = match kdl_required_arg_string(node, 0, "control kind")?.as_str() {
        "open-config" => TemplateControlKind::OpenConfig,
        "display-variable" => TemplateControlKind::DisplayVariable,
        "scroll-down" => TemplateControlKind::ScrollDown,
        "scroll-up" => TemplateControlKind::ScrollUp,
        "inspect-root" => TemplateControlKind::InspectRoot,
        other => {
            return Err(TemplateConfigError::Validation(format!(
                "unsupported control kind: {other}"
            )))
        }
    };
    let variable = kdl_prop_string(node, "variable");
    if kind == TemplateControlKind::DisplayVariable && variable.is_none() {
        return Err(TemplateConfigError::Validation(
            "display-variable control requires variable=\"...\"".to_owned(),
        ));
    }
    if kind != TemplateControlKind::DisplayVariable && variable.is_some() {
        return Err(TemplateConfigError::Validation(format!(
            "{} control does not accept variable",
            kdl_required_arg_string(node, 0, "control kind")?
        )));
    }
    Ok(TemplateControlSpec {
        kind,
        variable,
        glyph: kdl_prop_string(node, "glyph"),
    })
}

fn parse_kdl_chrome(node: &KdlNode) -> Result<ChromePrimitive, TemplateConfigError> {
    match node.name().value() {
        "box" => {
            let border = match kdl_prop_string(node, "border")
                .as_deref()
                .unwrap_or("single")
            {
                "single" => BoxBorderStyle::Single,
                other => {
                    return Err(TemplateConfigError::Validation(format!(
                        "unsupported box border: {other}"
                    )))
                }
            };
            Ok(ChromePrimitive::Box { border })
        }
        "toggle" => Ok(ChromePrimitive::Toggle {
            collapsed: kdl_required_prop_string(node, "collapsed")?,
            expanded: kdl_required_prop_string(node, "expanded")?,
        }),
        "fill" => {
            let glyph = kdl_required_prop_string(node, "glyph")?;
            let mut chars = glyph.chars();
            let glyph = chars.next().ok_or_else(|| {
                TemplateConfigError::Validation("fill glyph cannot be empty".to_owned())
            })?;
            if chars.next().is_some() {
                return Err(TemplateConfigError::Validation(
                    "fill glyph must be exactly one character".to_owned(),
                ));
            }
            Ok(ChromePrimitive::Fill { glyph })
        }
        "dim" => Ok(ChromePrimitive::Dim {
            when: kdl_prop_string(node, "when"),
        }),
        "indent" => {
            let level = node
                .get("level")
                .and_then(|entry| entry.value().as_i64())
                .ok_or_else(|| {
                    TemplateConfigError::Validation(
                        "indent node must include integer property level".to_owned(),
                    )
                })?;
            Ok(ChromePrimitive::Indent {
                level: usize::try_from(level).map_err(|_| {
                    TemplateConfigError::Validation(
                        "indent level must be a non-negative integer".to_owned(),
                    )
                })?,
            })
        }
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported chrome primitive: {other}"
        ))),
    }
}

fn parse_kdl_node_variable(node: &KdlNode) -> Result<NodeVariableDefinition, TemplateConfigError> {
    let variable = NodeVariableDefinition {
        name: kdl_required_arg_string(node, 0, "variable name")?,
        default: kdl_required_prop_string(node, "default")?,
        values: node
            .children()
            .map(|children| {
                children
                    .nodes()
                    .iter()
                    .filter(|child| child.name().value() == "value")
                    .map(|child| kdl_required_arg_string(child, 0, "allowed value"))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default(),
    };
    variable.validate()?;
    Ok(variable)
}

fn parse_kdl_variable_setter(
    node: &KdlNode,
) -> Result<TemplateVariableSetter, TemplateConfigError> {
    let setter = TemplateVariableSetter {
        name: kdl_required_arg_string(node, 0, "variable name")?,
        value: kdl_required_arg_string(node, 1, "variable value")?,
    };
    setter.validate()?;
    Ok(setter)
}

fn parse_kdl_display_variable(
    node: &KdlNode,
) -> Result<TemplateVariableDefinition, TemplateConfigError> {
    let name = kdl_required_arg_string(node, 0, "variable name")?;
    let variable_type_name = kdl_required_prop_string(node, "type")?;
    let (variable_type, default) = match variable_type_name.as_str() {
        "bool" => (
            TemplateVariableType::Bool,
            crate::DisplayVariableValue::Bool(
                node.get("default")
                    .and_then(|entry| entry.value().as_bool())
                    .ok_or_else(|| {
                        TemplateConfigError::Validation(format!(
                            "variable {name} must have a boolean default"
                        ))
                    })?,
            ),
        ),
        "enum" => {
            let values = node
                .children()
                .map(|children| {
                    children
                        .nodes()
                        .iter()
                        .filter(|child| child.name().value() == "value")
                        .filter_map(|child| kdl_node_arg_string(child, 0).map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let default = kdl_required_prop_string(node, "default")?;
            (
                TemplateVariableType::Enum { values },
                crate::DisplayVariableValue::Enum(default),
            )
        }
        other => {
            return Err(TemplateConfigError::Validation(format!(
                "unsupported variable type: {other}"
            )))
        }
    };
    let variable = TemplateVariableDefinition {
        name,
        variable_type,
        default,
        label: kdl_required_prop_string(node, "label")?,
        icon: kdl_required_prop_string(node, "icon")?,
        persist: node
            .get("persist")
            .and_then(|entry| entry.value().as_bool())
            .unwrap_or(true),
    };
    variable.validate()?;
    Ok(variable)
}

fn parse_kdl_field(node: &KdlNode) -> Result<TemplateConfigFieldSpec, TemplateConfigError> {
    let sources = if let Some(children) = node.children() {
        children
            .nodes()
            .iter()
            .filter(|child| child.name().value() == "value")
            .map(parse_kdl_value_source)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![parse_kdl_value_source(node)?]
    };
    let explicit_class = kdl_prop_string(node, "class");
    let class = explicit_class
        .as_deref()
        .map(parse_kdl_field_class)
        .transpose()?
        .unwrap_or(TemplateConfigFieldClass::Required);
    // Class-less KDL predates explicit field classes and treated every field as priority 100.
    let priority = if explicit_class.is_some() {
        kdl_prop_i64(node, "priority")
    } else {
        kdl_prop_i64(node, "priority").or(Some(100))
    };
    Ok(TemplateConfigFieldSpec {
        name: kdl_required_arg_string(node, 0, "field name")?,
        class,
        priority,
        sources,
        prefix: kdl_prop_string(node, "prefix").unwrap_or_default(),
        suffix: kdl_prop_string(node, "suffix").unwrap_or_default(),
        condition: parse_kdl_condition(
            kdl_prop_string(node, "condition")
                .as_deref()
                .unwrap_or("always"),
        )?,
    })
}

fn parse_kdl_value_source(
    node: &KdlNode,
) -> Result<TemplateConfigValueSource, TemplateConfigError> {
    if let Some(source) = kdl_prop_string(node, "source") {
        return match source.as_str() {
            "active-tab-name" => Ok(TemplateConfigValueSource::ActiveTabName),
            "tab-number" | "tab-number-from-position" => {
                Ok(TemplateConfigValueSource::TabNumberFromPosition)
            }
            "collapsed-toggle" => Ok(TemplateConfigValueSource::CollapsedToggle {
                collapsed: kdl_required_prop_string(node, "collapsed")?,
                expanded: kdl_required_prop_string(node, "expanded")?,
            }),
            "metadata-text" => Ok(TemplateConfigValueSource::MetadataText {
                key: kdl_required_prop_string(node, "key")?,
            }),
            "metadata-text-basename" => Ok(TemplateConfigValueSource::MetadataTextBasename {
                key: kdl_required_prop_string(node, "key")?,
            }),
            "metadata-display" => Ok(TemplateConfigValueSource::MetadataDisplay {
                key: kdl_required_prop_string(node, "key")?,
            }),
            "metadata-first-token" => Ok(TemplateConfigValueSource::MetadataFirstToken {
                key: kdl_required_prop_string(node, "key")?,
            }),
            "literal" => Ok(TemplateConfigValueSource::Literal {
                value: kdl_required_prop_string(node, "value")?,
            }),
            other => Err(TemplateConfigError::Validation(format!(
                "unsupported value source: {other}"
            ))),
        };
    }
    if let Some(key) = kdl_prop_string(node, "key") {
        return Ok(TemplateConfigValueSource::MetadataDisplay { key });
    }
    if let Some(value) = kdl_prop_string(node, "literal").or_else(|| kdl_prop_string(node, "text"))
    {
        return Ok(TemplateConfigValueSource::Literal { value });
    }
    Err(TemplateConfigError::Validation(format!(
        "{} node must include key, text, literal, or source",
        node.name().value()
    )))
}

fn parse_kdl_slot(value: &str) -> Result<TemplateConfigSlot, TemplateConfigError> {
    match value {
        "group-header" => Ok(TemplateConfigSlot::GroupHeader),
        "tab-title" => Ok(TemplateConfigSlot::TabTitle),
        "tab-status" => Ok(TemplateConfigSlot::TabStatus),
        "compact" => Ok(TemplateConfigSlot::Compact),
        "detail" => Ok(TemplateConfigSlot::Detail),
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported template slot: {other}"
        ))),
    }
}

fn parse_kdl_node_kind(value: &str) -> Result<TemplateConfigNodeKind, TemplateConfigError> {
    match value {
        "group" => Ok(TemplateConfigNodeKind::Group),
        "tab" => Ok(TemplateConfigNodeKind::Tab),
        "entity" => Ok(TemplateConfigNodeKind::Entity),
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported template node-kind: {other}"
        ))),
    }
}

fn parse_kdl_condition(value: &str) -> Result<TemplateConfigFieldCondition, TemplateConfigError> {
    match value {
        "always" => Ok(TemplateConfigFieldCondition::Always),
        "collapsed" => Ok(TemplateConfigFieldCondition::Collapsed),
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported field condition: {other}"
        ))),
    }
}

fn parse_kdl_field_class(value: &str) -> Result<TemplateConfigFieldClass, TemplateConfigError> {
    match value {
        "required" => Ok(TemplateConfigFieldClass::Required),
        "optional" => Ok(TemplateConfigFieldClass::Optional),
        "priority" => Ok(TemplateConfigFieldClass::Priority),
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported field class: {other}"
        ))),
    }
}

fn kdl_required_arg_string(
    node: &KdlNode,
    index: usize,
    label: &str,
) -> Result<String, TemplateConfigError> {
    kdl_node_arg_string(node, index)
        .map(str::to_owned)
        .ok_or_else(|| {
            TemplateConfigError::Validation(format!(
                "{} node must include string argument for {label}",
                node.name().value()
            ))
        })
}

fn kdl_node_arg_string(node: &KdlNode, index: usize) -> Option<&str> {
    node.get(index).and_then(|entry| entry.value().as_string())
}

fn kdl_required_prop_string(node: &KdlNode, key: &str) -> Result<String, TemplateConfigError> {
    kdl_prop_string(node, key).ok_or_else(|| {
        TemplateConfigError::Validation(format!(
            "{} node must include string property {key}",
            node.name().value()
        ))
    })
}

fn kdl_prop_string(node: &KdlNode, key: &str) -> Option<String> {
    node.get(key)
        .and_then(|entry| entry.value().as_string())
        .map(str::to_owned)
}

fn kdl_prop_i64(node: &KdlNode, key: &str) -> Option<i64> {
    node.get(key).and_then(|entry| entry.value().as_i64())
}

fn kdl_u32(value: &KdlValue) -> Result<u32, TemplateConfigError> {
    let Some(value) = value.as_i64() else {
        return Err(TemplateConfigError::Validation(
            "version must be an integer".to_owned(),
        ));
    };
    u32::try_from(value).map_err(|_| {
        TemplateConfigError::Validation(format!("version must be a positive u32: {value}"))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateConfigError {
    Io(String),
    Parse(String),
    Validation(String),
}

impl fmt::Display for TemplateConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateConfigError::Io(message) => write!(formatter, "{message}"),
            TemplateConfigError::Parse(message) => {
                write!(formatter, "invalid template config: {message}")
            }
            TemplateConfigError::Validation(message) => write!(formatter, "{message}"),
        }
    }
}

impl Error for TemplateConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity_context<'a>(
        metadata: &'a BTreeMap<String, MetadataValue>,
    ) -> TemplateConfigMatchContext<'a> {
        TemplateConfigMatchContext {
            slot: TemplateConfigSlot::Compact,
            node_kind: TemplateConfigNodeKind::Entity,
            metadata,
            collapsed: false,
            collapsible: false,
            active_tab_name: None,
        }
    }

    #[test]
    fn convention_binding_and_declaration_override_do_not_use_predicates() {
        let config = parse_template_config_kdl(
            r#"
            template "issue/compact" slot="compact" node-kind="entity" {
              field "label" source="literal" value="convention"
            }
            template "special/compact" slot="compact" node-kind="entity" {
              field "label" source="literal" value="declared"
            }
            "#,
        )
        .expect("templates parse");
        let catalog = TemplateConfigCatalog::from_config(config);
        let convention_metadata = BTreeMap::from([(
            "entity.kind".to_owned(),
            MetadataValue::Text("issue".to_owned()),
        )]);
        let declared_metadata = BTreeMap::from([
            (
                "entity.kind".to_owned(),
                MetadataValue::Text("issue".to_owned()),
            ),
            (
                "presentation.template".to_owned(),
                MetadataValue::Text("special/compact".to_owned()),
            ),
        ]);

        assert_eq!(
            catalog
                .resolve(entity_context(&convention_metadata))
                .expect("resolution succeeds")
                .expect("convention template")
                .name,
            "issue/compact"
        );
        assert_eq!(
            catalog
                .resolve(entity_context(&declared_metadata))
                .expect("resolution succeeds")
                .expect("declared template")
                .name,
            "special/compact"
        );
        assert!(parse_template_config_kdl(
            r#"
            template "issue/compact" slot="compact" node-kind="entity" {
              when exists="summary.text"
              field "label" key="display.label"
            }
            "#,
        )
        .expect_err("template predicates are removed")
        .to_string()
        .contains("template-level when"));
    }

    #[test]
    fn novel_entity_kind_uses_bundled_generic_form_fallback() {
        let catalog = TemplateConfigCatalog::default();
        let metadata = BTreeMap::from([
            (
                "entity.kind".to_owned(),
                MetadataValue::Text("deployment".to_owned()),
            ),
            (
                "entity.id".to_owned(),
                MetadataValue::Text("prod/api".to_owned()),
            ),
            (
                "display.label".to_owned(),
                MetadataValue::Text("api".to_owned()),
            ),
        ]);

        let resolved = catalog
            .resolve(entity_context(&metadata))
            .expect("resolution succeeds")
            .expect("generic entity template resolves");

        assert_eq!(resolved.name, "deployment/compact");
        assert!(resolved.is_bundled);
        assert_eq!(
            resolved.render_fields(entity_context(&metadata))[0].value,
            "api"
        );
        assert!(resolved
            .chain
            .iter()
            .any(|entry| entry.name == "flotilla/entity/compact"));
    }

    #[test]
    fn user_template_for_novel_entity_kind_wins_without_code_registration() {
        let user = parse_template_config_kdl(
            r#"
            template "deployment/compact" slot="compact" node-kind="entity" {
              field "label" source="literal" value="custom deployment"
            }
            "#,
        )
        .expect("user template parses");
        let catalog = TemplateConfigCatalog::with_bundled_defaults(user);
        let metadata = BTreeMap::from([(
            "entity.kind".to_owned(),
            MetadataValue::Text("deployment".to_owned()),
        )]);

        let resolved = catalog
            .resolve(entity_context(&metadata))
            .expect("resolution succeeds")
            .expect("user template resolves");

        assert_eq!(
            resolved.render_fields(entity_context(&metadata))[0].value,
            "custom deployment"
        );
        assert!(!resolved.is_bundled);
    }

    #[test]
    fn chrome_primitives_survive_flattening_and_explain_the_effective_document() {
        let config = parse_template_config_kdl(
            r#"
            template "tab/title" slot="tab-title" node-kind="tab" {
              box border="single"
              toggle collapsed=">" expanded="v"
              fill glyph="-"
              dim when="rail.tab.latent"
              indent level=2
              field "title" source="literal" value="title"
            }
            "#,
        )
        .expect("template parses");
        let catalog = TemplateConfigCatalog::from_config(config);
        let metadata = BTreeMap::new();
        let context = TemplateConfigMatchContext {
            slot: TemplateConfigSlot::TabTitle,
            node_kind: TemplateConfigNodeKind::Tab,
            metadata: &metadata,
            collapsed: false,
            collapsible: false,
            active_tab_name: None,
        };
        let resolved = catalog
            .resolve(context)
            .expect("resolution succeeds")
            .expect("template resolves");

        let dump = resolved.dump_kdl();
        assert!(dump.contains("box border=\"single\""));
        assert!(dump.contains("toggle collapsed=\">\" expanded=\"v\""));
        assert!(dump.contains("fill glyph=\"-\""));
        assert!(dump.contains("dim when=\"rail.tab.latent\""));
        assert!(dump.contains("indent level=2"));
        assert_eq!(resolved.render_ready().chrome, resolved.chrome);
    }

    #[test]
    fn extends_replaces_removes_interleaves_and_inlines_fragments_with_origins() {
        let bundled = parse_template_config_kdl(
            r#"
            fragment "flotilla/status" {
              field "status" class="priority" priority=60 key="status.state" prefix=" "
            }
            template "flotilla/issue/compact" slot="compact" node-kind="entity" {
              field "label" class="required" priority=100 key="display.label"
              field "summary" class="priority" priority=40 key="summary.text" prefix=": "
            }
            "#,
        )
        .expect("bundled templates parse");
        let user = parse_template_config_kdl(
            r#"
            template "issue/compact" extends="flotilla/issue/compact" {
              field "label" class="required" priority=100 key="short.label"
              use "flotilla/status"
              remove "summary"
            }
            "#,
        )
        .expect("user templates parse");
        let catalog = TemplateConfigCatalog::from_layers(vec![
            TemplateConfigLayer::user("user.kdl", user),
            TemplateConfigLayer::bundled("templates/flotilla-default.kdl", bundled),
        ]);
        let metadata = BTreeMap::from([(
            "entity.kind".to_owned(),
            MetadataValue::Text("issue".to_owned()),
        )]);

        let resolved = catalog
            .resolve(entity_context(&metadata))
            .expect("resolution succeeds")
            .expect("template resolves");

        assert_eq!(resolved.name, "issue/compact");
        assert_eq!(
            resolved
                .fields
                .iter()
                .map(|field| field.spec.name.as_str())
                .collect::<Vec<_>>(),
            vec!["label", "status"]
        );
        assert_eq!(resolved.fields[0].origin.source, "user.kdl");
        assert_eq!(
            resolved.fields[1].origin.source,
            "templates/flotilla-default.kdl"
        );
        assert_eq!(
            resolved
                .chain
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["issue/compact", "flotilla/issue/compact"]
        );
        let dump = resolved.dump_kdl();
        assert!(dump.contains("// chain: issue/compact"));
        assert!(dump.contains("// origin: user (user.kdl)"));
        assert!(dump.contains("// origin: bundled (templates/flotilla-default.kdl)"));
        assert!(dump.parse::<KdlDocument>().is_ok(), "{dump}");
    }

    #[test]
    fn applied_template_resolution_flattens_inheritance_operations_and_setters() {
        let config = parse_template_config_kdl(
            r#"
            variable "child-layout" default="cards" {
              value "cards"
              value "strip"
            }
            template "base/line" slot="detail" node-kind="entity" {
              field "inherited" source="literal" value="base"
              field "removed" source="literal" value="gone"
              control "open-config" glyph="gear"
              set "child-layout" "strip"
            }
            template "project/line" extends="base/line" {
              remove "removed"
              field "own" source="literal" value="project"
            }
            "#,
        )
        .expect("placement templates parse");
        let catalog = TemplateConfigCatalog::from_config(config);
        let resolved = catalog
            .resolve_placement_template("project/line", &BTreeMap::new())
            .expect("placement template resolves")
            .expect("placement template exists");

        assert_eq!(
            resolved
                .fields
                .iter()
                .map(|field| field.spec.name.as_str())
                .collect::<Vec<_>>(),
            vec!["inherited", "own"]
        );
        assert_eq!(resolved.controls.len(), 1);
        assert_eq!(resolved.setters[0].setter.name, "child-layout");
        assert_eq!(resolved.setters[0].setter.value, "strip");
        assert_eq!(resolved.slot, TemplateConfigSlot::Detail);
    }

    #[test]
    fn json_placement_predicate_requires_a_value_or_enclosing_binding() {
        let error = parse_template_config_json(
            r#"{
              "version": 1,
              "placements": [{
                "name": "tree",
                "loops": [{
                  "binding": "project",
                  "predicates": [{"key": "entity.kind"}]
                }]
              }]
            }"#,
        )
        .expect_err("a JSON predicate without value or of must fail validation");

        assert!(format!("{error:?}").contains("exactly one of value or of"));
    }

    #[test]
    fn project_layer_is_selected_per_node_and_does_not_leak() {
        let project = parse_template_config_kdl(
            r#"
            template "issue/compact" slot="compact" node-kind="entity" {
              field "label" source="literal" value="project-a"
            }
            "#,
        )
        .expect("project templates parse");
        let fleet = parse_template_config_kdl(
            r#"
            template "issue/compact" slot="compact" node-kind="entity" {
              field "label" source="literal" value="fleet"
            }
            "#,
        )
        .expect("fleet templates parse");
        let catalog = TemplateConfigCatalog::from_layers(vec![
            TemplateConfigLayer::project("project-a", "project-a/.flotilla/templates.kdl", project),
            TemplateConfigLayer::fleet("fleet.kdl", fleet),
        ]);
        let project_a = BTreeMap::from([
            (
                "entity.kind".to_owned(),
                MetadataValue::Text("issue".to_owned()),
            ),
            (
                "flotilla.project".to_owned(),
                MetadataValue::Text("project-a".to_owned()),
            ),
        ]);
        let project_b = BTreeMap::from([
            (
                "entity.kind".to_owned(),
                MetadataValue::Text("issue".to_owned()),
            ),
            (
                "flotilla.project".to_owned(),
                MetadataValue::Text("project-b".to_owned()),
            ),
        ]);

        let a = catalog
            .resolve(entity_context(&project_a))
            .expect("project A resolves")
            .expect("project A template");
        let b = catalog
            .resolve(entity_context(&project_b))
            .expect("project B resolves")
            .expect("project B template");

        assert_eq!(a.fields[0].origin.layer, TemplateConfigLayerKind::Project);
        assert_eq!(a.fields[0].origin.membership.as_deref(), Some("project-a"));
        assert_eq!(b.fields[0].origin.layer, TemplateConfigLayerKind::Fleet);
    }

    #[test]
    fn placement_loop_parses_kind_sugar_and_constant_matches() {
        let config = parse_template_config_kdl(
            r#"
version 1
placement "attention" {
  for "item" kind="vessel" {
    match "status.attention" value="true"
    field "label" {
      value source="metadata-text" key="display.label"
    }
  }
}
"#,
        )
        .expect("placement parses");

        let placement = &config.placements[0];
        let loop_definition = &placement.loops[0];
        assert_eq!(loop_definition.binding, "item");
        assert_eq!(
            loop_definition.predicates,
            vec![
                PlacementPredicate {
                    key: "entity.kind".to_owned(),
                    value: Some("vessel".to_owned()),
                    of: None,
                },
                PlacementPredicate {
                    key: "status.attention".to_owned(),
                    value: Some("true".to_owned()),
                    of: None,
                },
            ],
            "kind= is sugar for an entity.kind equality"
        );
        assert_eq!(loop_definition.fields.len(), 1);
    }

    #[test]
    fn placement_loop_parses_ordered_fact_keys_and_missing_policy() {
        let config = parse_template_config_kdl(
            r#"
version 1
placement "attention" {
  for "item" kind="vessel" {
    order "status.rank" direction="descending" absent="first"
    order "display.label"
  }
}
"#,
        )
        .expect("placement order parses");

        assert_eq!(
            config.placements[0].loops[0].order,
            vec![
                PlacementOrder {
                    key: "status.rank".to_owned(),
                    direction: PlacementOrderDirection::Descending,
                    absent: PlacementOrderAbsent::First,
                },
                PlacementOrder {
                    key: "display.label".to_owned(),
                    direction: PlacementOrderDirection::Ascending,
                    absent: PlacementOrderAbsent::Last,
                },
            ]
        );
    }

    #[test]
    fn placement_loop_refuses_unknown_order_options() {
        for (property, expected) in [
            (r#"direction="sideways""#, "order direction"),
            (r#"absent="somewhere""#, "absent placement"),
        ] {
            let error = parse_template_config_kdl(&format!(
                "version 1\nplacement \"p\" {{\n  for \"item\" kind=\"vessel\" {{\n    order \"display.label\" {property}\n  }}\n}}\n"
            ))
            .expect_err("unknown order option must be refused");
            assert!(format!("{error:?}").contains(expected));
        }
    }

    #[test]
    fn placement_refuses_predicates_the_index_cannot_serve() {
        // The index is keyed by (key, value). Each of these would force a scan,
        // which is what makes nested loops quadratic, so each is a config error.
        for (predicate, expected) in [
            (r#"match "status.attention" exists=true"#, "existence test"),
            (
                r#"match "flotilla.project" of="project""#,
                "unbound enclosing loop",
            ),
            (r#"match "status.attention""#, "equality against a constant"),
        ] {
            let error = parse_template_config_kdl(&format!(
                "version 1\nplacement \"p\" {{\n  for \"item\" kind=\"vessel\" {{\n    {predicate}\n  }}\n}}\n"
            ))
            .expect_err("unindexable predicate must be refused");
            let message = format!("{error:?}");
            assert!(
                message.contains(expected),
                "diagnostic for {predicate} should explain {expected}: {message}"
            );
        }
    }

    #[test]
    fn nested_loops_bind_lexically_and_reject_shadowing() {
        let config = parse_template_config_kdl(
            r#"
version 1
placement "tree" {
  for "project" kind="project" {
    for "convoy" kind="convoy" apply-template="convoy/line" {
      match "flotilla.project" of="project"
    }
  }
}
"#,
        )
        .expect("nested bindings parse");
        let nested = &config.placements[0].loops[0].loops[0];
        assert_eq!(nested.predicates[1].of.as_deref(), Some("project"));
        assert_eq!(nested.apply_template.as_deref(), Some("convoy/line"));

        let error = parse_template_config_kdl(
            r#"
version 1
placement "tree" {
  for "project" kind="project" {
    for "project" kind="convoy" {}
  }
}
"#,
        )
        .expect_err("shadowing is a config error");
        assert!(format!("{error:?}").contains("shadows an enclosing binding"));
    }

    #[test]
    fn applied_template_cannot_reference_the_callers_bindings() {
        let error = parse_template_config_kdl(
            r#"
version 1
template "convoy/line" {
  for "vessel" kind="vessel" {
    match "flotilla.project" of="project"
  }
}
"#,
        )
        .expect_err("the caller environment is not in template scope");
        assert!(format!("{error:?}").contains("unbound enclosing loop project"));
    }

    #[test]
    fn a_region_naming_an_undeclared_placement_is_refused() {
        let error = parse_template_config_kdl(
            r#"
version 1
region "attention" source="attention" root-template="r" form="full" placement="attetnion"
placement "attention" {
  for "item" kind="vessel" {
    match "status.attention" value="true"
  }
}
"#,
        )
        .expect_err("a typo must not fall through to the legacy path");
        let message = format!("{error:?}");
        assert!(
            message.contains("attetnion") && message.contains("not declared"),
            "the diagnostic should name the unresolved placement: {message}"
        );
    }

    #[test]
    fn attention_region_may_omit_attention_key_when_a_placement_supplies_it() {
        parse_template_config_kdl(
            r#"
version 1
region "attention" source="attention" root-template="r" form="full" placement="attention"
placement "attention" {
  for "item" kind="vessel" {
    match "status.attention" value="true"
  }
}
"#,
        )
        .expect("placement replaces the legacy attention filter");

        parse_template_config_kdl(
            r#"
version 1
region "attention" source="attention" root-template="r" form="full"
"#,
        )
        .expect_err("without a placement the legacy attention-key is still required");
    }

    #[test]
    fn node_layer_stack_has_documented_precedence_and_membership_scope() {
        let empty = || ExternalTemplateConfig {
            version: 1,
            templates: vec![],
            fragments: vec![],
            variables: vec![],
            display_variables: vec![],
            sets: vec![],
            regions: vec![],
            placements: vec![],
        };
        let catalog = TemplateConfigCatalog::from_layers(vec![
            TemplateConfigLayer::bundled("bundled.kdl", empty()),
            TemplateConfigLayer::fleet("fleet.kdl", empty()),
            TemplateConfigLayer::project("other", "other-project.kdl", empty()),
            TemplateConfigLayer::project("project-a", "project.kdl", empty()),
            TemplateConfigLayer::repo("other/repo", "other-repo.kdl", empty()),
            TemplateConfigLayer::repo("org/repo", "repo.kdl", empty()),
            TemplateConfigLayer::user("user.kdl", empty()),
        ]);
        let metadata = BTreeMap::from([
            (
                "vcs.repo".to_owned(),
                MetadataValue::Text("org/repo".to_owned()),
            ),
            (
                "flotilla.project".to_owned(),
                MetadataValue::Text("project-a".to_owned()),
            ),
        ]);

        assert_eq!(
            catalog
                .layer_stack(&metadata)
                .iter()
                .map(|layer| layer.origin.source.as_str())
                .collect::<Vec<_>>(),
            vec![
                "user.kdl",
                "repo.kdl",
                "project.kdl",
                "fleet.kdl",
                "bundled.kdl",
            ]
        );
    }

    #[test]
    fn inheritance_failures_are_visible_resolution_errors() {
        let metadata = BTreeMap::from([(
            "entity.kind".to_owned(),
            MetadataValue::Text("issue".to_owned()),
        )]);

        let unknown_parent = TemplateConfigCatalog::from_config(
            parse_template_config_kdl(
                r#"
                template "issue/compact" extends="missing/compact" {
                  field "label" source="literal" value="issue"
                }
                "#,
            )
            .expect("unknown parent is a resolution concern"),
        );
        assert!(matches!(
            unknown_parent.resolve(entity_context(&metadata)),
            Err(TemplateConfigResolveError::UnknownParent { .. })
        ));

        let unknown_fragment = TemplateConfigCatalog::from_config(
            parse_template_config_kdl(
                r#"
                template "issue/compact" slot="compact" node-kind="entity" {
                  use "missing/fragment"
                }
                "#,
            )
            .expect("unknown fragment is a resolution concern"),
        );
        assert!(matches!(
            unknown_fragment.resolve(entity_context(&metadata)),
            Err(TemplateConfigResolveError::UnknownFragment { .. })
        ));

        let cycle = TemplateConfigCatalog::from_config(
            parse_template_config_kdl(
                r#"
                template "issue/compact" extends="base/compact" {}
                template "base/compact" extends="issue/compact" {}
                "#,
            )
            .expect("inheritance cycle is a resolution concern"),
        );
        assert!(matches!(
            cycle.resolve(entity_context(&metadata)),
            Err(TemplateConfigResolveError::TemplateCycle(_))
        ));

        let self_cycle = TemplateConfigCatalog::from_config(
            parse_template_config_kdl(
                r#"
                template "issue/compact" extends="issue/compact" {}
                "#,
            )
            .expect("self-cycle is a resolution concern"),
        );
        assert!(matches!(
            self_cycle.resolve(entity_context(&metadata)),
            Err(TemplateConfigResolveError::TemplateCycle(chain))
                if chain == vec!["issue/compact", "issue/compact"]
        ));

        let fragment_cycle = TemplateConfigCatalog::from_config(
            parse_template_config_kdl(
                r#"
                fragment "a" {
                  use "b"
                }
                fragment "b" {
                  use "a"
                }
                template "issue/compact" slot="compact" node-kind="entity" {
                  use "a"
                }
                "#,
            )
            .expect("fragment cycle is a resolution concern"),
        );
        assert!(matches!(
            fragment_cycle.resolve(entity_context(&metadata)),
            Err(TemplateConfigResolveError::FragmentCycle(chain))
                if chain == vec!["a", "b", "a"]
        ));

        let remove_unknown = TemplateConfigCatalog::from_config(
            parse_template_config_kdl(
                r#"
                template "issue/compact" slot="compact" node-kind="entity" {
                  remove "missing"
                }
                "#,
            )
            .expect("unknown remove target is a resolution concern"),
        );
        assert!(matches!(
            remove_unknown.resolve(entity_context(&metadata)),
            Err(TemplateConfigResolveError::RemoveUnknownField { field, .. })
                if field == "missing"
        ));

        let binding_mismatch = TemplateConfigCatalog::from_config(
            parse_template_config_kdl(
                r#"
                template "issue/compact" slot="detail" node-kind="entity" {
                  field "label" source="literal" value="issue"
                }
                "#,
            )
            .expect("binding mismatch is a resolution concern"),
        );
        assert!(matches!(
            binding_mismatch.resolve(entity_context(&metadata)),
            Err(TemplateConfigResolveError::BindingMismatch { .. })
        ));
    }

    #[test]
    fn duplicate_direct_field_names_are_rejected() {
        let error = parse_template_config_kdl(
            r#"
            template "issue/compact" slot="compact" node-kind="entity" {
              field "label" source="literal" value="first"
              field "label" source="literal" value="second"
            }
            "#,
        )
        .expect_err("duplicate field names make override intent ambiguous");

        assert!(error
            .to_string()
            .contains("template issue/compact has duplicate field name: label"));
    }

    #[test]
    fn surface_regions_preserve_declaration_order_and_form_targets() {
        let config = parse_template_config_kdl(
            r#"
            region "attention" source="attention" root-template="my/attention" form="expanded" attention-key="status.attention"
            region "tree" source="tree" root-template="my/tree" form="compact" pinned=true
            "#,
        )
        .expect("regions parse");

        assert_eq!(
            config
                .regions
                .iter()
                .map(|region| region.name.as_str())
                .collect::<Vec<_>>(),
            vec!["attention", "tree"]
        );
        assert_eq!(config.regions[0].form, "expanded");
        assert_eq!(
            config.regions[0].attention_key.as_deref(),
            Some("status.attention")
        );
        assert!(config.regions[1].pinned);
    }

    #[test]
    fn node_variables_parse_with_template_and_config_setters_and_namespaced_reads() {
        let config = parse_template_config_kdl(
            r#"
            variable "child-layout" default="cards" {
              value "cards"
              value "strip"
            }
            set "child-layout" "strip"
            template "issue/compact" slot="compact" node-kind="entity" {
              set "child-layout" "cards"
              field "layout" key="var.child-layout"
            }
            "#,
        )
        .expect("node variables parse");
        let catalog = TemplateConfigCatalog::from_config(config);
        let metadata = BTreeMap::from([
            (
                "entity.kind".to_owned(),
                MetadataValue::Text("issue".to_owned()),
            ),
            (
                "var.child-layout".to_owned(),
                MetadataValue::Text("strip".to_owned()),
            ),
        ]);

        assert_eq!(catalog.variables(&metadata)[0].definition.default, "cards");
        assert_eq!(
            catalog.config_variable_setters(&metadata)[0].setter.value,
            "strip"
        );
        let resolved = catalog
            .resolve(entity_context(&metadata))
            .expect("template resolves")
            .expect("template is present");
        assert_eq!(resolved.setters[0].setter.value, "cards");
        assert_eq!(
            resolved.render_fields(entity_context(&metadata))[0].value,
            "strip"
        );
        assert!(resolved
            .dump_kdl()
            .contains("set \"child-layout\" \"cards\""));
    }

    #[test]
    fn node_variable_setters_reject_locally_disallowed_values() {
        let error = parse_template_config_kdl(
            r#"
            variable "child-layout" default="cards" {
              value "cards"
              value "strip"
            }
            set "child-layout" "grid"
            "#,
        )
        .expect_err("disallowed setter must fail validation");

        assert!(error
            .to_string()
            .contains("config sets variable child-layout to disallowed value grid"));
    }

    #[test]
    fn layered_node_variable_scope_warns_about_invalid_cross_layer_setters() {
        let bundled = parse_template_config_kdl(
            r#"
            variable "child-layout" default="cards" {
              value "cards"
              value "strip"
            }
            "#,
        )
        .expect("bundled declaration");
        let project = parse_template_config_kdl(
            r#"
            set "child-layout" "compact-strip"
            set "typo-variable" "anything"
            "#,
        )
        .expect("cross-layer setters parse before catalog resolution");
        let catalog = TemplateConfigCatalog::from_layers(vec![
            TemplateConfigLayer::project("flotilla-org/andamento", "project.kdl", project),
            TemplateConfigLayer::bundled("bundled.kdl", bundled),
        ]);
        let metadata = BTreeMap::from([(
            "flotilla.project".to_owned(),
            MetadataValue::Text("flotilla-org/andamento".to_owned()),
        )]);

        let scope = catalog.variable_scope(&metadata);

        assert!(scope.setters.is_empty());
        assert!(scope.warnings.iter().any(|warning| {
            warning.contains("child-layout") && warning.contains("disallowed value compact-strip")
        }));
        assert!(scope
            .warnings
            .iter()
            .any(|warning| warning.contains("undeclared node variable typo-variable")));
    }

    #[test]
    fn region_form_promotions_are_declared_in_order() {
        let config = parse_template_config_kdl(
            r#"
            region "tree" source="tree" root-template="tree" form="compact" {
              promote when="zellij.tab.active" form="full"
              promote when="rail.tab.pinned" form="full"
            }
            "#,
        )
        .expect("promotions parse");

        assert_eq!(config.regions[0].promotions.len(), 2);
        assert_eq!(config.regions[0].promotions[0].when, "zellij.tab.active");
        assert_eq!(config.regions[0].promotions[1].when, "rail.tab.pinned");
    }

    #[test]
    fn configured_region_stack_replaces_bundled_stack_as_one_ordered_surface() {
        let config = parse_template_config_kdl(
            r#"
            region "tree-first" source="tree" root-template="flotilla/region/tree" form="compact"
            region "attention-second" source="attention" root-template="flotilla/region/attention" form="full" attention-key="status.attention"
            "#,
        )
        .expect("regions parse");
        let catalog = TemplateConfigCatalog::with_bundled_defaults(config);

        assert_eq!(
            catalog
                .regions()
                .iter()
                .map(|region| region.name.as_str())
                .collect::<Vec<_>>(),
            vec!["tree-first", "attention-second"]
        );
    }

    #[test]
    fn control_only_template_is_legal_and_controls_survive_resolution() {
        let config = parse_template_config_kdl(
            r#"
            display-variable "show-issues" type="bool" default=true label="Issues" icon="I"
            template "region/controls" slot="compact" node-kind="entity" {
              control "open-config" glyph="⚙"
              control "display-variable" variable="show-issues"
              control "scroll-down"
              control "scroll-up"
              control "inspect-root"
            }
            "#,
        )
        .expect("a section template does not require a loop or field");
        let catalog = TemplateConfigCatalog::from_config(config);
        let metadata = BTreeMap::from([(
            "presentation.template".to_owned(),
            MetadataValue::Text("region/controls".to_owned()),
        )]);
        let resolved = catalog
            .resolve(TemplateConfigMatchContext {
                slot: TemplateConfigSlot::Compact,
                node_kind: TemplateConfigNodeKind::Entity,
                metadata: &metadata,
                collapsed: false,
                collapsible: false,
                active_tab_name: None,
            })
            .expect("control template resolves")
            .expect("declared template exists");

        assert!(resolved.fields.is_empty());
        assert_eq!(resolved.controls.len(), 5);
        assert_eq!(resolved.render_ready().controls, resolved.controls);
        assert!(resolved
            .dump_kdl()
            .contains("control \"display-variable\" variable=\"show-issues\""));
    }
}
