mod inline_layout;
pub mod render;

fn should_sync_graphics(controller_available: bool) -> bool {
    controller_available
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphicsSignatureEntry {
    placement_id: u32,
    rect: VisibleIconRect,
    icon: StatusIcon,
}

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

fn mode_info_needs_render(current: Option<&ModeInfo>, next: &ModeInfo) -> bool {
    current != Some(next)
}

fn tab_index_after_scroll(active_tab_idx: usize, tab_count: usize, delta: isize) -> usize {
    if delta < 0 {
        active_tab_idx.saturating_sub(delta.unsigned_abs()).max(1)
    } else {
        active_tab_idx
            .saturating_add(delta.unsigned_abs())
            .min(tab_count)
    }
}

fn should_forward_scroll_to_controller(rail_can_scroll: Option<bool>) -> bool {
    !matches!(rail_can_scroll, Some(false))
}

#[cfg(test)]
thread_local! {
    static SCROLL_TAB_SWITCH_TARGET: std::cell::Cell<Option<u32>> =
        const { std::cell::Cell::new(None) };
}

fn switch_tab_from_scroll(target: u32) {
    #[cfg(test)]
    SCROLL_TAB_SWITCH_TARGET.with(|recorded_target| recorded_target.set(Some(target)));
    switch_tab_to(target);
}

#[cfg(not(test))]
fn schedule_scroll_flush() {
    set_timeout(0.0);
}

#[cfg(test)]
fn schedule_scroll_flush() {}

fn rail_size_from_constraint(constraint: PaneDimensionConstraint) -> RailSize {
    match constraint {
        PaneDimensionConstraint::Fixed(size) => RailSize::Fixed(size),
        PaneDimensionConstraint::Percent(percent) => RailSize::Percent(percent),
    }
}

fn pane_dimension_constraint_from_rail_size(size: RailSize) -> PaneDimensionConstraint {
    match size {
        RailSize::Fixed(size) => PaneDimensionConstraint::Fixed(size),
        RailSize::Percent(percent) => PaneDimensionConstraint::Percent(percent),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailPlacement {
    Left,
    Right,
    Top,
    Bottom,
}

impl Default for RailPlacement {
    fn default() -> Self {
        Self::Left
    }
}

impl RailPlacement {
    fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

fn parse_rail_placement(value: &str) -> Option<RailPlacement> {
    match value {
        "left" => Some(RailPlacement::Left),
        "right" => Some(RailPlacement::Right),
        "top" => Some(RailPlacement::Top),
        "bottom" => Some(RailPlacement::Bottom),
        _ => None,
    }
}

fn rail_resize_boundary(placement: RailPlacement) -> Direction {
    match placement {
        RailPlacement::Left => Direction::Right,
        RailPlacement::Right => Direction::Left,
        RailPlacement::Top => Direction::Down,
        RailPlacement::Bottom => Direction::Up,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Left => "left",
        Direction::Right => "right",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

fn rail_size_from_constraints(
    placement: RailPlacement,
    rows: Option<PaneDimensionConstraint>,
    columns: Option<PaneDimensionConstraint>,
) -> Option<RailSize> {
    let constraint = match placement {
        RailPlacement::Left | RailPlacement::Right => columns,
        RailPlacement::Top | RailPlacement::Bottom => rows,
    }?;
    Some(rail_size_from_constraint(constraint))
}

fn rail_size_target_should_apply(
    target: &RailSizeTarget,
    own_client_id: Option<u16>,
    current_size: Option<RailSize>,
    applied_version: u64,
) -> bool {
    if own_client_id != Some(target.client_id) || target.version <= applied_version {
        return false;
    }
    !current_size
        .map(|current| current.within_tolerance(target.size))
        .unwrap_or(false)
}

fn own_plugin_tab_placement(
    pane_manifest: &PaneManifest,
    local_tabs: &[LocalTab],
    plugin_id: u32,
) -> Option<PluginPlacement> {
    let (tab_position, pane) = pane_manifest
        .panes
        .iter()
        .find_map(|(tab_position, panes)| {
            panes
                .iter()
                .find(|pane| pane.is_plugin && pane.id == plugin_id)
                .map(|pane| (*tab_position, pane))
        })?;
    let tab_id = local_tabs
        .iter()
        .find(|tab| tab.position == tab_position)
        .map(|tab| tab.tab_id)?;
    Some(PluginPlacement::Tab {
        tab_id,
        pane_kind: if pane.is_floating {
            PluginPaneKind::Floating
        } else {
            PluginPaneKind::Tiled
        },
    })
}

fn renderer_hello_payload(
    plugin_id: u32,
    client_id: u16,
    placement: PluginPlacement,
) -> Option<String> {
    serde_json::to_string(&PluginRegistrationHello {
        identity: RendererHello {
            plugin_id,
            client_id,
        },
        placement,
    })
    .ok()
}

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use andamento_shared::StatusIcon;
use andamento_shared::{
    ConfigInspectRequest, ControllerViewModel, GroupPath, NodeKey, PluginPaneKind, PluginPlacement,
    PluginRegistrationHello, PluginStatsRecorder, RailSizeObserved, RailUiAction, RailUiState,
    RendererHello, StatsCollectRequest, MSG_CONFIG_INSPECT, MSG_RAIL_SIZE_OBSERVED,
    MSG_RAIL_SIZE_TARGET, MSG_RAIL_UI_ACTION, MSG_RAIL_UI_STATE, MSG_RENDERER_HELLO,
    MSG_REQUEST_RAIL_UI_STATE, MSG_REQUEST_STATE, MSG_STATS_REPORT, MSG_STATS_REQUEST,
    MSG_TOGGLE_PIN, MSG_VIEW_MODEL,
};
use andamento_shared::{RailSize, RailSizeTarget};
use render::{hit_at, HitAction, HitRegion};
use render::{status_icon_is_renderable, LocalTab, VisibleCard, VisibleIconRect};
use zellij_tile::output::print;
use zellij_tile::prelude::*;
#[cfg(test)]
use zellij_tile::prelude::{Direction, ModeInfo, PaneDimensionConstraint, TabInfo};

const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";
const CONFIG_CONFIG_PLUGIN_URL: &str = "config_plugin_url";
const CONFIG_RAIL_PLACEMENT: &str = "rail_placement";

fn build_config_inspect_message(
    controller_plugin_url: &str,
    config_plugin_url: &str,
    client_id: u16,
    origin_tab_id: u64,
    node_key: NodeKey,
) -> Option<MessageToPlugin> {
    let request = ConfigInspectRequest {
        client_id,
        origin_tab_id,
        node_key,
        config_plugin_url: config_plugin_url.to_owned(),
        controller_plugin_url: controller_plugin_url.to_owned(),
    };
    let payload = serde_json::to_string(&request).ok()?;
    Some(
        MessageToPlugin::new(MSG_CONFIG_INSPECT)
            .with_plugin_url(controller_plugin_url.to_owned())
            .with_destination_client_id(client_id)
            .with_payload(payload),
    )
}

fn build_materialize_latent_message(
    controller_plugin_url: &str,
    client_id: u16,
    request: &andamento_shared::MaterializeLatentRequest,
) -> Option<MessageToPlugin> {
    let payload = serde_json::to_string(request).ok()?;
    let message = MessageToPlugin::new(andamento_shared::MSG_MATERIALIZE_LATENT)
        .with_destination_client_id(client_id)
        .with_payload(payload);
    Some(if controller_plugin_url.trim().is_empty() {
        message
    } else {
        message.with_plugin_url(controller_plugin_url.to_owned())
    })
}

fn build_activate_entity_message(
    controller_plugin_url: &str,
    config_plugin_url: &str,
    client_id: u16,
    origin_tab_id: u64,
    entity: &andamento_shared::EntityRef,
) -> Option<MessageToPlugin> {
    let request = andamento_shared::EntityActivationRequest {
        entity: entity.clone(),
        // The controller cannot build this itself — it only ever learns the
        // plugin urls from a request.
        inspect_fallback: ConfigInspectRequest {
            client_id,
            origin_tab_id,
            node_key: NodeKey::Entity(entity.clone()),
            config_plugin_url: config_plugin_url.to_owned(),
            controller_plugin_url: controller_plugin_url.to_owned(),
        },
    };
    let payload = serde_json::to_string(&request).ok()?;
    let message = MessageToPlugin::new(andamento_shared::MSG_ACTIVATE_ENTITY)
        .with_destination_client_id(client_id)
        .with_payload(payload);
    Some(if controller_plugin_url.trim().is_empty() {
        message
    } else {
        message.with_plugin_url(controller_plugin_url.to_owned())
    })
}

fn build_rail_ui_action_message(
    controller_plugin_url: &str,
    action: RailUiAction,
) -> Option<MessageToPlugin> {
    let payload = serde_json::to_string(&action).ok()?;
    let message = MessageToPlugin::new(MSG_RAIL_UI_ACTION).with_payload(payload);
    Some(if controller_plugin_url.trim().is_empty() {
        message
    } else {
        message.with_plugin_url(controller_plugin_url.to_owned())
    })
}

fn build_rail_ui_state_request_message(controller_plugin_url: &str) -> MessageToPlugin {
    let message = MessageToPlugin::new(MSG_REQUEST_RAIL_UI_STATE);
    if controller_plugin_url.trim().is_empty() {
        message
    } else {
        message.with_plugin_url(controller_plugin_url.to_owned())
    }
}

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
    rail_ui_state: RailUiState,
    // None until a controller-backed render can distinguish overflow from
    // placeholder content.
    rail_can_scroll: Option<bool>,
    pending_scroll_delta: isize,
    scroll_flush_scheduled: bool,
    ensure_active_visible: bool,
    last_pane_manifest: Option<PaneManifest>,
    stats: PluginStatsRecorder,
    rail_placement: RailPlacement,
    own_plugin_placement: Option<PluginPlacement>,
    observed_rail_size: Option<RailSize>,
    latest_rail_size_target: Option<RailSizeTarget>,
    applied_rail_size_version: u64,
    hovered_detail_target: Option<NodeKey>,
    selected_detail_target: Option<NodeKey>,
}

#[cfg(feature = "native-plugin-factory")]
register_plugin!(PluginState);

impl ZellijPlugin for PluginState {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_plugin_id = Some(ids.plugin_id);
        self.own_client_id = Some(ids.client_id);
        self.controller_plugin_url = configuration
            .get(CONFIG_CONTROLLER_PLUGIN_URL)
            .cloned()
            .unwrap_or_else(|| "andamento-controller".to_owned());
        self.config_plugin_url = configuration
            .get(CONFIG_CONFIG_PLUGIN_URL)
            .cloned()
            .unwrap_or_else(|| "andamento-config".to_owned());
        self.rail_placement = configuration
            .get(CONFIG_RAIL_PLACEMENT)
            .and_then(|value| parse_rail_placement(value))
            .unwrap_or_default();

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
            EventType::PaneUpdate,
            EventType::Mouse,
            EventType::Timer,
            EventType::Visible,
            EventType::PermissionRequestResult,
        ]);
        self.send_renderer_hello();
    }

    fn update(&mut self, event: Event) -> bool {
        self.stats.increment("update.total");
        match event {
            Event::PermissionRequestResult(status) => {
                self.stats.increment("update.permission-result");
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                if self.permissions_granted {
                    set_selectable(false);
                }
                self.send_renderer_hello();
                true
            }
            Event::ModeUpdate(mode_info) => {
                self.stats.increment("update.mode");
                if !mode_info_needs_render(self.mode_info.as_ref(), &mode_info) {
                    false
                } else {
                    self.mode_info = Some(mode_info);
                    true
                }
            }
            Event::TabUpdate(tabs) => {
                self.stats.increment("update.tab");
                let local_tabs = local_tabs_from_zellij(&tabs);
                if self.local_tabs == local_tabs {
                    return false;
                }
                let previous_active_tab_id = self.active_tab_id();
                self.tabs = tabs;
                self.local_tabs = local_tabs;
                self.ensure_active_visible |= previous_active_tab_id != self.active_tab_id()
                    && self.active_tab_id().is_some();
                if let Some(pane_manifest) = self.last_pane_manifest.clone() {
                    self.observe_own_rail_size(&pane_manifest);
                }
                true
            }
            Event::PaneUpdate(pane_manifest) => {
                self.stats.increment("update.pane");
                self.last_pane_manifest = Some(pane_manifest);
                if let Some(pane_manifest) = self.last_pane_manifest.clone() {
                    self.observe_own_rail_size(&pane_manifest);
                }
                false
            }
            Event::Visible(true) => {
                self.stats.increment("update.visible");
                self.reconcile_rail_size_target();
                false
            }
            Event::Mouse(mouse) => {
                self.stats.increment("update.mouse");
                self.handle_mouse(mouse)
            }
            Event::Timer(_) if self.scroll_flush_scheduled => {
                self.stats.increment("update.scroll-flush");
                self.flush_pending_scroll()
            }
            _ => false,
        }
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        self.stats.increment(format!("pipe.{}", message.name));
        if message.name == MSG_VIEW_MODEL {
            if let Some(payload) = message.payload.as_deref() {
                let started_at = Instant::now();
                match serde_json::from_str::<ControllerViewModel>(payload) {
                    Ok(model) => {
                        self.stats
                            .record_span_elapsed("json.decode-view-model", started_at);
                        self.controller_model = Some(model);
                        // Defensive: zellij may grant cached permissions
                        // without firing PermissionRequestResult. Receiving a
                        // view model proves the controller is talking to us
                        // (which requires permissions), so flip non-selectable
                        // now rather than waiting for an event that may never
                        // arrive.
                        if !self.permissions_granted {
                            self.permissions_granted = true;
                            set_selectable(false);
                        }
                        return true;
                    }
                    Err(error) => {
                        eprintln!("andamento-rail: failed to parse view model: {error}");
                    }
                }
            }
        }
        if message.name == MSG_RAIL_UI_STATE {
            if let Some(payload) = message.payload.as_deref() {
                match serde_json::from_str::<RailUiState>(payload) {
                    Ok(state) if state.revision > self.rail_ui_state.revision => {
                        self.rail_ui_state = state;
                        return true;
                    }
                    Ok(_) => return false,
                    Err(error) => {
                        eprintln!("andamento-rail: failed to parse rail UI state: {error}");
                    }
                }
            }
            return false;
        }
        if message.name == MSG_STATS_REQUEST {
            if let Some(payload) = message.payload.as_deref() {
                if let Ok(request) = serde_json::from_str::<StatsCollectRequest>(payload) {
                    self.send_stats_report_to(request.requester, request.collection_id);
                    return false;
                }
            }
        }
        if message.name == MSG_RAIL_SIZE_TARGET {
            if let Some(payload) = message.payload.as_deref() {
                match serde_json::from_str::<RailSizeTarget>(payload) {
                    Ok(target) => self.handle_rail_size_target(target),
                    Err(error) => {
                        eprintln!("andamento-rail: failed to parse rail size target: {error}")
                    }
                }
            }
            return false;
        }
        false
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.stats.increment("render.total");
        let started_at = Instant::now();
        let controller_available = self.controller_model.is_some();
        let metadata_controls = self
            .controller_model
            .as_ref()
            .map(|model| model.metadata_controls.clone())
            .unwrap_or_default();
        let should_ensure_active_visible =
            self.ensure_active_visible && self.own_tab_id() == self.active_tab_id();
        let rendered = render::render_lines_with_detail_surface(
            self.controller_model.as_ref(),
            &self.local_tabs,
            rows,
            cols,
            controller_available,
            self.mode_info
                .as_ref()
                .map(|mode_info| mode_info.style.colors.into()),
            terminal_pixel_cell_size(),
            &self.rail_ui_state.collapsed_groups,
            None,
            &metadata_controls,
            self.rail_ui_state.scroll_offset,
            should_ensure_active_visible,
            self.hovered_detail_target
                .as_ref()
                .or(self.selected_detail_target.as_ref()),
        );
        if should_ensure_active_visible && rendered.ensure_active_resolved {
            self.ensure_active_visible = false;
        }
        if let Some(offset) = rendered.ensure_visible_offset {
            self.send_rail_ui_action(RailUiAction::SetScrollOffset { offset });
        }
        self.rail_can_scroll = controller_available.then(|| rendered.can_scroll());
        if should_sync_graphics(controller_available) {
            self.sync_graphics(&rendered.visible_cards);
        }
        self.hit_regions = rendered.hit_regions;
        print!("{}", rendered.lines.join("\n"));
        self.stats.record_span_elapsed("render.rail", started_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use andamento_shared::RailUiRevision;
    use render::{VisibleCard, VisibleIconRect};
    use std::cell::Cell;

    // Zellij's native shim references this WASM host import when tests exercise
    // mouse handlers that can switch tabs.
    thread_local! {
        static HOST_PLUGIN_COMMAND_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    #[no_mangle]
    extern "C" fn host_run_plugin_command() {
        HOST_PLUGIN_COMMAND_COUNT.with(|count| count.set(count.get() + 1));
    }

    fn pipe(name: &str, payload: String) -> PipeMessage {
        PipeMessage {
            source: PipeSource::Plugin(1),
            name: name.to_owned(),
            payload: Some(payload),
            args: BTreeMap::new(),
            is_private: false,
        }
    }

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

    #[test]
    fn scroll_action_preserves_its_full_line_count() {
        let message = build_rail_ui_action_message(
            "andamento-controller",
            RailUiAction::ScrollBy { delta: 7 },
        )
        .unwrap();

        assert_eq!(message.plugin_url.as_deref(), Some("andamento-controller"));
        assert_eq!(message.destination_client_id, None);
        assert_eq!(message.message_name, MSG_RAIL_UI_ACTION);
        assert_eq!(
            serde_json::from_str::<RailUiAction>(message.message_payload.as_deref().unwrap())
                .unwrap(),
            RailUiAction::ScrollBy { delta: 7 }
        );
    }

    #[test]
    fn clicking_a_visible_tab_does_not_reset_the_shared_viewport() {
        let mut state = PluginState {
            hit_regions: vec![HitRegion {
                row_start: 0,
                row_end: 2,
                col_start: 0,
                col_end: 19,
                tab_id: 2,
                tab_position: 1,
                group_path: None,
                inspect_target: None,
                materialize_request: None,
                action: HitAction::SwitchTab,
            }],
            ..Default::default()
        };
        let commands_before = HOST_PLUGIN_COMMAND_COUNT.with(|count| count.get());

        state.handle_mouse(Mouse::LeftClick(1, 4));

        assert_eq!(
            HOST_PLUGIN_COMMAND_COUNT.with(|count| count.get()),
            commands_before + 1,
            "the click should switch tabs without also sending ResetScroll"
        );
    }

    #[test]
    fn detail_hover_updates_live_and_click_remains_as_selection_fallback() {
        let entity = andamento_shared::EntityRef {
            kind: "issue".to_owned(),
            id: "github/flotilla-org/andamento#27".to_owned(),
        };
        let target = NodeKey::Entity(entity);
        let mut state = PluginState {
            hit_regions: vec![HitRegion {
                row_start: 1,
                row_end: 1,
                col_start: 2,
                col_end: 8,
                tab_id: 0,
                tab_position: 0,
                group_path: None,
                inspect_target: Some(target.clone()),
                materialize_request: None,
                action: HitAction::ShowDetail,
            }],
            ..Default::default()
        };

        assert!(state.handle_mouse(Mouse::Hover(1, 3)));
        assert_eq!(state.hovered_detail_target, Some(target.clone()));
        assert!(state.handle_mouse(Mouse::LeftClick(1, 3)));
        assert_eq!(state.selected_detail_target, Some(target.clone()));
        assert!(state.handle_mouse(Mouse::Hover(2, 3)));
        assert_eq!(state.hovered_detail_target, None);
        assert_eq!(state.selected_detail_target, Some(target));
    }

    #[test]
    fn detail_hover_does_not_clear_between_compact_chips() {
        let first = NodeKey::Entity(andamento_shared::EntityRef {
            kind: "action".to_owned(),
            id: "tui".to_owned(),
        });
        let second = NodeKey::Entity(andamento_shared::EntityRef {
            kind: "action".to_owned(),
            id: "governor".to_owned(),
        });
        let detail_hit = |col_start, col_end, target| HitRegion {
            row_start: 1,
            row_end: 1,
            col_start,
            col_end,
            tab_id: 0,
            tab_position: 0,
            group_path: None,
            inspect_target: Some(target),
            materialize_request: None,
            action: HitAction::Materialize,
        };
        let mut state = PluginState {
            hit_regions: vec![
                detail_hit(2, 6, first.clone()),
                detail_hit(8, 15, second.clone()),
            ],
            ..Default::default()
        };

        assert!(state.handle_mouse(Mouse::Hover(1, 4)));
        assert!(!state.handle_mouse(Mouse::Hover(1, 7)));
        assert_eq!(state.hovered_detail_target, Some(first));
        assert!(state.handle_mouse(Mouse::Hover(1, 10)));
        assert_eq!(state.hovered_detail_target, Some(second));
    }

    #[test]
    fn detail_hover_clears_in_a_different_compact_row_gap() {
        let first = NodeKey::Entity(andamento_shared::EntityRef {
            kind: "action".to_owned(),
            id: "tui".to_owned(),
        });
        let second_row_target = NodeKey::Entity(andamento_shared::EntityRef {
            kind: "action".to_owned(),
            id: "filesystem".to_owned(),
        });
        let mut state = PluginState {
            hit_regions: vec![
                HitRegion {
                    row_start: 1,
                    row_end: 1,
                    col_start: 2,
                    col_end: 6,
                    tab_id: 0,
                    tab_position: 0,
                    group_path: None,
                    inspect_target: Some(first.clone()),
                    materialize_request: None,
                    action: HitAction::Materialize,
                },
                HitRegion {
                    row_start: 2,
                    row_end: 2,
                    col_start: 8,
                    col_end: 15,
                    tab_id: 0,
                    tab_position: 0,
                    group_path: None,
                    inspect_target: Some(second_row_target),
                    materialize_request: None,
                    action: HitAction::Materialize,
                },
            ],
            ..Default::default()
        };

        assert!(state.handle_mouse(Mouse::Hover(1, 4)));
        assert!(state.handle_mouse(Mouse::Hover(2, 7)));
        assert_eq!(state.hovered_detail_target, None);
    }

    #[test]
    fn ensure_visible_latch_survives_until_controller_render_can_resolve_target() {
        let mut state = PluginState {
            local_tabs: vec![LocalTab {
                tab_id: 1,
                position: 0,
                name: "work".to_owned(),
                active: true,
            }],
            own_plugin_placement: Some(PluginPlacement::Tab {
                tab_id: 1,
                pane_kind: PluginPaneKind::Tiled,
            }),
            ensure_active_visible: true,
            ..Default::default()
        };

        state.render(4, 20);

        assert!(state.ensure_active_visible);
    }

    #[test]
    fn wheel_before_first_controller_snapshot_sends_scroll_action() {
        let mut state = PluginState::default();
        state.render(4, 20);
        let commands_before = HOST_PLUGIN_COMMAND_COUNT.with(|command_count| command_count.get());

        // Empty local tabs make the fallback a no-op, so a new host command
        // proves that the wheel delta was forwarded to the controller.
        state.handle_mouse(Mouse::ScrollDown(1));
        state.flush_pending_scroll();

        assert_eq!(
            HOST_PLUGIN_COMMAND_COUNT.with(|command_count| command_count.get()),
            commands_before + 1,
            "unknown scrollability must request controller scrolling, not use the tab fallback"
        );
    }

    #[test]
    fn tab_fallback_requires_a_confirmed_fitting_rail() {
        assert!(should_forward_scroll_to_controller(None));
        assert!(should_forward_scroll_to_controller(Some(true)));
        assert!(!should_forward_scroll_to_controller(Some(false)));
    }

    #[test]
    fn confirmed_fitting_rail_cycles_tabs() {
        let mut state = PluginState {
            local_tabs: vec![
                LocalTab {
                    tab_id: 1,
                    position: 0,
                    name: "one".to_owned(),
                    active: true,
                },
                LocalTab {
                    tab_id: 2,
                    position: 1,
                    name: "two".to_owned(),
                    active: false,
                },
            ],
            rail_can_scroll: Some(false),
            ..Default::default()
        };
        SCROLL_TAB_SWITCH_TARGET.with(|target| target.set(None));

        state.handle_mouse(Mouse::ScrollDown(1));
        state.flush_pending_scroll();

        assert_eq!(
            SCROLL_TAB_SWITCH_TARGET.with(|target| target.get()),
            Some(2)
        );
    }

    #[test]
    fn wheel_burst_does_not_advance_render_state_event_by_event() {
        let mut state = PluginState {
            rail_can_scroll: Some(true),
            ..Default::default()
        };

        let render_requests = [
            state.handle_mouse(Mouse::ScrollDown(1)),
            state.handle_mouse(Mouse::ScrollDown(1)),
            state.handle_mouse(Mouse::ScrollUp(1)),
        ];

        assert_eq!(render_requests, [false, false, false]);
        assert_eq!(state.rail_ui_state.scroll_offset, 0);
        assert!(state.scroll_flush_scheduled);
        assert!(!state.flush_pending_scroll());
        assert_eq!(state.rail_ui_state.scroll_offset, 0);
        assert_eq!(state.pending_scroll_delta, 0);
        assert!(!state.scroll_flush_scheduled);
    }

    #[test]
    fn explicit_navigation_clears_scroll_queued_before_it() {
        let mut state = PluginState {
            rail_ui_state: RailUiState {
                scroll_offset: 4,
                variables: BTreeMap::new(),
                ..Default::default()
            },
            rail_can_scroll: Some(true),
            ..Default::default()
        };
        state.handle_mouse(Mouse::ScrollDown(3));

        state.reset_scroll_position();

        assert_eq!(state.rail_ui_state.scroll_offset, 4);
        assert_eq!(state.pending_scroll_delta, 0);
        assert!(!state.scroll_flush_scheduled);
        assert!(!state.flush_pending_scroll());
        assert_eq!(state.rail_ui_state.scroll_offset, 4);
    }

    #[test]
    fn tab_scroll_delta_is_clamped_against_current_state() {
        assert_eq!(tab_index_after_scroll(3, 8, 4), 7);
        assert_eq!(tab_index_after_scroll(3, 8, -2), 1);
        assert_eq!(tab_index_after_scroll(3, 8, 99), 8);
        assert_eq!(tab_index_after_scroll(3, 8, -99), 1);
    }

    #[test]
    fn converts_pane_dimension_constraint_to_shared_rail_size() {
        assert_eq!(
            rail_size_from_constraint(PaneDimensionConstraint::Fixed(24)),
            RailSize::Fixed(24)
        );
        assert_eq!(
            rail_size_from_constraint(PaneDimensionConstraint::Percent(18.75)),
            RailSize::Percent(18.75)
        );
    }

    #[test]
    fn rail_size_target_applies_only_for_new_different_same_client_target() {
        let target = RailSizeTarget {
            client_id: 7,
            size: RailSize::Percent(22.0),
            version: 3,
        };

        assert!(rail_size_target_should_apply(
            &target,
            Some(7),
            Some(RailSize::Percent(18.0)),
            2
        ));
        assert!(!rail_size_target_should_apply(
            &target,
            Some(8),
            Some(RailSize::Percent(18.0)),
            2
        ));
        assert!(!rail_size_target_should_apply(
            &target,
            Some(7),
            Some(RailSize::Percent(18.0)),
            3
        ));
        assert!(!rail_size_target_should_apply(
            &target,
            Some(7),
            Some(RailSize::Percent(22.0005)),
            2
        ));
    }

    #[test]
    fn parses_rail_placement_from_plugin_configuration() {
        assert_eq!(parse_rail_placement("left"), Some(RailPlacement::Left));
        assert_eq!(parse_rail_placement("right"), Some(RailPlacement::Right));
        assert_eq!(parse_rail_placement("top"), Some(RailPlacement::Top));
        assert_eq!(parse_rail_placement("bottom"), Some(RailPlacement::Bottom));
        assert_eq!(parse_rail_placement("sideways"), None);
    }

    #[test]
    fn rail_placement_maps_to_resized_boundary() {
        assert_eq!(rail_resize_boundary(RailPlacement::Left), Direction::Right);
        assert_eq!(rail_resize_boundary(RailPlacement::Right), Direction::Left);
        assert_eq!(rail_resize_boundary(RailPlacement::Top), Direction::Down);
        assert_eq!(rail_resize_boundary(RailPlacement::Bottom), Direction::Up);
    }

    #[test]
    fn rail_placement_selects_the_observed_size_axis() {
        let rows = Some(PaneDimensionConstraint::Percent(12.5));
        let columns = Some(PaneDimensionConstraint::Percent(25.0));

        assert_eq!(
            rail_size_from_constraints(RailPlacement::Left, rows, columns),
            Some(RailSize::Percent(25.0))
        );
        assert_eq!(
            rail_size_from_constraints(RailPlacement::Top, rows, columns),
            Some(RailSize::Percent(12.5))
        );
    }

    #[test]
    fn config_button_builds_controller_inspect_request() {
        let message = build_config_inspect_message(
            "andamento-controller",
            "andamento-config",
            4,
            7,
            NodeKey::Tab(7),
        )
        .unwrap();

        assert_eq!(message.plugin_url.as_deref(), Some("andamento-controller"));
        assert_eq!(message.destination_client_id, Some(4));
        assert_eq!(message.message_name, MSG_CONFIG_INSPECT);
        assert!(message.plugin_config.is_empty());
        assert!(message.new_plugin_args.is_none());

        let payload: ConfigInspectRequest =
            serde_json::from_str(message.message_payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload.client_id, 4);
        assert_eq!(payload.origin_tab_id, 7);
        assert_eq!(payload.node_key, NodeKey::Tab(7));
        assert_eq!(payload.config_plugin_url, "andamento-config");
        assert_eq!(payload.controller_plugin_url, "andamento-controller");
    }

    #[test]
    fn latent_open_builds_an_identity_bearing_controller_request() {
        let request = andamento_shared::MaterializeLatentRequest {
            action_target: "flotilla:convoys/dev/latent-tabs".to_owned(),
            path: andamento_shared::GroupPath(vec![andamento_shared::GroupSegment {
                key: "flotilla.convoy".to_owned(),
                value: andamento_shared::MetadataValue::Text("dev/latent-tabs".to_owned()),
                label: Some("latent tabs".to_owned()),
            }]),
            name: "latent tabs".to_owned(),
            recipe: "flotilla attach latent-tabs".to_owned(),
            checkout_path: Some("/work/andamento".to_owned()),
        };

        let message =
            build_materialize_latent_message("andamento-controller", 4, &request).unwrap();

        assert_eq!(message.plugin_url.as_deref(), Some("andamento-controller"));
        assert_eq!(message.destination_client_id, Some(4));
        assert_eq!(
            message.message_name,
            andamento_shared::MSG_MATERIALIZE_LATENT
        );
        let payload: andamento_shared::MaterializeLatentRequest =
            serde_json::from_str(message.message_payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload, request);
    }

    #[test]
    fn attention_activation_builds_an_entity_only_controller_request() {
        let entity = andamento_shared::EntityRef {
            kind: "vessel".to_owned(),
            id: "dev/focus/worker@lab".to_owned(),
        };

        let message = build_activate_entity_message(
            "andamento-controller",
            "andamento-config",
            4,
            7,
            &entity,
        )
        .unwrap();

        assert_eq!(message.plugin_url.as_deref(), Some("andamento-controller"));
        assert_eq!(message.destination_client_id, Some(4));
        assert_eq!(message.message_name, andamento_shared::MSG_ACTIVATE_ENTITY);
        let payload: andamento_shared::EntityActivationRequest =
            serde_json::from_str(message.message_payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload.entity, entity);
        assert_eq!(
            payload.inspect_fallback.node_key,
            NodeKey::Entity(entity.clone()),
            "a row that cannot be activated must still reach its inspector"
        );
        assert_eq!(payload.inspect_fallback.origin_tab_id, 7);
    }

    #[test]
    fn group_toggle_builds_controller_request() {
        let path = GroupPath(vec![andamento_shared::GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: andamento_shared::MetadataValue::Text("/repo".to_owned()),
            label: Some("repo".to_owned()),
        }]);

        let message = build_rail_ui_action_message(
            "andamento-controller",
            RailUiAction::ToggleGroup { path: path.clone() },
        )
        .unwrap();

        assert_eq!(message.plugin_url.as_deref(), Some("andamento-controller"));
        assert_eq!(message.destination_client_id, None);
        assert_eq!(message.message_name, MSG_RAIL_UI_ACTION);
        let payload: RailUiAction =
            serde_json::from_str(message.message_payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload, RailUiAction::ToggleGroup { path });
    }

    #[test]
    fn late_spawned_rail_requests_session_ui_state_without_a_client_filter() {
        let message = build_rail_ui_state_request_message("andamento-controller");

        assert_eq!(message.plugin_url.as_deref(), Some("andamento-controller"));
        assert_eq!(message.destination_plugin_id, None);
        assert_eq!(message.destination_client_id, None);
        assert_eq!(message.message_name, MSG_REQUEST_RAIL_UI_STATE);
    }

    #[test]
    fn broadcast_snapshot_aligns_existing_and_late_spawned_rails() {
        let path = GroupPath(vec![andamento_shared::GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: andamento_shared::MetadataValue::Text("/repo".to_owned()),
            label: Some("repo".to_owned()),
        }]);
        let snapshot = RailUiState {
            revision: RailUiRevision {
                sequence: 3,
                writer_client_id: 1,
            },
            collapsed_groups: vec![path],
            scroll_offset: 9,
            variables: BTreeMap::new(),
        };
        let payload = serde_json::to_string(&snapshot).unwrap();
        let mut existing = PluginState::default();
        let mut late_spawned = PluginState::default();

        assert!(existing.pipe(pipe(MSG_RAIL_UI_STATE, payload.clone())));
        assert!(late_spawned.pipe(pipe(MSG_RAIL_UI_STATE, payload)));

        assert_eq!(existing.rail_ui_state, snapshot);
        assert_eq!(late_spawned.rail_ui_state, snapshot);
    }

    #[test]
    fn rail_total_orders_same_sequence_broadcasts() {
        let mut rail = PluginState::default();
        let current = RailUiState {
            revision: RailUiRevision {
                sequence: 4,
                writer_client_id: 1,
            },
            collapsed_groups: vec![],
            scroll_offset: 12,
            variables: BTreeMap::new(),
        };
        let winner = RailUiState {
            revision: RailUiRevision {
                sequence: 4,
                writer_client_id: 2,
            },
            collapsed_groups: vec![],
            scroll_offset: 14,
            variables: BTreeMap::new(),
        };
        let stale = RailUiState {
            revision: RailUiRevision {
                sequence: 4,
                writer_client_id: 0,
            },
            collapsed_groups: vec![],
            scroll_offset: 1,
            variables: BTreeMap::new(),
        };

        assert!(rail.pipe(pipe(
            MSG_RAIL_UI_STATE,
            serde_json::to_string(&current).unwrap(),
        )));
        assert!(rail.pipe(pipe(
            MSG_RAIL_UI_STATE,
            serde_json::to_string(&winner).unwrap(),
        )));
        assert!(!rail.pipe(pipe(
            MSG_RAIL_UI_STATE,
            serde_json::to_string(&stale).unwrap(),
        )));

        assert_eq!(rail.rail_ui_state, winner);
    }

    #[test]
    fn inspect_defaults_to_rail_own_tab_before_global_active_tab() {
        let state = PluginState {
            local_tabs: vec![
                LocalTab {
                    tab_id: 1,
                    position: 0,
                    name: "main".to_owned(),
                    active: true,
                },
                LocalTab {
                    tab_id: 7,
                    position: 1,
                    name: "work".to_owned(),
                    active: false,
                },
            ],
            own_plugin_placement: Some(PluginPlacement::Tab {
                tab_id: 7,
                pane_kind: PluginPaneKind::Tiled,
            }),
            ..Default::default()
        };

        assert_eq!(state.active_tab_id(), Some(1));
        assert_eq!(state.own_tab_id(), Some(7));
        assert_eq!(state.active_inspect_node(), Some(NodeKey::Tab(7)));
    }

    #[test]
    fn main_tab_zero_is_a_tab_not_root() {
        let hit = HitRegion {
            row_start: 0,
            row_end: 0,
            col_start: 0,
            col_end: 0,
            tab_id: 0,
            tab_position: 0,
            group_path: None,
            inspect_target: Some(NodeKey::Tab(0)),
            materialize_request: None,
            action: HitAction::InspectNode,
        };

        assert_eq!(hit.inspect_target, Some(NodeKey::Tab(0)));
    }

    #[test]
    fn resolves_own_plugin_tab_placement_from_pane_manifest() {
        let pane_manifest = PaneManifest {
            panes: HashMap::from([(
                2,
                vec![PaneInfo {
                    id: 42,
                    is_plugin: true,
                    is_floating: false,
                    ..Default::default()
                }],
            )]),
        };
        let local_tabs = vec![LocalTab {
            tab_id: 70,
            position: 2,
            name: "work".to_owned(),
            active: true,
        }];

        assert_eq!(
            own_plugin_tab_placement(&pane_manifest, &local_tabs, 42),
            Some(PluginPlacement::Tab {
                tab_id: 70,
                pane_kind: PluginPaneKind::Tiled,
            })
        );
    }

    #[test]
    fn resolves_floating_plugin_placement_from_pane_manifest() {
        let pane_manifest = PaneManifest {
            panes: HashMap::from([(
                1,
                vec![PaneInfo {
                    id: 9,
                    is_plugin: true,
                    is_floating: true,
                    ..Default::default()
                }],
            )]),
        };
        let local_tabs = vec![LocalTab {
            tab_id: 15,
            position: 1,
            name: "settings".to_owned(),
            active: false,
        }];

        assert_eq!(
            own_plugin_tab_placement(&pane_manifest, &local_tabs, 9),
            Some(PluginPlacement::Tab {
                tab_id: 15,
                pane_kind: PluginPaneKind::Floating,
            })
        );
    }

    #[test]
    fn renderer_hello_payload_includes_registration_placement() {
        let payload = renderer_hello_payload(
            42,
            3,
            PluginPlacement::Tab {
                tab_id: 70,
                pane_kind: PluginPaneKind::Tiled,
            },
        )
        .unwrap();
        let decoded: PluginRegistrationHello = serde_json::from_str(&payload).unwrap();

        assert_eq!(
            decoded,
            PluginRegistrationHello {
                identity: RendererHello {
                    plugin_id: 42,
                    client_id: 3,
                },
                placement: PluginPlacement::Tab {
                    tab_id: 70,
                    pane_kind: PluginPaneKind::Tiled,
                },
            }
        );
    }
}

impl PluginState {
    fn controller_message(&self, name: &str) -> MessageToPlugin {
        let message = MessageToPlugin::new(name);
        if self.controller_plugin_url.trim().is_empty() {
            message
        } else {
            message.with_plugin_url(self.controller_plugin_url.clone())
        }
    }

    fn send_renderer_hello(&self) {
        let (Some(plugin_id), Some(client_id)) = (self.own_plugin_id, self.own_client_id) else {
            log::info!("andamento-rail: send_renderer_hello SKIPPED (own_plugin_id={:?}, own_client_id={:?})", self.own_plugin_id, self.own_client_id);
            return;
        };
        log::info!("andamento-rail: send_renderer_hello plugin_id={plugin_id} client_id={client_id} → controller={:?}", self.controller_plugin_url);
        let Some(payload) = renderer_hello_payload(
            plugin_id,
            client_id,
            self.own_plugin_placement
                .clone()
                .unwrap_or(PluginPlacement::Unknown),
        ) else {
            return;
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_RENDERER_HELLO)
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
        pipe_message_to_plugin(
            self.controller_message(MSG_REQUEST_STATE)
                .with_destination_client_id(client_id),
        );
        // UI state is session-wide and intentionally broadcast. Request the
        // controller's current snapshot on every hello so rails created after
        // earlier collapse/scroll actions immediately catch up.
        pipe_message_to_plugin(build_rail_ui_state_request_message(
            &self.controller_plugin_url,
        ));
    }

    fn send_stats_report_to(&self, requester: RendererHello, collection_id: u64) {
        let (Some(plugin_id), Some(client_id)) = (self.own_plugin_id, self.own_client_id) else {
            return;
        };
        let mut snapshot = self.stats.snapshot(
            collection_id,
            RendererHello {
                plugin_id,
                client_id,
            },
            "rail",
        );
        snapshot.counters.insert(
            format!("rail.placement.{}", self.rail_placement.as_str()),
            1,
        );
        snapshot.counters.insert(
            format!(
                "rail.boundary.{}",
                direction_name(rail_resize_boundary(self.rail_placement))
            ),
            1,
        );
        if let Some(size) = self.observed_rail_size {
            match size {
                RailSize::Fixed(cells) => {
                    snapshot
                        .counters
                        .insert("rail.size.fixed-cells".to_owned(), cells as u64);
                }
                RailSize::Percent(percent) => {
                    snapshot.counters.insert(
                        "rail.size.percent-x1000".to_owned(),
                        (percent * 1000.0).round() as u64,
                    );
                }
            }
        }
        let Ok(payload) = serde_json::to_string(&snapshot) else {
            return;
        };
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_STATS_REPORT)
                .with_destination_plugin_id(requester.plugin_id)
                .with_destination_client_id(requester.client_id)
                .with_payload(payload),
        );
    }

    fn observe_own_rail_size(&mut self, pane_manifest: &PaneManifest) {
        let Some(plugin_id) = self.own_plugin_id else {
            return;
        };
        let next_placement = own_plugin_tab_placement(pane_manifest, &self.local_tabs, plugin_id);
        if next_placement.is_some() && self.own_plugin_placement != next_placement {
            self.own_plugin_placement = next_placement;
            self.send_renderer_hello();
        }
        let own_pane = pane_manifest
            .panes
            .values()
            .flat_map(|panes| panes.iter())
            .find(|pane| pane.is_plugin && pane.id == plugin_id);
        if let Some(pane) = own_pane {
            // Defensive auto-correct: the rail should never be selectable
            // once we're past the initial permission window (signalled by
            // having a controller_model). Some lifecycle path is flipping
            // it back to selectable; force-correct here whenever we observe
            // it instead of relying on event-driven state.
            if pane.is_selectable && self.controller_model.is_some() {
                set_selectable(false);
                self.stats.increment("rail.auto-unselectable");
            }
        }
        let Some(size) = own_pane.and_then(|pane| {
            rail_size_from_constraints(
                self.rail_placement,
                pane.pane_rows_constraint,
                pane.pane_columns_constraint,
            )
        }) else {
            return;
        };
        if self
            .observed_rail_size
            .map(|current| current.within_tolerance(size))
            .unwrap_or(false)
        {
            return;
        }
        self.observed_rail_size = Some(size);
        self.send_rail_size_observed(size);
        self.reconcile_rail_size_target();
    }

    fn send_rail_size_observed(&self, size: RailSize) {
        let (Some(plugin_id), Some(client_id)) = (self.own_plugin_id, self.own_client_id) else {
            return;
        };
        let observed = RailSizeObserved {
            rail: RendererHello {
                plugin_id,
                client_id,
            },
            size,
        };
        let Ok(payload) = serde_json::to_string(&observed) else {
            return;
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_RAIL_SIZE_OBSERVED)
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
    }

    fn handle_rail_size_target(&mut self, target: RailSizeTarget) {
        if self.own_client_id != Some(target.client_id) {
            return;
        }
        if self
            .latest_rail_size_target
            .as_ref()
            .map(|current| current.version < target.version)
            .unwrap_or(true)
        {
            self.latest_rail_size_target = Some(target);
        }
        self.reconcile_rail_size_target();
    }

    fn reconcile_rail_size_target(&mut self) {
        let Some(target) = self.latest_rail_size_target.clone() else {
            return;
        };
        let Some(plugin_id) = self.own_plugin_id else {
            return;
        };
        if !rail_size_target_should_apply(
            &target,
            self.own_client_id,
            self.observed_rail_size,
            self.applied_rail_size_version,
        ) {
            self.applied_rail_size_version = self.applied_rail_size_version.max(target.version);
            return;
        }
        resize_pane_with_id_to(
            PaneId::Plugin(plugin_id),
            rail_resize_boundary(self.rail_placement),
            pane_dimension_constraint_from_rail_size(target.size),
        );
        self.applied_rail_size_version = target.version;
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
                    HitAction::ActivateEntity => {
                        if let (Some(NodeKey::Entity(entity)), Some(client_id)) =
                            (hit.inspect_target.as_ref(), self.own_client_id)
                        {
                            if let Some(message) = build_activate_entity_message(
                                &self.controller_plugin_url,
                                &self.config_plugin_url,
                                client_id,
                                self.own_tab_id()
                                    .or_else(|| self.active_tab_id())
                                    .unwrap_or(0),
                                entity,
                            ) {
                                pipe_message_to_plugin(message);
                            }
                        }
                        false
                    }
                    HitAction::Materialize => {
                        if let (Some(request), Some(client_id)) =
                            (hit.materialize_request.as_ref(), self.own_client_id)
                        {
                            self.reset_scroll_position();
                            if let Some(message) = build_materialize_latent_message(
                                &self.controller_plugin_url,
                                client_id,
                                request,
                            ) {
                                pipe_message_to_plugin(message);
                            }
                        }
                        false
                    }
                    HitAction::TogglePin => {
                        self.toggle_pin(hit.tab_id);
                        false
                    }
                    HitAction::ToggleGroup => {
                        if let Some(group_path) = hit.group_path {
                            self.toggle_group(group_path);
                        }
                        false
                    }
                    HitAction::OpenConfig => {
                        self.open_config_pane();
                        false
                    }
                    HitAction::ScrollRailUp => {
                        self.send_rail_ui_action(RailUiAction::ScrollBy { delta: -1 });
                        false
                    }
                    HitAction::ScrollRailDown => {
                        self.send_rail_ui_action(RailUiAction::ScrollBy { delta: 1 });
                        false
                    }
                    HitAction::InspectNode => {
                        if let Some(key) = hit.inspect_target {
                            self.open_config_for_node(key);
                        }
                        false
                    }
                    HitAction::ShowDetail => {
                        self.selected_detail_target = hit.inspect_target;
                        true
                    }
                    HitAction::ToggleVariable(index) => {
                        if let Some(name) = self
                            .controller_model
                            .as_ref()
                            .and_then(|model| model.display_variables.get(index))
                            .map(|variable| variable.name.clone())
                        {
                            self.send_rail_ui_action(RailUiAction::ToggleVariable { name });
                        }
                        false
                    }
                }
            }
            Mouse::Hover(row, col) if row >= 0 => {
                let row = row as usize;
                let target = hit_at(&self.hit_regions, row, col).and_then(|hit| {
                    match hit.inspect_target.as_ref() {
                        Some(target @ NodeKey::Entity(_)) => Some(target.clone()),
                        _ => None,
                    }
                });
                if target.is_none()
                    && self.hovered_detail_target.is_some()
                    && self.hit_regions.iter().any(|hit| {
                        hit.inspect_target.as_ref() == self.hovered_detail_target.as_ref()
                            && row >= hit.row_start
                            && row <= hit.row_end
                    })
                {
                    return false;
                }
                if self.hovered_detail_target == target {
                    false
                } else {
                    self.hovered_detail_target = target;
                    true
                }
            }
            Mouse::ScrollUp(lines) => self.queue_scroll_delta(
                isize::try_from(lines)
                    .unwrap_or(isize::MAX)
                    .saturating_neg(),
            ),
            Mouse::ScrollDown(lines) => {
                self.queue_scroll_delta(isize::try_from(lines).unwrap_or(isize::MAX))
            }
            _ => false,
        }
    }

    fn queue_scroll_delta(&mut self, delta: isize) -> bool {
        if delta == 0 {
            return false;
        }
        self.pending_scroll_delta = self.pending_scroll_delta.saturating_add(delta);
        if !self.scroll_flush_scheduled {
            self.scroll_flush_scheduled = true;
            // The timer is enqueued behind scroll updates already waiting in
            // Zellij. Suppress their individual draws, then apply their net
            // effect against the latest state when the timer reaches us.
            schedule_scroll_flush();
        }
        false
    }

    fn flush_pending_scroll(&mut self) -> bool {
        self.scroll_flush_scheduled = false;
        let delta = std::mem::take(&mut self.pending_scroll_delta);
        if delta == 0 {
            return false;
        }
        if should_forward_scroll_to_controller(self.rail_can_scroll) {
            self.send_rail_ui_action(RailUiAction::ScrollBy { delta });
            return false;
        }
        if let Some(active_tab_idx) = self.active_tab_idx() {
            let target = tab_index_after_scroll(active_tab_idx, self.local_tabs.len(), delta);
            if target != active_tab_idx {
                switch_tab_from_scroll(target as u32);
            }
        }
        false
    }

    fn reset_scroll_position(&mut self) {
        self.pending_scroll_delta = 0;
        self.scroll_flush_scheduled = false;
        self.send_rail_ui_action(RailUiAction::ResetScroll);
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
            self.controller_message(MSG_TOGGLE_PIN)
                .with_destination_client_id(client_id)
                .with_args(args),
        );
    }

    fn toggle_group(&self, group_path: GroupPath) {
        self.send_rail_ui_action(RailUiAction::ToggleGroup { path: group_path });
    }

    fn send_rail_ui_action(&self, action: RailUiAction) {
        let Some(message) = build_rail_ui_action_message(&self.controller_plugin_url, action)
        else {
            return;
        };
        pipe_message_to_plugin(message);
    }

    fn open_config_pane(&self) {
        self.open_config_for_node(self.active_inspect_node().unwrap_or(NodeKey::Root));
    }

    fn open_config_for_node(&self, node_key: NodeKey) {
        let Some(client_id) = self.own_client_id else {
            return;
        };
        let Some(message) = build_config_inspect_message(
            &self.controller_plugin_url,
            &self.config_plugin_url,
            client_id,
            self.own_tab_id()
                .or_else(|| self.active_tab_id())
                .unwrap_or(0),
            node_key,
        ) else {
            return;
        };
        pipe_message_to_plugin(message);
    }

    fn active_tab_id(&self) -> Option<u64> {
        self.local_tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.tab_id)
    }

    fn own_tab_id(&self) -> Option<u64> {
        match self.own_plugin_placement {
            Some(PluginPlacement::Tab { tab_id, .. }) => Some(tab_id),
            _ => None,
        }
    }

    fn active_inspect_node(&self) -> Option<NodeKey> {
        self.own_tab_id()
            .or_else(|| self.active_tab_id())
            .map(NodeKey::Tab)
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
