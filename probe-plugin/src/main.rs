use std::collections::BTreeMap;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    tabs: Vec<TabInfo>,
    permission_status: Option<PermissionStatus>,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        request_permission(&[PermissionType::ReadApplicationState]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PermissionRequestResult,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permission_status = Some(status);
                true
            },
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                true
            },
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        let mut lines = vec![format!("probe plugin loaded: {rows}x{cols}")];
        match self.permission_status {
            None => lines.push("permissions: waiting".to_owned()),
            Some(PermissionStatus::Granted) => lines.push("permissions: granted".to_owned()),
            Some(PermissionStatus::Denied) => lines.push("permissions: denied".to_owned()),
        }
        lines.push(format!("tabs: {}", self.tabs.len()));
        for tab in &self.tabs {
            let marker = if tab.active { ">" } else { " " };
            let name = if tab.name.is_empty() {
                format!("Tab {}", tab.position + 1)
            } else {
                tab.name.clone()
            };
            lines.push(format!("{} {}", marker, name));
        }
        let output = lines.join("\n");
        println!("{}", output);
    }
}
