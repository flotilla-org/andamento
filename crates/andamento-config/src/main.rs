#[cfg(not(target_family = "wasm"))]
fn main() {}

use std::collections::BTreeMap;
use std::time::Instant;

use andamento_shared::{
    ControllerViewModel, GroupPath, MetadataValue, PluginStatsSnapshot, RailConfig,
    RailGroupingMode, RailSizingPreset, RailStructure, RailViewMode,
};
use unicode_width::UnicodeWidthStr;

use andamento_shared::StatsCollectRequest;
use andamento_shared::{
    PluginStatsRecorder, RendererHello, MSG_CONFIG_EDITOR_HELLO, MSG_REQUEST_STATE,
    MSG_SET_RAIL_CONFIG, MSG_STATS_COLLECT, MSG_STATS_REPORT, MSG_STATS_REQUEST, MSG_VIEW_MODEL,
};
use zellij_tile::output::print;
use zellij_tile::prelude::*;

const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigAction {
    SetPage(ConfigPage),
    SetStructure(RailStructure),
    SetSizing(RailSizingPreset),
    SetGrouping(RailGroupingMode),
    SetView(RailViewMode),
    CollectStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPage {
    Settings,
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

#[derive(Default)]
struct PluginState {
    controller_plugin_url: String,
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
                if config_should_close_on_visibility(is_visible) {
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
        let rendered = render_config(
            config,
            self.model.as_ref(),
            self.page,
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
        ConfigAction::SetView(view) => config.view = view,
        ConfigAction::CollectStats => {}
    }
    config
}

fn permission_result_should_resync(granted: bool) -> bool {
    granted
}

fn config_should_close_on_visibility(is_visible: bool) -> bool {
    !is_visible
}

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
    if rows == 0 || cols == 0 {
        return RenderedConfig {
            lines: vec![],
            hit_regions: vec![],
            stats_scroll_offset: 0,
        };
    }
    let mut lines = vec![];
    let mut hit_regions = vec![];
    let mut effective_stats_scroll_offset = stats_scroll_offset;
    push_tab_row(&mut lines, &mut hit_regions, cols, page);
    push_plain(&mut lines, cols, "");
    match page {
        ConfigPage::Settings => {
            push_settings_page(&mut lines, &mut hit_regions, cols, config, model)
        }
        ConfigPage::Templates => push_templates_page(&mut lines, cols, model),
        ConfigPage::Stats => {
            effective_stats_scroll_offset = push_stats_page(
                &mut lines,
                &mut hit_regions,
                cols,
                stats_reports,
                stats_collection_pending,
                rows,
                stats_scroll_offset,
            );
        }
    }

    lines.truncate(rows);
    hit_regions.retain(|hit| hit.row < rows);
    while lines.len() < rows {
        lines.push(" ".repeat(cols));
    }

    RenderedConfig {
        lines,
        hit_regions,
        stats_scroll_offset: effective_stats_scroll_offset,
    }
}

fn push_stats_page(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    cols: usize,
    stats_reports: &[PluginStatsSnapshot],
    stats_collection_pending: bool,
    rows: usize,
    stats_scroll_offset: usize,
) -> usize {
    push_plain(lines, cols, "stats");
    let row = lines.len();
    push_plain(lines, cols, "[collect stats]");
    hit_regions.push(HitRegion {
        row,
        col_start: 0,
        col_end: "[collect stats]".width().saturating_sub(1),
        action: ConfigAction::CollectStats,
    });
    push_plain(lines, cols, "");
    let body_rows = rows.saturating_sub(lines.len());
    if stats_reports.is_empty() {
        if stats_collection_pending {
            push_plain(lines, cols, "collecting...");
        } else {
            push_plain(lines, cols, "no stats collected yet");
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
        push_plain(lines, cols, line);
    }
    effective_scroll_offset
}

fn push_settings_page(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    cols: usize,
    config: RailConfig,
    model: Option<&ControllerViewModel>,
) {
    push_plain(lines, cols, "structure");
    push_option(
        lines,
        hit_regions,
        cols,
        "joined cells",
        config.structure == RailStructure::JoinedCells,
        ConfigAction::SetStructure(RailStructure::JoinedCells),
    );
    push_option(
        lines,
        hit_regions,
        cols,
        "split around active",
        config.structure == RailStructure::SplitAroundActive,
        ConfigAction::SetStructure(RailStructure::SplitAroundActive),
    );
    push_option(
        lines,
        hit_regions,
        cols,
        "box per tab",
        config.structure == RailStructure::BoxPerTab,
        ConfigAction::SetStructure(RailStructure::BoxPerTab),
    );
    push_plain(lines, cols, "");
    push_plain(lines, cols, "grouping");
    push_option(
        lines,
        hit_regions,
        cols,
        "ungrouped",
        config.grouping == RailGroupingMode::None,
        ConfigAction::SetGrouping(RailGroupingMode::None),
    );
    push_option(
        lines,
        hit_regions,
        cols,
        "directory",
        config.grouping == RailGroupingMode::Directory,
        ConfigAction::SetGrouping(RailGroupingMode::Directory),
    );
    push_plain(lines, cols, "");
    push_plain(lines, cols, "view");
    push_option(
        lines,
        hit_regions,
        cols,
        "normal",
        config.view == RailViewMode::Normal,
        ConfigAction::SetView(RailViewMode::Normal),
    );
    push_option(
        lines,
        hit_regions,
        cols,
        "metadata",
        config.view == RailViewMode::Metadata,
        ConfigAction::SetView(RailViewMode::Metadata),
    );
    push_plain(lines, cols, "");
    push_plain(lines, cols, "sizing");
    push_option(
        lines,
        hit_regions,
        cols,
        "compact",
        config.sizing == RailSizingPreset::Compact,
        ConfigAction::SetSizing(RailSizingPreset::Compact),
    );
    push_option(
        lines,
        hit_regions,
        cols,
        "large",
        config.sizing == RailSizingPreset::Large,
        ConfigAction::SetSizing(RailSizingPreset::Large),
    );
    push_option(
        lines,
        hit_regions,
        cols,
        "active large",
        config.sizing == RailSizingPreset::ActiveLarge,
        ConfigAction::SetSizing(RailSizingPreset::ActiveLarge),
    );
    push_option(
        lines,
        hit_regions,
        cols,
        "pinned large",
        config.sizing == RailSizingPreset::PinnedLarge,
        ConfigAction::SetSizing(RailSizingPreset::PinnedLarge),
    );
    push_plain(lines, cols, "");
    push_cwd_metadata(lines, cols, model);
    push_plain(lines, cols, "");
    push_plain(lines, cols, "click to apply");
}

fn push_templates_page(lines: &mut Vec<String>, cols: usize, model: Option<&ControllerViewModel>) {
    push_plain(lines, cols, "templates");
    let Some(model) = model else {
        push_plain(lines, cols, "no controller state yet");
        return;
    };
    let diagnostics = &model.template_config;
    push_plain(
        lines,
        cols,
        &format!("state: {}", template_config_state_text(diagnostics.state)),
    );
    push_plain(
        lines,
        cols,
        &format!(
            "path: {}",
            diagnostics.path.as_deref().unwrap_or("<not configured>")
        ),
    );
    push_plain(
        lines,
        cols,
        &format!("template count: {}", diagnostics.template_count),
    );
    if !diagnostics.template_names.is_empty() {
        push_plain(lines, cols, "template names");
        for name in &diagnostics.template_names {
            push_plain(lines, cols, &format!("  {name}"));
        }
    }
    if let Some(error) = diagnostics.last_error.as_ref() {
        push_plain(lines, cols, "last error");
        push_plain(lines, cols, &format!("  {error}"));
    }
    push_plain(lines, cols, "");
    push_plain(lines, cols, "resolved slots");
    for row in &model.rows {
        match row {
            andamento_shared::RailRow::GroupHeader {
                label, templates, ..
            } => {
                if let Some(slot) = templates.group_header.as_ref() {
                    push_plain(
                        lines,
                        cols,
                        &format!("group {label}: {}", format_resolved_slot(slot)),
                    );
                }
            }
            andamento_shared::RailRow::Tab { .. } => {
                if let Some(tab) = model.tab_for_row(row) {
                    push_tab_template_slot(
                        lines,
                        cols,
                        &tab.name,
                        "title",
                        tab.templates.tab_title.as_ref(),
                    );
                    push_tab_template_slot(
                        lines,
                        cols,
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
    lines: &mut Vec<String>,
    cols: usize,
    tab_name: &str,
    slot_name: &str,
    slot: Option<&andamento_shared::ResolvedTemplateSlot>,
) {
    if let Some(slot) = slot {
        push_plain(
            lines,
            cols,
            &format!("tab {tab_name} {slot_name}: {}", format_resolved_slot(slot)),
        );
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

fn push_tab_row(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    cols: usize,
    page: ConfigPage,
) {
    let row = lines.len();
    let settings = if page == ConfigPage::Settings {
        "[settings]"
    } else {
        " settings "
    };
    let templates = if page == ConfigPage::Templates {
        "[templates]"
    } else {
        " templates "
    };
    let stats = if page == ConfigPage::Stats {
        "[stats]"
    } else {
        " stats "
    };
    let text = format!("{settings} {templates} {stats}");
    lines.push(pad_to_width(&truncate_to_width(&text, cols), cols));
    hit_regions.push(HitRegion {
        row,
        col_start: 0,
        col_end: settings.width().saturating_sub(1),
        action: ConfigAction::SetPage(ConfigPage::Settings),
    });
    hit_regions.push(HitRegion {
        row,
        col_start: settings.width().saturating_add(1),
        col_end: settings
            .width()
            .saturating_add(templates.width())
            .saturating_add(1),
        action: ConfigAction::SetPage(ConfigPage::Templates),
    });
    let stats_start = settings
        .width()
        .saturating_add(templates.width())
        .saturating_add(2);
    hit_regions.push(HitRegion {
        row,
        col_start: stats_start,
        col_end: stats_start.saturating_add(stats.width()).saturating_sub(1),
        action: ConfigAction::SetPage(ConfigPage::Stats),
    });
}

fn push_cwd_metadata(lines: &mut Vec<String>, cols: usize, model: Option<&ControllerViewModel>) {
    push_plain(lines, cols, "cwd metadata");
    let Some(model) = model else {
        push_plain(lines, cols, "no controller state yet");
        return;
    };
    if model.tabs.is_empty() {
        push_plain(lines, cols, "no tabs");
        return;
    }
    for tab in &model.tabs {
        let Some(grouping) = tab.grouping.as_ref() else {
            push_plain(
                lines,
                cols,
                &format!("{}: zellij.pane.cwd=<none>", tab.name),
            );
            continue;
        };
        push_plain(
            lines,
            cols,
            &format!(
                "{}: {} {}",
                tab.name,
                grouping.label,
                format_group_path(&grouping.path)
            ),
        );
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
    use andamento_shared::{GroupPath, GroupSegment, MetadataValue, SortMode, TabCard, TabGroupingInfo};

    #[test]
    fn renders_selected_config() {
        let rendered = render_config(
            RailConfig {
                structure: RailStructure::BoxPerTab,
                sizing: RailSizingPreset::Compact,
                grouping: RailGroupingMode::Directory,
                view: RailViewMode::Normal,
            },
            None,
            ConfigPage::Settings,
            22,
            30,
            &[],
            false,
            0,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "> box per tab"));
        assert!(rendered.lines.iter().any(|line| line.trim() == "> compact"));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.trim() == "> directory"));
    }

    #[test]
    fn maps_click_rows_to_config_actions() {
        let rendered = render_config(
            RailConfig::default(),
            None,
            ConfigPage::Settings,
            22,
            30,
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
    fn view_click_action_updates_config_locally() {
        let updated = apply_config_action(
            RailConfig::default(),
            ConfigAction::SetView(RailViewMode::Metadata),
        );

        assert_eq!(updated.view, RailViewMode::Metadata);
    }

    #[test]
    fn renders_view_options() {
        let rendered = render_config(
            RailConfig::default(),
            None,
            ConfigPage::Settings,
            22,
            30,
            &[],
            false,
            0,
        );

        assert!(rendered.lines.iter().any(|line| line.contains("view")));
        assert!(rendered.lines.iter().any(|line| line.contains("metadata")));
        assert!(rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == ConfigAction::SetView(RailViewMode::Metadata)));
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
                },
            ],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
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
    fn config_closes_when_it_becomes_invisible() {
        assert!(config_should_close_on_visibility(false));
        assert!(!config_should_close_on_visibility(true));
    }
}
