#[cfg(not(target_family = "wasm"))]
fn main() {}

#[cfg(target_family = "wasm")]
use std::collections::BTreeMap;

use tabs_shared::{
    ControllerViewModel, GroupPath, MetadataValue, RailConfig, RailGroupingMode, RailSizingPreset,
    RailStructure, RailViewMode,
};
use unicode_width::UnicodeWidthStr;

#[cfg(target_family = "wasm")]
use tabs_shared::{
    RendererHello, MSG_CONFIG_EDITOR_HELLO, MSG_REQUEST_STATE, MSG_SET_RAIL_CONFIG, MSG_VIEW_MODEL,
};
#[cfg(target_family = "wasm")]
use zellij_tile::prelude::*;

#[cfg(target_family = "wasm")]
const CONFIG_CONTROLLER_PLUGIN_URL: &str = "controller_plugin_url";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigAction {
    SetPage(ConfigPage),
    SetStructure(RailStructure),
    SetSizing(RailSizingPreset),
    SetGrouping(RailGroupingMode),
    SetView(RailViewMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPage {
    Settings,
    Templates,
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
}

#[cfg(target_family = "wasm")]
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
        let rendered = render_config(config, self.model.as_ref(), self.page, rows, cols);
        self.hit_regions = rendered.hit_regions;
        print!("{}", rendered.lines.join("\n"));
        render_tab_ribbons(self.page, cols);
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
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
        pipe_message_to_plugin(
            MessageToPlugin::new(MSG_REQUEST_STATE)
                .with_plugin_url(self.controller_plugin_url.clone())
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
            MessageToPlugin::new(MSG_SET_RAIL_CONFIG)
                .with_plugin_url(self.controller_plugin_url.clone())
                .with_destination_client_id(client_id)
                .with_payload(payload),
        );
        true
    }
}

fn apply_config_action(mut config: RailConfig, action: ConfigAction) -> RailConfig {
    match action {
        ConfigAction::SetPage(_) => {}
        ConfigAction::SetStructure(structure) => config.structure = structure,
        ConfigAction::SetSizing(sizing) => config.sizing = sizing,
        ConfigAction::SetGrouping(grouping) => config.grouping = grouping,
        ConfigAction::SetView(view) => config.view = view,
    }
    config
}

fn permission_result_should_resync(granted: bool) -> bool {
    granted
}

fn render_config(
    config: RailConfig,
    model: Option<&ControllerViewModel>,
    page: ConfigPage,
    rows: usize,
    cols: usize,
) -> RenderedConfig {
    if rows == 0 || cols == 0 {
        return RenderedConfig {
            lines: vec![],
            hit_regions: vec![],
        };
    }
    let mut lines = vec![];
    let mut hit_regions = vec![];
    push_tab_row(&mut lines, &mut hit_regions, cols, page);
    push_plain(&mut lines, cols, "");
    match page {
        ConfigPage::Settings => {
            push_settings_page(&mut lines, &mut hit_regions, cols, config, model)
        }
        ConfigPage::Templates => push_templates_page(&mut lines, cols, model),
    }

    lines.truncate(rows);
    hit_regions.retain(|hit| hit.row < rows);
    while lines.len() < rows {
        lines.push(" ".repeat(cols));
    }

    RenderedConfig { lines, hit_regions }
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
            tabs_shared::RailRow::GroupHeader {
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
            tabs_shared::RailRow::Tab { tab, .. } => {
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

fn push_tab_template_slot(
    lines: &mut Vec<String>,
    cols: usize,
    tab_name: &str,
    slot_name: &str,
    slot: Option<&tabs_shared::ResolvedTemplateSlot>,
) {
    if let Some(slot) = slot {
        push_plain(
            lines,
            cols,
            &format!("tab {tab_name} {slot_name}: {}", format_resolved_slot(slot)),
        );
    }
}

fn format_resolved_slot(slot: &tabs_shared::ResolvedTemplateSlot) -> String {
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

fn template_config_state_text(state: tabs_shared::TemplateConfigState) -> &'static str {
    match state {
        tabs_shared::TemplateConfigState::NotConfigured => "not configured",
        tabs_shared::TemplateConfigState::PendingPermission => "pending permission",
        tabs_shared::TemplateConfigState::Loaded => "loaded",
        tabs_shared::TemplateConfigState::Error => "error",
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
    let text = format!("{settings} {templates}");
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
}

#[cfg(target_family = "wasm")]
fn render_tab_ribbons(page: ConfigPage, _cols: usize) {
    let mut settings = Text::new("settings");
    let mut templates = Text::new("templates");
    if page == ConfigPage::Settings {
        settings = settings.selected();
    }
    if page == ConfigPage::Templates {
        templates = templates.selected();
    }
    print_ribbon_with_coordinates(settings, 0, 0, None, None);
    print_ribbon_with_coordinates(templates, 12, 0, None, None);
}

#[cfg(not(target_family = "wasm"))]
fn render_tab_ribbons(_page: ConfigPage, _cols: usize) {}

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
    use tabs_shared::{GroupPath, GroupSegment, MetadataValue, SortMode, TabCard, TabGroupingInfo};

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
        let rendered = render_config(RailConfig::default(), None, ConfigPage::Settings, 22, 30);

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
        let rendered = render_config(RailConfig::default(), None, ConfigPage::Settings, 22, 30);

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
            template_config: tabs_shared::TemplateConfigDiagnostics {
                path: Some("/host/tmp/andamento.kdl".to_owned()),
                state: tabs_shared::TemplateConfigState::Loaded,
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
    fn renders_collected_cwd_metadata_from_model() {
        let model = ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig::default(),
            template_config: tabs_shared::TemplateConfigDiagnostics::default(),
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
                        }]),
                        label: "app".to_owned(),
                        full_label: "/repo/app".to_owned(),
                    }),
                    templates: tabs_shared::ResolvedTemplateSlots::default(),
                },
                TabCard {
                    tab_id: 2,
                    position: 1,
                    name: "scratch".to_owned(),
                    active: false,
                    pinned: false,
                    status: None,
                    grouping: None,
                    templates: tabs_shared::ResolvedTemplateSlots::default(),
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
}
