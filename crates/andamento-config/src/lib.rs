use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::HashMap;
use std::time::Instant;

use andamento_shared::segment_bar;
use andamento_shared::{
    ConfigInspectRequest, ControllerViewModel, MetadataEntry, MetadataSourceEntry,
    MetadataTriState, MetadataValue, MetadataVisibilitySetRequest, NodeKey, NodeVariableSetRequest,
    PluginPaneKind, PluginPlacement, PluginRegistrationHello, PluginStatsSnapshot, RailConfig,
    RailStructure, ResolvedMetadataTarget, TabCard, NODE_VARIABLE_CONFIG_OVERRIDE_SETTER,
};
use unicode_width::UnicodeWidthStr;

use andamento_shared::StatsCollectRequest;
use andamento_shared::{
    PluginStatsRecorder, RendererHello, MSG_CONFIG_EDITOR_HELLO, MSG_CONFIG_INSPECT,
    MSG_REQUEST_STATE, MSG_SET_METADATA_VISIBILITY, MSG_SET_NODE_VARIABLE, MSG_SET_RAIL_CONFIG,
    MSG_STATS_COLLECT, MSG_STATS_REPORT, MSG_STATS_REQUEST, MSG_VIEW_MODEL,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use zellij_tile::output::print;
use zellij_tile::prelude::*;
use zellij_tile::ui_components::serialize_ribbon_with_coordinates;

const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";
const CONFIG_CLOSE_ON_HIDDEN: &str = "close_on_hidden";
const CONFIG_ORIGIN_TAB_ID: &str = "origin_tab_id";
const CONFIG_PANE_KIND: &str = "pane_kind";
const CONFIG_RAIL_SCOPE: &str = "rail_scope";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigLocalTab {
    tab_id: u64,
    position: usize,
}

fn local_tabs_from_zellij(tabs: &[TabInfo]) -> Vec<ConfigLocalTab> {
    tabs.iter()
        .map(|tab| ConfigLocalTab {
            tab_id: tab.tab_id as u64,
            position: tab.position,
        })
        .collect()
}

fn own_plugin_tab_placement(
    pane_manifest: &PaneManifest,
    local_tabs: &[ConfigLocalTab],
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

fn config_editor_hello_payload(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigAction {
    SetPage(ConfigPage),
    SetStructure(RailStructure),
    CollectStats,
    SetInspectedMetadata(Option<MetadataTriState>),
    /// Indices into the inspected node's declarations, and into that
    /// declaration's allowed values. `None` clears the override (inherit).
    SetNodeVariable {
        variable: usize,
        value: Option<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPage {
    Settings,
    Inspect,
    Templates,
    Stats,
}

impl Default for ConfigPage {
    fn default() -> Self {
        Self::Settings
    }
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
    ribbons: Vec<RibbonOverlay>,
    body_scroll_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RibbonOverlay {
    row: usize,
    col: usize,
    width: usize,
    label: String,
    selected: bool,
}

struct ConfigUiFrame {
    rows: usize,
    cols: usize,
    next_row: usize,
    lines: Vec<String>,
    hit_regions: Vec<HitRegion>,
    ribbons: Vec<RibbonOverlay>,
}

impl ConfigUiFrame {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            next_row: 0,
            lines: vec![],
            hit_regions: vec![],
            ribbons: vec![],
        }
    }

    fn current_row(&self) -> usize {
        self.next_row
    }

    fn push_plain(&mut self, text: &str) -> Option<usize> {
        self.push_line(Line::from(text.to_owned()))
    }

    fn push_blank(&mut self) -> Option<usize> {
        self.push_plain("")
    }

    fn push_line(&mut self, line: Line<'static>) -> Option<usize> {
        if self.next_row >= self.rows {
            return None;
        }
        let row = self.next_row;
        let mut buffer = Buffer::empty(Rect::new(0, 0, self.cols as u16, 1));
        Paragraph::new(line).render(Rect::new(0, 0, self.cols as u16, 1), &mut buffer);
        let rendered = (0..self.cols)
            .map(|col| buffer[(col as u16, 0)].symbol())
            .collect::<String>();
        self.lines.push(pad_to_width(
            &truncate_to_width(&rendered, self.cols),
            self.cols,
        ));
        self.next_row = self.next_row.saturating_add(1);
        Some(row)
    }

    fn add_hit(&mut self, row: usize, col_start: usize, col_end: usize, action: ConfigAction) {
        if row >= self.rows || col_start >= self.cols {
            return;
        }
        self.hit_regions.push(HitRegion {
            row,
            col_start,
            col_end: col_end.min(self.cols.saturating_sub(1)),
            action,
        });
    }

    fn add_ribbon(&mut self, ribbon: RibbonOverlay) {
        if ribbon.row < self.rows && ribbon.col < self.cols {
            self.ribbons.push(ribbon);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedTabRow {
    line: String,
    hit_regions: Vec<HitRegion>,
    ribbons: Vec<RibbonOverlay>,
}

#[derive(Default)]
pub struct PluginState {
    controller_plugin_url: String,
    close_on_hidden: bool,
    rail_scope: Option<String>,
    local_tabs: Vec<ConfigLocalTab>,
    last_pane_manifest: Option<PaneManifest>,
    own_plugin_id: Option<u32>,
    own_client_id: Option<u16>,
    rendered_for_node: Option<NodeKey>,
    own_plugin_placement: Option<PluginPlacement>,
    model: Option<ControllerViewModel>,
    pending_config: Option<RailConfig>,
    page: ConfigPage,
    hit_regions: Vec<HitRegion>,
    permissions_granted: bool,
    stats: PluginStatsRecorder,
    stats_collection_id: u64,
    stats_collection_pending: bool,
    stats_reports: Vec<PluginStatsSnapshot>,
    body_scroll_offset: usize,
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
        self.close_on_hidden = configuration
            .get(CONFIG_CLOSE_ON_HIDDEN)
            .is_some_and(|value| config_bool(value));
        self.rail_scope = config_scope(&configuration);
        self.own_plugin_placement = launch_config_placement(&configuration);
        self.page = initial_page_for_scope(self.rail_scope.as_deref());

        request_permission(&[
            PermissionType::ChangeApplicationState,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::Mouse,
            EventType::PermissionRequestResult,
            EventType::Visible,
        ]);
        self.send_hello();
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        self.stats.increment(format!("pipe.{}", message.name));
        if message.name == MSG_REQUEST_STATE {
            if self.permissions_granted {
                show_self(true);
            }
            self.send_hello();
            return true;
        }
        if message.name == MSG_CONFIG_INSPECT {
            if let Some(payload) = message.payload.as_deref() {
                match serde_json::from_str::<ConfigInspectRequest>(payload) {
                    Ok(request) => {
                        apply_config_inspect_request(&mut self.rail_scope, &mut self.page, request);
                        if self.permissions_granted {
                            show_self(true);
                        }
                        self.send_hello();
                        return true;
                    }
                    Err(error) => {
                        eprintln!("andamento-config: failed to parse inspect request: {error}");
                    }
                }
            }
        }
        if message.name == MSG_VIEW_MODEL {
            if let Some(payload) = message.payload.as_deref() {
                let started_at = Instant::now();
                match serde_json::from_str::<ControllerViewModel>(payload) {
                    Ok(model) => {
                        self.stats
                            .record_span_elapsed("json.decode-view-model", started_at);
                        self.model = Some(model);
                        self.pending_config = None;
                        return true;
                    }
                    Err(error) => {
                        eprintln!("andamento-config: failed to parse view model: {error}");
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
        if message.name == MSG_STATS_REPORT {
            if let Some(payload) = message.payload.as_deref() {
                let started_at = Instant::now();
                match serde_json::from_str::<PluginStatsSnapshot>(payload) {
                    Ok(snapshot) if snapshot.collection_id == self.stats_collection_id => {
                        self.stats
                            .record_span_elapsed("json.decode-stats-report", started_at);
                        self.stats_collection_pending = false;
                        self.stats_reports.push(snapshot);
                        self.stats_reports.sort_by(|a, b| {
                            a.plugin_kind
                                .cmp(&b.plugin_kind)
                                .then_with(|| a.plugin_id.cmp(&b.plugin_id))
                        });
                        return true;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("andamento-config: failed to parse stats report: {error}");
                    }
                }
            }
        }
        false
    }

    fn update(&mut self, event: Event) -> bool {
        self.stats.increment("update.total");
        match event {
            Event::Mouse(Mouse::LeftClick(row, col)) if row >= 0 => {
                self.stats.increment("update.mouse.left-click");
                self.handle_click(row as usize, col as usize)
            }
            Event::Mouse(Mouse::ScrollUp(_)) => {
                self.stats.increment("update.mouse.scroll-up");
                self.body_scroll_offset = self.body_scroll_offset.saturating_sub(1);
                true
            }
            Event::Mouse(Mouse::ScrollDown(_)) => {
                self.stats.increment("update.mouse.scroll-down");
                self.body_scroll_offset = self.body_scroll_offset.saturating_add(1);
                true
            }
            Event::Visible(is_visible) => {
                self.stats.increment("update.visible");
                if config_should_close_on_visibility(is_visible, self.close_on_hidden) {
                    close_self();
                }
                false
            }
            Event::PermissionRequestResult(status) => {
                self.stats.increment("update.permission-result");
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                let should_resync = permission_result_should_resync(self.permissions_granted);
                if should_resync {
                    self.send_hello();
                }
                should_resync
            }
            Event::TabUpdate(tabs) => {
                self.stats.increment("update.tab");
                let local_tabs = local_tabs_from_zellij(&tabs);
                if self.local_tabs == local_tabs {
                    return false;
                }
                self.local_tabs = local_tabs;
                if let Some(pane_manifest) = self.last_pane_manifest.clone() {
                    self.observe_own_config_placement(&pane_manifest);
                }
                false
            }
            Event::PaneUpdate(pane_manifest) => {
                self.stats.increment("update.pane");
                self.last_pane_manifest = Some(pane_manifest);
                if let Some(pane_manifest) = self.last_pane_manifest.clone() {
                    self.observe_own_config_placement(&pane_manifest);
                }
                false
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.stats.increment("render.total");
        let started_at = Instant::now();
        let config = self
            .pending_config
            .or_else(|| self.model.as_ref().map(|model| model.config))
            .unwrap_or_default();
        let rendered = render_config_with_scope(
            config,
            self.model.as_ref(),
            self.page,
            self.rail_scope.as_deref(),
            rows,
            cols,
            &self.stats_reports,
            self.stats_collection_pending,
            self.body_scroll_offset,
        );
        self.hit_regions = rendered.hit_regions;
        // Hit regions describe the node this frame was drawn for. A controller
        // push can replace the model — and with it the inspected node — before
        // the click arrives, so record what they refer to.
        self.rendered_for_node = Some(self.current_inspected_node());
        self.body_scroll_offset = rendered.body_scroll_offset;
        print!("{}", rendered.lines.join("\n"));
        for ribbon in rendered.ribbons {
            print!("{}", ribbon.to_zellij_component());
        }
        self.stats.record_span_elapsed("render.config", started_at);
    }
}

impl RibbonOverlay {
    fn to_zellij_component(&self) -> String {
        let mut text = Text::new(self.label.as_str());
        if self.selected {
            text = text.selected();
        }
        serialize_ribbon_with_coordinates(&text, self.col, self.row, Some(self.width), Some(1))
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

    fn send_hello(&self) {
        let (Some(plugin_id), Some(client_id)) = (self.own_plugin_id, self.own_client_id) else {
            return;
        };
        let Some(payload) = config_editor_hello_payload(
            plugin_id,
            client_id,
            self.own_plugin_placement
                .clone()
                .unwrap_or(PluginPlacement::Unknown),
        ) else {
            return;
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_CONFIG_EDITOR_HELLO)
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
        pipe_message_to_plugin(
            self.controller_message(MSG_REQUEST_STATE)
                .with_destination_client_id(client_id),
        );
    }

    fn observe_own_config_placement(&mut self, pane_manifest: &PaneManifest) {
        let Some(plugin_id) = self.own_plugin_id else {
            return;
        };
        let next_placement = own_plugin_tab_placement(pane_manifest, &self.local_tabs, plugin_id);
        if next_placement.is_some() && self.own_plugin_placement != next_placement {
            self.own_plugin_placement = next_placement;
            self.send_hello();
        }
    }

    fn handle_click(&mut self, row: usize, col: usize) -> bool {
        let Some(hit) = self
            .hit_regions
            .iter()
            .find(|hit| hit.row == row && col >= hit.col_start && col <= hit.col_end)
        else {
            return false;
        };
        if let ConfigAction::SetPage(page) = hit.action {
            self.page = page;
            return true;
        }
        if hit.action == ConfigAction::CollectStats {
            self.collect_stats();
            return true;
        }
        if let ConfigAction::SetInspectedMetadata(state) = hit.action {
            self.set_inspected_metadata(state);
            return true;
        }
        if let ConfigAction::SetNodeVariable { variable, value } = hit.action {
            self.set_node_variable(variable, value);
            return true;
        }
        let mut config = self
            .pending_config
            .or_else(|| self.model.as_ref().map(|model| model.config))
            .unwrap_or_default();
        config = apply_config_action(config, hit.action);
        self.pending_config = Some(config);
        let Ok(payload) = serde_json::to_string(&config) else {
            return true;
        };
        let Some(client_id) = self.own_client_id else {
            return true;
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_SET_RAIL_CONFIG)
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
        true
    }

    fn collect_stats(&mut self) {
        let (Some(plugin_id), Some(client_id)) = (self.own_plugin_id, self.own_client_id) else {
            return;
        };
        self.stats_collection_id = self.stats_collection_id.saturating_add(1);
        self.stats_collection_pending = true;
        self.stats_reports.clear();
        self.body_scroll_offset = 0;
        let request = StatsCollectRequest {
            requester: RendererHello {
                plugin_id,
                client_id,
            },
            collection_id: self.stats_collection_id,
        };
        let Ok(payload) = serde_json::to_string(&request) else {
            return;
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_STATS_COLLECT)
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
    }

    fn send_stats_report_to(&self, requester: RendererHello, collection_id: u64) {
        let (Some(plugin_id), Some(client_id)) = (self.own_plugin_id, self.own_client_id) else {
            return;
        };
        let snapshot = self.stats.snapshot(
            collection_id,
            RendererHello {
                plugin_id,
                client_id,
            },
            "config",
        );
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

    fn set_inspected_metadata(&self, state: Option<MetadataTriState>) {
        let Some(request) = self.metadata_visibility_request(state) else {
            return;
        };
        let client_id = request.client_id;
        let Ok(payload) = serde_json::to_string(&request) else {
            return;
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_SET_METADATA_VISIBILITY)
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
    }

    /// Resolve a metadata tri-state click into a set request.
    ///
    /// Split from the send for the same reason as `node_variable_request`: the
    /// click is only valid while the frame it was aimed at is still on screen,
    /// and that is worth a test at this call site rather than only where the
    /// shared guard is defined.
    fn metadata_visibility_request(
        &self,
        state: Option<MetadataTriState>,
    ) -> Option<MetadataVisibilitySetRequest> {
        Some(MetadataVisibilitySetRequest {
            client_id: self.own_client_id?,
            node_key: self.clicked_node()?,
            state,
        })
    }

    fn set_node_variable(&self, variable: usize, value: Option<usize>) {
        let Some(request) = self.node_variable_request(variable, value) else {
            return;
        };
        let client_id = request.client_id;
        let Ok(payload) = serde_json::to_string(&request) else {
            return;
        };
        pipe_message_to_plugin(
            self.controller_message(MSG_SET_NODE_VARIABLE)
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
    }

    /// Resolve a clicked control back into a set request.
    ///
    /// Split from the send so the resolution is testable: the indices come from
    /// a rendered frame and the model may have moved on since, so a stale index
    /// must resolve to nothing rather than to a different variable or value.
    fn node_variable_request(
        &self,
        variable: usize,
        value: Option<usize>,
    ) -> Option<NodeVariableSetRequest> {
        let client_id = self.own_client_id?;
        let node_key = match self.clicked_node()? {
            NodeKey::Root => NodeKey::Root,
            NodeKey::Placement(key) => NodeKey::Placement(key),
            NodeKey::Tab(_) | NodeKey::Entity(_) => return None,
        };
        let declaration = self
            .model
            .as_ref()
            .and_then(|model| {
                model
                    .template_config
                    .effective_variables
                    .iter()
                    .find(|variables| variables.node == node_key)
            })
            .and_then(|variables| variables.declarations.get(variable))?;
        let value = match value {
            None => None,
            Some(index) => Some(declaration.values.get(index)?.clone()),
        };
        Some(NodeVariableSetRequest {
            client_id,
            node_key,
            name: declaration.name.clone(),
            value,
        })
    }

    /// The node a click applies to, or `None` if the frame it was aimed at is
    /// no longer the one on screen.
    ///
    /// Hit regions are captured at render and consumed on a later event. A
    /// controller push in between swaps the model, so resolving a click against
    /// whatever is inspected *now* can silently apply it to a different node
    /// that happens to be the same shape.
    fn clicked_node(&self) -> Option<NodeKey> {
        let current = self.current_inspected_node();
        (self.rendered_for_node.as_ref() == Some(&current)).then_some(current)
    }

    fn current_inspected_node(&self) -> NodeKey {
        self.model
            .as_ref()
            .and_then(|model| model.inspected_node.clone())
            .or_else(|| self.rail_scope.as_deref().and_then(parse_scope_node_key))
            .unwrap_or(NodeKey::Root)
    }
}

fn apply_config_action(mut config: RailConfig, action: ConfigAction) -> RailConfig {
    match action {
        ConfigAction::SetPage(_) => {}
        ConfigAction::SetStructure(structure) => config.structure = structure,
        ConfigAction::CollectStats
        | ConfigAction::SetInspectedMetadata(_)
        | ConfigAction::SetNodeVariable { .. } => {}
    }
    config
}

fn permission_result_should_resync(granted: bool) -> bool {
    granted
}

fn config_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn config_should_close_on_visibility(is_visible: bool, close_on_hidden: bool) -> bool {
    close_on_hidden && !is_visible
}

fn config_scope(configuration: &BTreeMap<String, String>) -> Option<String> {
    configuration
        .get(CONFIG_RAIL_SCOPE)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn launch_config_placement(configuration: &BTreeMap<String, String>) -> Option<PluginPlacement> {
    let tab_id = configuration
        .get(CONFIG_ORIGIN_TAB_ID)?
        .trim()
        .parse()
        .ok()?;
    let pane_kind = match configuration
        .get(CONFIG_PANE_KIND)
        .map(|value| value.trim())
    {
        Some("floating") => PluginPaneKind::Floating,
        Some("tiled") => PluginPaneKind::Tiled,
        _ => return None,
    };
    Some(PluginPlacement::Tab { tab_id, pane_kind })
}

fn initial_page_for_scope(rail_scope: Option<&str>) -> ConfigPage {
    if rail_scope.is_some() {
        ConfigPage::Inspect
    } else {
        ConfigPage::default()
    }
}

fn apply_config_inspect_request(
    rail_scope: &mut Option<String>,
    page: &mut ConfigPage,
    request: ConfigInspectRequest,
) {
    *rail_scope = Some(inspect_scope_label(&request.node_key));
    *page = ConfigPage::Inspect;
}

fn inspect_scope_label(key: &NodeKey) -> String {
    match key {
        NodeKey::Root => "root".to_owned(),
        NodeKey::Tab(tab_id) => format!("tab:{tab_id}"),
        NodeKey::Entity(entity) => format!("entity:{}:{}", entity.kind, entity.id),
        NodeKey::Placement(key) => format!(
            "placement:{}",
            key.0
                .iter()
                .map(|segment| format!(
                    "{}={}:{}",
                    segment.loop_name, segment.entity.kind, segment.entity.id
                ))
                .collect::<Vec<_>>()
                .join("/")
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectTargetKind {
    Root,
    Tab,
    Entity,
    Missing,
}

struct InspectTargetView<'a> {
    label: String,
    kind: InspectTargetKind,
    node_key: NodeKey,
    tab: Option<&'a TabCard>,
    placement: Option<&'a andamento_shared::presentation::PlacementNode>,
    metadata: BTreeMap<String, MetadataEntry>,
    sources: BTreeMap<String, Vec<MetadataSourceEntry>>,
}

fn inspect_target_for<'a>(
    inspected_node: Option<&NodeKey>,
    rail_scope: Option<&str>,
    model: &'a ControllerViewModel,
) -> InspectTargetView<'a> {
    let node_key = inspected_node
        .cloned()
        .or_else(|| rail_scope.and_then(parse_scope_node_key))
        .unwrap_or(NodeKey::Root);
    let metadata = metadata_for_node(model, &node_key);
    let sources = sources_for_node(model, &node_key);
    match node_key.clone() {
        NodeKey::Root => InspectTargetView {
            label: "Root".to_owned(),
            kind: InspectTargetKind::Root,
            node_key,
            tab: None,
            placement: None,
            metadata,
            sources,
        },
        NodeKey::Tab(tab_id) => match model.tab_by_id(tab_id) {
            Some(tab) => InspectTargetView {
                label: format!("Tab \"{}\" #{}", tab.name, tab.tab_id),
                kind: InspectTargetKind::Tab,
                node_key,
                tab: Some(tab),
                placement: None,
                metadata,
                sources,
            },
            None => InspectTargetView {
                label: format!("Missing tab #{tab_id}"),
                kind: InspectTargetKind::Missing,
                node_key,
                tab: None,
                placement: None,
                metadata,
                sources,
            },
        },
        NodeKey::Entity(entity) => InspectTargetView {
            label: format!("{} {}", entity.kind, entity.id),
            kind: InspectTargetKind::Entity,
            node_key,
            tab: None,
            placement: None,
            metadata,
            sources,
        },
        NodeKey::Placement(key) => {
            let entity = key.0.last().map(|segment| &segment.entity);
            InspectTargetView {
                label: entity
                    .map(|entity| format!("Placement of {} {}", entity.kind, entity.id))
                    .unwrap_or_else(|| "Placement".to_owned()),
                kind: InspectTargetKind::Entity,
                node_key,
                tab: None,
                placement: model
                    .presentation
                    .as_ref()
                    .and_then(|surface| surface.node(&key)),
                metadata,
                sources,
            }
        }
    }
}

fn parse_scope_node_key(scope: &str) -> Option<NodeKey> {
    if scope == "root" {
        return Some(NodeKey::Root);
    }
    parse_tab_scope(scope).map(NodeKey::Tab)
}

fn metadata_for_node(
    model: &ControllerViewModel,
    node_key: &NodeKey,
) -> BTreeMap<String, MetadataEntry> {
    resolved_metadata_for_node(model, node_key)
        .map(|metadata| metadata.values.clone())
        .unwrap_or_default()
}

fn sources_for_node(
    model: &ControllerViewModel,
    node_key: &NodeKey,
) -> BTreeMap<String, Vec<MetadataSourceEntry>> {
    resolved_metadata_for_node(model, node_key)
        .map(|metadata| metadata.source_entries.clone())
        .unwrap_or_default()
}

fn resolved_metadata_for_node<'a>(
    model: &'a ControllerViewModel,
    node_key: &NodeKey,
) -> Option<&'a andamento_shared::ResolvedMetadata> {
    let target = match node_key {
        NodeKey::Root => ResolvedMetadataTarget::Root,
        NodeKey::Tab(tab_id) => ResolvedMetadataTarget::Tab(*tab_id),
        NodeKey::Entity(entity) => ResolvedMetadataTarget::Entity(entity.clone()),
        NodeKey::Placement(key) => ResolvedMetadataTarget::Entity(key.0.last()?.entity.clone()),
    };
    model
        .resolved_metadata
        .iter()
        .find(|metadata| metadata.target == target)
}

#[cfg(test)]
fn render_config(
    config: RailConfig,
    model: Option<&ControllerViewModel>,
    page: ConfigPage,
    rows: usize,
    cols: usize,
    stats_reports: &[PluginStatsSnapshot],
    stats_collection_pending: bool,
    body_scroll_offset: usize,
) -> RenderedConfig {
    render_config_with_scope(
        config,
        model,
        page,
        None,
        rows,
        cols,
        stats_reports,
        stats_collection_pending,
        body_scroll_offset,
    )
}

fn render_config_with_scope(
    config: RailConfig,
    model: Option<&ControllerViewModel>,
    page: ConfigPage,
    rail_scope: Option<&str>,
    rows: usize,
    cols: usize,
    stats_reports: &[PluginStatsSnapshot],
    stats_collection_pending: bool,
    body_scroll_offset: usize,
) -> RenderedConfig {
    if rows == 0 || cols == 0 {
        return RenderedConfig {
            lines: vec![],
            hit_regions: vec![],
            ribbons: vec![],
            body_scroll_offset: 0,
        };
    }
    let mut tab_frame = ConfigUiFrame::new(1, cols);
    push_tab_row(&mut tab_frame, page);
    let mut body_frame = ConfigUiFrame::new(usize::MAX, cols);
    match page {
        ConfigPage::Settings => push_settings_page(&mut body_frame, config, model),
        ConfigPage::Inspect => push_inspect_page(&mut body_frame, model, rail_scope),
        ConfigPage::Templates => push_templates_page(&mut body_frame, model),
        ConfigPage::Stats => {
            push_stats_page(&mut body_frame, stats_reports, stats_collection_pending);
        }
    }
    render_with_scroll(tab_frame, body_frame, rows, cols, body_scroll_offset)
}

fn render_with_scroll(
    tab_frame: ConfigUiFrame,
    body_frame: ConfigUiFrame,
    rows: usize,
    cols: usize,
    scroll_offset: usize,
) -> RenderedConfig {
    let body_rows = rows.saturating_sub(1);
    let max_scroll_offset = body_frame.lines.len().saturating_sub(body_rows);
    let effective_scroll_offset = scroll_offset.min(max_scroll_offset);
    let mut lines = Vec::with_capacity(rows);
    lines.push(
        tab_frame
            .lines
            .first()
            .cloned()
            .unwrap_or_else(|| " ".repeat(cols)),
    );
    for line in body_frame
        .lines
        .iter()
        .skip(effective_scroll_offset)
        .take(body_rows)
    {
        lines.push(line.clone());
    }
    while lines.len() < rows {
        lines.push(" ".repeat(cols));
    }
    let mut hit_regions = tab_frame.hit_regions;
    let mut ribbons = tab_frame.ribbons;
    hit_regions.extend(body_frame.hit_regions.into_iter().filter_map(|mut hit| {
        let visible_end = effective_scroll_offset.saturating_add(body_rows);
        if hit.row < effective_scroll_offset || hit.row >= visible_end {
            return None;
        }
        hit.row = hit
            .row
            .saturating_sub(effective_scroll_offset)
            .saturating_add(1);
        Some(hit)
    }));
    ribbons.extend(body_frame.ribbons.into_iter().filter_map(|mut ribbon| {
        let visible_end = effective_scroll_offset.saturating_add(body_rows);
        if ribbon.row < effective_scroll_offset || ribbon.row >= visible_end {
            return None;
        }
        ribbon.row = ribbon
            .row
            .saturating_sub(effective_scroll_offset)
            .saturating_add(1);
        Some(ribbon)
    }));
    RenderedConfig {
        lines,
        hit_regions,
        ribbons,
        body_scroll_offset: effective_scroll_offset,
    }
}

fn push_inspect_page(
    frame: &mut ConfigUiFrame,
    model: Option<&ControllerViewModel>,
    rail_scope: Option<&str>,
) {
    let Some(model) = model else {
        frame.push_plain("Inspect");
        frame.push_blank();
        frame.push_plain("no controller state yet");
        return;
    };
    let target = inspect_target_for(model.inspected_node.as_ref(), rail_scope, model);

    push_inspect_header(frame, &target);
    frame.push_blank();
    push_inspect_identity_section(frame, &target);
    frame.push_blank();
    push_section_header(frame, "Unmatched catalog entities");
    push_key_value(frame, "count", &model.unmatched_entities.len().to_string());
    for entity in &model.unmatched_entities {
        frame.push_plain(&format!("{}: {}", entity.kind, entity.id));
    }
    frame.push_blank();
    push_inspect_options_section(frame, model, &target);
    frame.push_blank();
    push_inspect_variables_section(frame, model, &target);
    frame.push_blank();
    push_inspect_metadata_section(frame, &target);
    frame.push_blank();
    push_inspect_sources_section(frame, &target);
    frame.push_blank();
    push_inspect_templates_section(frame, &target);
}

fn push_inspect_variables_section(
    frame: &mut ConfigUiFrame,
    model: &ControllerViewModel,
    target: &InspectTargetView<'_>,
) {
    push_section_header(frame, "Variables");
    let Some(variables) = model
        .template_config
        .effective_variables
        .iter()
        .find(|variables| variables.node == target.node_key)
    else {
        frame.push_plain("  <none>");
        return;
    };
    for (name, effective) in &variables.values {
        frame.push_plain(&format!("  {name} = {}", effective.value));
        frame.push_plain(&format!(
            "    {} at {:?} [{}]",
            effective.provenance.setter,
            effective.provenance.ancestor,
            effective.provenance.origin.label(),
        ));
        for overridden in &effective.overridden {
            frame.push_plain(&format!(
                "    overrides {} at {:?} [{}]",
                overridden.setter,
                overridden.ancestor,
                overridden.origin.label(),
            ));
        }
    }
}

fn parse_tab_scope(scope: &str) -> Option<u64> {
    scope.strip_prefix("tab:")?.parse().ok()
}

fn push_inspect_header(frame: &mut ConfigUiFrame, target: &InspectTargetView<'_>) {
    frame.push_plain(&format!("Inspect: {}", target.label));
}

fn push_inspect_identity_section(frame: &mut ConfigUiFrame, target: &InspectTargetView<'_>) {
    push_section_header(frame, "Identity");
    push_key_value(frame, "type", inspect_target_kind_label(target.kind));
    match target.kind {
        InspectTargetKind::Root => {
            push_key_value(frame, "scope", "session");
        }
        InspectTargetKind::Tab => {
            if let Some(tab) = target.tab {
                push_key_value(frame, "name", &tab.name);
                push_key_value(frame, "tab id", &tab.tab_id.to_string());
                push_key_value(frame, "position", &tab.position.to_string());
                push_key_value(frame, "active", if tab.active { "yes" } else { "no" });
                push_key_value(frame, "pinned", if tab.pinned { "yes" } else { "no" });
                if let Some(active_pane) = tab.active_pane.as_ref() {
                    push_key_value(frame, "pane", &format_pane_target(active_pane));
                }
            }
        }
        InspectTargetKind::Missing => {
            push_key_value(frame, "state", "missing");
        }
        InspectTargetKind::Entity => {
            let entity = match &target.node_key {
                NodeKey::Entity(entity) => Some(entity),
                NodeKey::Placement(key) => key.0.last().map(|segment| &segment.entity),
                _ => None,
            };
            if let Some(entity) = entity {
                push_key_value(frame, "kind", &entity.kind);
                push_key_value(frame, "id", &entity.id);
            }
        }
    }
}

fn push_inspect_options_section(
    frame: &mut ConfigUiFrame,
    model: &ControllerViewModel,
    target: &InspectTargetView<'_>,
) {
    push_section_header(frame, "Options");
    // One row per node variable declared in scope here, so the shared column
    // width has to come from the names actually rendered. A hardcoded
    // placeholder would misalign every declaration that is not its length, and
    // silently — the control still works, it just stops lining up.
    let node_variables = model
        .template_config
        .effective_variables
        .iter()
        .find(|variables| variables.node == target.node_key);
    let declarations = node_variables
        .map(|variables| variables.declarations.as_slice())
        .unwrap_or_default();
    let label_width = std::iter::once("Metadata")
        .chain(
            declarations
                .iter()
                .map(|declaration| declaration.name.as_str()),
        )
        .map(|label| label.width())
        .max()
        .unwrap_or(0);
    let explicit_state = model
        .metadata_controls
        .per_node
        .get(&target.node_key)
        .copied();
    push_segmented_form_choice(
        frame,
        "Metadata",
        label_width,
        &[
            (
                "inherit",
                explicit_state.is_none(),
                ConfigAction::SetInspectedMetadata(None),
            ),
            (
                "hidden",
                explicit_state == Some(MetadataTriState::Clean),
                ConfigAction::SetInspectedMetadata(Some(MetadataTriState::Clean)),
            ),
            (
                "item",
                explicit_state == Some(MetadataTriState::Meta),
                ConfigAction::SetInspectedMetadata(Some(MetadataTriState::Meta)),
            ),
            (
                "subtree",
                explicit_state == Some(MetadataTriState::MetaChildren),
                ConfigAction::SetInspectedMetadata(Some(MetadataTriState::MetaChildren)),
            ),
        ],
    );
    // Node variables render from their declarations: one segmented control per
    // variable in scope at this node, offering inherit plus its allowed values.
    for (variable, declaration) in declarations.iter().enumerate() {
        let effective =
            node_variables.and_then(|variables| variables.values.get(&declaration.name));
        let explicit = effective.is_some_and(|value| {
            value.provenance.ancestor == target.node_key
                && value.provenance.setter == NODE_VARIABLE_CONFIG_OVERRIDE_SETTER
        });
        let mut options: Vec<(&str, bool, ConfigAction)> = vec![(
            "inherit",
            !explicit,
            ConfigAction::SetNodeVariable {
                variable,
                value: None,
            },
        )];
        for (value, allowed) in declaration.values.iter().enumerate() {
            options.push((
                allowed.as_str(),
                explicit && effective.is_some_and(|current| &current.value == allowed),
                ConfigAction::SetNodeVariable {
                    variable,
                    value: Some(value),
                },
            ));
        }
        push_segmented_form_choice(frame, &declaration.name, label_width, &options);
    }
}

fn push_inspect_metadata_section(frame: &mut ConfigUiFrame, target: &InspectTargetView<'_>) {
    push_section_header(frame, "Metadata");
    if target.metadata.is_empty() {
        frame.push_plain("  <none>");
        return;
    }
    frame.push_plain(&format!(
        "{:<22} {:<30} {:>7} {:>6}",
        "key", "value", "updated", "ttl"
    ));
    for (key, entry) in &target.metadata {
        frame.push_plain(&format!(
            "{:<22} {:<30} {:>7} {:>6}",
            truncate_to_width(key, 22),
            truncate_to_width(&format_metadata_value(&entry.value), 30),
            entry.updated_at,
            format_ttl(entry.ttl_ms)
        ));
    }
}

fn push_inspect_sources_section(frame: &mut ConfigUiFrame, target: &InspectTargetView<'_>) {
    push_section_header(frame, "Sources");
    if target.sources.is_empty() {
        frame.push_plain("  <none>");
        return;
    }
    frame.push_plain(&format!(
        "{:<22} {:<22} {:<26} {:>6} {:>5} {:>5}",
        "key", "source", "value", "ttl", "prec", "ord"
    ));
    for (key, entries) in &target.sources {
        for source in entries {
            frame.push_plain(&format!(
                "{:<22} {:<22} {:<26} {:>6} {:>5} {:>5}",
                truncate_to_width(key, 22),
                truncate_to_width(&source.source_id, 22),
                truncate_to_width(&format_metadata_value(&source.entry.value), 26),
                format_ttl(source.entry.ttl_ms),
                source.entry.precedence,
                source.entry.ordinal,
            ));
        }
    }
}

fn push_inspect_templates_section(frame: &mut ConfigUiFrame, target: &InspectTargetView<'_>) {
    push_section_header(frame, "Templates");
    let Some(node) = target.placement else {
        frame.push_plain("  <none>");
        return;
    };
    for (label, content) in [("line", &node.content), ("detail", &node.detail)] {
        push_key_value(
            frame,
            label,
            content.template_name.as_deref().unwrap_or("<none>"),
        );
        for line in content.effective_kdl.lines() {
            frame.push_plain(line);
        }
        if let Some(error) = &content.error {
            frame.push_plain(error);
        }
    }
}

fn push_key_value(frame: &mut ConfigUiFrame, key: &str, value: &str) {
    frame.push_plain(&format!("{key:<9} {value}"));
}

fn push_section_header(frame: &mut ConfigUiFrame, title: &str) {
    frame.push_plain(&section_divider(title, frame.cols));
}

fn section_divider(title: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let prefix = format!("── {title} ");
    if prefix.width() >= width {
        return truncate_to_width(&prefix, width);
    }
    format!("{prefix}{}", "─".repeat(width - prefix.width()))
}

fn push_button(frame: &mut ConfigUiFrame, label: &str, action: ConfigAction) {
    let text = format!("● {label}");
    let Some(row) = frame.push_plain(&text) else {
        return;
    };
    frame.add_hit(row, 0, text.width().saturating_sub(1), action);
}

fn inspect_target_kind_label(kind: InspectTargetKind) -> &'static str {
    match kind {
        InspectTargetKind::Root => "Root",
        InspectTargetKind::Tab => "Tab",
        InspectTargetKind::Missing => "Missing",
        InspectTargetKind::Entity => "Entity",
    }
}

fn format_ttl(ttl_ms: Option<u64>) -> String {
    match ttl_ms {
        Some(ttl_ms) => format!("{}s", ttl_ms / 1000),
        None => "-".to_owned(),
    }
}

fn format_pane_target(target: &andamento_shared::PaneTarget) -> String {
    match target {
        andamento_shared::PaneTarget::Terminal(id) => format!("terminal:{id}"),
        andamento_shared::PaneTarget::Plugin(id) => format!("plugin:{id}"),
    }
}

fn push_stats_page(
    frame: &mut ConfigUiFrame,
    stats_reports: &[PluginStatsSnapshot],
    stats_collection_pending: bool,
) {
    push_section_header(frame, "Stats");
    push_button(frame, "collect stats", ConfigAction::CollectStats);
    frame.push_blank();
    if stats_reports.is_empty() {
        if stats_collection_pending {
            frame.push_plain("collecting...");
        } else {
            frame.push_plain("no stats collected yet");
        }
        return;
    }
    for report in stats_reports {
        frame.push_plain(&format!(
            "{} #{} c{}",
            report.plugin_kind, report.plugin_id, report.client_id
        ));
        for (name, count) in &report.counters {
            frame.push_plain(&format!("  {name}: {count}"));
        }
        for span in &report.spans {
            let avg_us = if span.count == 0 {
                0
            } else {
                span.total_us / span.count
            };
            frame.push_plain(&format!(
                "  {}: n={} total={}us avg={}us max={}us",
                span.name, span.count, span.total_us, avg_us, span.max_us
            ));
        }
    }
}

fn push_settings_page(
    frame: &mut ConfigUiFrame,
    config: RailConfig,
    model: Option<&ControllerViewModel>,
) {
    push_segmented_choice(
        frame,
        "structure",
        &[
            (
                "joined cells",
                config.structure == RailStructure::JoinedCells,
                ConfigAction::SetStructure(RailStructure::JoinedCells),
            ),
            (
                "split around active",
                config.structure == RailStructure::SplitAroundActive,
                ConfigAction::SetStructure(RailStructure::SplitAroundActive),
            ),
            (
                "box per tab",
                config.structure == RailStructure::BoxPerTab,
                ConfigAction::SetStructure(RailStructure::BoxPerTab),
            ),
        ],
    );
    frame.push_blank();
    push_cwd_metadata(frame, model);
    frame.push_blank();
    frame.push_plain("click to apply");
}

fn push_templates_page(frame: &mut ConfigUiFrame, model: Option<&ControllerViewModel>) {
    frame.push_plain("templates");
    let Some(model) = model else {
        frame.push_plain("no controller state yet");
        return;
    };
    let diagnostics = &model.template_config;
    frame.push_plain(&format!(
        "state: {}",
        template_config_state_text(diagnostics.state)
    ));
    frame.push_plain(&format!(
        "path: {}",
        diagnostics.path.as_deref().unwrap_or("<not configured>")
    ));
    frame.push_plain(&format!("template count: {}", diagnostics.template_count));
    if !diagnostics.template_names.is_empty() {
        frame.push_plain("template names");
        for name in &diagnostics.template_names {
            frame.push_plain(&format!("  {name}"));
        }
    }
    if let Some(error) = diagnostics.last_error.as_ref() {
        frame.push_plain("last error");
        frame.push_plain(&format!("  {error}"));
    }
    if !diagnostics.warnings.is_empty() {
        frame.push_plain("warnings");
        for warning in &diagnostics.warnings {
            frame.push_plain(&format!("  {warning}"));
        }
    }
    frame.push_blank();
    frame.push_plain("resolved templates");
    fn show(frame: &mut ConfigUiFrame, nodes: &[andamento_shared::presentation::PlacementNode]) {
        for node in nodes {
            frame.push_plain(&format!(
                "{}: {}",
                node.label,
                node.content.template_name.as_deref().unwrap_or("<none>")
            ));
            show(frame, &node.children);
        }
    }
    if let Some(surface) = &model.presentation {
        for section in &surface.sections {
            show(frame, &section.nodes);
        }
    }
}

fn template_config_state_text(state: andamento_shared::TemplateConfigState) -> &'static str {
    match state {
        andamento_shared::TemplateConfigState::NotConfigured => "not configured",
        andamento_shared::TemplateConfigState::PendingPermission => "pending permission",
        andamento_shared::TemplateConfigState::Loaded => "loaded",
        andamento_shared::TemplateConfigState::Error => "error",
    }
}

fn push_tab_row(frame: &mut ConfigUiFrame, page: ConfigPage) {
    let rendered = render_tab_row(page, frame.cols);
    let Some(row) = frame.push_plain(&rendered.line) else {
        return;
    };
    for mut hit in rendered.hit_regions {
        hit.row = row;
        frame.hit_regions.push(hit);
    }
    for mut ribbon in rendered.ribbons {
        ribbon.row = row;
        frame.add_ribbon(ribbon);
    }
}

fn render_tab_row(page: ConfigPage, cols: usize) -> RenderedTabRow {
    let tabs = [
        (ConfigPage::Settings, "settings"),
        (ConfigPage::Inspect, "inspect"),
        (ConfigPage::Templates, "templates"),
        (ConfigPage::Stats, "stats"),
    ];
    let items = tabs
        .iter()
        .map(|(tab_page, label)| segment_bar::SegmentItem {
            label: (*label).to_owned(),
            active: *tab_page == page,
        })
        .collect::<Vec<_>>();
    let (line, segment_hits) = segment_bar::render(&items, &segment_bar::ZellijRibbonStyle, cols);
    let mut hit_regions = vec![];
    let mut ribbons = vec![];
    for hit in segment_hits {
        let Some((tab_page, label)) = tabs.get(hit.index) else {
            continue;
        };
        hit_regions.push(HitRegion {
            row: 0,
            col_start: hit.col_start,
            col_end: hit.col_end,
            action: ConfigAction::SetPage(*tab_page),
        });
        ribbons.push(RibbonOverlay {
            row: 0,
            col: hit.col_start,
            width: hit.col_end.saturating_sub(hit.col_start).saturating_add(1),
            label: (*label).to_owned(),
            selected: *tab_page == page,
        });
    }
    if let Some(last_ribbon) = ribbons.last_mut() {
        last_ribbon.width = cols.saturating_sub(last_ribbon.col).max(last_ribbon.width);
    }
    RenderedTabRow {
        line,
        hit_regions,
        ribbons,
    }
}

fn push_cwd_metadata(frame: &mut ConfigUiFrame, model: Option<&ControllerViewModel>) {
    frame.push_plain("cwd metadata");
    let Some(model) = model else {
        frame.push_plain("no controller state yet");
        return;
    };
    for tab in &model.tabs {
        let metadata = metadata_for_node(model, &NodeKey::Tab(tab.tab_id));
        let value = metadata
            .get("zellij.pane.cwd")
            .map(|v| format_metadata_value(&v.value))
            .unwrap_or_else(|| "<none>".into());
        frame.push_plain(&format!("{}: {}", tab.name, value));
    }
}

fn format_metadata_value(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Text(value) => value.clone(),
        MetadataValue::Bool(value) => value.to_string(),
        MetadataValue::Integer(value) => value.to_string(),
        MetadataValue::StringList(values) => values.join(","),
        MetadataValue::EntityRefs(values) => values
            .iter()
            .map(|value| format!("{}:{}", value.kind, value.id))
            .collect::<Vec<_>>()
            .join(","),
        MetadataValue::GroupPath(segments) => segments
            .iter()
            .map(|segment| {
                segment
                    .label
                    .clone()
                    .unwrap_or_else(|| segment.value.display())
            })
            .collect::<Vec<_>>()
            .join(" / "),
    }
}

fn push_segmented_choice(
    frame: &mut ConfigUiFrame,
    label: &str,
    options: &[(&str, bool, ConfigAction)],
) {
    const MIN_LABEL_WIDTH: usize = 9;
    push_segmented_form_choice(frame, label, MIN_LABEL_WIDTH.max(label.width()), options);
}

fn push_segmented_form_choice(
    frame: &mut ConfigUiFrame,
    label: &str,
    label_width: usize,
    options: &[(&str, bool, ConfigAction)],
) {
    let label_width = label_width.max(label.width());
    let mut text = format!("{label:<label_width$}  ");
    let mut col = text.width();
    for (index, (option_label, selected, action)) in options.iter().enumerate() {
        if index > 0 {
            text.push_str("  ");
            col = col.saturating_add(2);
        }
        let segment = format!("{} {option_label}", if *selected { "●" } else { "○" });
        let segment_width = segment.width();
        let segment_start = col;
        text.push_str(&segment);
        col = col.saturating_add(segment_width);
        if segment_start < frame.cols {
            let row = frame.current_row();
            frame.add_hit(
                row,
                segment_start,
                segment_start
                    .saturating_add(segment_width)
                    .saturating_sub(1),
                *action,
            );
        }
    }
    frame.push_plain(&text);
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
fn display_width_slice(text: &str, start: usize, end: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    let mut out = String::new();
    let mut col: usize = 0;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0).max(1);
        let ch_start = col;
        let ch_end = col.saturating_add(ch_width).saturating_sub(1);
        if ch_end >= start && ch_start <= end {
            out.push(ch);
        }
        col = col.saturating_add(ch_width);
        if col > end {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model_with_tab(tab_id: u64, name: &str) -> ControllerViewModel {
        ControllerViewModel {
            unmatched_entities: vec![],
            sort_mode: SortMode::Position,
            config: RailConfig::default(),
            template_config: andamento_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![TabCard {
                tab_id,
                position: 0,
                name: name.to_owned(),
                active: true,
                pinned: false,
                status: None,

                templates: andamento_shared::ResolvedTemplateSlots::default(),
                active_pane: None,
            }],

            resolved_metadata: vec![],
            observed_identities: vec![],

            metadata_controls: andamento_shared::MetadataControls::default(),
            inspected_node: None,

            collapsed_placements: vec![],
            sibling_orders: vec![],
            display_variables: vec![],
            display_variable_values: BTreeMap::new(),

            presentation: None,
        }
    }

    fn placement_model_with_declarations(
        declarations: Vec<andamento_shared::template_config::NodeVariableDefinition>,
    ) -> ControllerViewModel {
        let mut model = model_with_tab(1, "fixture");
        let key = andamento_shared::PlacementKey(vec![andamento_shared::PlacementSegment {
            loop_name: "project".into(),
            entity: andamento_shared::EntityRef {
                kind: "project".into(),
                id: "p".into(),
            },
        }]);
        model.inspected_node = Some(NodeKey::Placement(key.clone()));
        model.template_config.effective_variables =
            vec![andamento_shared::EffectiveNodeVariables {
                node: NodeKey::Placement(key),
                values: BTreeMap::new(),
                declarations,
            }];
        model
    }

    use andamento_shared::{MetadataValue, SortMode};

    #[test]
    fn inspect_lists_unmatched_catalog_count_and_ids() {
        let mut model = model_with_tab(7, "workspace");
        model.unmatched_entities = vec![
            andamento_shared::EntityRef {
                kind: "novel".into(),
                id: "unmatched-one".into(),
            },
            andamento_shared::EntityRef {
                kind: "repo".into(),
                id: "unmatched-two".into(),
            },
        ];
        let mut frame = ConfigUiFrame::new(100, 100);
        push_inspect_page(&mut frame, Some(&model), None);
        let text = frame.lines.join("\n");
        assert!(text.contains("Unmatched catalog entities"));
        assert!(frame
            .lines
            .iter()
            .any(|line| line.contains("count") && line.contains('2')));
        assert!(text.contains("novel: unmatched-one"));
        assert!(text.contains("repo: unmatched-two"));
    }

    #[test]
    fn renders_selected_config() {
        let rendered = render_config(
            RailConfig {
                structure: RailStructure::BoxPerTab,
                segment_between_color: None,
            },
            None,
            ConfigPage::Settings,
            22,
            90,
            &[],
            false,
            0,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim().ends_with("● box per tab")));
    }

    #[test]
    fn maps_click_rows_to_config_actions() {
        let rendered = render_config(
            RailConfig::default(),
            None,
            ConfigPage::Settings,
            22,
            90,
            &[],
            false,
            0,
        );

        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::SetStructure(RailStructure::BoxPerTab)));
    }

    #[test]
    fn settings_page_renders_enum_choices_inline() {
        let rendered = render_config(
            RailConfig {
                structure: RailStructure::BoxPerTab,
                segment_between_color: None,
            },
            None,
            ConfigPage::Settings,
            16,
            90,
            &[],
            false,
            0,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim()
                == "structure  ○ joined cells  ○ split around active  ● box per tab"));
    }

    #[test]
    fn settings_inline_choices_have_separate_click_targets() {
        let rendered = render_config(
            RailConfig::default(),
            None,
            ConfigPage::Settings,
            16,
            90,
            &[],
            false,
            0,
        );

        let structure_hits = rendered
            .hit_regions
            .iter()
            .filter(|hit| {
                matches!(
                    hit.action,
                    ConfigAction::SetStructure(RailStructure::JoinedCells)
                        | ConfigAction::SetStructure(RailStructure::SplitAroundActive)
                        | ConfigAction::SetStructure(RailStructure::BoxPerTab)
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(structure_hits.len(), 3);
        assert!(structure_hits
            .windows(2)
            .all(|pair| pair[0].row == pair[1].row && pair[0].col_end < pair[1].col_start));
    }

    #[test]
    fn config_tab_row_keeps_existing_pages_and_hit_regions() {
        let rendered = render_tab_row(ConfigPage::Templates, 60);
        let line = rendered.line;
        let hits = rendered.hit_regions;

        assert!(line.contains("settings"));
        assert!(line.contains("inspect"));
        assert!(line.contains("templates"));
        assert!(line.contains("stats"));
        assert!(
            !line.contains("TEMPLATES"),
            "active tab should be styled by the segment, not uppercased"
        );
        assert!(
            !line.contains(""),
            "config tabs should use a consistent Zellij ribbon shape"
        );
        assert!(line.starts_with(" settings  inspect "));
        assert_eq!(
            rendered
                .ribbons
                .iter()
                .map(|ribbon| (ribbon.label.as_str(), ribbon.selected))
                .collect::<Vec<_>>(),
            vec![
                ("settings", false),
                ("inspect", false),
                ("templates", true),
                ("stats", false)
            ]
        );
        let last_ribbon = rendered.ribbons.last().expect("last ribbon");
        assert_eq!(
            last_ribbon.col + last_ribbon.width,
            60,
            "final ribbon should fill the rest of the tab row"
        );
        assert_eq!(
            hits.iter().map(|hit| hit.action).collect::<Vec<_>>(),
            vec![
                ConfigAction::SetPage(ConfigPage::Settings),
                ConfigAction::SetPage(ConfigPage::Inspect),
                ConfigAction::SetPage(ConfigPage::Templates),
                ConfigAction::SetPage(ConfigPage::Stats),
            ]
        );
    }

    #[test]
    fn tab_row_hit_regions_match_rendered_segments() {
        let rendered = render_tab_row(ConfigPage::Inspect, 80);
        let line = rendered.line;
        let hits = rendered.hit_regions;
        let expectations = [
            (ConfigAction::SetPage(ConfigPage::Settings), "settings"),
            (ConfigAction::SetPage(ConfigPage::Inspect), "inspect"),
            (ConfigAction::SetPage(ConfigPage::Templates), "templates"),
            (ConfigAction::SetPage(ConfigPage::Stats), "stats"),
        ];

        for (action, label) in expectations {
            let hit = hits
                .iter()
                .find(|hit| hit.action == action)
                .expect("hit for tab");
            let segment = display_width_slice(&line, hit.col_start, hit.col_end);
            assert!(
                segment.contains(label),
                "segment {segment:?} should contain {label:?}"
            );
        }
    }

    #[test]
    fn config_scope_opens_inspect_when_present() {
        let mut configuration = BTreeMap::new();
        configuration.insert("rail_scope".to_owned(), " tab:7 ".to_owned());

        assert_eq!(config_scope(&configuration), Some("tab:7".to_owned()));
        assert_eq!(
            initial_page_for_scope(config_scope(&configuration).as_deref()),
            ConfigPage::Inspect
        );
    }

    #[test]
    fn launch_config_origin_tab_sets_initial_floating_placement() {
        let mut configuration = BTreeMap::new();
        configuration.insert("origin_tab_id".to_owned(), "7".to_owned());
        configuration.insert("pane_kind".to_owned(), "floating".to_owned());

        assert_eq!(
            launch_config_placement(&configuration),
            Some(PluginPlacement::Tab {
                tab_id: 7,
                pane_kind: PluginPaneKind::Floating,
            })
        );
    }

    #[test]
    fn config_inspect_request_updates_scope_and_page() {
        let request = ConfigInspectRequest {
            client_id: 4,
            origin_tab_id: 7,
            node_key: NodeKey::Tab(7),
            config_plugin_url: "andamento-config".to_owned(),
            controller_plugin_url: "andamento-controller".to_owned(),
        };
        let mut scope = None;
        let mut page = ConfigPage::Settings;

        apply_config_inspect_request(&mut scope, &mut page, request);

        assert_eq!(scope.as_deref(), Some("tab:7"));
        assert_eq!(page, ConfigPage::Inspect);
    }

    #[test]
    fn resolves_own_config_tab_placement_from_pane_manifest() {
        let pane_manifest = PaneManifest {
            panes: HashMap::from([(
                2,
                vec![PaneInfo {
                    id: 21,
                    is_plugin: true,
                    is_floating: false,
                    ..Default::default()
                }],
            )]),
        };
        let local_tabs = vec![ConfigLocalTab {
            tab_id: 7,
            position: 2,
        }];

        assert_eq!(
            own_plugin_tab_placement(&pane_manifest, &local_tabs, 21),
            Some(PluginPlacement::Tab {
                tab_id: 7,
                pane_kind: PluginPaneKind::Tiled,
            })
        );
    }

    #[test]
    fn resolves_floating_config_placement_from_pane_manifest() {
        let pane_manifest = PaneManifest {
            panes: HashMap::from([(
                3,
                vec![PaneInfo {
                    id: 22,
                    is_plugin: true,
                    is_floating: true,
                    ..Default::default()
                }],
            )]),
        };
        let local_tabs = vec![ConfigLocalTab {
            tab_id: 8,
            position: 3,
        }];

        assert_eq!(
            own_plugin_tab_placement(&pane_manifest, &local_tabs, 22),
            Some(PluginPlacement::Tab {
                tab_id: 8,
                pane_kind: PluginPaneKind::Floating,
            })
        );
    }

    #[test]
    fn config_editor_hello_payload_includes_registration_placement() {
        let payload = config_editor_hello_payload(
            22,
            4,
            PluginPlacement::Tab {
                tab_id: 8,
                pane_kind: PluginPaneKind::Floating,
            },
        )
        .unwrap();
        let decoded: PluginRegistrationHello = serde_json::from_str(&payload).unwrap();

        assert_eq!(
            decoded,
            PluginRegistrationHello {
                identity: RendererHello {
                    plugin_id: 22,
                    client_id: 4,
                },
                placement: PluginPlacement::Tab {
                    tab_id: 8,
                    pane_kind: PluginPaneKind::Floating,
                },
            }
        );
    }

    #[test]
    fn inspect_target_resolves_selected_tab() {
        let mut model = model_with_tab(7, "repo");
        model.inspected_node = Some(NodeKey::Tab(7));

        let target = inspect_target_for(model.inspected_node.as_ref(), None, &model);

        assert_eq!(target.node_key, NodeKey::Tab(7));
        assert_eq!(target.kind, InspectTargetKind::Tab);
        assert_eq!(target.label, "Tab \"repo\" #7");
        assert_eq!(target.tab.map(|tab| tab.name.as_str()), Some("repo"));
    }

    #[test]
    fn inspect_target_resolves_root() {
        let mut model = model_with_tab(7, "repo");
        model.inspected_node = Some(NodeKey::Root);

        let target = inspect_target_for(model.inspected_node.as_ref(), Some("tab:7"), &model);

        assert_eq!(target.node_key, NodeKey::Root);
        assert_eq!(target.kind, InspectTargetKind::Root);
        assert_eq!(target.label, "Root");
        assert!(target.tab.is_none());
    }

    #[test]
    fn inspect_target_handles_missing_tab() {
        let model = model_with_tab(7, "repo");

        let target = inspect_target_for(Some(&NodeKey::Tab(99)), None, &model);

        assert_eq!(target.node_key, NodeKey::Tab(99));
        assert_eq!(target.kind, InspectTargetKind::Missing);
        assert_eq!(target.label, "Missing tab #99");
        assert!(target.tab.is_none());
    }

    #[test]
    fn inspect_target_falls_back_to_rail_scope() {
        let model = model_with_tab(7, "repo");

        let target = inspect_target_for(None, Some("tab:7"), &model);

        assert_eq!(target.node_key, NodeKey::Tab(7));
        assert_eq!(target.kind, InspectTargetKind::Tab);
        assert_eq!(target.tab.map(|tab| tab.tab_id), Some(7));
    }

    fn child_layout_declaration() -> andamento_shared::template_config::NodeVariableDefinition {
        andamento_shared::template_config::NodeVariableDefinition {
            name: "child-layout".to_owned(),
            default: "cards".to_owned(),
            values: vec!["cards".to_owned(), "strip".to_owned()],
        }
    }

    fn option_column(lines: &[String], label: &str) -> Option<usize> {
        lines
            .iter()
            .find(|line| line.trim_start().starts_with(label))
            .and_then(|line| line.find("● inherit").or_else(|| line.find("○ inherit")))
    }

    #[test]
    fn a_stale_control_index_resolves_to_nothing_rather_than_a_foreign_variable() {
        // The indices in a hit region come from a rendered frame. The model can
        // move on before the click lands, so resolution must fail closed.
        let mut plugin = PluginState::default();
        plugin.own_client_id = Some(3);
        plugin.model = Some(placement_model_with_declarations(vec![
            child_layout_declaration(),
        ]));
        plugin.rendered_for_node = Some(plugin.current_inspected_node());

        let request = plugin
            .node_variable_request(0, Some(1))
            .expect("a live index resolves");
        assert_eq!(request.name, "child-layout");
        assert_eq!(request.value.as_deref(), Some("strip"));

        assert_eq!(
            plugin.node_variable_request(1, None),
            None,
            "a variable index past the declarations must not fall through to another variable"
        );
        assert_eq!(
            plugin.node_variable_request(0, Some(9)),
            None,
            "a value index past the allowed values must not set a foreign value"
        );

        // The same click against a model whose declarations have changed under
        // it resolves to the new declaration or to nothing, never to the old one.
        plugin.model = Some(placement_model_with_declarations(vec![
            andamento_shared::template_config::NodeVariableDefinition {
                name: "caption".to_owned(),
                default: "none".to_owned(),
                values: vec![],
            },
        ]));
        assert_eq!(
            plugin.node_variable_request(0, Some(1)),
            None,
            "a value index that was valid for the previous declaration must not carry over"
        );
    }

    #[test]
    fn a_stale_frame_click_is_dropped_on_the_metadata_control_too() {
        // Same guard as the variable path. Tested at this call site rather than
        // only where clicked_node is defined, so dropping the guard here would
        // fail a test rather than pass silently.
        let mut plugin = PluginState::default();
        plugin.own_client_id = Some(3);
        plugin.model = Some(placement_model_with_declarations(vec![
            child_layout_declaration(),
        ]));
        plugin.rendered_for_node = Some(plugin.current_inspected_node());

        assert!(
            plugin
                .metadata_visibility_request(Some(MetadataTriState::Meta))
                .is_some(),
            "a click against the frame on screen resolves"
        );

        plugin.rendered_for_node = Some(NodeKey::Root);
        assert_eq!(
            plugin.metadata_visibility_request(Some(MetadataTriState::Meta)),
            None,
            "the click was aimed at a frame that is no longer on screen"
        );
    }

    #[test]
    fn declared_variable_rows_share_one_option_column_whatever_they_are_named() {
        // The column width used to come from a hardcoded "Child layout"
        // placeholder, so any declaration named something longer rendered its
        // control further right than the Metadata row and its siblings.
        let model = placement_model_with_declarations(vec![
            child_layout_declaration(),
            andamento_shared::template_config::NodeVariableDefinition {
                name: "attention.density.preference".to_owned(),
                default: "roomy".to_owned(),
                values: vec!["roomy".to_owned(), "tight".to_owned()],
            },
        ]);

        let rendered = render_config_with_scope(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Inspect,
            None,
            60,
            100,
            &[],
            false,
            0,
        );

        let metadata = option_column(&rendered.lines, "Metadata").expect("metadata row");
        let short = option_column(&rendered.lines, "child-layout").expect("child-layout row");
        let long =
            option_column(&rendered.lines, "attention.density.preference").expect("long-name row");
        assert_eq!(
            (metadata, short),
            (long, long),
            "every option row shares one column: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn an_unbounded_variable_offers_only_inherit() {
        // A declaration with no allowed values accepts anything, so there is no
        // finite set of segments to offer. It renders inherit alone rather than
        // an empty row. Setting a free-form value from the inspector needs a
        // different control — see the follow-up issue.
        let model = placement_model_with_declarations(vec![
            andamento_shared::template_config::NodeVariableDefinition {
                name: "caption".to_owned(),
                default: "none".to_owned(),
                values: vec![],
            },
        ]);

        let rendered = render_config_with_scope(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Inspect,
            None,
            60,
            100,
            &[],
            false,
            0,
        );

        assert!(option_column(&rendered.lines, "caption").is_some());
        assert_eq!(
            rendered
                .hit_regions
                .iter()
                .filter(|hit| matches!(
                    hit.action,
                    ConfigAction::SetNodeVariable { variable: 0, .. }
                ))
                .count(),
            1,
            "only the inherit segment is offered: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn inspect_root_options_expose_inheritable_child_layout_actions() {
        let mut model = model_with_tab(7, "repo");
        model.inspected_node = Some(NodeKey::Root);
        model.template_config.effective_variables = vec![
            andamento_shared::EffectiveNodeVariables {
                node: NodeKey::Root,
                values: BTreeMap::from([(
                    "child-layout".to_owned(),
                    andamento_shared::EffectiveVariableValue {
                        value: "strip".to_owned(),
                        provenance: andamento_shared::VariableSetterProvenance {
                            setter: "config".to_owned(),
                            ancestor: NodeKey::Root,
                            origin: andamento_shared::template_config::TemplateConfigOrigin {
                                layer:
                                    andamento_shared::template_config::TemplateConfigLayerKind::User,
                                membership: None,
                                source: "user.kdl".to_owned(),
                            },
                        },
                        overridden: vec![andamento_shared::VariableSetterProvenance {
                            setter: "default".to_owned(),
                            ancestor: NodeKey::Root,
                            origin: andamento_shared::template_config::TemplateConfigOrigin {
                                layer: andamento_shared::template_config::TemplateConfigLayerKind::Bundled,
                                membership: None,
                                source: "templates/flotilla-default.kdl".to_owned(),
                            },
                        }],
                    },
                )]),
                declarations: vec![child_layout_declaration()],
            },
        ];

        let rendered = render_config_with_scope(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Inspect,
            None,
            40,
            100,
            &[],
            false,
            0,
        );
        let rendered_text = rendered.lines.join("\n");

        assert!(rendered_text.contains("Options"));
        assert!(rendered_text.contains("child-layout"));
        assert!(rendered_text.contains("Variables"));
        assert!(rendered_text.contains("child-layout = strip"));
        assert!(rendered_text.contains("config at Root [user (user.kdl)]"));
        assert!(rendered_text.contains("overrides default at Root"));
        assert!(rendered.hit_regions.iter().any(|hit| {
            hit.action
                == ConfigAction::SetNodeVariable {
                    variable: 0,
                    value: Some(1),
                }
        }));
    }

    #[test]
    fn section_divider_uses_rail_border_characters() {
        assert_eq!(section_divider("Metadata", 24), "── Metadata ────────────");
    }

    #[test]
    fn inspect_page_renders_metadata_and_sources_as_tables() {
        let mut model = model_with_tab(7, "repo");
        model.inspected_node = Some(NodeKey::Tab(7));
        let entry = MetadataEntry {
            value: MetadataValue::Text("flotilla-org/flotilla".to_owned()),
            updated_at: 150,
            ttl_ms: Some(10_000),
            precedence: 4,
            ordinal: 2,
        };
        model.resolved_metadata = vec![andamento_shared::ResolvedMetadata {
            target: ResolvedMetadataTarget::Tab(7),
            values: BTreeMap::from([("git.repo".to_owned(), entry.clone())]),
            source_entries: BTreeMap::from([(
                "git.repo".to_owned(),
                vec![MetadataSourceEntry {
                    source_id: "andamento-git-watcher".to_owned(),
                    entry,
                }],
            )]),
            reachable_identities: vec![],
        }];

        let rendered = render_config(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Inspect,
            40,
            100,
            &[],
            false,
            0,
        );
        let rendered_text = rendered.lines.join("\n");

        assert!(rendered_text.contains("key                    value"));
        assert!(rendered_text.contains("git.repo"));
        assert!(rendered_text.contains("flotilla-org/flotilla"));
        assert!(rendered_text.contains("andamento-git-watcher"));
        assert!(rendered_text.contains("10s"));
        assert!(rendered_text.contains("    4     2"));
    }

    #[test]
    fn inspect_page_scrolls_body_content() {
        let mut model = model_with_tab(7, "repo");
        model.inspected_node = Some(NodeKey::Tab(7));
        let values = (0..16)
            .map(|index| {
                (
                    format!("meta.{index:02}"),
                    MetadataEntry {
                        value: MetadataValue::Text(format!("value-{index:02}")),
                        updated_at: index,
                        ttl_ms: None,
                        precedence: 0,
                        ordinal: 0,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        model.resolved_metadata = vec![andamento_shared::ResolvedMetadata {
            target: ResolvedMetadataTarget::Tab(7),
            values,
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let top = render_config(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Inspect,
            10,
            80,
            &[],
            false,
            0,
        );
        let scrolled = render_config(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Inspect,
            10,
            80,
            &[],
            false,
            usize::MAX,
        );

        assert!(top.lines.join("\n").contains("Inspect: Tab \"repo\" #7"));
        assert!(!top.lines.join("\n").contains("meta.15"));
        assert!(scrolled.lines.join("\n").contains("meta.15"));
        assert_eq!(top.lines[0], scrolled.lines[0], "tab bar stays fixed");
    }

    #[test]
    fn click_action_updates_config_locally() {
        let updated = apply_config_action(
            RailConfig::default(),
            ConfigAction::SetStructure(RailStructure::BoxPerTab),
        );

        assert_eq!(updated.structure, RailStructure::BoxPerTab);
    }

    #[test]
    fn renders_stats_page_collect_action() {
        let rendered = render_config(
            RailConfig::default(),
            None,
            ConfigPage::Stats,
            10,
            40,
            &[],
            false,
            0,
        );

        assert!(rendered.lines.iter().any(|line| line.contains("stats")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("collect stats")));
        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::CollectStats));
    }

    #[test]
    fn renders_stats_collection_pending_feedback() {
        let rendered = render_config(
            RailConfig::default(),
            None,
            ConfigPage::Stats,
            10,
            40,
            &[],
            true,
            0,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("collecting")));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.contains("no stats collected yet")));
    }

    #[test]
    fn scrolls_collected_stats_body_under_fixed_tab_bar() {
        let mut counters = std::collections::BTreeMap::new();
        for idx in 0..8 {
            counters.insert(format!("counter-{idx:02}"), idx);
        }
        let reports = vec![PluginStatsSnapshot {
            collection_id: 1,
            plugin_id: 7,
            client_id: 2,
            plugin_kind: "config".to_owned(),
            counters,
            spans: vec![],
        }];

        let rendered = render_config(
            RailConfig::default(),
            None,
            ConfigPage::Stats,
            8,
            80,
            &reports,
            false,
            5,
        );

        assert!(rendered.lines[0].contains("stats"));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.contains("counter-00")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("counter-03")));
    }

    #[test]
    fn granted_permission_should_resync_with_controller() {
        assert!(permission_result_should_resync(true));
        assert!(!permission_result_should_resync(false));
    }

    #[test]
    fn floating_config_closes_when_it_becomes_invisible() {
        assert!(config_should_close_on_visibility(false, true));
        assert!(!config_should_close_on_visibility(true, true));
    }

    #[test]
    fn embedded_config_stays_open_when_it_becomes_invisible() {
        assert!(!config_should_close_on_visibility(false, false));
        assert!(!config_should_close_on_visibility(true, false));
    }

    #[test]
    fn config_bool_accepts_common_true_values() {
        assert!(config_bool("true"));
        assert!(config_bool("1"));
        assert!(config_bool("yes"));
        assert!(config_bool("on"));
        assert!(!config_bool("false"));
    }
}
