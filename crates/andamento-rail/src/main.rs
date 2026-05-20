mod render;

#[cfg(not(target_family = "wasm"))]
fn main() {}

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

use std::cmp::{max, min};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use render::{hit_at, HitAction, HitRegion};
use render::{status_icon_is_renderable, LocalTab, VisibleCard, VisibleIconRect};
use andamento_shared::StatusIcon;
use andamento_shared::{
    ControllerViewModel, GroupPath, NodeKey, PluginStatsRecorder, RailSizeObserved, RendererHello,
    StatsCollectRequest, MSG_CYCLE_METADATA_TRISTATE, MSG_RAIL_SIZE_OBSERVED,
    MSG_RAIL_SIZE_TARGET, MSG_RENDERER_HELLO, MSG_REQUEST_STATE, MSG_STATS_REPORT,
    MSG_STATS_REQUEST, MSG_TOGGLE_METADATA_ROOT, MSG_TOGGLE_PIN, MSG_VIEW_MODEL,
};
use andamento_shared::{RailSize, RailSizeTarget};
use zellij_tile::output::print;
use zellij_tile::prelude::*;
#[cfg(test)]
use zellij_tile::prelude::{Direction, ModeInfo, PaneDimensionConstraint, TabInfo};

const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";
const CONFIG_CONFIG_PLUGIN_URL: &str = "config_plugin_url";
const CONFIG_CLOSE_ON_HIDDEN: &str = "close_on_hidden";
const CONFIG_RAIL_SCOPE: &str = "rail_scope";
const CONFIG_RAIL_PLACEMENT: &str = "rail_placement";

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
    rail_scroll_offset: isize,
    rail_can_scroll: bool,
    own_is_selectable: bool,
    own_is_focused: bool,
    collapsed_groups: BTreeSet<GroupPath>,
    stats: PluginStatsRecorder,
    rail_placement: RailPlacement,
    observed_rail_size: Option<RailSize>,
    latest_rail_size_target: Option<RailSizeTarget>,
    applied_rail_size_version: u64,
}

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
                self.tabs = tabs;
                self.local_tabs = local_tabs;
                true
            }
            Event::PaneUpdate(pane_manifest) => {
                self.stats.increment("update.pane");
                self.observe_own_rail_size(pane_manifest);
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
                    Err(error) => eprintln!("andamento-rail: failed to parse rail size target: {error}"),
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
        let collapsed_groups = self.collapsed_groups.iter().cloned().collect::<Vec<_>>();
        let metadata_controls = self
            .controller_model
            .as_ref()
            .map(|model| model.metadata_controls.clone())
            .unwrap_or_default();
        let rendered = render::render_lines_with_rail_scroll(
            self.controller_model.as_ref(),
            &self.local_tabs,
            rows,
            cols,
            controller_available,
            self.mode_info
                .as_ref()
                .map(|mode_info| mode_info.style.colors.into()),
            terminal_pixel_cell_size(),
            &collapsed_groups,
            None,
            &metadata_controls,
            self.rail_scroll_offset,
        );
        self.rail_can_scroll = rendered.can_scroll();
        if should_sync_graphics(controller_available) {
            self.sync_graphics(&rendered.visible_cards);
        }
        self.hit_regions = rendered.hit_regions;
        print!("{}", rendered.lines.join("\n"));
        // Temporary diagnostic: instance fingerprint + selectable/focused
        // state of our own pane, just above the footer so we don't clobber
        // the gear/toggle/scroll controls on the footer row itself.
        let inst = (self as *const _) as usize & 0xFFFFFF;
        let sel = if self.own_is_selectable { '✓' } else { '✗' };
        let focus = if self.own_is_focused { '◉' } else { '○' };
        let label = format!("[{inst:06x} sel:{sel} focus:{focus}]");
        let label_len = label.chars().count();
        if rows >= 2 && cols >= label_len {
            let col = cols.saturating_sub(label_len) + 1;
            let row_above_footer = rows - 1;
            print!("\x1b[{row_above_footer};{col}H\x1b[2m{label}\x1b[0m");
        }
        self.stats.record_span_elapsed("render.rail", started_at);
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
        let hello = RendererHello {
            plugin_id,
            client_id,
        };
        let Ok(payload) = serde_json::to_string(&hello) else {
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

    fn observe_own_rail_size(&mut self, pane_manifest: PaneManifest) {
        let Some(plugin_id) = self.own_plugin_id else {
            return;
        };
        let own_pane = pane_manifest
            .panes
            .values()
            .flat_map(|panes| panes.iter())
            .find(|pane| pane.is_plugin && pane.id == plugin_id);
        if let Some(pane) = own_pane {
            self.own_is_selectable = pane.is_selectable;
            self.own_is_focused = pane.is_focused;
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
                        self.rail_scroll_offset = 0;
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
                    HitAction::ScrollRailUp => {
                        self.rail_scroll_offset = self.rail_scroll_offset.saturating_sub(1);
                        true
                    }
                    HitAction::ScrollRailDown => {
                        self.rail_scroll_offset = self.rail_scroll_offset.saturating_add(1);
                        true
                    }
                    HitAction::ToggleMetadataRoot => {
                        self.send_toggle_metadata_root();
                        false
                    }
                    HitAction::CycleMetadataTriState => {
                        // Discriminate by group_path presence — tab_id 0 is a
                        // valid id (the initial tab), so we can't use it as
                        // an "absent" sentinel.
                        let key = if let Some(group_path) = hit.group_path {
                            NodeKey::Group(group_path)
                        } else {
                            NodeKey::Tab(hit.tab_id)
                        };
                        self.send_cycle_metadata_tristate(key);
                        false
                    }
                    HitAction::CycleRootMetadataTriState => {
                        self.send_cycle_metadata_tristate(NodeKey::Root);
                        false
                    }
                }
            }
            Mouse::ScrollUp(_) => {
                if self.rail_can_scroll {
                    self.rail_scroll_offset = self.rail_scroll_offset.saturating_sub(1);
                    return true;
                }
                if let Some(active_tab_idx) = self.active_tab_idx() {
                    let prev = max(active_tab_idx.saturating_sub(1), 1);
                    switch_tab_to(prev as u32);
                }
                false
            }
            Mouse::ScrollDown(_) => {
                if self.rail_can_scroll {
                    self.rail_scroll_offset = self.rail_scroll_offset.saturating_add(1);
                    return true;
                }
                if let Some(active_tab_idx) = self.active_tab_idx() {
                    let next = min(active_tab_idx + 1, self.local_tabs.len());
                    switch_tab_to(next as u32);
                }
                false
            }
            _ => false,
        }
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

    fn send_toggle_metadata_root(&self) {
        let Some(client_id) = self.own_client_id else {
            return;
        };
        if self.controller_model.is_none() {
            return;
        }
        pipe_message_to_plugin(
            self.controller_message(MSG_TOGGLE_METADATA_ROOT)
                .with_destination_client_id(client_id),
        );
    }

    fn send_cycle_metadata_tristate(&self, key: NodeKey) {
        let Some(client_id) = self.own_client_id else {
            return;
        };
        if self.controller_model.is_none() {
            return;
        }
        let payload = match serde_json::to_string(&key) {
            Ok(payload) => payload,
            Err(_) => return,
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_CYCLE_METADATA_TRISTATE)
                .with_destination_client_id(client_id)
                .with_payload(payload),
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
        configuration.insert(CONFIG_CLOSE_ON_HIDDEN.to_owned(), "true".to_owned());
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
