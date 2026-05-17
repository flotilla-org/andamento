mod render;

#[cfg(not(target_family = "wasm"))]
fn main() {}

fn should_sync_graphics(controller_available: bool) -> bool {
    controller_available
}

#[cfg(any(test, target_family = "wasm"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphicsSignatureEntry {
    placement_id: u32,
    rect: VisibleIconRect,
    icon: StatusIcon,
}

#[cfg(any(test, target_family = "wasm"))]
fn visible_graphics_signature(visible_cards: &[VisibleCard]) -> Vec<GraphicsSignatureEntry> {
    visible_cards
        .iter()
        .enumerate()
        .filter_map(|(index, card)| {
            let (Some(rect), Some(icon)) = (card.status_icon_rect, card.status_icon.as_ref())
            else {
                return None;
            };
            if !status_icon_is_renderable(icon) {
                return None;
            }
            Some(GraphicsSignatureEntry {
                placement_id: 10_000 + index as u32,
                rect,
                icon: icon.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
fn graphics_signature_needs_sync(
    previous: Option<&[GraphicsSignatureEntry]>,
    visible_cards: &[VisibleCard],
) -> bool {
    let current = visible_graphics_signature(visible_cards);
    previous
        .map(|previous| previous != current.as_slice())
        .unwrap_or(true)
}

#[cfg(any(test, target_family = "wasm"))]
fn local_tabs_from_zellij(tabs: &[TabInfo]) -> Vec<LocalTab> {
    tabs.iter()
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
        .collect()
}

#[cfg(test)]
fn local_tabs_need_render(current: &[LocalTab], zellij_tabs: &[TabInfo]) -> bool {
    current != local_tabs_from_zellij(zellij_tabs).as_slice()
}

#[cfg(any(test, target_family = "wasm"))]
fn mode_info_needs_render(current: Option<&ModeInfo>, next: &ModeInfo) -> bool {
    current != Some(next)
}

#[cfg(target_family = "wasm")]
use std::cmp::{max, min};
#[cfg(target_family = "wasm")]
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(target_family = "wasm")]
use render::{hit_at, HitAction, HitRegion};
#[cfg(any(test, target_family = "wasm"))]
use render::{status_icon_is_renderable, LocalTab, VisibleCard, VisibleIconRect};
#[cfg(any(test, target_family = "wasm"))]
use tabs_shared::StatusIcon;
#[cfg(target_family = "wasm")]
use tabs_shared::{
    ControllerViewModel, GroupPath, RailViewMode, RendererHello, MSG_RENDERER_HELLO,
    MSG_REQUEST_STATE, MSG_TOGGLE_PIN, MSG_VIEW_MODEL,
};
#[cfg(target_family = "wasm")]
use zellij_tile::prelude::*;
#[cfg(test)]
use zellij_tile::prelude::{ModeInfo, TabInfo};

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
    last_graphics_signature: Option<Vec<GraphicsSignatureEntry>>,
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
                if !mode_info_needs_render(self.mode_info.as_ref(), &mode_info) {
                    false
                } else {
                    self.mode_info = Some(mode_info);
                    true
                }
            }
            Event::TabUpdate(tabs) => {
                let local_tabs = local_tabs_from_zellij(&tabs);
                if self.local_tabs == local_tabs {
                    return false;
                }
                self.tabs = tabs;
                self.local_tabs = local_tabs;
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
    use render::{VisibleCard, VisibleIconRect};

    fn tab_info(tab_id: usize, position: usize, name: &str, active: bool) -> TabInfo {
        TabInfo {
            position,
            name: name.to_owned(),
            active,
            panes_to_hide: 0,
            is_fullscreen_active: false,
            is_sync_panes_active: false,
            are_floating_panes_visible: false,
            other_focused_clients: vec![],
            active_swap_layout_name: None,
            is_swap_layout_dirty: false,
            viewport_rows: 0,
            viewport_columns: 0,
            display_area_rows: 0,
            display_area_columns: 0,
            selectable_tiled_panes_count: 0,
            selectable_floating_panes_count: 0,
            tab_id,
            has_bell_notification: false,
            is_flashing_bell: false,
        }
    }

    #[test]
    fn graphics_sync_waits_for_controller_model() {
        assert!(!should_sync_graphics(false));
        assert!(should_sync_graphics(true));
    }

    #[test]
    fn unchanged_graphics_signature_does_not_need_sync() {
        let visible_cards = vec![VisibleCard {
            tab_id: 1,
            tab_position: 0,
            row_start: 0,
            status_row: Some(1),
            status_icon_rect: Some(VisibleIconRect {
                x: 2,
                y: 1,
                columns: 1,
                rows: 1,
            }),
            status_priority: None,
            status_icon: Some(StatusIcon::PngFile("/tmp/icon.png".into())),
        }];
        let signature = visible_graphics_signature(&visible_cards);

        assert!(!graphics_signature_needs_sync(
            Some(signature.as_slice()),
            &visible_cards
        ));
    }

    #[test]
    fn unchanged_local_tab_projection_does_not_need_render() {
        let current = vec![LocalTab {
            tab_id: 1,
            position: 0,
            name: "work".to_owned(),
            active: true,
        }];
        let zellij_tabs = vec![tab_info(1, 0, "work", true)];

        assert!(!local_tabs_need_render(&current, &zellij_tabs));
    }

    #[test]
    fn unchanged_mode_info_does_not_need_render() {
        let mode_info = ModeInfo::default();

        assert!(!mode_info_needs_render(Some(&mode_info), &mode_info));
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
        let signature = visible_graphics_signature(visible_cards);
        if self
            .last_graphics_signature
            .as_deref()
            .map(|previous| previous == signature.as_slice())
            .unwrap_or(false)
        {
            return;
        }
        let mut ops = vec![PluginGraphicsOp::ClearPlacements];
        for entry in &signature {
            let asset_id = self.asset_id_for_icon(&entry.icon, &mut ops);
            ops.push(PluginGraphicsOp::PlaceImage {
                placement_id: entry.placement_id,
                asset_id,
                destination: PluginCellRect {
                    x: entry.rect.x as u32,
                    y: entry.rect.y as u32,
                    columns: None,
                    rows: Some(entry.rect.rows as u32),
                },
                source: None,
                z_index: 1,
            });
        }
        apply_graphics_update(ops);
        self.last_graphics_signature = Some(signature);
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
