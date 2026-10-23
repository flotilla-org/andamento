//! A subject's tool strip, as its `workspace kind="…"` declaration's `strip`
//! block declares it: the related workspaces and linked items a host lists
//! beside the workspace's Slots and layouts
//! (docs/sidebar-design/tool-strip.md).
//!
//! Each section is a list of placement loops evaluated with the subject bound
//! by its kind's name, so `for "vessel" kind="vessel" { match
//! "flotilla.convoy" of="convoy"; }` lists a convoy's vessels and `for
//! "project" kind="project" in="flotilla.project" of="convoy"` its project.
//! The strip lists the matches; the host reads each one's detail card for its
//! label, state, hover card and actions.
use std::collections::BTreeSet;

use kdl::KdlNode;
use serde::Deserialize;

use crate::template_config::{PlacementLoop, TemplateConfigError};
use crate::EntityRef;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct StripDefinition {
    /// The workspaces next to this one: ancestors, then children.
    #[serde(default)]
    pub related: Vec<PlacementLoop>,
    /// The items the subject links to, such as its change requests and issues.
    #[serde(default)]
    pub linked: Vec<PlacementLoop>,
}

impl StripDefinition {
    pub fn is_empty(&self) -> bool {
        self.related.is_empty() && self.linked.is_empty()
    }

    pub(crate) fn sections(&self) -> [(StripSection, &[PlacementLoop]); 2] {
        [(StripSection::Related, &self.related), (StripSection::Linked, &self.linked)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StripSection {
    Related,
    Linked,
}

/// One entity a strip section lists, in section order, then loop order,
/// then each loop's own order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripEntry {
    pub section: StripSection,
    pub entity: EntityRef,
}

/// Read a `strip` node: its `related` and `linked` sections, each a list of
/// `for` loops.
pub(crate) fn parse_kdl_strip(node: &KdlNode, kind: &str) -> Result<StripDefinition, TemplateConfigError> {
    let at = format!("workspace {kind} strip");
    if !node.entries().is_empty() {
        return Err(invalid(format!("{at} takes no arguments or properties")));
    }
    let scope = BTreeSet::from([kind.to_owned()]);
    let mut strip = StripDefinition::default();
    let mut seen = BTreeSet::new();
    for child in node.children().map(|children| children.nodes()).unwrap_or_default() {
        let name = child.name().value();
        let section = match name {
            "related" => &mut strip.related,
            "linked" => &mut strip.linked,
            other => return Err(invalid(format!("{at} only accepts related and linked, not {other}"))),
        };
        if !seen.insert(name) {
            return Err(invalid(format!("{at} declares {name} twice")));
        }
        if !child.entries().is_empty() {
            return Err(invalid(format!("{at}: {name} takes no arguments or properties")));
        }
        for item in child.children().map(|children| children.nodes()).unwrap_or_default() {
            if item.name().value() != "for" {
                return Err(invalid(format!("{at}: {name} only accepts for, not {}", item.name().value())));
            }
            section.push(crate::template_config::parse_kdl_placement_loop(item, &scope)?);
        }
    }
    validate(kind, &strip)?;
    Ok(strip)
}

/// A strip's loops list entities and nothing else: they nest no loops and
/// declare no fields, templates or presentation, which a strip would ignore.
pub(crate) fn validate(kind: &str, strip: &StripDefinition) -> Result<(), TemplateConfigError> {
    let scope = BTreeSet::from([kind.to_owned()]);
    for (_, loops) in strip.sections() {
        crate::template_config::validate_placement_loops(loops, &scope)?;
        for item in loops {
            let ignored = !item.loops.is_empty()
                || !item.fields.is_empty()
                || item.apply_template.is_some()
                || item.layout.is_some()
                || item.default_collapsed
                || item.tier.is_some();
            if ignored {
                return Err(invalid(format!(
                    "workspace {kind} strip loop {} only selects and orders: it takes no for, field, apply-template, layout, default-collapsed or tier",
                    item.binding
                )));
            }
        }
    }
    Ok(())
}

fn invalid(message: String) -> TemplateConfigError {
    TemplateConfigError::Validation(message)
}
