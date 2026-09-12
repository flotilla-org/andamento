//! Zellij observations translated into core-owned values.
use andamento_core::host::PaneObservation;
pub use andamento_core::state::*;
use andamento_shared::PaneTarget;
use zellij_tile::prelude::{PaneManifest, TabInfo};

pub trait ZellijObservations {
    fn update_tabs_from_zellij(&mut self, tabs: Vec<TabInfo>) -> bool;
    fn update_panes_from_manifest(&mut self, manifest: PaneManifest) -> bool;
}

impl ZellijObservations for ControllerState {
    fn update_tabs_from_zellij(&mut self, tabs: Vec<TabInfo>) -> bool {
        self.observe_workspaces(
            tabs.into_iter()
                .map(|tab| ControllerTab {
                    tab_id: tab.tab_id as u64,
                    position: tab.position,
                    name: tab.name,
                    active: tab.active,
                })
                .collect(),
        )
    }

    fn update_panes_from_manifest(&mut self, manifest: PaneManifest) -> bool {
        let tabs = self.workspaces();
        let observations = manifest
            .panes
            .into_iter()
            .flat_map(|(position, panes)| {
                let workspace_id = tabs
                    .iter()
                    .find(|t| t.position == position)
                    .map(|t| t.tab_id);
                panes
                    .into_iter()
                    .enumerate()
                    .filter_map(move |(ordinal, pane)| {
                        Some(PaneObservation {
                            workspace_id: workspace_id?,
                            pane_id: if pane.is_plugin {
                                PaneTarget::Plugin(pane.id)
                            } else {
                                PaneTarget::Terminal(pane.id)
                            },
                            is_selectable: pane.is_selectable,
                            is_focused: pane.is_focused,
                            ordinal: ordinal as i64,
                        })
                    })
            })
            .collect();
        self.observe_panes(observations)
    }
}
