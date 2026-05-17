#[cfg(not(target_family = "wasm"))]
fn main() {}

#[cfg(target_family = "wasm")]
use std::collections::BTreeMap;

use tabs_shared::{RailConfig, RailSizingPreset, RailStructure};
use unicode_width::UnicodeWidthStr;

#[cfg(target_family = "wasm")]
use tabs_shared::{
    ControllerViewModel, RendererHello, MSG_CONFIG_EDITOR_HELLO, MSG_REQUEST_STATE,
    MSG_SET_RAIL_CONFIG, MSG_VIEW_MODEL,
};
#[cfg(target_family = "wasm")]
use zellij_tile::prelude::*;

#[cfg(target_family = "wasm")]
const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigAction {
    SetStructure(RailStructure),
    SetSizing(RailSizingPreset),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HitRegion {
    row: usize,
    col_start: usize,
    col_end: usize,
    action: ConfigAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedConfig {
    lines: Vec<String>,
    hit_regions: Vec<HitRegion>,
}

#[cfg(target_family = "wasm")]
#[derive(Default)]
struct PluginState {
    controller_plugin_url: String,
    own_plugin_id: Option<u32>,
    own_client_id: Option<u16>,
    model: Option<ControllerViewModel>,
    pending_config: Option<RailConfig>,
    hit_regions: Vec<HitRegion>,
    permissions_granted: bool,
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

        request_permission(&[
            PermissionType::ChangeApplicationState,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);
        subscribe(&[EventType::Mouse, EventType::PermissionRequestResult]);
        self.send_hello();
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        if message.name == MSG_REQUEST_STATE {
            if self.permissions_granted {
                show_self(true);
            }
            self.send_hello();
            return true;
        }
        if message.name == MSG_VIEW_MODEL {
            if let Some(payload) = message.payload {
                match serde_json::from_str::<ControllerViewModel>(&payload) {
                    Ok(model) => {
                        self.model = Some(model);
                        self.pending_config = None;
                        return true;
                    }
                    Err(error) => {
                        eprintln!("tabs-rail-config: failed to parse view model: {error}");
                    }
                }
            }
        }
        false
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Mouse(Mouse::LeftClick(row, col)) if row >= 0 => {
                self.handle_click(row as usize, col as usize)
            }
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                let should_resync = permission_result_should_resync(self.permissions_granted);
                if should_resync {
                    self.send_hello();
                }
                should_resync
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        let config = self
            .pending_config
            .or_else(|| self.model.as_ref().map(|model| model.config))
            .unwrap_or_default();
        let rendered = render_config(config, rows, cols);
        self.hit_regions = rendered.hit_regions;
        print!("{}", rendered.lines.join("\n"));
    }
}

#[cfg(target_family = "wasm")]
impl PluginState {
    fn send_hello(&self) {
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
            MessageToPlugin::new(MSG_CONFIG_EDITOR_HELLO)
                .with_plugin_url(self.controller_plugin_url.clone())
                .with_payload(payload),
        );
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_REQUEST_STATE)
                .with_plugin_url(self.controller_plugin_url.clone()),
        );
    }

    fn handle_click(&mut self, row: usize, col: usize) -> bool {
        let Some(hit) = self
            .hit_regions
            .iter()
            .find(|hit| hit.row == row && col >= hit.col_start && col <= hit.col_end)
        else {
            return false;
        };
        let mut config = self
            .pending_config
            .or_else(|| self.model.as_ref().map(|model| model.config))
            .unwrap_or_default();
        config = apply_config_action(config, hit.action);
        self.pending_config = Some(config);
        let Ok(payload) = serde_json::to_string(&config) else {
            return true;
        };
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_SET_RAIL_CONFIG)
                .with_plugin_url(self.controller_plugin_url.clone())
                .with_payload(payload),
        );
        true
    }
}

fn apply_config_action(mut config: RailConfig, action: ConfigAction) -> RailConfig {
    match action {
        ConfigAction::SetStructure(structure) => config.structure = structure,
        ConfigAction::SetSizing(sizing) => config.sizing = sizing,
    }
    config
}

fn permission_result_should_resync(granted: bool) -> bool {
    granted
}

fn render_config(config: RailConfig, rows: usize, cols: usize) -> RenderedConfig {
    if rows == 0 || cols == 0 {
        return RenderedConfig {
            lines: vec![],
            hit_regions: vec![],
        };
    }
    let mut lines = vec![];
    let mut hit_regions = vec![];
    push_plain(&mut lines, cols, "tab rail config");
    push_plain(&mut lines, cols, "");
    push_plain(&mut lines, cols, "structure");
    push_option(
        &mut lines,
        &mut hit_regions,
        cols,
        "joined cells",
        config.structure == RailStructure::JoinedCells,
        ConfigAction::SetStructure(RailStructure::JoinedCells),
    );
    push_option(
        &mut lines,
        &mut hit_regions,
        cols,
        "split around active",
        config.structure == RailStructure::SplitAroundActive,
        ConfigAction::SetStructure(RailStructure::SplitAroundActive),
    );
    push_option(
        &mut lines,
        &mut hit_regions,
        cols,
        "box per tab",
        config.structure == RailStructure::BoxPerTab,
        ConfigAction::SetStructure(RailStructure::BoxPerTab),
    );
    push_plain(&mut lines, cols, "");
    push_plain(&mut lines, cols, "sizing");
    push_option(
        &mut lines,
        &mut hit_regions,
        cols,
        "compact",
        config.sizing == RailSizingPreset::Compact,
        ConfigAction::SetSizing(RailSizingPreset::Compact),
    );
    push_option(
        &mut lines,
        &mut hit_regions,
        cols,
        "large",
        config.sizing == RailSizingPreset::Large,
        ConfigAction::SetSizing(RailSizingPreset::Large),
    );
    push_option(
        &mut lines,
        &mut hit_regions,
        cols,
        "active large",
        config.sizing == RailSizingPreset::ActiveLarge,
        ConfigAction::SetSizing(RailSizingPreset::ActiveLarge),
    );
    push_option(
        &mut lines,
        &mut hit_regions,
        cols,
        "pinned large",
        config.sizing == RailSizingPreset::PinnedLarge,
        ConfigAction::SetSizing(RailSizingPreset::PinnedLarge),
    );
    push_plain(&mut lines, cols, "");
    push_plain(&mut lines, cols, "click to apply");

    lines.truncate(rows);
    hit_regions.retain(|hit| hit.row < rows);
    while lines.len() < rows {
        lines.push(" ".repeat(cols));
    }

    RenderedConfig { lines, hit_regions }
}

fn push_plain(lines: &mut Vec<String>, cols: usize, text: &str) {
    lines.push(pad_to_width(&truncate_to_width(text, cols), cols));
}

fn push_option(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    cols: usize,
    label: &str,
    selected: bool,
    action: ConfigAction,
) {
    let row = lines.len();
    let marker = if selected { ">" } else { " " };
    let text = format!("{marker} {label}");
    lines.push(pad_to_width(&truncate_to_width(&text, cols), cols));
    hit_regions.push(HitRegion {
        row,
        col_start: 0,
        col_end: cols.saturating_sub(1),
        action,
    });
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    text.chars().take(max_width).collect()
}

fn pad_to_width(text: &str, width: usize) -> String {
    let text_width = text.width();
    if text_width >= width {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + (width - text_width));
    out.push_str(text);
    out.push_str(&" ".repeat(width - text_width));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_selected_config() {
        let rendered = render_config(
            RailConfig {
                structure: RailStructure::BoxPerTab,
                sizing: RailSizingPreset::Compact,
            },
            16,
            30,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "> box per tab"));
        assert!(rendered.lines.iter().any(|line| line.trim() == "> compact"));
    }

    #[test]
    fn maps_click_rows_to_config_actions() {
        let rendered = render_config(RailConfig::default(), 16, 30);

        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::SetStructure(RailStructure::BoxPerTab)));
        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::SetSizing(RailSizingPreset::PinnedLarge)));
    }

    #[test]
    fn click_action_updates_config_locally() {
        let updated = apply_config_action(
            RailConfig::default(),
            ConfigAction::SetStructure(RailStructure::BoxPerTab),
        );

        assert_eq!(updated.structure, RailStructure::BoxPerTab);
        assert_eq!(updated.sizing, RailSizingPreset::ActiveLarge);
    }

    #[test]
    fn granted_permission_should_resync_with_controller() {
        assert!(permission_result_should_resync(true));
        assert!(!permission_result_should_resync(false));
    }
}
