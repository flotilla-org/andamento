//! Host-owned observations. IDs are scoped to one presentation client.
use crate::{PaneTarget, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneObservation {
    pub workspace_id: WorkspaceId,
    pub pane_id: PaneTarget,
    pub is_selectable: bool,
    pub is_focused: bool,
    pub ordinal: i64,
}

/// The Workspace ID the in-repo hosts (Zellij, tmux, and tests) give tab or
/// window number `tab`: the number, big-endian, in the last eight bytes. It is
/// a host's choice like any other; Andamento never interprets it.
pub const fn tab_workspace_id(tab: u64) -> WorkspaceId {
    WorkspaceId::from_bytes((tab as u128).to_be_bytes())
}

/// The tab number in a Workspace ID [`tab_workspace_id`] made, for a host's
/// own labels and settings.
pub fn workspace_tab(id: WorkspaceId) -> Option<u64> {
    u64::try_from(u128::from_be_bytes(id.to_bytes())).ok()
}

/// A Workspace ID as a host shows it: its tab number, or else the UUID.
pub fn tab_label(id: WorkspaceId) -> String {
    workspace_tab(id).map_or_else(|| id.to_string(), |tab| tab.to_string())
}

/// [`tab_label`]'s inverse.
pub fn parse_tab_label(text: &str) -> Option<WorkspaceId> {
    match text.parse::<u64>() {
        Ok(tab) => Some(tab_workspace_id(tab)),
        Err(_) => text.parse().ok(),
    }
}
