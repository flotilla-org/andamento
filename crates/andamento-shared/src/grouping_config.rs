use std::{collections::BTreeMap, error::Error, fmt, path::Path};

use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::{Deserialize, Serialize};

use crate::{EntityKind, MetadataValue};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExternalGroupingConfig {
    #[serde(default = "default_grouping_config_version")]
    pub version: u32,
    #[serde(default)]
    pub rules: Vec<GroupingRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GroupingRule {
    pub name: String,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub levels: Vec<GroupingLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<EntityFilter>,
    #[serde(default)]
    pub presence: Vec<PresenceMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GroupingLevel {
    pub key: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub label_key: Option<String>,
    #[serde(default)]
    pub collapse_single_member: bool,
    #[serde(default)]
    pub show_empty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EntityFilter {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<MetadataValue>,
    #[serde(default = "default_filter_exists")]
    pub exists: bool,
}

impl EntityFilter {
    pub fn matches(&self, facts: &BTreeMap<String, MetadataValue>) -> bool {
        let value = facts.get(&self.key);
        if self.exists && value.is_none() {
            return false;
        }
        self.equals
            .as_ref()
            .is_none_or(|expected| value == Some(expected))
    }
}

fn default_filter_exists() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresenceClass {
    Tab,
    Section,
    Hidden,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayForm {
    #[default]
    Full,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PresenceMapping {
    pub kind: EntityKind,
    pub class: PresenceClass,
    #[serde(default)]
    pub form: DisplayForm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupingConfigCatalog {
    pub rules: Vec<GroupingRule>,
}

impl Default for GroupingConfigCatalog {
    fn default() -> Self {
        Self {
            rules: vec![bundled_default_rule()],
        }
    }
}

impl GroupingConfigCatalog {
    pub fn from_config(config: ExternalGroupingConfig) -> Self {
        Self::from_rules(config.rules)
    }

    pub fn with_bundled_defaults(config: ExternalGroupingConfig) -> Self {
        let mut rules = config.rules;
        if !rules.iter().any(|rule| rule.name == "flotilla.default") {
            rules.push(bundled_default_rule());
        }
        Self::from_rules(rules)
    }

    fn from_rules(rules: Vec<GroupingRule>) -> Self {
        let mut indexed = rules.into_iter().enumerate().collect::<Vec<_>>();
        indexed.sort_by_key(|(index, rule)| (-rule.priority, *index));
        Self {
            rules: indexed.into_iter().map(|(_, rule)| rule).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn named(&self, name: &str) -> Option<&GroupingRule> {
        self.rules.iter().find(|rule| rule.name == name)
    }
}

pub fn bundled_default_rule() -> GroupingRule {
    let level = |key: &str, label_key: &str, collapse_single_member: bool, show_empty: bool| {
        GroupingLevel {
            key: key.to_owned(),
            optional: true,
            label_key: Some(label_key.to_owned()),
            collapse_single_member,
            show_empty,
        }
    };
    GroupingRule {
        name: "flotilla.default".to_owned(),
        priority: -1_000,
        levels: vec![
            level("flotilla.project", "flotilla.project.name", false, false),
            level("vcs.repo", "vcs.repo.name", false, false),
            level("flotilla.convoy", "flotilla.convoy.name", true, false),
            level("flotilla.vessel", "flotilla.vessel.name", false, false),
            level("flotilla.session", "display.label", false, false),
            level("flotilla.issue", "display.label", false, true),
            level("flotilla.checkout", "display.label", false, true),
        ],
        filter: Some(EntityFilter {
            key: "entity.kind".to_owned(),
            equals: None,
            exists: true,
        }),
        presence: vec![
            PresenceMapping {
                kind: EntityKind::Convoy,
                class: PresenceClass::Tab,
                form: DisplayForm::Full,
                visible_when: None,
            },
            PresenceMapping {
                kind: EntityKind::Vessel,
                class: PresenceClass::Tab,
                form: DisplayForm::Full,
                visible_when: None,
            },
            PresenceMapping {
                kind: EntityKind::Session,
                class: PresenceClass::Tab,
                form: DisplayForm::Full,
                visible_when: None,
            },
            PresenceMapping {
                kind: EntityKind::Issue,
                class: PresenceClass::Section,
                form: DisplayForm::Compact,
                visible_when: Some("show-issues".to_owned()),
            },
            PresenceMapping {
                kind: EntityKind::Project,
                class: PresenceClass::Section,
                form: DisplayForm::Full,
                visible_when: None,
            },
            PresenceMapping {
                kind: EntityKind::Repo,
                class: PresenceClass::Section,
                form: DisplayForm::Full,
                visible_when: None,
            },
            PresenceMapping {
                kind: EntityKind::Checkout,
                class: PresenceClass::Section,
                form: DisplayForm::Full,
                visible_when: None,
            },
        ],
    }
}

pub fn parse_grouping_config_kdl(
    input: &str,
) -> Result<ExternalGroupingConfig, GroupingConfigError> {
    let document = input
        .parse::<KdlDocument>()
        .map_err(|source| GroupingConfigError::Parse(source.to_string()))?;
    let version = document
        .get_arg("version")
        .map(kdl_u32)
        .transpose()?
        .unwrap_or_else(default_grouping_config_version);
    let rules = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == "grouping")
        .map(parse_kdl_grouping_rule)
        .collect::<Result<Vec<_>, _>>()?;
    let config = ExternalGroupingConfig { version, rules };
    validate_grouping_config(&config)?;
    Ok(config)
}

pub fn parse_grouping_config_json(
    input: &str,
) -> Result<ExternalGroupingConfig, GroupingConfigError> {
    let config = serde_json::from_str::<ExternalGroupingConfig>(input)
        .map_err(|source| GroupingConfigError::Parse(source.to_string()))?;
    validate_grouping_config(&config)?;
    Ok(config)
}

pub fn load_grouping_catalog_from_file(
    path: &str,
) -> Result<GroupingConfigCatalog, GroupingConfigError> {
    let resolved = zellij_tile::vfs::resolve_host_path(path)
        .map_err(|err| GroupingConfigError::Io(err.to_string()))?;
    let content = std::fs::read_to_string(&resolved).map_err(|source| {
        GroupingConfigError::Io(format!("failed to read {}: {source}", resolved.display()))
    })?;
    let config = if Path::new(path)
        .extension()
        .is_some_and(|extension| extension == "kdl")
    {
        parse_grouping_config_kdl(&content)?
    } else {
        parse_grouping_config_json(&content)?
    };
    Ok(GroupingConfigCatalog::with_bundled_defaults(config))
}

fn parse_kdl_grouping_rule(node: &KdlNode) -> Result<GroupingRule, GroupingConfigError> {
    let name = node
        .entries()
        .iter()
        .find(|entry| entry.name().is_none())
        .and_then(|entry| match entry.value() {
            KdlValue::String(value) => Some(value.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            GroupingConfigError::Validation("grouping must have a string name".to_owned())
        })?;
    let property_priority = node
        .get("priority")
        .map(|entry| kdl_i64(entry.value()))
        .transpose()?;
    let child_priority = node
        .children()
        .and_then(|children| children.get("priority"))
        .and_then(|priority| priority.entries().first())
        .map(|entry| kdl_i64(entry.value()))
        .transpose()?;
    let priority = property_priority.or(child_priority).unwrap_or_default();
    let children = node.children();
    let levels = children
        .map(|children| {
            children
                .nodes()
                .iter()
                .filter(|child| child.name().value() == "level")
                .map(parse_kdl_grouping_level)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let filter = children
        .and_then(|children| {
            children
                .nodes()
                .iter()
                .find(|child| child.name().value() == "filter")
        })
        .map(parse_kdl_filter)
        .transpose()?;
    let presence = children
        .map(|children| {
            children
                .nodes()
                .iter()
                .filter(|child| child.name().value() == "presence")
                .map(parse_kdl_presence)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(GroupingRule {
        name,
        priority,
        levels,
        filter,
        presence,
    })
}

fn parse_kdl_grouping_level(node: &KdlNode) -> Result<GroupingLevel, GroupingConfigError> {
    let key = string_property(node, "key", "grouping level must have key=\"...\"")?;
    let optional = bool_property(node, "optional")?.unwrap_or(false);
    let label_key = node
        .get("label-key")
        .or_else(|| node.get("label-template"))
        .and_then(|entry| entry.value().as_string().map(str::to_owned));
    let collapse_single_member = bool_property(node, "collapse-single-member")?.unwrap_or(false);
    let show_empty = bool_property(node, "show-empty")?.unwrap_or(false);
    Ok(GroupingLevel {
        key,
        optional,
        label_key,
        collapse_single_member,
        show_empty,
    })
}

fn parse_kdl_filter(node: &KdlNode) -> Result<EntityFilter, GroupingConfigError> {
    let key = string_property(node, "key", "filter must have key=\"...\"")?;
    let equals = node
        .get("equals")
        .map(|entry| kdl_metadata_value(entry.value()))
        .transpose()?;
    let exists = bool_property(node, "exists")?.unwrap_or(true);
    Ok(EntityFilter {
        key,
        equals,
        exists,
    })
}

fn parse_kdl_presence(node: &KdlNode) -> Result<PresenceMapping, GroupingConfigError> {
    let kind = match string_property(node, "kind", "presence must have kind=\"...\"")?.as_str() {
        "project" => EntityKind::Project,
        "repo" => EntityKind::Repo,
        "convoy" => EntityKind::Convoy,
        "vessel" => EntityKind::Vessel,
        "issue" => EntityKind::Issue,
        "session" => EntityKind::Session,
        "checkout" => EntityKind::Checkout,
        other => {
            return Err(GroupingConfigError::Validation(format!(
                "unknown entity kind: {other}"
            )))
        }
    };
    let class = match string_property(node, "class", "presence must have class=\"...\"")?.as_str() {
        "tab" => PresenceClass::Tab,
        "section" => PresenceClass::Section,
        "hidden" => PresenceClass::Hidden,
        other => {
            return Err(GroupingConfigError::Validation(format!(
                "unknown presence class: {other}"
            )))
        }
    };
    let form = match node
        .get("form")
        .and_then(|entry| entry.value().as_string())
        .unwrap_or("full")
    {
        "full" => DisplayForm::Full,
        "compact" => DisplayForm::Compact,
        other => {
            return Err(GroupingConfigError::Validation(format!(
                "unknown display form: {other}"
            )))
        }
    };
    let visible_when = node
        .get("visible-when")
        .and_then(|entry| entry.value().as_string())
        .map(str::to_owned);
    Ok(PresenceMapping {
        kind,
        class,
        form,
        visible_when,
    })
}

fn string_property(
    node: &KdlNode,
    name: &str,
    message: &str,
) -> Result<String, GroupingConfigError> {
    node.get(name)
        .and_then(|entry| entry.value().as_string().map(str::to_owned))
        .ok_or_else(|| GroupingConfigError::Validation(message.to_owned()))
}

fn bool_property(node: &KdlNode, name: &str) -> Result<Option<bool>, GroupingConfigError> {
    node.get(name)
        .map(|entry| kdl_bool(entry.value()))
        .transpose()
}

fn validate_grouping_config(config: &ExternalGroupingConfig) -> Result<(), GroupingConfigError> {
    if config.version != 1 {
        return Err(GroupingConfigError::Validation(format!(
            "unsupported grouping config version: {}",
            config.version
        )));
    }
    for rule in &config.rules {
        if rule.name.trim().is_empty() {
            return Err(GroupingConfigError::Validation(
                "grouping name cannot be empty".to_owned(),
            ));
        }
        if rule.levels.is_empty() {
            return Err(GroupingConfigError::Validation(format!(
                "grouping {} must define at least one level",
                rule.name
            )));
        }
        if rule.levels.iter().any(|level| level.key.trim().is_empty()) {
            return Err(GroupingConfigError::Validation(format!(
                "grouping {} has a level with an empty key",
                rule.name
            )));
        }
    }
    Ok(())
}

fn default_grouping_config_version() -> u32 {
    1
}

fn kdl_u32(value: &KdlValue) -> Result<u32, GroupingConfigError> {
    let Some(value) = value.as_i64() else {
        return Err(GroupingConfigError::Validation(
            "expected integer value".to_owned(),
        ));
    };
    value.try_into().map_err(|_| {
        GroupingConfigError::Validation(format!("expected positive integer, got {value}"))
    })
}

fn kdl_i64(value: &KdlValue) -> Result<i64, GroupingConfigError> {
    value
        .as_i64()
        .ok_or_else(|| GroupingConfigError::Validation("expected integer value".to_owned()))
}

fn kdl_bool(value: &KdlValue) -> Result<bool, GroupingConfigError> {
    if let Some(value) = value.as_bool() {
        return Ok(value);
    }
    match value.as_string() {
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err(GroupingConfigError::Validation(
            "expected boolean value".to_owned(),
        )),
    }
}

fn kdl_metadata_value(value: &KdlValue) -> Result<MetadataValue, GroupingConfigError> {
    if let Some(value) = value.as_string() {
        return Ok(MetadataValue::Text(value.to_owned()));
    }
    if let Some(value) = value.as_bool() {
        return Ok(MetadataValue::Bool(value));
    }
    if let Some(value) = value.as_i64() {
        return Ok(MetadataValue::Integer(value));
    }
    Err(GroupingConfigError::Validation(
        "filter equals must be text, bool, or integer".to_owned(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupingConfigError {
    Io(String),
    Parse(String),
    Validation(String),
}

impl fmt::Display for GroupingConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::Parse(message) | Self::Validation(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for GroupingConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kdl_parses_level_policy_filter_and_presence_mapping() {
        let config = parse_grouping_config_kdl(
            r#"
            version 1
            grouping "active" priority=10 {
              filter key="status.state" equals="active"
              presence kind="issue" class="section"
              level key="flotilla.project" label-key="flotilla.project.name" show-empty=false
              level key="flotilla.convoy" label-key="flotilla.convoy.name" collapse-single-member=true
            }
            "#,
        )
        .expect("config parses");
        let rule = &config.rules[0];
        assert_eq!(
            rule.filter,
            Some(EntityFilter {
                key: "status.state".to_owned(),
                equals: Some(MetadataValue::Text("active".to_owned())),
                exists: true,
            })
        );
        assert!(rule.levels[1].collapse_single_member);
        assert_eq!(
            rule.presence,
            vec![PresenceMapping {
                kind: EntityKind::Issue,
                class: PresenceClass::Section,
                form: DisplayForm::Full,
                visible_when: None,
            }]
        );
    }

    #[test]
    fn bundled_template_is_the_ruled_spine_and_keeps_issues_out_of_tabs() {
        let rule = bundled_default_rule();
        assert_eq!(
            rule.levels
                .iter()
                .map(|level| level.key.as_str())
                .collect::<Vec<_>>(),
            vec![
                "flotilla.project",
                "vcs.repo",
                "flotilla.convoy",
                "flotilla.vessel",
                "flotilla.session",
                "flotilla.issue",
                "flotilla.checkout",
            ]
        );
        assert_eq!(
            rule.presence
                .iter()
                .find(|mapping| mapping.kind == EntityKind::Issue)
                .map(|mapping| (mapping.class, mapping.form, mapping.visible_when.as_deref())),
            Some((
                PresenceClass::Section,
                DisplayForm::Compact,
                Some("show-issues")
            ))
        );
        assert!(rule.levels[2].collapse_single_member);
    }

    #[test]
    fn published_default_template_matches_the_bundled_policy() {
        let config =
            parse_grouping_config_kdl(include_str!("../../../templates/flotilla-default.kdl"))
                .expect("published default parses");
        let published = &config.rules[0];
        let bundled = bundled_default_rule();

        assert_eq!(published.name, bundled.name);
        assert_eq!(published.levels, bundled.levels);
        assert_eq!(published.presence, bundled.presence);
        assert_eq!(published.filter, bundled.filter);
    }
}
