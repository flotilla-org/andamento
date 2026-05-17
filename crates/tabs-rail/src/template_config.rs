use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateConfigError {
    Parse(String),
    Validation(String),
}

impl fmt::Display for TemplateConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
}
