use std::collections::BTreeMap;
use std::time::Instant;

use andamento_shared::{
    ConfigInspectRequest, ControllerViewModel, GroupPath, MetadataValue, PluginStatsSnapshot,
    RailConfig, RailGroupingMode, RailSizingPreset, RailStructure,
};
use unicode_width::UnicodeWidthStr;

use andamento_shared::StatsCollectRequest;
use andamento_shared::{
    PluginStatsRecorder, RendererHello, MSG_CONFIG_EDITOR_HELLO, MSG_CONFIG_INSPECT,
    MSG_REQUEST_STATE, MSG_SET_RAIL_CONFIG, MSG_STATS_COLLECT, MSG_STATS_REPORT, MSG_STATS_REQUEST,
    MSG_VIEW_MODEL,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Tabs, Widget};
use zellij_tile::output::print;
use zellij_tile::prelude::*;

const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";
const CONFIG_CLOSE_ON_HIDDEN: &str = "close_on_hidden";
const CONFIG_RAIL_SCOPE: &str = "rail_scope";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigAction {
    SetPage(ConfigPage),
    SetStructure(RailStructure),
    SetSizing(RailSizingPreset),
    SetGrouping(RailGroupingMode),
    CollectStats,
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
    stats_scroll_offset: usize,
}

struct ConfigUiFrame {
    buffer: Buffer,
    rows: usize,
    cols: usize,
    next_row: usize,
    hit_regions: Vec<HitRegion>,
}

impl ConfigUiFrame {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            buffer: Buffer::empty(Rect::new(0, 0, cols as u16, rows as u16)),
            rows,
            cols,
            next_row: 0,
            hit_regions: vec![],
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
        Paragraph::new(line).render(
            Rect::new(0, row as u16, self.cols as u16, 1),
            &mut self.buffer,
        );
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

    fn into_rendered(self, stats_scroll_offset: usize) -> RenderedConfig {
        let mut lines = vec![];
        for row in 0..self.rows {
            let line = (0..self.cols)
                .map(|col| self.buffer[(col as u16, row as u16)].symbol())
                .collect::<String>();
            lines.push(pad_to_width(
                &truncate_to_width(&line, self.cols),
                self.cols,
            ));
        }
        RenderedConfig {
            lines,
            hit_regions: self.hit_regions,
            stats_scroll_offset,
        }
    }
}

#[derive(Default)]
pub struct PluginState {
    controller_plugin_url: String,
    close_on_hidden: bool,
    rail_scope: Option<String>,
    own_plugin_id: Option<u32>,
    own_client_id: Option<u16>,
    model: Option<ControllerViewModel>,
    pending_config: Option<RailConfig>,
    page: ConfigPage,
    hit_regions: Vec<HitRegion>,
    permissions_granted: bool,
    stats: PluginStatsRecorder,
    stats_collection_id: u64,
    stats_collection_pending: bool,
    stats_reports: Vec<PluginStatsSnapshot>,
    stats_scroll_offset: usize,
}

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
        self.page = initial_page_for_scope(self.rail_scope.as_deref());

        request_permission(&[
            PermissionType::ChangeApplicationState,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);
        subscribe(&[
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
            Event::Mouse(Mouse::ScrollUp(_)) if self.page == ConfigPage::Stats => {
                self.stats.increment("update.mouse.scroll-up");
                self.stats_scroll_offset = self.stats_scroll_offset.saturating_sub(1);
                true
            }
            Event::Mouse(Mouse::ScrollDown(_)) if self.page == ConfigPage::Stats => {
                self.stats.increment("update.mouse.scroll-down");
                self.stats_scroll_offset = self.stats_scroll_offset.saturating_add(1);
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
            self.stats_scroll_offset,
        );
        self.hit_regions = rendered.hit_regions;
        self.stats_scroll_offset = rendered.stats_scroll_offset;
        print!("{}", rendered.lines.join("\n"));
        self.stats.record_span_elapsed("render.config", started_at);
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
        let hello = RendererHello {
            plugin_id,
            client_id,
        };
        let Ok(payload) = serde_json::to_string(&hello) else {
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
        self.stats_scroll_offset = 0;
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
}

fn apply_config_action(mut config: RailConfig, action: ConfigAction) -> RailConfig {
    match action {
        ConfigAction::SetPage(_) => {}
        ConfigAction::SetStructure(structure) => config.structure = structure,
        ConfigAction::SetSizing(sizing) => config.sizing = sizing,
        ConfigAction::SetGrouping(grouping) => config.grouping = grouping,
        ConfigAction::CollectStats => {}
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
    *rail_scope = Some(request.scope);
    *page = ConfigPage::Inspect;
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
    stats_scroll_offset: usize,
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
        stats_scroll_offset,
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
    stats_scroll_offset: usize,
) -> RenderedConfig {
    if rows == 0 || cols == 0 {
        return RenderedConfig {
            lines: vec![],
            hit_regions: vec![],
            stats_scroll_offset: 0,
        };
    }
    let mut frame = ConfigUiFrame::new(rows, cols);
    let mut effective_stats_scroll_offset = stats_scroll_offset;
    push_tab_row(&mut frame, page);
    frame.push_blank();
    match page {
        ConfigPage::Settings => push_settings_page(&mut frame, config, model),
        ConfigPage::Inspect => push_inspect_page(&mut frame, model, rail_scope),
        ConfigPage::Templates => push_templates_page(&mut frame, model),
        ConfigPage::Stats => {
            effective_stats_scroll_offset = push_stats_page(
                &mut frame,
                stats_reports,
                stats_collection_pending,
                rows,
                stats_scroll_offset,
            );
        }
    }
    frame.into_rendered(effective_stats_scroll_offset)
}

fn push_inspect_page(
    frame: &mut ConfigUiFrame,
    model: Option<&ControllerViewModel>,
    rail_scope: Option<&str>,
) {
    frame.push_plain("inspect");
    let scope = rail_scope.unwrap_or("<none>");
    frame.push_plain(&format!("scope: {scope}"));

    let Some(model) = model else {
        frame.push_plain("no controller state yet");
        return;
    };

    frame.push_plain(&format!(
        "model: tabs={} rows={} metadata={} identities={}",
        model.tabs.len(),
        model.rows.len(),
        model.resolved_metadata.len(),
        model.observed_identities.len()
    ));
    frame.push_plain(&format!(
        "metadata view: root={} explicit={}",
        if model.metadata_controls.root_enabled {
            "on"
        } else {
            "off"
        },
        model.metadata_controls.per_node.len()
    ));

    let Some(tab_id) = rail_scope.and_then(parse_tab_scope) else {
        return;
    };
    let Some(tab) = model.tab_by_id(tab_id) else {
        frame.push_plain(&format!("tab: <missing> #{tab_id}"));
        return;
    };
    frame.push_blank();
    frame.push_plain(&format!("tab: {} #{}", tab.name, tab.tab_id));
    frame.push_plain(&format!(
        "state: position={} active={} pinned={}",
        tab.position, tab.active, tab.pinned
    ));
    if let Some(grouping) = tab.grouping.as_ref() {
        frame.push_plain(&format!(
            "group: {} {}",
            grouping.label,
            format_group_path(&grouping.path)
        ));
    } else {
        frame.push_plain("group: <none>");
    }
    if let Some(active_pane) = tab.active_pane.as_ref() {
        frame.push_plain(&format!("active pane: {}", format_pane_target(active_pane)));
    }
}

fn parse_tab_scope(scope: &str) -> Option<u64> {
    scope.strip_prefix("tab:")?.parse().ok()
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
    rows: usize,
    stats_scroll_offset: usize,
) -> usize {
    frame.push_plain("stats");
    if let Some(row) = frame.push_plain("[collect stats]") {
        frame.add_hit(
            row,
            0,
            "[collect stats]".width().saturating_sub(1),
            ConfigAction::CollectStats,
        );
    }
    frame.push_blank();
    let body_rows = rows.saturating_sub(frame.current_row());
    if stats_reports.is_empty() {
        if stats_collection_pending {
            frame.push_plain("collecting...");
        } else {
            frame.push_plain("no stats collected yet");
        }
        return 0;
    }
    let mut body = vec![];
    for report in stats_reports {
        body.push(format!(
            "{} #{} c{}",
            report.plugin_kind, report.plugin_id, report.client_id
        ));
        for (name, count) in &report.counters {
            body.push(format!("  {name}: {count}"));
        }
        for span in &report.spans {
            let avg_us = if span.count == 0 {
                0
            } else {
                span.total_us / span.count
            };
            body.push(format!(
                "  {}: n={} total={}us avg={}us max={}us",
                span.name, span.count, span.total_us, avg_us, span.max_us
            ));
        }
    }
    let max_scroll_offset = body.len().saturating_sub(body_rows);
    let effective_scroll_offset = stats_scroll_offset.min(max_scroll_offset);
    for line in body.iter().skip(effective_scroll_offset).take(body_rows) {
        frame.push_plain(line);
    }
    effective_scroll_offset
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
    push_segmented_choice(
        frame,
        "grouping",
        &[
            (
                "ungrouped",
                config.grouping == RailGroupingMode::None,
                ConfigAction::SetGrouping(RailGroupingMode::None),
            ),
            (
                "directory",
                config.grouping == RailGroupingMode::Directory,
                ConfigAction::SetGrouping(RailGroupingMode::Directory),
            ),
        ],
    );
    push_segmented_choice(
        frame,
        "sizing",
        &[
            (
                "compact",
                config.sizing == RailSizingPreset::Compact,
                ConfigAction::SetSizing(RailSizingPreset::Compact),
            ),
            (
                "large",
                config.sizing == RailSizingPreset::Large,
                ConfigAction::SetSizing(RailSizingPreset::Large),
            ),
            (
                "active large",
                config.sizing == RailSizingPreset::ActiveLarge,
                ConfigAction::SetSizing(RailSizingPreset::ActiveLarge),
            ),
            (
                "pinned large",
                config.sizing == RailSizingPreset::PinnedLarge,
                ConfigAction::SetSizing(RailSizingPreset::PinnedLarge),
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
    frame.push_blank();
    frame.push_plain("resolved slots");
    for row in &model.rows {
        match row {
            andamento_shared::RailRow::GroupHeader {
                label, templates, ..
            } => {
                if let Some(slot) = templates.group_header.as_ref() {
                    frame.push_plain(&format!("group {label}: {}", format_resolved_slot(slot)));
                }
            }
            andamento_shared::RailRow::Tab { .. } => {
                if let Some(tab) = model.tab_for_row(row) {
                    push_tab_template_slot(
                        frame,
                        &tab.name,
                        "title",
                        tab.templates.tab_title.as_ref(),
                    );
                    push_tab_template_slot(
                        frame,
                        &tab.name,
                        "status",
                        tab.templates.tab_status.as_ref(),
                    );
                }
            }
        }
    }
}

fn push_tab_template_slot(
    frame: &mut ConfigUiFrame,
    tab_name: &str,
    slot_name: &str,
    slot: Option<&andamento_shared::ResolvedTemplateSlot>,
) {
    if let Some(slot) = slot {
        frame.push_plain(&format!(
            "tab {tab_name} {slot_name}: {}",
            format_resolved_slot(slot)
        ));
    }
}

fn format_resolved_slot(slot: &andamento_shared::ResolvedTemplateSlot) -> String {
    let fields = slot
        .fields
        .iter()
        .map(|field| format!("{}({})", field.text, field.priority))
        .collect::<Vec<_>>();
    if fields.is_empty() {
        slot.template_name.clone()
    } else {
        format!("{} [{}]", slot.template_name, fields.join(", "))
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
    let (line, mut row_hits) = render_tab_row_with_ratatui(page, frame.cols);
    let Some(row) = frame.push_plain(&line) else {
        return;
    };
    for hit in &mut row_hits {
        hit.row = row;
    }
    frame.hit_regions.extend(row_hits);
}

fn render_tab_row_with_ratatui(page: ConfigPage, cols: usize) -> (String, Vec<HitRegion>) {
    let tabs = [
        (ConfigPage::Settings, "settings"),
        (ConfigPage::Inspect, "inspect"),
        (ConfigPage::Templates, "templates"),
        (ConfigPage::Stats, "stats"),
    ];
    let labels = tabs
        .iter()
        .map(|(tab_page, label)| {
            if *tab_page == page {
                format!("[{label}]")
            } else {
                format!(" {label} ")
            }
        })
        .collect::<Vec<_>>();
    let text = labels.join(" ");
    let mut buffer = Buffer::empty(Rect::new(0, 0, cols as u16, 1));
    let tab_lines = labels
        .iter()
        .map(|label| Line::from(label.as_str()))
        .collect::<Vec<_>>();
    Tabs::new(tab_lines)
        .divider(" ")
        .render(Rect::new(0, 0, cols as u16, 1), &mut buffer);

    let line = if cols == 0 {
        String::new()
    } else {
        let rendered = (0..cols)
            .map(|col| buffer[(col as u16, 0)].symbol())
            .collect::<String>();
        pad_to_width(&truncate_to_width(&rendered, cols), cols)
    };
    let line = if line.trim().is_empty() {
        pad_to_width(&truncate_to_width(&text, cols), cols)
    } else {
        line
    };
    let mut hit_regions = vec![];
    let mut col = 0usize;
    for ((tab_page, _), label) in tabs.iter().zip(labels.iter()) {
        hit_regions.push(HitRegion {
            row: 0,
            col_start: col,
            col_end: col.saturating_add(label.width()).saturating_sub(1),
            action: ConfigAction::SetPage(*tab_page),
        });
        col = col.saturating_add(label.width()).saturating_add(1);
    }
    (line, hit_regions)
}

fn push_cwd_metadata(frame: &mut ConfigUiFrame, model: Option<&ControllerViewModel>) {
    frame.push_plain("cwd metadata");
    let Some(model) = model else {
        frame.push_plain("no controller state yet");
        return;
    };
    if model.tabs.is_empty() {
        frame.push_plain("no tabs");
        return;
    }
    for tab in &model.tabs {
        let Some(grouping) = tab.grouping.as_ref() else {
            frame.push_plain(&format!("{}: zellij.pane.cwd=<none>", tab.name));
            continue;
        };
        frame.push_plain(&format!(
            "{}: {} {}",
            tab.name,
            grouping.label,
            format_group_path(&grouping.path)
        ));
    }
}

fn format_group_path(path: &GroupPath) -> String {
    if path.0.is_empty() {
        return "<none>".to_owned();
    }
    path.0
        .iter()
        .map(|segment| format!("{}={}", segment.key, format_metadata_value(&segment.value)))
        .collect::<Vec<_>>()
        .join(" > ")
}

fn format_metadata_value(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Text(value) => value.clone(),
        MetadataValue::Bool(value) => value.to_string(),
        MetadataValue::Integer(value) => value.to_string(),
        MetadataValue::StringList(values) => values.join(","),
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
    const LABEL_WIDTH: usize = 9;
    let mut text = format!("{label:<LABEL_WIDTH$}  ");
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
mod tests {
    use super::*;
    use andamento_shared::{
        GroupPath, GroupSegment, MetadataValue, SortMode, TabCard, TabGroupingInfo,
    };

    #[test]
    fn renders_selected_config() {
        let rendered = render_config(
            RailConfig {
                structure: RailStructure::BoxPerTab,
                sizing: RailSizingPreset::Compact,
                grouping: RailGroupingMode::Directory,
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
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim().contains("● compact")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim().ends_with("● directory")));
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
        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::SetSizing(RailSizingPreset::PinnedLarge)));
        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::SetGrouping(RailGroupingMode::Directory)));
    }

    #[test]
    fn settings_page_renders_enum_choices_inline() {
        let rendered = render_config(
            RailConfig {
                structure: RailStructure::BoxPerTab,
                sizing: RailSizingPreset::Compact,
                grouping: RailGroupingMode::Directory,
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
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "grouping   ○ ungrouped  ● directory"));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim()
                == "sizing     ● compact  ○ large  ○ active large  ○ pinned large"));
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

        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::SetSizing(RailSizingPreset::PinnedLarge)));
    }

    #[test]
    fn ratatui_tab_row_keeps_existing_pages_and_hit_regions() {
        let (line, hits) = render_tab_row_with_ratatui(ConfigPage::Templates, 60);

        assert!(line.contains("settings"));
        assert!(line.contains("inspect"));
        assert!(line.contains("templates"));
        assert!(line.contains("stats"));
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
    fn config_inspect_request_updates_scope_and_page() {
        let request = ConfigInspectRequest {
            scope: "tab:7".to_owned(),
            client_id: 4,
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
    fn inspect_page_renders_selected_tab_details() {
        let model = ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig::default(),
            template_config: andamento_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![TabCard {
                tab_id: 7,
                position: 2,
                name: "repo".to_owned(),
                active: true,
                pinned: false,
                status: None,
                grouping: Some(TabGroupingInfo {
                    key: "git:/repo".to_owned(),
                    path: GroupPath(vec![GroupSegment {
                        key: "git.repo".to_owned(),
                        value: MetadataValue::Text("flotilla-org/flotilla".to_owned()),
                        label: Some("flotilla".to_owned()),
                    }]),
                    label: "flotilla".to_owned(),
                    full_label: "flotilla-org/flotilla".to_owned(),
                }),
                templates: andamento_shared::ResolvedTemplateSlots::default(),
                active_pane: None,
            }],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
            metadata_controls: andamento_shared::MetadataControls::default(),
        };

        let rendered = render_config_with_scope(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Inspect,
            Some("tab:7"),
            18,
            80,
            &[],
            false,
            0,
        );

        assert!(rendered.lines[0].contains("[inspect]"));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "scope: tab:7"));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "tab: repo #7"));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "group: flotilla git.repo=flotilla-org/flotilla"));
    }

    #[test]
    fn click_action_updates_config_locally() {
        let updated = apply_config_action(
            RailConfig::default(),
            ConfigAction::SetStructure(RailStructure::BoxPerTab),
        );

        assert_eq!(updated.structure, RailStructure::BoxPerTab);
        assert_eq!(updated.sizing, RailSizingPreset::ActiveLarge);
        assert_eq!(updated.grouping, RailGroupingMode::None);
    }

    #[test]
    fn grouping_click_action_updates_config_locally() {
        let updated = apply_config_action(
            RailConfig::default(),
            ConfigAction::SetGrouping(RailGroupingMode::Directory),
        );

        assert_eq!(updated.grouping, RailGroupingMode::Directory);
    }

    #[test]
    fn renders_template_page_diagnostics() {
        let model = ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig::default(),
            template_config: andamento_shared::TemplateConfigDiagnostics {
                path: Some("/host/tmp/andamento.kdl".to_owned()),
                state: andamento_shared::TemplateConfigState::Loaded,
                template_count: 1,
                template_names: vec!["andamento.git.group-header".to_owned()],
                last_error: None,
            },
            tabs: vec![],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
            metadata_controls: andamento_shared::MetadataControls::default(),
        };

        let rendered = render_config(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Templates,
            16,
            80,
            &[],
            false,
            0,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("state: loaded")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("/host/tmp/andamento.kdl")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("andamento.git.group-header")));
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
    fn scrolls_collected_stats_body_under_fixed_controls() {
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
            40,
            &reports,
            false,
            4,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("collect stats")));
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
    fn renders_collected_cwd_metadata_from_model() {
        let model = ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig::default(),
            template_config: andamento_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![
                TabCard {
                    tab_id: 1,
                    position: 0,
                    name: "server".to_owned(),
                    active: true,
                    pinned: false,
                    status: None,
                    grouping: Some(TabGroupingInfo {
                        key: "cwd:/repo/app".to_owned(),
                        path: GroupPath(vec![GroupSegment {
                            key: "zellij.pane.cwd".to_owned(),
                            value: MetadataValue::Text("/repo/app".to_owned()),
                            label: None,
                        }]),
                        label: "app".to_owned(),
                        full_label: "/repo/app".to_owned(),
                    }),
                    templates: andamento_shared::ResolvedTemplateSlots::default(),
                    active_pane: None,
                },
                TabCard {
                    tab_id: 2,
                    position: 1,
                    name: "scratch".to_owned(),
                    active: false,
                    pinned: false,
                    status: None,
                    grouping: None,
                    templates: andamento_shared::ResolvedTemplateSlots::default(),
                    active_pane: None,
                },
            ],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
            metadata_controls: andamento_shared::MetadataControls::default(),
        };

        let rendered = render_config(
            RailConfig::default(),
            Some(&model),
            ConfigPage::Settings,
            24,
            40,
            &[],
            false,
            0,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "cwd metadata"));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "server: app zellij.pane.cwd=/repo/app"));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "scratch: zellij.pane.cwd=<none>"));
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
