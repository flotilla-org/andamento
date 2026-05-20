use std::error::Error;
use std::fmt;
use std::path::Path;

use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GroupingLevel {
    pub key: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub label_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupingConfigCatalog {
    pub rules: Vec<GroupingRule>,
}

impl GroupingConfigCatalog {
    pub fn from_config(config: ExternalGroupingConfig) -> Self {
        let mut indexed = config.rules.into_iter().enumerate().collect::<Vec<_>>();
        indexed.sort_by_key(|(index, rule)| (-rule.priority, *index));
        Self {
            rules: indexed.into_iter().map(|(_, rule)| rule).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
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
    Ok(GroupingConfigCatalog::from_config(config))
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
    let levels = node
        .children()
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

    Ok(GroupingRule {
        name,
        priority,
        levels,
    })
}

fn parse_kdl_grouping_level(node: &KdlNode) -> Result<GroupingLevel, GroupingConfigError> {
    let key = node
        .get("key")
        .and_then(|entry| entry.value().as_string().map(str::to_owned))
        .ok_or_else(|| {
            GroupingConfigError::Validation("grouping level must have key=\"...\"".to_owned())
        })?;
    let optional = node
        .get("optional")
        .map(|entry| kdl_bool(entry.value()))
        .transpose()?
        .unwrap_or(false);
    let label_key = node
        .get("label-key")
        .or_else(|| node.get("label-template"))
        .and_then(|entry| entry.value().as_string().map(str::to_owned));
    Ok(GroupingLevel {
        key,
        optional,
        label_key,
    })
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
        for level in &rule.levels {
            if level.key.trim().is_empty() {
                return Err(GroupingConfigError::Validation(format!(
                    "grouping {} has a level with an empty key",
                    rule.name
                )));
            }
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
