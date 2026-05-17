use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use tabs_shared::MetadataValue;

pub fn parse_template_config_json(
    input: &str,
) -> Result<ExternalTemplateConfig, TemplateConfigError> {
    let config = serde_json::from_str::<ExternalTemplateConfig>(input)
        .map_err(|source| TemplateConfigError::Parse(source.to_string()))?;
    config.validate()?;
    Ok(config)
}

pub fn load_template_catalog_from_json_file(
    path: impl AsRef<Path>,
) -> Result<TemplateConfigCatalog, TemplateConfigError> {
    let content = std::fs::read_to_string(path.as_ref()).map_err(|source| {
        TemplateConfigError::Io(format!(
            "failed to read {}: {source}",
            path.as_ref().display()
        ))
    })?;
    parse_template_config_json(&content).map(TemplateConfigCatalog::from_config)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExternalTemplateConfig {
    #[serde(default = "default_template_config_version")]
    pub version: u32,
    #[serde(default)]
    pub templates: Vec<TemplateConfigDefinition>,
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
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigCatalog {
    templates: Vec<TemplateConfigDefinition>,
}

impl TemplateConfigCatalog {
    pub fn from_config(config: ExternalTemplateConfig) -> Self {
        Self {
            templates: config.templates,
        }
    }

    pub fn resolve<'a>(
        &'a self,
        context: TemplateConfigMatchContext<'_>,
    ) -> Option<TemplateConfigResolved<'a>> {
        let candidates = self.matching_candidates(context);
        let template = candidates
            .iter()
            .max_by_key(|candidate| candidate.specificity)
            .and_then(|candidate| {
                self.templates
                    .iter()
                    .find(|template| template.name == candidate.name)
            })?;
        Some(TemplateConfigResolved {
            template,
            specificity: template.specificity(),
            candidates,
        })
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

#[derive(Debug, Clone, Copy)]
pub struct TemplateConfigMatchContext<'a> {
    pub slot: TemplateConfigSlot,
    pub node_kind: TemplateConfigNodeKind,
    pub metadata: &'a BTreeMap<String, MetadataValue>,
    pub collapsed: bool,
    pub active_tab_name: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigResolved<'a> {
    pub template: &'a TemplateConfigDefinition,
    pub specificity: usize,
    pub candidates: Vec<TemplateConfigCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigCandidate {
    pub name: String,
    pub specificity: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConfigRenderedField {
    pub class: TemplateConfigFieldClass,
    pub value: String,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigNodeKind {
    Group,
    Tab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateConfigSizingHint {
    Auto,
}

fn default_template_config_sizing() -> TemplateConfigSizingHint {
    TemplateConfigSizingHint::Auto
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TemplateConfigPredicate {
    Exists { key: String },
    TextEquals { key: String, value: String },
    TextPrefix { key: String, prefix: String },
}

impl TemplateConfigPredicate {
    fn matches(&self, metadata: &BTreeMap<String, MetadataValue>) -> bool {
        match self {
            TemplateConfigPredicate::Exists { key } => metadata.contains_key(key),
            TemplateConfigPredicate::TextEquals { key, value } => {
                metadata_text(metadata, key) == Some(value.as_str())
            }
            TemplateConfigPredicate::TextPrefix { key, prefix } => {
                metadata_text(metadata, key).is_some_and(|value| value.starts_with(prefix))
            }
        }
    }

    fn specificity(&self) -> usize {
        match self {
            TemplateConfigPredicate::Exists { .. } => 1,
            TemplateConfigPredicate::TextPrefix { .. } => 2,
            TemplateConfigPredicate::TextEquals { .. } => 3,
        }
    }
}

fn metadata_text<'a>(metadata: &'a BTreeMap<String, MetadataValue>, key: &str) -> Option<&'a str> {
    match metadata.get(key) {
        Some(MetadataValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn metadata_display_value(metadata: &BTreeMap<String, MetadataValue>, key: &str) -> Option<String> {
    metadata.get(key).map(format_metadata_value)
}

fn format_metadata_value(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Text(value) => value.clone(),
        MetadataValue::Bool(value) => value.to_string(),
        MetadataValue::Integer(value) => value.to_string(),
        MetadataValue::StringList(values) => values.join(", "),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TemplateConfigFieldSpec {
    pub class: TemplateConfigFieldClass,
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
        let value = self
            .sources
            .iter()
            .find_map(|source| source.resolve(context))?;
        let value = format!("{}{}{}", self.prefix, value, self.suffix);
        Some(TemplateConfigRenderedField {
            class: self.class,
            value,
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
    MetadataDisplay { key: String },
    TabNumberFromPosition,
    ActiveTabName,
    CollapsedToggle { collapsed: String, expanded: String },
}

impl TemplateConfigValueSource {
    fn resolve(&self, context: TemplateConfigMatchContext<'_>) -> Option<String> {
        let value = match self {
            TemplateConfigValueSource::Literal { value } => value.clone(),
            TemplateConfigValueSource::MetadataText { key } => {
                metadata_text(context.metadata, key)?.to_owned()
            }
            TemplateConfigValueSource::MetadataDisplay { key } => {
                metadata_display_value(context.metadata, key)?
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
                if context.collapsed {
                    collapsed.clone()
                } else {
                    expanded.clone()
                }
            }
        };
        (!value.is_empty()).then_some(value)
    }
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
                    value: "waiting".to_owned(),
                },
                TemplateConfigRenderedField {
                    class: TemplateConfigFieldClass::Priority,
                    value: ": input".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn loads_template_catalog_from_json_file() {
        let path = std::env::temp_dir().join(format!(
            "tabs-rail-template-config-{}.json",
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

        let catalog = load_template_catalog_from_json_file(&path).expect("load catalog");
        std::fs::remove_file(&path).expect("remove fixture");
        let metadata = BTreeMap::new();

        let resolved = catalog
            .resolve(TemplateConfigMatchContext {
                slot: TemplateConfigSlot::TabTitle,
                node_kind: TemplateConfigNodeKind::Tab,
                metadata: &metadata,
                collapsed: false,
                active_tab_name: None,
            })
            .expect("matching template");

        assert_eq!(resolved.template.name, "file-tab-title");
    }
}
