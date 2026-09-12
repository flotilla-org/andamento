//! Orchestrator-independent state and seams for an Andamento sidebar.

mod inline_layout;
pub mod render;

use andamento_shared::ControllerViewModel;
use serde::{Deserialize, Serialize};

/// The host operations initiated by the sidebar.
///
/// Positions are one-based, matching terminal multiplexer user interfaces.
pub trait HostControl {
    type Error;

    fn switch_tab(&mut self, position: usize) -> Result<(), Self::Error>;
    fn open_tab(&mut self, name: &str) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub id: u64,
    pub position: usize,
    pub name: String,
    pub active: bool,
}

/// A transport-neutral fact. Transports deliver the same JSON representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Facts {
    ViewModel { model: ControllerViewModel },
    Tabs { tabs: Vec<Tab> },
}

#[derive(Debug, Default)]
pub struct SidebarCore {
    model: Option<ControllerViewModel>,
    tabs: Vec<Tab>,
}

impl SidebarCore {
    pub fn apply(&mut self, facts: Facts) -> bool {
        match facts {
            Facts::ViewModel { model } => {
                self.model = Some(model);
                true
            }
            Facts::Tabs { tabs } if tabs != self.tabs => {
                self.tabs = tabs;
                true
            }
            Facts::Tabs { .. } => false,
        }
    }

    /// Accept the controller payload used by both Zellij pipes and pm-connect.
    pub fn apply_view_model_json(&mut self, payload: &str) -> Result<(), serde_json::Error> {
        self.apply(Facts::ViewModel {
            model: serde_json::from_str(payload)?,
        });
        Ok(())
    }

    pub fn model(&self) -> Option<&ControllerViewModel> {
        self.model.as_ref()
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn activate<H: HostControl>(&self, tab_id: u64, host: &mut H) -> Result<(), H::Error> {
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) {
            self.activate_position(tab.position, host)?;
        }
        Ok(())
    }

    pub fn activate_position<H: HostControl>(
        &self,
        zero_based_position: usize,
        host: &mut H,
    ) -> Result<(), H::Error> {
        host.switch_tab(zero_based_position + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Host(Vec<usize>);
    impl HostControl for Host {
        type Error = ();
        fn switch_tab(&mut self, position: usize) -> Result<(), Self::Error> {
            self.0.push(position);
            Ok(())
        }
        fn open_tab(&mut self, _: &str) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn facts_drive_host_without_knowing_the_adapter() {
        let mut core = SidebarCore::default();
        core.apply(Facts::Tabs {
            tabs: vec![Tab {
                id: 9,
                position: 2,
                name: "work".into(),
                active: false,
            }],
        });
        let mut host = Host::default();
        core.activate(9, &mut host).unwrap();
        assert_eq!(host.0, vec![3]);
    }
}
