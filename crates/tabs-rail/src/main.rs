mod render;
pub mod template_config;

#[cfg(not(target_family = "wasm"))]
fn main() {}

fn should_sync_graphics(controller_available: bool) -> bool {
    controller_available
}

#[cfg(target_family = "wasm")]
use std::cmp::{max, min};
#[cfg(target_family = "wasm")]
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(target_family = "wasm")]
use render::{hit_at, status_icon_is_renderable, HitAction, HitRegion, LocalTab, VisibleCard};
#[cfg(target_family = "wasm")]
use tabs_shared::{
    ControllerViewModel, GroupPath, RailViewMode, RendererHello, StatusIcon, MSG_RENDERER_HELLO,
    MSG_REQUEST_STATE, MSG_TOGGLE_PIN, MSG_VIEW_MODEL,
};
#[cfg(target_family = "wasm")]
use zellij_tile::prelude::*;

#[cfg(target_family = "wasm")]
const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";
#[cfg(target_family = "wasm")]
const CONFIG_CONFIG_PLUGIN_URL: &str = "config_plugin_url";
#[cfg(target_family = "wasm")]
const CONFIG_RAIL_SCOPE: &str = "rail_scope";

#[cfg(target_family = "wasm")]
#[derive(Default)]
pub struct PluginState {
    tabs: Vec<TabInfo>,
    local_tabs: Vec<LocalTab>,
    hit_regions: Vec<HitRegion>,
    controller_model: Option<ControllerViewModel>,
    controller_plugin_url: String,
    config_plugin_url: String,
    own_plugin_id: Option<u32>,
    own_client_id: Option<u16>,
    permissions_granted: bool,
    mode_info: Option<ModeInfo>,
    icon_asset_ids: HashMap<StatusIcon, u32>,
    next_icon_asset_id: u32,
    metadata_scroll_offset: usize,
    collapsed_groups: BTreeSet<GroupPath>,
}

#[cfg(target_family = "wasm")]
register_plugin!(PluginState);

#[cfg(target_family = "wasm")]
impl ZellijPlugin for PluginState {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_plugin_id = Some(ids.plugin_id);
        self.own_client_id = Some(ids.client_id);
        self.controller_plugin_url = configuration
            .get(CONFIG_CONTROLLER_PLUGIN_URL)
            .cloned()
            .unwrap_or_else(|| "tabs-controller".to_owned());
        self.config_plugin_url = configuration
            .get(CONFIG_CONFIG_PLUGIN_URL)
            .cloned()
            .unwrap_or_else(|| "tabs-rail-config".to_owned());

        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::MessageAndLaunchOtherPlugins,
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::OpenFiles,
        ]);
        // Keep the rail selectable until permissions are granted, otherwise
        // Zellij can show a permission prompt in a pane the user cannot focus.
        set_selectable(true);
        subscribe(&[
            EventType::TabUpdate,
            EventType::ModeUpdate,
            EventType::Mouse,
            EventType::PermissionRequestResult,
        ]);
        self.send_renderer_hello();
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    set_selectable(false);
                }
                self.send_renderer_hello();
                true
            }
            Event::ModeUpdate(mode_info) => {
                self.mode_info = Some(mode_info);
                true
            }
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                self.local_tabs = self
                    .tabs
                    .iter()
                    .map(|tab| LocalTab {
                        tab_id: tab.tab_id as u64,
                        position: tab.position,
                        name: if tab.name.is_empty() {
                            format!("Tab {}", tab.position + 1)
                        } else {
                            tab.name.clone()
                        },
                        active: tab.active,
                    })
                    .collect();
                true
            }
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            _ => false,
        }
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        if message.name == MSG_VIEW_MODEL {
            if let Some(payload) = message.payload {
                match serde_json::from_str::<ControllerViewModel>(&payload) {
                    Ok(model) => {
                        self.controller_model = Some(model);
                        return true;
                    }
                    Err(error) => {
                        eprintln!("tabs-rail: failed to parse view model: {error}");
                    }
                }
            }
        }
        false
    }

    fn render(&mut self, rows: usize, cols: usize) {
        let controller_available = self.controller_model.is_some();
        let collapsed_groups = self.collapsed_groups.iter().cloned().collect::<Vec<_>>();
        let rendered = render::render_lines_with_options(
            self.controller_model.as_ref(),
            &self.local_tabs,
            rows,
            cols,
            controller_available,
            self.mode_info
                .as_ref()
                .map(|mode_info| mode_info.style.colors.into()),
            self.metadata_scroll_offset,
            terminal_pixel_cell_size(),
            &collapsed_groups,
            None,
        );
        self.metadata_scroll_offset = rendered.metadata_scroll_offset;
        if should_sync_graphics(controller_available) {
            self.sync_graphics(&rendered.visible_cards);
        }
        self.hit_regions = rendered.hit_regions;
        print!("{}", rendered.lines.join("\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_sync_waits_for_controller_model() {
        assert!(!should_sync_graphics(false));
        assert!(should_sync_graphics(true));
    }
}

#[cfg(target_family = "wasm")]
impl PluginState {
    fn send_renderer_hello(&self) {
        let (Some(plugin_id), Some(client_id)) = (self.own_plugin_id, self.own_client_id) else {
            return;
        };
        let hello = RendererHello {
            plugin_id,
            client_id,
        };
        let Ok(payload) = serde_json::to_string(&hello) else {
            return;
        };
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_RENDERER_HELLO)
                .with_plugin_url(self.controller_plugin_url.clone())
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_REQUEST_STATE)
                .with_plugin_url(self.controller_plugin_url.clone())
                .with_destination_client_id(client_id),
        );
    }

    fn handle_mouse(&mut self, mouse: Mouse) -> bool {
        match mouse {
            Mouse::LeftClick(row, col) if row >= 0 => {
                let Some(hit) = hit_at(&self.hit_regions, row as usize, col as usize) else {
                    return false;
                };
                match hit.action {
                    HitAction::SwitchTab => {
                        switch_tab_to((hit.tab_position + 1) as u32);
                        false
                    }
                    HitAction::TogglePin => {
                        self.toggle_pin(hit.tab_id);
                        false
                    }
                    HitAction::ToggleGroup => {
                        if let Some(group_path) = hit.group_path {
                            self.toggle_group(group_path);
                            true
                        } else {
                            false
                        }
                    }
                    HitAction::OpenConfig => {
                        self.open_config_pane();
                        false
                    }
                    HitAction::ScrollMetadataUp => {
                        self.metadata_scroll_offset = self.metadata_scroll_offset.saturating_sub(1);
                        true
                    }
                    HitAction::ScrollMetadataDown => {
                        self.metadata_scroll_offset = self.metadata_scroll_offset.saturating_add(1);
                        true
                    }
                }
            }
            Mouse::ScrollUp(_) => {
                if self.is_metadata_view() {
                    self.metadata_scroll_offset = self.metadata_scroll_offset.saturating_sub(1);
                    return true;
                } else if let Some(active_tab_idx) = self.active_tab_idx() {
                    let prev = max(active_tab_idx.saturating_sub(1), 1);
                    switch_tab_to(prev as u32);
                }
                false
            }
            Mouse::ScrollDown(_) => {
                if self.is_metadata_view() {
                    self.metadata_scroll_offset = self.metadata_scroll_offset.saturating_add(1);
                    return true;
                } else if let Some(active_tab_idx) = self.active_tab_idx() {
                    let next = min(active_tab_idx + 1, self.local_tabs.len());
                    switch_tab_to(next as u32);
                }
                false
            }
            _ => false,
        }
    }

    fn is_metadata_view(&self) -> bool {
        self.controller_model
            .as_ref()
            .map(|model| model.config.view == RailViewMode::Metadata)
            .unwrap_or(false)
    }

    fn active_tab_idx(&self) -> Option<usize> {
        self.local_tabs
            .iter()
            .position(|tab| tab.active)
            .map(|idx| idx + 1)
    }

    fn toggle_pin(&self, tab_id: u64) {
        let Some(client_id) = self.own_client_id else {
            return;
        };
        if self.controller_model.is_none() {
            return;
        }
        let mut args = BTreeMap::new();
        args.insert("tab_id".to_owned(), tab_id.to_string());
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_TOGGLE_PIN)
                .with_plugin_url(self.controller_plugin_url.clone())
                .with_destination_client_id(client_id)
                .with_args(args),
        );
    }

    fn toggle_group(&mut self, group_path: GroupPath) {
        if !self.collapsed_groups.insert(group_path.clone()) {
            self.collapsed_groups.remove(&group_path);
        }
    }

    fn open_config_pane(&self) {
        let Some(client_id) = self.own_client_id else {
            return;
        };
        let mut configuration = BTreeMap::new();
        configuration.insert(
            CONFIG_CONTROLLER_PLUGIN_URL.to_owned(),
            self.controller_plugin_url.clone(),
        );
        configuration.insert(CONFIG_RAIL_SCOPE.to_owned(), self.config_editor_scope());
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_REQUEST_STATE)
                .with_plugin_url(self.config_plugin_url.clone())
                .with_destination_client_id(client_id)
                .with_plugin_config(configuration)
                .new_plugin_instance_should_float(true)
                .new_plugin_instance_should_be_focused(),
        );
    }

    fn config_editor_scope(&self) -> String {
        if let Some(tab_id) = self
            .local_tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.tab_id)
        {
            return format!("tab:{tab_id}");
        }
        if let Some(plugin_id) = self.own_plugin_id {
            return format!("plugin:{plugin_id}");
        }
        "unknown".to_owned()
    }

    fn sync_graphics(&mut self, visible_cards: &[VisibleCard]) {
        let mut ops = vec![PluginGraphicsOp::ClearPlacements];
        for (index, card) in visible_cards.iter().enumerate() {
            let (Some(rect), Some(icon)) = (card.status_icon_rect, card.status_icon.as_ref())
            else {
                continue;
            };
            if !status_icon_is_renderable(icon) {
                continue;
            }
            let asset_id = self.asset_id_for_icon(icon, &mut ops);
            ops.push(PluginGraphicsOp::PlaceImage {
                placement_id: 10_000 + index as u32,
                asset_id,
                destination: PluginCellRect {
                    x: rect.x as u32,
                    y: rect.y as u32,
                    columns: None,
                    rows: Some(rect.rows as u32),
                },
                source: None,
                z_index: 1,
            });
        }
        apply_graphics_update(ops);
    }

    fn asset_id_for_icon(&mut self, icon: &StatusIcon, ops: &mut Vec<PluginGraphicsOp>) -> u32 {
        if let Some(asset_id) = self.icon_asset_ids.get(icon).copied() {
            return asset_id;
        }
        if self.next_icon_asset_id == 0 {
            self.next_icon_asset_id = 1;
        }
        let asset_id = self.next_icon_asset_id;
        self.next_icon_asset_id = self.next_icon_asset_id.saturating_add(1);
        self.icon_asset_ids.insert(icon.clone(), asset_id);
        if let StatusIcon::PngFile(path) = icon {
            ops.push(PluginGraphicsOp::SetAsset {
                asset_id,
                source: PluginImageSource::PngFile(path.clone()),
            });
        }
        asset_id
    }
}
