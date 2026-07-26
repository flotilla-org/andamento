use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

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
    let variables = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "variable")
        .map(parse_kdl_variable)
        .collect::<Result<Vec<_>, _>>()?;
    let config = ExternalTemplateConfig {
        version,
        templates,
        variables,
    };
    config.validate()?;
    Ok(config)
}

pub fn load_template_catalog_from_json_file(
    path: &str,
) -> Result<TemplateConfigCatalog, TemplateConfigError> {
    let resolved = zellij_tile::vfs::resolve_host_path(path)
        .map_err(|err| TemplateConfigError::Io(err.to_string()))?;
    let content = std::fs::read_to_string(&resolved).map_err(|source| {
        TemplateConfigError::Io(format!("failed to read {}: {source}", resolved.display()))
    })?;
    parse_template_config_json(&content).map(TemplateConfigCatalog::with_bundled_defaults)
}

pub fn load_template_catalog_from_file(
    path: &str,
) -> Result<TemplateConfigCatalog, TemplateConfigError> {
    let resolved = zellij_tile::vfs::resolve_host_path(path)
        .map_err(|err| TemplateConfigError::Io(err.to_string()))?;
    let content = std::fs::read_to_string(&resolved).map_err(|source| {
        TemplateConfigError::Io(format!("failed to read {}: {source}", resolved.display()))
    })?;
    if Path::new(path)
        .extension()
        .is_some_and(|extension| extension == "kdl")
    {
        parse_template_config_kdl(&content).map(TemplateConfigCatalog::with_bundled_defaults)
    } else {
        parse_template_config_json(&content).map(TemplateConfigCatalog::with_bundled_defaults)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExternalTemplateConfig {
    #[serde(default = "default_template_config_version")]
    pub version: u32,
    #[serde(default)]
    pub templates: Vec<TemplateConfigDefinition>,
    #[serde(default)]
    pub variables: Vec<TemplateVariableDefinition>,
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
            if template.fields.is_empty() {
                return Err(TemplateConfigError::Validation(format!(
                    "template {} must define at least one field",
                    template.name
                )));
            }
            for field in &template.fields {
                if field.sources.is_empty() {
                    return Err(TemplateConfigError::Validation(format!(
                        "template {} has a field with no sources",
                        template.name
                    )));
                }
            }
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
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigCatalog {
    templates: Vec<TemplateConfigDefinition>,
    variables: Vec<TemplateVariableDefinition>,
    configured_template_names: BTreeSet<String>,
}

impl Default for TemplateConfigCatalog {
    fn default() -> Self {
        let config = bundled_default_config();
        Self {
            templates: config.templates,
            variables: config.variables,
            configured_template_names: BTreeSet::new(),
        }
    }
}

impl TemplateConfigCatalog {
    pub fn from_config(config: ExternalTemplateConfig) -> Self {
        let configured_template_names = config
            .templates
            .iter()
            .map(|template| template.name.clone())
            .collect();
        Self {
            templates: config.templates,
            variables: config.variables,
            configured_template_names,
        }
    }

    pub fn with_bundled_defaults(mut config: ExternalTemplateConfig) -> Self {
        let configured_template_names = config
            .templates
            .iter()
            .map(|template| template.name.clone())
            .collect::<BTreeSet<_>>();
        let mut bundled = bundled_default_config();
        bundled
            .templates
            .retain(|template| !configured_template_names.contains(&template.name));
        bundled.templates.append(&mut config.templates);

        let configured_variable_names = config
            .variables
            .iter()
            .map(|variable| variable.name.clone())
            .collect::<BTreeSet<_>>();
        bundled
            .variables
            .retain(|variable| !configured_variable_names.contains(&variable.name));
        bundled.variables.append(&mut config.variables);
        Self {
            templates: bundled.templates,
            variables: bundled.variables,
            configured_template_names,
        }
    }

    pub fn resolve<'a>(
        &'a self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Option<TemplateConfigResolved<'a>> {
        let candidates = self.matching_candidates(context);
        let template = candidates
            .iter()
            .max_by_key(|candidate| {
                (
                    context.slot.is_rail_local()
                        && self.configured_template_names.contains(&candidate.name),
                    candidate.specificity,
                )
            })
            .and_then(|candidate| {
                self.templates
                    .iter()
                    .find(|template| template.name == candidate.name)
            })?;
        Some(TemplateConfigResolved {
            template,
            specificity: template.specificity(),
            candidates,
            is_bundled: !self.configured_template_names.contains(&template.name),
        })
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    pub fn template_names(&self) -> Vec<String> {
        self.templates
            .iter()
            .map(|template| template.name.clone())
            .collect()
    }

    pub fn variables(&self) -> &[TemplateVariableDefinition] {
        &self.variables
    }

    fn matching_candidates(
        &self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Vec<TemplateConfigCandidate> {
        self.templates
            .iter()
            .filter(|template| template.matches(context))
            .map(|template| TemplateConfigCandidate {
                name: template.name.clone(),
                specificity: template.specificity(),
            })
            .collect()
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
pub struct TemplateConfigResolved<'a> {
    pub template: &'a TemplateConfigDefinition,
    pub specificity: usize,
    pub candidates: Vec<TemplateConfigCandidate>,
    pub is_bundled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigCandidate {
    pub name: String,
    pub specificity: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigRenderedField {
    pub class: TemplateConfigFieldClass,
    pub priority: Option<i64>,
    pub value: String,
    pub source: Option<ResolvedTemplateFieldSource>,
}

fn default_template_config_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateConfigDefinition {
    pub name: String,
    pub slot: TemplateConfigSlot,
    pub node_kind: TemplateConfigNodeKind,
    #[serde(default)]
    pub predicates: Vec<TemplateConfigPredicate>,
    #[serde(default = "default_template_config_sizing")]
    pub sizing: TemplateConfigSizingHint,
    pub fields: Vec<TemplateConfigFieldSpec>,
}

impl TemplateConfigDefinition {
    pub fn render_fields(
        &self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Vec<TemplateConfigRenderedField> {
        self.fields
            .iter()
            .filter_map(|field| field.render(context))
            .collect()
    }

    fn matches(&self, context: TemplateConfigMatchContext<'_>) -> bool {
        self.slot == context.slot
            && self.node_kind == context.node_kind
            && self
                .predicates
                .iter()
                .all(|predicate| predicate.matches(context.metadata))
    }

    fn specificity(&self) -> usize {
        self.predicates
            .iter()
            .map(TemplateConfigPredicate::specificity)
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigNodeKind {
    Group,
    Tab,
    Entity,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigSizingHint {
    Auto,
}

impl TemplateConfigSizingHint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
        }
    }
}

fn default_template_config_sizing() -> TemplateConfigSizingHint {
    TemplateConfigSizingHint::Auto
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TemplateConfigPredicate {
    Exists { key: String },
    TextEquals { key: String, value: String },
    TextOneOf { key: String, values: Vec<String> },
    TextPrefix { key: String, prefix: String },
}

impl TemplateConfigPredicate {
    fn matches(&self, metadata: &BTreeMap<String, MetadataValue>) -> bool {
        match self {
            TemplateConfigPredicate::Exists { key } => metadata.contains_key(key),
            TemplateConfigPredicate::TextEquals { key, value } => {
                metadata_text(metadata, key) == Some(value.as_str())
            }
            TemplateConfigPredicate::TextOneOf { key, values } => metadata_text(metadata, key)
                .is_some_and(|value| values.iter().any(|candidate| candidate == value)),
            TemplateConfigPredicate::TextPrefix { key, prefix } => {
                metadata_text(metadata, key).is_some_and(|value| value.starts_with(prefix))
            }
        }
    }

    fn specificity(&self) -> usize {
        match self {
            TemplateConfigPredicate::Exists { .. } => 1,
            TemplateConfigPredicate::TextPrefix { .. } => 2,
            TemplateConfigPredicate::TextEquals { .. }
            | TemplateConfigPredicate::TextOneOf { .. } => 3,
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            Self::Exists { key } => format!("exists({key})"),
            Self::TextEquals { key, value } => format!("{key} == {value}"),
            Self::TextOneOf { key, values } => {
                format!("{key} in [{}]", values.join(", "))
            }
            Self::TextPrefix { key, prefix } => format!("{key} starts_with {prefix}"),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateConfigFieldSpec {
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
    fn render(
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigFieldClass {
    Required,
    Optional,
    Priority,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
    let mut predicates = vec![];
    let mut fields = vec![];
    for child in children.nodes() {
        match child.name().value() {
            "when" => predicates.push(parse_kdl_when(child)?),
            "predicate" => predicates.push(parse_kdl_predicate(child)?),
            "field" => fields.push(parse_kdl_field(child)?),
            other => {
                return Err(TemplateConfigError::Validation(format!(
                    "template {} has unsupported child node: {other}",
                    kdl_required_arg_string(node, 0, "template name")?
                )));
            }
        }
    }
    Ok(TemplateConfigDefinition {
        name: kdl_required_arg_string(node, 0, "template name")?,
        slot: parse_kdl_slot(&kdl_required_prop_string(node, "slot")?)?,
        node_kind: parse_kdl_node_kind(&kdl_required_prop_string(node, "node-kind")?)?,
        predicates,
        sizing: parse_kdl_sizing(kdl_prop_string(node, "sizing").as_deref().unwrap_or("auto"))?,
        fields,
    })
}

fn parse_kdl_variable(node: &KdlNode) -> Result<TemplateVariableDefinition, TemplateConfigError> {
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

fn parse_kdl_when(node: &KdlNode) -> Result<TemplateConfigPredicate, TemplateConfigError> {
    if let Some(key) = kdl_prop_string(node, "exists") {
        return Ok(TemplateConfigPredicate::Exists { key });
    }
    if let Some(key) = kdl_prop_string(node, "text-equals") {
        return Ok(TemplateConfigPredicate::TextEquals {
            key,
            value: kdl_required_prop_string(node, "value")?,
        });
    }
    if let Some(key) = kdl_prop_string(node, "text-one-of") {
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
        if values.is_empty() {
            return Err(TemplateConfigError::Validation(
                "when text-one-of must define at least one value".to_owned(),
            ));
        }
        return Ok(TemplateConfigPredicate::TextOneOf { key, values });
    }
    if let Some(key) = kdl_prop_string(node, "text-prefix") {
        return Ok(TemplateConfigPredicate::TextPrefix {
            key,
            prefix: kdl_required_prop_string(node, "prefix")?,
        });
    }
    Err(TemplateConfigError::Validation(
        "when must set exists, text-equals, text-one-of, or text-prefix".to_owned(),
    ))
}

fn parse_kdl_predicate(node: &KdlNode) -> Result<TemplateConfigPredicate, TemplateConfigError> {
    match kdl_required_arg_string(node, 0, "predicate kind")?.as_str() {
        "exists" => Ok(TemplateConfigPredicate::Exists {
            key: kdl_required_prop_string(node, "key")?,
        }),
        "text-equals" => Ok(TemplateConfigPredicate::TextEquals {
            key: kdl_required_prop_string(node, "key")?,
            value: kdl_required_prop_string(node, "value")?,
        }),
        "text-one-of" => {
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
            if values.is_empty() {
                return Err(TemplateConfigError::Validation(
                    "text-one-of predicate must define at least one value".to_owned(),
                ));
            }
            Ok(TemplateConfigPredicate::TextOneOf {
                key: kdl_required_prop_string(node, "key")?,
                values,
            })
        }
        "text-prefix" => Ok(TemplateConfigPredicate::TextPrefix {
            key: kdl_required_prop_string(node, "key")?,
            prefix: kdl_required_prop_string(node, "prefix")?,
        }),
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported predicate kind: {other}"
        ))),
    }
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

fn parse_kdl_sizing(value: &str) -> Result<TemplateConfigSizingHint, TemplateConfigError> {
    match value {
        "auto" => Ok(TemplateConfigSizingHint::Auto),
        other => Err(TemplateConfigError::Validation(format!(
            "unsupported template sizing: {other}"
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

    #[test]
    fn text_one_of_matches_list_members_and_has_exact_match_specificity() {
        let predicate = TemplateConfigPredicate::TextOneOf {
            key: "group.key".to_owned(),
            values: vec!["vcs.repo".to_owned(), "flotilla.project".to_owned()],
        };

        assert!(predicate.matches(&BTreeMap::from([(
            "group.key".to_owned(),
            MetadataValue::Text("vcs.repo".to_owned()),
        )])));
        assert!(!predicate.matches(&BTreeMap::from([(
            "group.key".to_owned(),
            MetadataValue::Text("session".to_owned()),
        )])));
        assert_eq!(predicate.specificity(), 3);
    }

    #[test]
    fn bundled_catalog_supplies_compact_issue_templates_and_toggle() {
        let catalog = TemplateConfigCatalog::default();
        let metadata = BTreeMap::from([(
            "entity.kind".to_owned(),
            MetadataValue::Text("issue".to_owned()),
        )]);

        assert!(catalog.variables().iter().any(|variable| {
            variable.name == "show-issues"
                && variable.default == crate::DisplayVariableValue::Bool(true)
        }));
        for slot in [TemplateConfigSlot::Compact, TemplateConfigSlot::Detail] {
            assert!(catalog
                .resolve(TemplateConfigMatchContext {
                    slot,
                    node_kind: TemplateConfigNodeKind::Entity,
                    metadata: &metadata,
                    collapsed: false,
                    collapsible: true,
                    active_tab_name: None,
                })
                .is_some());
        }
    }

    #[test]
    fn published_default_template_kdl_matches_the_bundled_catalog() {
        let published =
            parse_template_config_kdl(include_str!("../../../templates/flotilla-default.kdl"))
                .expect("published default template KDL parses");
        let published_names = published
            .templates
            .iter()
            .map(|template| template.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            published_names,
            vec![
                "flotilla.issue.compact",
                "flotilla.issue.detail",
                "flotilla/group-header/vcs-repo",
                "flotilla/group-header/project",
                "flotilla/group-header/convoy",
                "flotilla/group-header/vessel",
                "flotilla/group-header/independent",
                "flotilla/group-header/session",
                "flotilla/group-header/checkout",
                "flotilla/group-header/issue",
                "flotilla/group-header/recognized",
                "flotilla/group-header/fallback",
                "flotilla/group-header/legacy",
                "flotilla/tab-title",
                "flotilla/tab-status",
                "flotilla/tab-status/waiting",
                "flotilla/tab-status/terminal-source",
            ]
        );

        let catalog = TemplateConfigCatalog::default();
        assert_eq!(
            catalog.template_names(),
            published_names
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn configured_templates_override_more_specific_bundled_templates() {
        let configured = parse_template_config_kdl(
            r#"
            template "user.group-header" slot="group-header" node-kind="group" {
              field source="literal" value="user"
            }
            "#,
        )
        .expect("user template parses");
        let catalog = TemplateConfigCatalog::with_bundled_defaults(configured);
        let metadata = BTreeMap::from([
            (
                "group.key".to_owned(),
                MetadataValue::Text("vcs.repo".to_owned()),
            ),
            (
                "vcs.repo".to_owned(),
                MetadataValue::Text("flotilla-org/andamento".to_owned()),
            ),
        ]);

        let resolved = catalog
            .resolve(TemplateConfigMatchContext {
                slot: TemplateConfigSlot::GroupHeader,
                node_kind: TemplateConfigNodeKind::Group,
                metadata: &metadata,
                collapsed: false,
                collapsible: true,
                active_tab_name: None,
            })
            .expect("group template resolves");

        assert_eq!(resolved.template.name, "user.group-header");
        assert!(!resolved.is_bundled);
    }

    #[test]
    fn entity_template_specificity_is_unchanged_by_rail_slot_layering() {
        let configured = parse_template_config_kdl(
            r#"
            template "user.compact" slot="compact" node-kind="entity" {
              field source="literal" value="user"
            }
            "#,
        )
        .expect("user template parses");
        let catalog = TemplateConfigCatalog::with_bundled_defaults(configured);
        let metadata = BTreeMap::from([(
            "entity.kind".to_owned(),
            MetadataValue::Text("issue".to_owned()),
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
            .expect("compact template resolves");

        assert_eq!(resolved.template.name, "flotilla.issue.compact");
        assert!(resolved.is_bundled);
    }

    #[test]
    fn parses_external_template_config_json() {
        let config = parse_template_config_json(
            r#"
            {
              "version": 1,
              "templates": [
                {
                  "name": "custom.tab-title",
                  "slot": "tab-title",
                  "node-kind": "tab",
                  "predicates": [
                    { "kind": "exists", "key": "zellij.tab.name" }
                  ],
                  "sizing": "auto",
                  "fields": [
                    {
                      "class": "required",
                      "sources": [
                        { "kind": "metadata-text", "key": "zellij.tab.name" },
                        { "kind": "literal", "value": "Tab" }
                      ]
                    }
                  ]
                }
              ]
            }
            "#,
        )
        .expect("valid template config");

        assert_eq!(config.version, 1);
        assert_eq!(config.templates[0].name, "custom.tab-title");
        assert_eq!(config.templates[0].slot, TemplateConfigSlot::TabTitle);
        assert_eq!(config.templates[0].node_kind, TemplateConfigNodeKind::Tab);
        assert_eq!(config.templates[0].sizing, TemplateConfigSizingHint::Auto);
        assert_eq!(config.templates[0].fields[0].prefix, "");
        assert_eq!(
            config.templates[0].fields[0].sources[0],
            TemplateConfigValueSource::MetadataText {
                key: "zellij.tab.name".to_owned()
            }
        );
    }

    #[test]
    fn parses_typed_display_variables_and_entity_forms() {
        let config = parse_template_config_kdl(
            r##"
            version 1
            variable "show-issues" type="bool" default=true label="Issues" icon="I" persist=true
            template "issue.compact" slot="compact" node-kind="entity" {
              field source="metadata-first-token" key="display.label"
            }
            template "issue.detail" slot="detail" node-kind="entity" {
              field key="display.label"
            }
            "##,
        )
        .expect("display model parses");

        assert_eq!(
            config.variables,
            vec![TemplateVariableDefinition {
                name: "show-issues".to_owned(),
                variable_type: TemplateVariableType::Bool,
                default: crate::DisplayVariableValue::Bool(true),
                label: "Issues".to_owned(),
                icon: "I".to_owned(),
                persist: true,
            }]
        );
        assert_eq!(config.templates[0].slot, TemplateConfigSlot::Compact);
        assert_eq!(config.templates[1].slot, TemplateConfigSlot::Detail);
        assert_eq!(
            config.templates[0].fields[0].sources[0],
            TemplateConfigValueSource::MetadataFirstToken {
                key: "display.label".to_owned()
            }
        );
    }

    #[test]
    fn parses_compact_template_config_kdl() {
        let config = parse_template_config_kdl(
            r#"
            version 1

            template "git.group-header" slot="group-header" node-kind="group" {
              when exists="git.repo"

              field priority=100 {
                value source="collapsed-toggle" collapsed="▶" expanded="▼"
              }
              field priority=100 {
                value key="git.repo"
                value key="group.label"
              }
              field key="git.branch" priority=60 prefix=" "
              field source="active-tab-name" priority=80 condition="collapsed" prefix=": "
            }
            "#,
        )
        .expect("valid template config");

        assert_eq!(config.version, 1);
        assert_eq!(config.templates[0].name, "git.group-header");
        assert_eq!(config.templates[0].slot, TemplateConfigSlot::GroupHeader);
        assert_eq!(config.templates[0].node_kind, TemplateConfigNodeKind::Group);
        assert_eq!(
            config.templates[0].predicates,
            vec![TemplateConfigPredicate::Exists {
                key: "git.repo".to_owned()
            }]
        );
        assert_eq!(config.templates[0].fields[0].priority, Some(100));
        assert_eq!(
            config.templates[0].fields[0].sources[0],
            TemplateConfigValueSource::CollapsedToggle {
                collapsed: "▶".to_owned(),
                expanded: "▼".to_owned()
            }
        );
        assert_eq!(config.templates[0].fields[2].prefix, " ");
        assert_eq!(config.templates[0].fields[2].priority, Some(60));
        assert_eq!(
            config.templates[0].fields[2].sources,
            vec![TemplateConfigValueSource::MetadataDisplay {
                key: "git.branch".to_owned()
            }]
        );
        assert_eq!(
            config.templates[0].fields[3].condition,
            TemplateConfigFieldCondition::Collapsed
        );
    }

    #[test]
    fn rejects_duplicate_template_names() {
        let error = parse_template_config_json(
            r#"
            {
              "templates": [
                {
                  "name": "dup",
                  "slot": "tab-title",
                  "node-kind": "tab",
                  "fields": [
                    { "class": "required", "sources": [{ "kind": "literal", "value": "one" }] }
                  ]
                },
                {
                  "name": "dup",
                  "slot": "tab-title",
                  "node-kind": "tab",
                  "fields": [
                    { "class": "required", "sources": [{ "kind": "literal", "value": "two" }] }
                  ]
                }
              ]
            }
            "#,
        )
        .expect_err("duplicate names should be rejected");

        assert!(error.to_string().contains("duplicate template name: dup"));
    }

    #[test]
    fn catalog_resolves_highest_specificity_template() {
        let config = parse_template_config_json(
            r#"
            {
              "templates": [
                {
                  "name": "generic-status",
                  "slot": "tab-status",
                  "node-kind": "tab",
                  "predicates": [
                    { "kind": "exists", "key": "status.title" }
                  ],
                  "fields": [
                    { "class": "required", "sources": [{ "kind": "metadata-text", "key": "status.title" }] }
                  ]
                },
                {
                  "name": "waiting-status",
                  "slot": "tab-status",
                  "node-kind": "tab",
                  "predicates": [
                    { "kind": "exists", "key": "status.title" },
                    { "kind": "text-equals", "key": "status.priority", "value": "waiting" }
                  ],
                  "fields": [
                    { "class": "required", "sources": [{ "kind": "metadata-text", "key": "status.title" }] }
                  ]
                }
              ]
            }
            "#,
        )
        .expect("valid template config");
        let catalog = TemplateConfigCatalog::from_config(config);
        let metadata = BTreeMap::from([
            (
                "status.title".to_owned(),
                MetadataValue::Text("waiting".to_owned()),
            ),
            (
                "status.priority".to_owned(),
                MetadataValue::Text("waiting".to_owned()),
            ),
        ]);

        let resolved = catalog
            .resolve(TemplateConfigMatchContext {
                slot: TemplateConfigSlot::TabStatus,
                node_kind: TemplateConfigNodeKind::Tab,
                metadata: &metadata,
                collapsed: false,
                collapsible: true,
                active_tab_name: None,
            })
            .expect("matching template");

        assert_eq!(resolved.template.name, "waiting-status");
        assert_eq!(resolved.specificity, 4);
        assert_eq!(
            resolved.candidates,
            vec![
                TemplateConfigCandidate {
                    name: "generic-status".to_owned(),
                    specificity: 1,
                },
                TemplateConfigCandidate {
                    name: "waiting-status".to_owned(),
                    specificity: 4,
                },
            ]
        );
    }

    #[test]
    fn catalog_renders_field_specs_from_resolved_template() {
        let config = parse_template_config_json(
            r#"
            {
              "templates": [
                {
                  "name": "status",
                  "slot": "tab-status",
                  "node-kind": "tab",
                  "predicates": [
                    { "kind": "exists", "key": "status.title" }
                  ],
                  "fields": [
                    {
                      "class": "required",
                      "sources": [{ "kind": "metadata-text", "key": "status.title" }]
                    },
                    {
                      "class": "priority",
                      "prefix": ": ",
                      "sources": [{ "kind": "metadata-text", "key": "status.detail" }]
                    }
                  ]
                }
              ]
            }
            "#,
        )
        .expect("valid template config");
        let catalog = TemplateConfigCatalog::from_config(config);
        let metadata = BTreeMap::from([
            (
                "status.title".to_owned(),
                MetadataValue::Text("waiting".to_owned()),
            ),
            (
                "status.detail".to_owned(),
                MetadataValue::Text("input".to_owned()),
            ),
        ]);
        let context = TemplateConfigMatchContext {
            slot: TemplateConfigSlot::TabStatus,
            node_kind: TemplateConfigNodeKind::Tab,
            metadata: &metadata,
            collapsed: false,
            collapsible: true,
            active_tab_name: None,
        };

        let rendered = catalog
            .resolve(context)
            .expect("matching template")
            .template
            .render_fields(context);

        assert_eq!(
            rendered,
            vec![
                TemplateConfigRenderedField {
                    class: TemplateConfigFieldClass::Required,
                    priority: None,
                    value: "waiting".to_owned(),
                    source: Some(ResolvedTemplateFieldSource {
                        key: "status.title".to_owned(),
                        value: MetadataValue::Text("waiting".to_owned()),
                    }),
                },
                TemplateConfigRenderedField {
                    class: TemplateConfigFieldClass::Priority,
                    priority: None,
                    value: ": input".to_owned(),
                    source: Some(ResolvedTemplateFieldSource {
                        key: "status.detail".to_owned(),
                        value: MetadataValue::Text("input".to_owned()),
                    }),
                },
            ]
        );
    }

    #[test]
    fn loads_template_catalog_from_json_file() {
        let path = std::env::temp_dir().join(format!(
            "andamento-rail-template-config-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"
            {
              "templates": [
                {
                  "name": "file-tab-title",
                  "slot": "tab-title",
                  "node-kind": "tab",
                  "fields": [
                    { "class": "required", "sources": [{ "kind": "literal", "value": "From file" }] }
                  ]
                }
              ]
            }
            "#,
        )
        .expect("write fixture");

        let catalog =
            load_template_catalog_from_json_file(&path.to_string_lossy()).expect("load catalog");
        std::fs::remove_file(&path).expect("remove fixture");
        let metadata = BTreeMap::new();

        let resolved = catalog
            .resolve(TemplateConfigMatchContext {
                slot: TemplateConfigSlot::TabTitle,
                node_kind: TemplateConfigNodeKind::Tab,
                metadata: &metadata,
                collapsed: false,
                collapsible: true,
                active_tab_name: None,
            })
            .expect("matching template");

        assert_eq!(resolved.template.name, "file-tab-title");
    }
}
