//! The Dashboard over its template (Wheelhouse ADR 0013): the dashboard
//! record's keys are per-key overrides of what the template declares. Its
//! collapse, sibling order and placement-variable keys embed the template's
//! region and loop names, display variables name declared variables, and the
//! sidebar arrangement names sections. When the template changes, keys that
//! no longer resolve are kept and flagged, never dropped. The record names
//! the template version its keys were made against.
//!
//! Dashboard items can also pin a View: a local ref with a `.view` fact,
//! `"<workspace-id>/<slot-key>"`. When that View goes away (its workspace is
//! forgotten, or the slot is removed) the pin is kept and flagged; the host
//! offers to remove it (`remove_local`).
use crate::{slots, PlacementKey, PlacementLoopKey, WorkspaceId};

/// A ref's fact naming the View it pins: `"<workspace-id>/<slot-key>"`.
pub const VIEW: &str = ".view";

/// The kind of a dashboard key that no longer resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyKind {
    /// A recorded display variable the template no longer declares.
    Display,
    /// A collapsed row whose key names a loop the template no longer has.
    Collapse,
    /// A sibling order for a region or loop the template no longer has.
    Order,
    /// A placement variable on a row whose loop the template no longer has.
    Variable,
    /// A sidebar arrangement key (tabbed or closed) that no longer resolves:
    /// the sidebar arrangement's own unresolved keys.
    Section,
    /// A local ref pinning a View that has gone; the key is the ref's ID.
    Pin,
}

/// The Dashboard's relation to its template, as the host reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DashboardOverlay {
    /// The configured template's version.
    pub template: String,
    /// The template version the last imported dashboard record was made
    /// against, if it recorded one.
    pub recorded: Option<String>,
    /// Keys that no longer resolve, by kind, in key order.
    pub unresolved: Vec<(KeyKind, String)>,
}

/// A template's version: a digest of its configuration text. It changes
/// whenever the text does.
pub fn template_version(config: &str) -> String {
    // FNV-1a, 64 bits: stable across builds and platforms.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in config.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A placement key as text: `loop=provider:kind:id`, joined by `/`.
pub fn placement_key_text(key: &PlacementKey) -> String {
    key.0
        .iter()
        .map(|segment| {
            format!(
                "{}={}:{}:{}",
                segment.loop_name, segment.entity.provider, segment.entity.kind, segment.entity.id
            )
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A sibling order's loop key as text: `region` then the parent key and the
/// binding, joined by `/`.
pub fn loop_key_text(key: &PlacementLoopKey) -> String {
    let mut parts = vec![key.region.clone()];
    if !key.parent.0.is_empty() {
        parts.push(placement_key_text(&key.parent));
    }
    parts.push(key.binding.clone());
    parts.join("/")
}

/// Read a pinned View, `"<workspace-id>/<slot-key>"`.
pub fn parse_view(text: &str) -> Result<(WorkspaceId, String), String> {
    let invalid = || format!("{VIEW} is \"<workspace-id>/<slot-key>\", not {text:?}");
    let (workspace, key) = text.split_once('/').ok_or_else(invalid)?;
    let workspace: WorkspaceId = workspace.parse().map_err(|_| invalid())?;
    let id = key.strip_prefix(slots::USER_PREFIX).unwrap_or(key);
    slots::check_key(id).map_err(|_| invalid())?;
    Ok((workspace, key.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_versions_follow_the_text() {
        assert_eq!(template_version("a"), template_version("a"));
        assert_ne!(template_version("a"), template_version("b"));
        assert_eq!(template_version("").len(), 16);
    }

    #[test]
    fn pinned_views_name_a_workspace_and_a_slot() {
        let id = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7c";
        let (workspace, key) = parse_view(&format!("{id}/u:3")).unwrap();
        assert_eq!(workspace.to_string(), id);
        assert_eq!(key, "u:3");
        assert_eq!(parse_view(&format!("{id}/primary")).unwrap().1, "primary");
        for bad in [
            "",
            "x/primary",
            &format!("{id}"),
            &format!("{id}/A"),
            &format!("{id}/u:"),
        ] {
            assert!(parse_view(bad).is_err(), "{bad}");
        }
    }
}
