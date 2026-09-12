//! Host-owned observations. IDs are scoped to one presentation client.
use crate::PaneTarget;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneObservation {
    pub workspace_id: u64,
    pub pane_id: PaneTarget,
    pub is_selectable: bool,
    pub is_focused: bool,
    pub ordinal: i64,
}
