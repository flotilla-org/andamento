//! Structured detail content keyed by catalog identity, independent of placements.
use crate::{template_config::DetailRole, EntityRef, MetadataEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailField {
    pub name: String,
    pub section: String,
    pub role: DetailRole,
    pub label: String,
    /// Unadorned resolved text. None is missing; Some("") is known empty.
    pub text: Option<String>,
    pub source_key: String,
    pub observation: Option<MetadataEntry>,
    /// Winning producer identity, preserved along with retained observations.
    pub source_id: Option<String>,
    pub relations: Vec<EntityRef>,
}

/// Primary control intent, shared by native text and default labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailIntent {
    OpenUrl,
    FocusWorkspace,
    MaterializeWorkspace,
    Inspect,
}
impl DetailIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenUrl => "open-url",
            Self::FocusWorkspace => "focus-workspace",
            Self::MaterializeWorkspace => "materialize-workspace",
            Self::Inspect => "inspect",
        }
    }
    pub fn default_label(self) -> &'static str {
        match self {
            Self::OpenUrl | Self::MaterializeWorkspace => "Open",
            Self::FocusWorkspace => "Focus",
            Self::Inspect => "Inspect",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailCard {
    pub entity: EntityRef,
    pub label: String,
    pub fields: Vec<DetailField>,
    /// Preview identity is independent of the semantic fields.
    pub workspace_id: Option<crate::WorkspaceId>,
    pub primary_label: String,
    pub primary_intent: DetailIntent,
    pub error: Option<String>,
}

/// Resolve semantic content directly from typed sources. Prefix/suffix remain
/// exclusively in the legacy flat rendering; missing declarations stay visible.
pub(crate) fn resolve_field(
    spec: &crate::template_config::TemplateConfigFieldSpec,
    context: crate::template_config::TemplateConfigMatchContext<'_>,
    entries: &std::collections::BTreeMap<String, MetadataEntry>,
) -> Option<DetailField> {
    use crate::template_config::{
        TemplateConfigResolvedValue, TemplateConfigValueSource as Source,
    };
    let role = spec.role?;
    if !spec.condition.matches(context) {
        return None;
    }
    let resolved = spec.sources.iter().find_map(|source| match source {
        // Known-empty literals are values in structured content. Flat rendering
        // preserves its existing omission of empty literals.
        Source::Literal { value } => Some(TemplateConfigResolvedValue {
            value: value.clone(),
            source: None,
        }),
        _ => source.resolve(context),
    });
    let source = resolved.as_ref().and_then(|value| value.source.as_ref());
    // A missing field has no winning source; do not guess one from fallback order.
    let source_key = source.map(|source| source.key.clone()).unwrap_or_default();
    let observation = source.and_then(|source| entries.get(&source.key)).cloned();
    let relations = if role == DetailRole::Relation {
        match source.map(|source| &source.value) {
            Some(crate::MetadataValue::EntityRefs(refs)) => {
                let mut seen = std::collections::BTreeSet::new();
                refs.iter()
                    .filter(|entity| seen.insert((*entity).clone()))
                    .cloned()
                    .collect()
            }
            _ => vec![],
        }
    } else {
        vec![]
    };
    Some(DetailField {
        name: spec.name.clone(),
        section: spec.section.clone().unwrap_or_default(),
        role,
        label: spec.label.clone().unwrap_or_default(),
        text: resolved.map(|value| value.value),
        source_key,
        observation,
        source_id: None,
        relations,
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        template_config::*, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate,
        Sidebar,
    };
    use std::collections::BTreeMap;

    // Enumerate every semantic role and both source-presence states. Configuration
    // inheritance and KDL dumps must preserve declarations without parsing text.
    #[test]
    fn declarations_round_trip_and_text_remains_unadorned() {
        for role in ["identity", "title", "state", "fact", "relation"] {
            for value in [None, Some("")] {
                let config = format!(
                    r#"
                    template "item/detail" slot="detail" node-kind="entity" {{
                      field "value" section="custom" role="{role}" label="Label" key="fact" prefix="Prefix: " suffix="!"
                      field "literal" role="fact" source="literal" value=""
                    }}
                "#
                );
                let parsed = parse_template_config_kdl(&config).unwrap();
                let json = format!(
                    r#"{{"templates":[{{"name":"item/detail","slot":"detail","node-kind":"entity","operations":[{{"kind":"set","field":{{"name":"value","class":"required","section":"custom","role":"{role}","label":"Label","sources":[{{"kind":"metadata-display","key":"fact"}}],"prefix":"Prefix: ","suffix":"!","priority":100}}}},{{"kind":"set","field":{{"name":"literal","class":"required","role":"fact","sources":[{{"kind":"literal","value":""}}],"priority":100}}}}]}}]}}"#
                );
                assert_eq!(parse_template_config_json(&json).unwrap(), parsed);
                let facts =
                    BTreeMap::from([("entity.kind".into(), MetadataValue::Text("item".into()))]);
                let context = TemplateConfigMatchContext {
                    slot: TemplateConfigSlot::Detail,
                    node_kind: TemplateConfigNodeKind::Entity,
                    metadata: &facts,
                    collapsed: false,
                    collapsible: false,
                    active_tab_name: None,
                };
                let resolved = TemplateConfigCatalog::from_config(parsed.clone())
                    .resolve(context)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    parse_template_config_kdl(&resolved.dump_kdl())
                        .unwrap()
                        .templates[0]
                        .operations,
                    parsed.templates[0].operations
                );
                let mut sidebar = Sidebar::new(&config).unwrap();
                let mut set = BTreeMap::from([(
                    "display.label".into(),
                    MetadataValueUpdate {
                        value: MetadataValue::Text("Entity".into()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                )]);
                if let Some(value) = value {
                    set.insert(
                        "fact".into(),
                        MetadataValueUpdate {
                            value: MetadataValue::Text(value.into()),
                            ttl_ms: Some(10),
                            precedence: None,
                            ordinal: None,
                        },
                    );
                }
                sidebar.apply(
                    0,
                    [MetadataPatch {
                        target: MetadataTarget::Entity(crate::EntityRef {
                            kind: "item".into(),
                            id: "id".into(),
                        }),
                        source_id: "producer".into(),
                        set,
                        unset: vec![],
                    }],
                );
                let (_, cards) = sidebar.detail_cards();
                let f = &cards[0].fields[0];
                assert_eq!(f.role.as_str(), role);
                assert_eq!(f.text.as_deref(), value);
                assert_eq!(f.source_key, if value.is_some() { "fact" } else { "" });
                assert_eq!(f.observation.is_some(), value.is_some());
                assert_eq!(cards[0].fields[1].text.as_deref(), Some(""));
            }
        }
    }

    // Invalid semantic roles must be rejected by both frontend-neutral formats.
    #[test]
    fn invalid_roles_are_rejected() {
        assert!(parse_template_config_kdl(
            r#"template "item/detail" { field "x" role=1 literal="x"; }"#
        )
        .is_err());
        assert!(parse_template_config_kdl(
            r#"template "item/detail" { field "x" label=true literal="x"; }"#
        )
        .is_err());
        assert!(parse_template_config_kdl(
            r#"template "item/detail" { field "x" role="color-red" literal="x"; }"#
        )
        .is_err());
        assert!(parse_template_config_json(r#"{"templates":[{"name":"item/detail","operations":[{"kind":"set","field":{"name":"x","class":"required","role":"color-red","sources":[{"kind":"literal","value":"x"}]}}]}]}"#).is_err());
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::{template_config::*, MetadataValue};
    use std::collections::BTreeMap;
    // Structured-only declarations still obey conditions, missing fallbacks have
    // no winner, and duplicate relations retain the first occurrence order.
    #[test]
    fn conditional_fields_fallbacks_and_duplicate_relations() {
        let parsed = parse_template_config_kdl(r#"
            template "item/detail" {
                field "hidden" role="fact" structured-only=true condition="collapsed" literal="value"
                field "fallback" role="fact" { value key="missing-first"; value key="missing-second"; }
                field "relations" role="relation" key="refs"
            }
        "#).unwrap();
        let targets = [
            EntityRef {
                kind: "item".into(),
                id: "a".into(),
            },
            EntityRef {
                kind: "item".into(),
                id: "b".into(),
            },
        ];
        let metadata = BTreeMap::from([(
            "refs".into(),
            MetadataValue::EntityRefs(vec![
                targets[0].clone(),
                targets[0].clone(),
                targets[1].clone(),
                targets[0].clone(),
            ]),
        )]);
        let context = TemplateConfigMatchContext {
            slot: TemplateConfigSlot::Detail,
            node_kind: TemplateConfigNodeKind::Entity,
            metadata: &metadata,
            collapsed: false,
            collapsible: true,
            active_tab_name: None,
        };
        let fields: Vec<_> = parsed.templates[0]
            .operations
            .iter()
            .map(|op| {
                let TemplateConfigFieldOperation::Set { field } = op else {
                    panic!("field expected")
                };
                field
            })
            .collect();
        assert!(resolve_field(fields[0], context, &BTreeMap::new()).is_none());
        assert_eq!(
            resolve_field(
                fields[0],
                TemplateConfigMatchContext {
                    collapsed: true,
                    ..context
                },
                &BTreeMap::new()
            )
            .unwrap()
            .text
            .as_deref(),
            Some("value")
        );
        let missing = resolve_field(fields[1], context, &BTreeMap::new()).unwrap();
        assert_eq!(missing.text, None);
        assert_eq!(missing.source_key, "");
        assert_eq!(
            resolve_field(fields[2], context, &BTreeMap::new())
                .unwrap()
                .relations,
            targets
        );
        assert_eq!(
            [
                DetailRole::Identity,
                DetailRole::Title,
                DetailRole::State,
                DetailRole::Fact,
                DetailRole::Relation
            ]
            .map(|role| role as u32),
            [0, 1, 2, 3, 4]
        );
    }
}
