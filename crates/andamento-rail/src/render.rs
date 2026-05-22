use std::collections::{BTreeMap, BTreeSet, HashMap};

use andamento_shared::segment_bar::{self, SegmentItem};
use andamento_shared::template_config::{
    TemplateConfigCatalog, TemplateConfigFieldClass, TemplateConfigMatchContext,
    TemplateConfigNodeKind, TemplateConfigSlot,
};
use andamento_shared::RAIL_CHILD_LAYOUT_METADATA_KEY;
use andamento_shared::{
    ChildLayoutSetting, ControllerViewModel, GroupPath, GroupSegment, MetadataControls,
    MetadataEntry, MetadataSourceEntry, MetadataTarget, MetadataValue, NodeKey, PaneTarget,
    Priority, RailConfig, RailRgbColor, RailRow, RailSizingPreset, RailStructure,
    ReachableMetadataIdentity, ResolvedMetadata, ResolvedTemplateFieldSource, ResolvedTemplateSlot,
    ResolvedTemplateSlots, StatusIcon, TabCard, TabGroupingInfo, TabStatusSummary,
};
use ansi_term::{Color, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::{PaletteColor, SizeInPixels, Styling};

const ACTIVE_CELL_HEIGHT: usize = 5;
const COMPACT_CELL_HEIGHT: usize = 2;

type RenderMetadata = BTreeMap<String, MetadataValue>;
type RenderMetadataSources = BTreeMap<String, Vec<MetadataSourceEntry>>;
type RenderReachableIdentities = Vec<ReachableMetadataIdentity>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTab {
    pub tab_id: u64,
    pub position: usize,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitAction {
    SwitchTab,
    TogglePin,
    ToggleGroup,
    OpenConfig,
    ScrollRailUp,
    ScrollRailDown,
    InspectNode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitRegion {
    pub row_start: usize,
    pub row_end: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub tab_id: u64,
    pub tab_position: usize,
    pub group_path: Option<GroupPath>,
    pub inspect_target: Option<NodeKey>,
    pub action: HitAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedRail {
    pub lines: Vec<String>,
    pub hit_regions: Vec<HitRegion>,
    pub visible_cards: Vec<VisibleCard>,
    /// Total content height vs the available rows. When > available_rows the
    /// user can usefully scroll the rail's window; the scroll wheel falls
    /// back to tab-switching otherwise.
    pub content_height: usize,
    pub available_rows: usize,
}

impl RenderedRail {
    pub fn can_scroll(&self) -> bool {
        self.content_height > self.available_rows
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleCard {
    pub tab_id: u64,
    pub tab_position: usize,
    pub row_start: usize,
    pub status_row: Option<usize>,
    pub status_icon_rect: Option<VisibleIconRect>,
    pub status_priority: Option<Priority>,
    pub status_icon: Option<StatusIcon>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleIconRect {
    pub x: usize,
    pub y: usize,
    pub columns: usize,
    pub rows: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct RenderTheme {
    pub active_border: PaletteColor,
    pub inactive_border: PaletteColor,
    pub body_foreground: PaletteColor,
    pub segment_active_background: PaletteColor,
    pub segment_active_foreground: PaletteColor,
    pub segment_inactive_background: PaletteColor,
    pub segment_inactive_foreground: PaletteColor,
    pub segment_between_background: PaletteColor,
}

impl From<Styling> for RenderTheme {
    fn from(colors: Styling) -> Self {
        Self {
            // Match the normal tab bar's selected/unselected foreground choices,
            // but do not paint a background over the terminal default.
            active_border: colors.ribbon_selected.background,
            inactive_border: colors.ribbon_unselected.background,
            body_foreground: colors.text_unselected.base,
            segment_active_background: colors.ribbon_selected.background,
            segment_active_foreground: colors.ribbon_selected.base,
            segment_inactive_background: colors.ribbon_unselected.background,
            segment_inactive_foreground: colors.ribbon_unselected.base,
            // The plugin API Styling currently does not expose Zellij's top-level
            // theme background. Keep this explicit in RenderTheme so config/API
            // work can supply the real terminal-like background without changing
            // compact strip rendering.
            segment_between_background: colors.text_unselected.background,
        }
    }
}

impl RenderTheme {
    fn with_config(mut self, config: RailConfig) -> Self {
        if let Some(color) = config.segment_between_color {
            self.segment_between_background = palette_color_from_rgb(color);
        }
        self
    }
}

fn palette_color_from_rgb(color: RailRgbColor) -> PaletteColor {
    PaletteColor::Rgb((color.red, color.green, color.blue))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderCard {
    tab_id: u64,
    position: usize,
    name: String,
    active: bool,
    pinned: bool,
    status: Option<TabStatusSummary>,
    metadata: RenderMetadata,
    metadata_sources: RenderMetadataSources,
    reachable_identities: RenderReachableIdentities,
    templates: ResolvedTemplateSlots,
    /// Metadata-panel content rendered inside the card body when set.
    /// Populated by `append_tab_run`/`render_nodes` based on
    /// `MetadataControls`; the card grows by `meta_panel.len()` rows.
    meta_panel: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RenderNode {
    Group(RenderGroup),
    Tab(RenderTab),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderGroup {
    path: GroupPath,
    conflated_paths: Vec<GroupPath>,
    label: String,
    full_label: String,
    tab_count: usize,
    collapsed: bool,
    indent: usize,
    metadata: RenderMetadata,
    metadata_sources: RenderMetadataSources,
    reachable_identities: RenderReachableIdentities,
    templates: ResolvedTemplateSlots,
    children: Vec<RenderNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderTab {
    card: RenderCard,
    indent: usize,
    grouping: Option<TabGroupingInfo>,
    parent_path: Option<GroupPath>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingRenderNode {
    GroupHeader {
        path: GroupPath,
        label: String,
        full_label: String,
        tab_count: usize,
        templates: ResolvedTemplateSlots,
    },
    Tab(RenderTab),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentGroupHeader {
    path: GroupPath,
    label: String,
    full_label: String,
    templates: ResolvedTemplateSlots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InheritedRailSettings {
    child_layout: ChildLayoutSetting,
}

impl Default for InheritedRailSettings {
    fn default() -> Self {
        Self {
            child_layout: ChildLayoutSetting::Cards,
        }
    }
}

impl InheritedRailSettings {
    fn with_node_metadata(self, metadata: &RenderMetadata) -> Self {
        Self {
            child_layout: child_layout_from_metadata(metadata).unwrap_or(self.child_layout),
        }
    }
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub fn render_lines(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
) -> RenderedRail {
    render_lines_with_theme(model, tabs, rows, cols, controller_available, None)
}

pub fn render_lines_with_theme(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
) -> RenderedRail {
    render_lines_with_options(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        theme,
        None,
        &[],
        None,
    )
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub fn render_lines_with_collapsed_groups(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    collapsed_groups: &[GroupPath],
) -> RenderedRail {
    render_lines_with_options(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        None,
        None,
        collapsed_groups,
        None,
    )
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub fn render_lines_with_template_catalog(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> RenderedRail {
    render_lines_with_options(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        None,
        None,
        &[],
        template_catalog,
    )
}

pub fn render_lines_with_options(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_groups: &[GroupPath],
    template_catalog: Option<&TemplateConfigCatalog>,
) -> RenderedRail {
    render_lines_with_metadata_controls(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        theme,
        terminal_cell_size,
        collapsed_groups,
        template_catalog,
        &MetadataControls::default(),
    )
}

pub fn render_lines_with_metadata_controls(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_groups: &[GroupPath],
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
) -> RenderedRail {
    render_lines_with_rail_scroll(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        theme,
        terminal_cell_size,
        collapsed_groups,
        template_catalog,
        metadata_controls,
        0,
    )
}

pub fn render_lines_with_rail_scroll(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_groups: &[GroupPath],
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    rail_scroll_offset: isize,
) -> RenderedRail {
    if rows == 0 || cols == 0 {
        return RenderedRail {
            lines: vec![],
            hit_regions: vec![],
            visible_cards: vec![],
            content_height: 0,
            available_rows: 0,
        };
    }

    let footer_rows = 1.min(rows);
    let card_rows_available = rows.saturating_sub(footer_rows);
    let mut lines = vec![blank(cols); rows];
    let mut hit_regions = vec![];
    let mut visible_cards = vec![];
    let nodes = nodes_to_render(model, tabs, collapsed_groups);
    let config = model.map(|model| model.config).unwrap_or_default();
    let theme = theme.map(|theme| theme.with_config(config));
    let inspected_node = model.and_then(|model| model.inspected_node.as_ref());
    let root_settings = root_inherited_settings_for_model(model);
    let mut content_height = 0;
    if nodes.is_empty() {
        lines[0] = pad_to_width("tabs: waiting for tab state", cols);
    } else {
        content_height = render_nodes(
            &mut lines,
            &mut hit_regions,
            &mut visible_cards,
            &nodes,
            card_rows_available,
            cols,
            controller_available,
            config,
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
            root_settings,
            rail_scroll_offset,
        );
    }

    let rail_can_scroll = content_height > card_rows_available;
    if controller_available {
        render_footer(
            &mut lines,
            &mut hit_regions,
            rows - 1,
            cols,
            theme,
            metadata_controls,
            inspected_node,
            rail_can_scroll,
        );
    } else {
        lines[rows - 1] = style_body_text(
            pad_to_width(&truncate_to_width("controller unavailable", cols), cols),
            theme,
        );
    }

    RenderedRail {
        lines,
        hit_regions,
        visible_cards,
        content_height,
        available_rows: card_rows_available,
    }
}

/// Push N rows to `lines` rendering the meta panel for a node. Each row is:
/// `<outer_indent spaces><red │><space><content>`. Content is taken from the
/// supplied panel_lines (typically from `group_metadata_block` or
/// `tab_metadata_block`).
fn append_meta_panel(
    lines: &mut Vec<String>,
    outer_indent: usize,
    cols: usize,
    panel_lines: Vec<String>,
    theme: Option<RenderTheme>,
) {
    let outer_indent = outer_indent.min(cols);
    let bar_col = outer_indent;
    let content_col = (outer_indent + 2).min(cols);
    let content_width = cols.saturating_sub(content_col);
    for content in panel_lines {
        if content_width == 0 {
            continue;
        }
        let mut line = " ".repeat(bar_col);
        line.push_str(&style_meta_panel_bar(theme));
        line.push(' ');
        let text = truncate_to_width(&content, content_width);
        let padded = pad_to_width(&text, content_width);
        line.push_str(&style_body_text(padded, theme));
        lines.push(line);
    }
}

fn style_meta_panel_bar(theme: Option<RenderTheme>) -> String {
    if theme.is_none() {
        return "│".to_owned();
    }
    Style::new().fg(Color::Red).bold().paint("│").to_string()
}

fn group_metadata_block(group: &RenderGroup) -> Vec<String> {
    let mut lines = vec![];
    push_metadata_text_line(
        &mut lines,
        0,
        "group",
        metadata_text(&group.metadata, "group.label").unwrap_or(&group.label),
    );
    push_metadata_section_header(&mut lines, 2, "metadata");
    push_metadata_text_line(
        &mut lines,
        4,
        "group.full_label",
        metadata_text(&group.metadata, "group.full_label").unwrap_or(&group.full_label),
    );
    push_metadata_text_line(
        &mut lines,
        4,
        "group.tab_count",
        &metadata_display_value(&group.metadata, "group.tab_count")
            .unwrap_or_else(|| group.tab_count.to_string()),
    );
    let excluded_keys = ["group.label", "group.full_label", "group.tab_count"]
        .into_iter()
        .chain(group.path.0.iter().map(|segment| segment.key.as_str()))
        .collect::<Vec<_>>();
    push_additional_metadata_lines(&mut lines, 4, &group.metadata, &excluded_keys);
    push_metadata_source_detail_lines(&mut lines, 2, &group.metadata_sources);
    push_reachable_identity_lines(&mut lines, 2, &group.reachable_identities);
    let template_rows: Vec<Vec<String>> = [(
        "group_header",
        group.templates.group_header.as_ref(),
        TemplateRenderContext {
            slot: TemplateSlot::GroupHeader,
            node_kind: RenderNodeKind::Group,
            metadata: &group.metadata,
            collapsed: group.collapsed,
            active_tab_name: active_tab_name(&group.children),
        },
    )]
    .into_iter()
    .filter_map(|(name, slot, ctx)| template_diagnostic_row(name, slot, ctx))
    .collect();
    if !template_rows.is_empty() {
        push_metadata_section_header(&mut lines, 2, "templates");
        push_aligned_table(&mut lines, 4, TEMPLATE_TABLE_COLUMNS, &template_rows);
    }
    push_metadata_section_header(&mut lines, 2, "group_path");
    push_group_path_metadata(&mut lines, 4, &group.path);
    if group.conflated_paths.len() > 1 {
        push_metadata_section_header(&mut lines, 2, "conflated_paths");
        for path in &group.conflated_paths {
            push_metadata_text_line(&mut lines, 4, "path", &format_group_path(path));
        }
    }
    lines
}

fn tab_metadata_block(tab: &RenderTab) -> Vec<String> {
    let mut lines = vec![];
    let indent = tab.indent;
    let card = &tab.card;
    push_metadata_text_line(
        &mut lines,
        indent,
        "tab",
        metadata_text(&card.metadata, "zellij.tab.name").unwrap_or(&card.name),
    );
    push_metadata_section_header(&mut lines, indent + 2, "metadata");
    push_metadata_text_line(
        &mut lines,
        indent + 4,
        "zellij.tab.id",
        &metadata_display_value(&card.metadata, "zellij.tab.id")
            .unwrap_or_else(|| card.tab_id.to_string()),
    );
    push_metadata_text_line(
        &mut lines,
        indent + 4,
        "zellij.tab.position",
        &metadata_display_value(&card.metadata, "zellij.tab.position")
            .unwrap_or_else(|| card.position.to_string()),
    );
    push_metadata_text_line(
        &mut lines,
        indent + 4,
        "zellij.tab.active",
        &metadata_display_value(&card.metadata, "zellij.tab.active")
            .unwrap_or_else(|| bool_text(card.active).to_owned()),
    );
    push_metadata_text_line(
        &mut lines,
        indent + 4,
        "rail.tab.pinned",
        &metadata_display_value(&card.metadata, "rail.tab.pinned")
            .unwrap_or_else(|| bool_text(card.pinned).to_owned()),
    );
    push_additional_metadata_lines(
        &mut lines,
        indent + 4,
        &card.metadata,
        &[
            "zellij.tab.id",
            "zellij.tab.position",
            "zellij.tab.name",
            "zellij.tab.active",
            "rail.tab.pinned",
            "group.label",
            "group.full_label",
            "status.priority",
            "status.title",
            "status.detail",
            "status.source_pane",
        ],
    );
    push_metadata_source_detail_lines(&mut lines, indent + 2, &card.metadata_sources);
    push_reachable_identity_lines(&mut lines, indent + 2, &card.reachable_identities);
    let mut template_rows: Vec<Vec<String>> = vec![];
    if let Some(row) = template_diagnostic_row(
        "tab_title",
        card.templates.tab_title.as_ref(),
        TemplateRenderContext {
            slot: TemplateSlot::TabTitle,
            node_kind: RenderNodeKind::Tab,
            metadata: &card.metadata,
            collapsed: false,
            active_tab_name: None,
        },
    ) {
        template_rows.push(row);
    }
    if card.status.is_some() {
        if let Some(row) = template_diagnostic_row(
            "tab_status",
            card.templates.tab_status.as_ref(),
            TemplateRenderContext {
                slot: TemplateSlot::TabStatus,
                node_kind: RenderNodeKind::Tab,
                metadata: &card.metadata,
                collapsed: false,
                active_tab_name: None,
            },
        ) {
            template_rows.push(row);
        }
    }
    if !template_rows.is_empty() {
        push_metadata_section_header(&mut lines, indent + 2, "templates");
        push_aligned_table(
            &mut lines,
            indent + 4,
            TEMPLATE_TABLE_COLUMNS,
            &template_rows,
        );
    }
    if let Some(grouping) = tab.grouping.as_ref() {
        push_metadata_section_header(&mut lines, indent + 2, "grouping");
        push_metadata_text_line(&mut lines, indent + 4, "group.label", &grouping.label);
        push_metadata_text_line(
            &mut lines,
            indent + 4,
            "group.full_label",
            &grouping.full_label,
        );
        push_group_path_metadata(&mut lines, indent + 4, &grouping.path);
    }
    if let Some(status) = card.status.as_ref() {
        push_metadata_section_header(&mut lines, indent + 2, "status");
        push_metadata_text_line(
            &mut lines,
            indent + 4,
            "status.priority",
            &format!("{:?}", status.priority).to_ascii_lowercase(),
        );
        push_metadata_text_line(&mut lines, indent + 4, "status.title", &status.title);
        if let Some(detail) = status.detail.as_ref() {
            push_metadata_text_line(&mut lines, indent + 4, "status.detail", detail);
        }
        push_metadata_text_line(
            &mut lines,
            indent + 4,
            "status.source_pane",
            &format_pane_target(status.source_pane),
        );
    }
    lines
}

fn push_group_path_metadata(lines: &mut Vec<String>, indent: usize, path: &GroupPath) {
    for segment in &path.0 {
        push_metadata_text_line(
            lines,
            indent,
            &segment.key,
            &format_metadata_value(&segment.value),
        );
    }
}

fn format_group_path(path: &GroupPath) -> String {
    path.0
        .iter()
        .map(|segment| format!("{}={}", segment.key, format_metadata_value(&segment.value)))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn push_additional_metadata_lines(
    lines: &mut Vec<String>,
    indent: usize,
    metadata: &RenderMetadata,
    excluded_keys: &[&str],
) {
    for (key, value) in metadata {
        if excluded_keys.contains(&key.as_str()) {
            continue;
        }
        push_metadata_text_line(lines, indent, key, &format_metadata_value(value));
    }
}

fn push_metadata_source_detail_lines(
    lines: &mut Vec<String>,
    indent: usize,
    source_entries: &RenderMetadataSources,
) {
    if source_entries.values().all(Vec::is_empty) {
        return;
    }
    let mut rows: Vec<Vec<String>> = vec![];
    for (key, entries) in source_entries {
        for entry in entries {
            rows.push(vec![
                key.clone(),
                entry.source_id.clone(),
                format_metadata_value(&entry.entry.value),
                entry.entry.updated_at.to_string(),
                entry
                    .entry
                    .ttl_ms
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                entry.entry.precedence.to_string(),
                entry.entry.ordinal.to_string(),
            ]);
        }
    }
    if rows.is_empty() {
        return;
    }
    push_metadata_section_header(lines, indent, "sources");
    const COLUMNS: &[TableColumn] = &[
        TableColumn {
            header: "key",
            align: ColumnAlign::Left,
        },
        TableColumn {
            header: "src",
            align: ColumnAlign::Left,
        },
        TableColumn {
            header: "value",
            align: ColumnAlign::Left,
        },
        TableColumn {
            header: "updated",
            align: ColumnAlign::Right,
        },
        TableColumn {
            header: "ttl",
            align: ColumnAlign::Right,
        },
        TableColumn {
            header: "prec",
            align: ColumnAlign::Right,
        },
        TableColumn {
            header: "ord",
            align: ColumnAlign::Right,
        },
    ];
    push_aligned_table(lines, indent + 2, COLUMNS, &rows);
}

fn push_reachable_identity_lines(
    lines: &mut Vec<String>,
    indent: usize,
    reachable_identities: &[ReachableMetadataIdentity],
) {
    if reachable_identities.is_empty() {
        return;
    }
    push_metadata_section_header(lines, indent, "reachable_identities");
    let rows: Vec<Vec<String>> = reachable_identities
        .iter()
        .map(|reachable| {
            vec![
                reachable.identity.key.clone(),
                format_metadata_value(&reachable.identity.value),
                reachable.distance.to_string(),
            ]
        })
        .collect();
    const COLUMNS: &[TableColumn] = &[
        TableColumn {
            header: "identity",
            align: ColumnAlign::Left,
        },
        TableColumn {
            header: "value",
            align: ColumnAlign::Left,
        },
        TableColumn {
            header: "distance",
            align: ColumnAlign::Right,
        },
    ];
    push_aligned_table(lines, indent + 2, COLUMNS, &rows);
}

fn push_metadata_text_line(lines: &mut Vec<String>, indent: usize, key: &str, value: &str) {
    lines.push(format!("{}{}: {}", " ".repeat(indent), key, value));
}

fn push_metadata_section_header(lines: &mut Vec<String>, indent: usize, label: &str) {
    lines.push(format!("{}[{}]", " ".repeat(indent), label));
}

#[derive(Debug, Clone, Copy)]
enum ColumnAlign {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
struct TableColumn {
    header: &'static str,
    align: ColumnAlign,
}

/// Render a header row plus N data rows as a single aligned text table.
/// Per-column widths span all rows so columns line up across the whole
/// section (unlike the previous per-key micro-tables). Columns whose
/// values are all empty are dropped entirely.
fn push_aligned_table(
    lines: &mut Vec<String>,
    indent: usize,
    columns: &[TableColumn],
    rows: &[Vec<String>],
) {
    if rows.is_empty() || columns.is_empty() {
        return;
    }
    // Drop columns where every row is empty.
    let keep: Vec<usize> = (0..columns.len())
        .filter(|&i| {
            rows.iter()
                .any(|row| row.get(i).map(|c| !c.is_empty()).unwrap_or(false))
        })
        .collect();
    if keep.is_empty() {
        return;
    }
    let widths: Vec<usize> = keep
        .iter()
        .map(|&i| {
            let header_len = columns[i].header.len();
            rows.iter()
                .map(|row| row.get(i).map(String::len).unwrap_or(0))
                .max()
                .unwrap_or(0)
                .max(header_len)
        })
        .collect();
    let pad = " ".repeat(indent);
    let format_cell = |text: &str, width: usize, align: ColumnAlign| match align {
        ColumnAlign::Left => format!("{text:<width$}"),
        ColumnAlign::Right => format!("{text:>width$}"),
    };
    // Header
    let mut header = pad.clone();
    for (slot, &i) in keep.iter().enumerate() {
        if slot > 0 {
            header.push_str("  ");
        }
        header.push_str(&format_cell(
            columns[i].header,
            widths[slot],
            columns[i].align,
        ));
    }
    lines.push(header);
    // Rows
    for row in rows {
        let mut line = pad.clone();
        for (slot, &i) in keep.iter().enumerate() {
            if slot > 0 {
                line.push_str("  ");
            }
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            line.push_str(&format_cell(cell, widths[slot], columns[i].align));
        }
        lines.push(line);
    }
}

fn format_metadata_value(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Text(value) => value.clone(),
        MetadataValue::Bool(value) => bool_text(*value).to_owned(),
        MetadataValue::Integer(value) => value.to_string(),
        MetadataValue::StringList(values) => values.join(", "),
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

/// Returns a row of cells for a single template slot's diagnostic, suitable
/// for stacking in a slot-rowed templates table. Columns:
/// [slot, template, fields, sizing, specificity, predicates, candidates].
/// Returns None when there's nothing to show for that slot. Empty cells
/// where N/A; `push_aligned_table` drops fully-empty columns.
fn template_diagnostic_row(
    slot_name: &str,
    resolved_slot: Option<&ResolvedTemplateSlot>,
    context: TemplateRenderContext<'_>,
) -> Option<Vec<String>> {
    if let Some(slot) = resolved_slot {
        let fields = slot
            .fields
            .iter()
            .map(|field| format!("{}({})", field.text, field.priority))
            .collect::<Vec<_>>()
            .join(", ");
        return Some(vec![
            slot_name.to_owned(),
            slot.template_name.clone(),
            fields,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ]);
    }
    if let Some(template) = matched_template(context) {
        return Some(vec![
            slot_name.to_owned(),
            template.name.to_owned(),
            String::new(),
            template.sizing.as_text().to_owned(),
            template.specificity().to_string(),
            template.predicates_text(),
            matched_template_candidates_text(context),
        ]);
    }
    None
}

const TEMPLATE_TABLE_COLUMNS: &[TableColumn] = &[
    TableColumn {
        header: "slot",
        align: ColumnAlign::Left,
    },
    TableColumn {
        header: "template",
        align: ColumnAlign::Left,
    },
    TableColumn {
        header: "fields",
        align: ColumnAlign::Left,
    },
    TableColumn {
        header: "sizing",
        align: ColumnAlign::Left,
    },
    TableColumn {
        header: "specificity",
        align: ColumnAlign::Right,
    },
    TableColumn {
        header: "predicates",
        align: ColumnAlign::Left,
    },
    TableColumn {
        header: "candidates",
        align: ColumnAlign::Left,
    },
];

fn bool_text(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn format_pane_target(pane_target: PaneTarget) -> String {
    match pane_target {
        PaneTarget::Terminal(id) => format!("terminal:{id}"),
        PaneTarget::Plugin(id) => format!("plugin:{id}"),
    }
}

fn style_body_text(line: String, theme: Option<RenderTheme>) -> String {
    let Some(theme) = theme else {
        return line;
    };
    foreground_style(theme.body_foreground)
        .paint(line)
        .to_string()
}

fn style_border_text(line: String, active: bool, theme: Option<RenderTheme>) -> String {
    let Some(theme) = theme else {
        return line;
    };
    foreground_style(if active {
        theme.active_border
    } else {
        theme.inactive_border
    })
    .bold()
    .paint(line)
    .to_string()
}

fn style_title_text(line: String, active: bool, theme: Option<RenderTheme>) -> String {
    let Some(theme) = theme else {
        return line;
    };
    foreground_style(if active {
        theme.active_border
    } else {
        theme.inactive_border
    })
    .bold()
    .paint(line)
    .to_string()
}

fn style_group_header_text(
    line: String,
    contains_active_tab: bool,
    theme: Option<RenderTheme>,
) -> String {
    let Some(theme) = theme else {
        return line;
    };
    if contains_active_tab {
        foreground_style(theme.active_border)
            .bold()
            .paint(line)
            .to_string()
    } else {
        foreground_style(theme.body_foreground)
            .paint(line)
            .to_string()
    }
}

fn foreground_style(color: PaletteColor) -> Style {
    Style::new().fg(ansi_color(color))
}

fn style_cell(foreground: PaletteColor, background: PaletteColor, bold: bool) -> Style {
    let style = Style::new()
        .fg(ansi_color(foreground))
        .on(ansi_color(background));
    if bold {
        style.bold()
    } else {
        style
    }
}

fn ansi_color(color: PaletteColor) -> Color {
    match color {
        PaletteColor::Rgb((r, g, b)) => Color::RGB(r, g, b),
        PaletteColor::EightBit(color) => Color::Fixed(color),
    }
}

pub fn hit_at(hit_regions: &[HitRegion], row: usize, col: usize) -> Option<HitRegion> {
    // Pick the smallest matching region. Card-wide SwitchTab hits would
    // otherwise shadow single-cell controls (tristate glyph, gear, etc.)
    // because they're registered after the controls. Ties broken by
    // registration order (later wins) via `rev`.
    hit_regions
        .iter()
        .rev()
        .filter(|region| {
            row >= region.row_start
                && row <= region.row_end
                && col >= region.col_start
                && col <= region.col_end
        })
        .min_by_key(|region| {
            let rows = region.row_end.saturating_sub(region.row_start) + 1;
            let cols = region.col_end.saturating_sub(region.col_start) + 1;
            rows * cols
        })
        .cloned()
}

fn render_cards(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    cards: &[RenderCard],
    available_rows: usize,
    cols: usize,
    controller_available: bool,
    config: RailConfig,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) {
    match config.structure {
        RailStructure::JoinedCells => render_joined_cells(
            lines,
            hit_regions,
            visible_cards,
            cards,
            available_rows,
            cols,
            controller_available,
            config.sizing,
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
        ),
        RailStructure::SplitAroundActive => render_split_around_active(
            lines,
            hit_regions,
            visible_cards,
            cards,
            available_rows,
            cols,
            controller_available,
            config.sizing,
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
        ),
        RailStructure::BoxPerTab => render_box_per_tab(
            lines,
            hit_regions,
            visible_cards,
            cards,
            available_rows,
            cols,
            controller_available,
            config.sizing,
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
        ),
    }
}

fn render_nodes(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    nodes: &[RenderNode],
    available_rows: usize,
    cols: usize,
    controller_available: bool,
    config: RailConfig,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
    root_settings: InheritedRailSettings,
    rail_scroll_offset: isize,
) -> usize {
    if nodes.is_empty() || available_rows == 0 {
        return 0;
    }

    // Root's MetaChildren cascades to descendants.
    let root_meta_children = metadata_controls.propagates_to_children(&NodeKey::Root, false);

    if let Some(top_tabs) = top_level_tabs(nodes) {
        let cards = top_tabs
            .iter()
            .map(|tab| {
                let mut card = tab.card.clone();
                let key = NodeKey::Tab(card.tab_id);
                if metadata_controls.effective_show(&key, root_meta_children) {
                    card.meta_panel = Some(tab_metadata_block(tab));
                }
                card
            })
            .collect::<Vec<_>>();
        render_cards(
            lines,
            hit_regions,
            visible_cards,
            &cards,
            available_rows,
            cols,
            controller_available,
            config,
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
        );
        // Top-level (ungrouped) path doesn't go through copy_visible_buffer;
        // visible_cells already fits content. No scroll concept here yet.
        return 0;
    }
    let mut buffered_lines = vec![];
    let mut buffered_hits = vec![];
    let mut buffered_cards = vec![];
    render_nodes_to_buffer(
        &mut buffered_lines,
        &mut buffered_hits,
        &mut buffered_cards,
        nodes,
        cols,
        controller_available,
        config,
        theme,
        terminal_cell_size,
        template_catalog,
        &BTreeSet::new(),
        metadata_controls,
        inspected_node,
        root_meta_children,
        root_settings,
    );
    let content_height = buffered_lines.len();
    copy_visible_buffer(
        lines,
        hit_regions,
        visible_cards,
        buffered_lines,
        buffered_hits,
        buffered_cards,
        active_tab_id(nodes),
        available_rows,
        rail_scroll_offset,
    );
    content_height
}

fn root_inherited_settings_for_model(model: Option<&ControllerViewModel>) -> InheritedRailSettings {
    let Some(model) = model else {
        return InheritedRailSettings::default();
    };
    let Some(metadata) = model
        .resolved_metadata
        .iter()
        .find(|metadata| metadata.target == MetadataTarget::Root)
    else {
        return InheritedRailSettings::default();
    };
    metadata.values.iter().fold(
        InheritedRailSettings::default(),
        |settings, (key, entry)| {
            if key == RAIL_CHILD_LAYOUT_METADATA_KEY {
                ChildLayoutSetting::from_metadata_value(&entry.value)
                    .map(|child_layout| InheritedRailSettings { child_layout })
                    .unwrap_or(settings)
            } else {
                settings
            }
        },
    )
}

fn top_level_tabs(nodes: &[RenderNode]) -> Option<Vec<&RenderTab>> {
    nodes
        .iter()
        .map(|node| match node {
            RenderNode::Tab(tab) if tab.indent == 0 => Some(tab),
            _ => None,
        })
        .collect()
}

fn render_nodes_to_buffer(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    nodes: &[RenderNode],
    cols: usize,
    controller_available: bool,
    config: RailConfig,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    ancestor_template_fields: &BTreeSet<ResolvedTemplateFieldSource>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
    ancestor_meta_children: bool,
    inherited_settings: InheritedRailSettings,
) {
    let mut pending_tabs = vec![];
    for node in nodes {
        match node {
            RenderNode::Tab(tab) => pending_tabs.push(tab.clone()),
            RenderNode::Group(group) => {
                let _tab_allocations = append_tab_run(
                    lines,
                    hit_regions,
                    visible_cards,
                    &pending_tabs,
                    cols,
                    controller_available,
                    config,
                    theme,
                    terminal_cell_size,
                    template_catalog,
                    metadata_controls,
                    inspected_node,
                    ancestor_meta_children,
                );
                pending_tabs.clear();
                let (visible_header_sources, _group_allocation) = append_group_header(
                    lines,
                    hit_regions,
                    group,
                    cols,
                    theme,
                    template_catalog,
                    ancestor_template_fields,
                    metadata_controls,
                    inspected_node,
                );
                let group_key = NodeKey::Group(group.path.clone());
                if metadata_controls.effective_show(&group_key, ancestor_meta_children) {
                    append_meta_panel(
                        lines,
                        group.indent + 2,
                        cols,
                        group_metadata_block(group),
                        theme,
                    );
                }
                if !group.collapsed {
                    let mut child_ancestor_template_fields = ancestor_template_fields.clone();
                    child_ancestor_template_fields.extend(visible_header_sources);
                    let child_meta_children = metadata_controls
                        .propagates_to_children(&group_key, ancestor_meta_children);
                    let child_settings = inherited_settings.with_node_metadata(&group.metadata);
                    if child_settings.child_layout == ChildLayoutSetting::CompactStrip {
                        let (direct_tabs, remaining_children) =
                            direct_tabs_and_child_groups(&group.children);
                        append_compact_tab_strip(
                            lines,
                            hit_regions,
                            &direct_tabs,
                            cols,
                            theme,
                            template_catalog,
                        );
                        render_nodes_to_buffer(
                            lines,
                            hit_regions,
                            visible_cards,
                            &remaining_children,
                            cols,
                            controller_available,
                            config,
                            theme,
                            terminal_cell_size,
                            template_catalog,
                            &child_ancestor_template_fields,
                            metadata_controls,
                            inspected_node,
                            child_meta_children,
                            child_settings,
                        );
                        continue;
                    }
                    render_nodes_to_buffer(
                        lines,
                        hit_regions,
                        visible_cards,
                        &group.children,
                        cols,
                        controller_available,
                        config,
                        theme,
                        terminal_cell_size,
                        template_catalog,
                        &child_ancestor_template_fields,
                        metadata_controls,
                        inspected_node,
                        child_meta_children,
                        child_settings,
                    );
                }
            }
        }
    }
    let _trailing_tab_allocations = append_tab_run(
        lines,
        hit_regions,
        visible_cards,
        &pending_tabs,
        cols,
        controller_available,
        config,
        theme,
        terminal_cell_size,
        template_catalog,
        metadata_controls,
        inspected_node,
        ancestor_meta_children,
    );
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct NodeRowAllocation {
    pub key: NodeKey,
    pub rows: std::ops::Range<usize>,
}

fn append_group_header(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    group: &RenderGroup,
    cols: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
    ancestor_template_fields: &BTreeSet<ResolvedTemplateFieldSource>,
    _metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) -> (BTreeSet<ResolvedTemplateFieldSource>, NodeRowAllocation) {
    let row = lines.len();
    let indent = group.indent.min(cols);
    let inner_width = cols.saturating_sub(indent);
    // Reserve the last 2 cells when the metadata root toggle is on: glyph
    // at width-2 (vertically aligning with the tab cards' glyphs, which sit
    // just inside the `┐` corner), and a `─` filler at width-1 so the
    // horizontal line visually continues past the glyph the way the tab
    // border's corner does.
    let glyph_reserved = inner_width >= 4;
    let header_width = if glyph_reserved {
        inner_width.saturating_sub(2)
    } else {
        inner_width
    };
    let mut line = " ".repeat(indent);
    let rendered = group_header_line(
        &group.metadata,
        group.collapsed,
        contains_active_tab(&group.children),
        active_tab_name(&group.children),
        group.templates.group_header.as_ref(),
        header_width,
        theme,
        template_catalog,
        ancestor_template_fields,
    );
    line.push_str(&rendered.text);
    if glyph_reserved {
        let key = NodeKey::Group(group.path.clone());
        let glyph = inspect_node_glyph(inspected_node, &key);
        let styled_tail = style_group_header_text(
            format!("{glyph}─"),
            contains_active_tab(&group.children),
            theme,
        );
        line.push_str(&styled_tail);
        hit_regions.push(HitRegion {
            row_start: row,
            row_end: row,
            col_start: indent + inner_width - 2,
            col_end: indent + inner_width - 2,
            tab_id: 0,
            tab_position: 0,
            group_path: Some(group.path.clone()),
            inspect_target: Some(NodeKey::Group(group.path.clone())),
            action: HitAction::InspectNode,
        });
    }
    lines.push(line);
    hit_regions.push(HitRegion {
        row_start: row,
        row_end: row,
        col_start: indent,
        col_end: indent,
        tab_id: 0,
        tab_position: 0,
        group_path: Some(group.path.clone()),
        inspect_target: None,
        action: HitAction::ToggleGroup,
    });
    let allocation = NodeRowAllocation {
        key: NodeKey::Group(group.path.clone()),
        rows: row..row + 1,
    };
    (rendered.visible_sources, allocation)
}

fn child_layout_from_metadata(metadata: &RenderMetadata) -> Option<ChildLayoutSetting> {
    ChildLayoutSetting::from_metadata_value(metadata.get(RAIL_CHILD_LAYOUT_METADATA_KEY)?)
}

fn direct_tabs_and_child_groups(children: &[RenderNode]) -> (Vec<RenderTab>, Vec<RenderNode>) {
    children
        .iter()
        .cloned()
        .fold((vec![], vec![]), |mut split, child| {
            match child {
                RenderNode::Tab(tab) => split.0.push(tab),
                RenderNode::Group(group) => split.1.push(RenderNode::Group(group)),
            }
            split
        })
}

fn append_compact_tab_strip(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    tabs: &[RenderTab],
    cols: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
) {
    if tabs.is_empty() {
        return;
    }
    let indent = tabs
        .iter()
        .map(|tab| tab.indent)
        .min()
        .unwrap_or(0)
        .min(cols.saturating_sub(1));
    let inner_width = cols.saturating_sub(indent);
    if inner_width == 0 {
        return;
    }

    let items = tabs
        .iter()
        .map(|tab| SegmentItem {
            label: tab_title_with_template_catalog(&tab.card, template_catalog),
            active: tab.card.active,
        })
        .collect::<Vec<_>>();
    let rendered = render_compact_segment_run(&items, inner_width, theme);
    let base_row = lines.len();
    for line in rendered.lines {
        lines.push(format!("{}{}", " ".repeat(indent), line));
    }
    for hit in rendered.hits {
        let Some(tab) = tabs.get(hit.index) else {
            continue;
        };
        hit_regions.push(HitRegion {
            row_start: base_row + hit.row,
            row_end: base_row + hit.row,
            col_start: indent + hit.col_start,
            col_end: indent + hit.col_end,
            tab_id: tab.card.tab_id,
            tab_position: tab.card.position,
            group_path: tab.parent_path.clone(),
            inspect_target: None,
            action: HitAction::SwitchTab,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactSegmentRun {
    lines: Vec<String>,
    hits: Vec<segment_bar::SegmentHitBox>,
}

fn render_compact_segment_run(
    items: &[SegmentItem],
    width: usize,
    theme: Option<RenderTheme>,
) -> CompactSegmentRun {
    if width == 0 || items.is_empty() {
        return CompactSegmentRun {
            lines: vec![],
            hits: vec![],
        };
    }
    if theme.is_none() {
        let rendered = segment_bar::render_wrapped(items, &segment_bar::ZellijRibbonStyle, width);
        return CompactSegmentRun {
            lines: rendered.lines,
            hits: rendered.hits,
        };
    }

    let mut lines = vec![];
    let mut hits = vec![];
    let mut line = String::new();
    let mut col = 0usize;
    let mut row = 0usize;

    for (index, item) in items.iter().enumerate() {
        let segment_width = compact_segment_width(&item.label);
        if !line.is_empty() && segment_width > width.saturating_sub(col) {
            lines.push(pad_styled_line_to_width(&line, col, width));
            line.clear();
            col = 0;
            row = row.saturating_add(1);
        }

        let item_start = col;
        let remaining_width = width.saturating_sub(col);
        if remaining_width == 0 {
            break;
        }
        let (segment, visible_width) = render_compact_segment(item, remaining_width, theme);
        if visible_width == 0 {
            continue;
        }
        line.push_str(&segment);
        col = col.saturating_add(visible_width);
        hits.push(segment_bar::SegmentHitBox {
            index,
            row,
            col_start: item_start,
            col_end: col.saturating_sub(1),
        });
    }

    if !line.is_empty() {
        lines.push(pad_styled_line_to_width(&line, col, width));
    }
    CompactSegmentRun { lines, hits }
}

fn pad_styled_line_to_width(line: &str, visible_width: usize, width: usize) -> String {
    if visible_width >= width {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len() + width - visible_width);
    out.push_str(line);
    out.push_str(&" ".repeat(width - visible_width));
    out
}

fn compact_segment_width(label: &str) -> usize {
    label.width().saturating_add(4)
}

fn render_compact_segment(
    item: &SegmentItem,
    max_width: usize,
    theme: Option<RenderTheme>,
) -> (String, usize) {
    if max_width == 0 {
        return (String::new(), 0);
    }
    let visible_width = compact_segment_width(&item.label).min(max_width);
    if theme.is_none() {
        let plain = format!(" {} ", item.label);
        return (truncate_to_width(&plain, max_width), visible_width);
    }

    let Some(theme) = theme else {
        unreachable!();
    };
    let (segment_bg, segment_fg) = if item.active {
        (
            theme.segment_active_background,
            theme.segment_active_foreground,
        )
    } else {
        (
            theme.segment_inactive_background,
            theme.segment_inactive_foreground,
        )
    };
    let between_bg = theme.segment_between_background;
    let mut remaining_width = visible_width;
    let mut rendered = String::new();
    if remaining_width > 0 {
        rendered.push_str(
            &style_cell(between_bg, segment_bg, true)
                .paint("")
                .to_string(),
        );
        remaining_width = remaining_width.saturating_sub(1);
    }
    if remaining_width == 1 {
        rendered.push_str(
            &style_cell(segment_bg, between_bg, true)
                .paint("")
                .to_string(),
        );
        return (rendered, visible_width);
    }
    if remaining_width > 1 {
        let text_width = remaining_width.saturating_sub(1);
        let text = truncate_to_width(&format!(" {} ", item.label), text_width);
        rendered.push_str(
            &style_cell(segment_fg, segment_bg, true)
                .paint(text)
                .to_string(),
        );
        rendered.push_str(
            &style_cell(segment_bg, between_bg, true)
                .paint("")
                .to_string(),
        );
    }
    (rendered, visible_width)
}

fn append_tab_run(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    tabs: &[RenderTab],
    cols: usize,
    controller_available: bool,
    config: RailConfig,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
    ancestor_meta_children: bool,
) -> Vec<NodeRowAllocation> {
    if tabs.is_empty() {
        return vec![];
    }
    let indent = tabs
        .iter()
        .map(|tab| tab.indent)
        .min()
        .unwrap_or(0)
        .min(cols.saturating_sub(1));
    let inner_cols = cols.saturating_sub(indent);
    let cards = tabs
        .iter()
        .map(|tab| {
            let mut card = tab.card.clone();
            let key = NodeKey::Tab(card.tab_id);
            if metadata_controls.effective_show(&key, ancestor_meta_children) {
                card.meta_panel = Some(tab_metadata_block(tab));
            }
            card
        })
        .collect::<Vec<_>>();
    let run_height = generous_card_run_height(&cards, config.sizing);
    let mut local_lines = vec![blank(inner_cols); run_height];
    let mut local_hits = vec![];
    let mut local_cards = vec![];
    render_cards(
        &mut local_lines,
        &mut local_hits,
        &mut local_cards,
        &cards,
        run_height,
        inner_cols,
        controller_available,
        config,
        theme,
        terminal_cell_size,
        template_catalog,
        metadata_controls,
        inspected_node,
    );
    let used_rows = local_lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map(|index| index + 1)
        .unwrap_or(0);
    let row_offset = lines.len();
    // Per-tab row ranges derived from the SwitchTab hit regions that
    // add_card_metadata emits for every rendered card. The hit region's
    // row_start/row_end span the card body inclusive.
    let allocations = local_hits
        .iter()
        .filter(|hit| hit.action == HitAction::SwitchTab)
        .map(|hit| NodeRowAllocation {
            key: NodeKey::Tab(hit.tab_id),
            rows: (hit.row_start + row_offset)..(hit.row_end + row_offset + 1),
        })
        .collect::<Vec<_>>();
    lines.extend(
        local_lines
            .into_iter()
            .take(used_rows)
            .map(|line| format!("{}{}", " ".repeat(indent), line)),
    );
    hit_regions.extend(local_hits.into_iter().map(|mut hit| {
        hit.row_start += row_offset;
        hit.row_end += row_offset;
        hit.col_start += indent;
        hit.col_end += indent;
        hit
    }));
    visible_cards.extend(local_cards.into_iter().map(|mut visible_card| {
        visible_card.row_start += row_offset;
        visible_card.status_row = visible_card.status_row.map(|row| row + row_offset);
        if let Some(rect) = visible_card.status_icon_rect.as_mut() {
            rect.x += indent;
            rect.y += row_offset;
        }
        visible_card
    }));
    allocations
}

fn generous_card_run_height(cards: &[RenderCard], sizing: RailSizingPreset) -> usize {
    cards
        .iter()
        .map(|card| cell_height(card, sizing, false).max(ACTIVE_CELL_HEIGHT) + 1)
        .sum::<usize>()
        + 1
}

fn copy_visible_buffer(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    buffered_lines: Vec<String>,
    buffered_hits: Vec<HitRegion>,
    buffered_cards: Vec<VisibleCard>,
    active_tab_id: Option<u64>,
    available_rows: usize,
    user_scroll_offset: isize,
) {
    if buffered_lines.is_empty() || available_rows == 0 {
        return;
    }
    let active_row = active_tab_id
        .and_then(|tab_id| {
            buffered_hits
                .iter()
                .find(|hit| hit.action == HitAction::SwitchTab && hit.tab_id == tab_id)
                .map(|hit| hit.row_start)
        })
        .or_else(|| {
            buffered_cards
                .iter()
                .find(|card| card.status_priority.is_some() || card.status_row.is_some())
                .map(|card| card.row_start)
        })
        .or_else(|| {
            buffered_hits
                .iter()
                .find(|hit| hit.action == HitAction::SwitchTab)
                .map(|hit| hit.row_start)
        })
        .unwrap_or(0);
    let max_start = buffered_lines.len().saturating_sub(available_rows);
    let auto_start = active_row.saturating_sub(available_rows / 2).min(max_start);
    let visible_start =
        ((auto_start as isize) + user_scroll_offset).clamp(0, max_start as isize) as usize;
    let visible_end = buffered_lines.len().min(visible_start + available_rows);

    for (output_row, line) in buffered_lines[visible_start..visible_end]
        .iter()
        .enumerate()
    {
        lines[output_row] = line.clone();
    }
    hit_regions.extend(buffered_hits.into_iter().filter_map(|mut hit| {
        if hit.row_end < visible_start || hit.row_start >= visible_end {
            return None;
        }
        hit.row_start = hit.row_start.saturating_sub(visible_start);
        hit.row_end = hit.row_end.min(visible_end - 1) - visible_start;
        Some(hit)
    }));
    visible_cards.extend(buffered_cards.into_iter().filter_map(|mut visible_card| {
        if visible_card.row_start < visible_start || visible_card.row_start >= visible_end {
            return None;
        }
        visible_card.row_start -= visible_start;
        visible_card.status_row = visible_card
            .status_row
            .filter(|row| *row >= visible_start && *row < visible_end)
            .map(|row| row - visible_start);
        if let Some(rect) = visible_card.status_icon_rect.as_mut() {
            if rect.y < visible_start || rect.y >= visible_end {
                visible_card.status_icon_rect = None;
            } else {
                rect.y -= visible_start;
            }
        }
        Some(visible_card)
    }));
}

fn active_tab_id(nodes: &[RenderNode]) -> Option<u64> {
    nodes.iter().find_map(|node| match node {
        RenderNode::Tab(tab) if tab.card.active => Some(tab.card.tab_id),
        RenderNode::Tab(_) => None,
        RenderNode::Group(group) => active_tab_id(&group.children),
    })
}

fn contains_active_tab(nodes: &[RenderNode]) -> bool {
    active_tab_id(nodes).is_some()
}

fn active_tab_name(nodes: &[RenderNode]) -> Option<&str> {
    nodes.iter().find_map(|node| match node {
        RenderNode::Tab(tab) if tab.card.active => {
            metadata_text(&tab.card.metadata, "zellij.tab.name")
        }
        RenderNode::Tab(_) => None,
        RenderNode::Group(group) => active_tab_name(&group.children),
    })
}

fn render_joined_cells(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    cards: &[RenderCard],
    available_rows: usize,
    cols: usize,
    controller_available: bool,
    sizing: RailSizingPreset,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) {
    let visible = visible_cells(cards, available_rows, sizing);
    for (visible_index, (card_index, cell_height)) in visible.iter().copied().enumerate() {
        let card = &cards[card_index];
        let row = visible
            .iter()
            .take(visible_index)
            .map(|(_, height)| *height)
            .sum();
        write_tab_top_border(
            lines,
            hit_regions,
            row,
            &tab_title_with_template_catalog(card, template_catalog),
            cols,
            visible_index == 0,
            card,
            theme,
            metadata_controls,
            inspected_node,
        );
        render_card_body(
            lines,
            hit_regions,
            visible_cards,
            card,
            row,
            cell_height,
            cols,
            controller_available,
            theme,
            terminal_cell_size,
            template_catalog,
        );
    }
    if let Some(last_line) = lines.get_mut(visible.iter().map(|(_, height)| *height).sum::<usize>())
    {
        let bottom_border_active = visible
            .last()
            .map(|(card_index, _)| cards[*card_index].active)
            .unwrap_or(false);
        *last_line = bottom_border_line(cols, bottom_border_active, theme);
    }
}

fn render_split_around_active(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    cards: &[RenderCard],
    available_rows: usize,
    cols: usize,
    controller_available: bool,
    sizing: RailSizingPreset,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) {
    let Some(active_index) = cards.iter().position(|card| card.active) else {
        return render_joined_cells(
            lines,
            hit_regions,
            visible_cards,
            cards,
            available_rows,
            cols,
            controller_available,
            sizing,
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
        );
    };
    let visible = visible_cells(cards, available_rows, sizing);
    let mut row = 0;
    for (visible_index, (card_index, cell_height)) in visible.iter().copied().enumerate() {
        let card = &cards[card_index];
        if row >= lines.len() {
            break;
        }
        if card_index == active_index {
            row = render_standalone_box(
                lines,
                hit_regions,
                visible_cards,
                card,
                row,
                cell_height + 1,
                cols,
                controller_available,
                theme,
                terminal_cell_size,
                template_catalog,
                metadata_controls,
                inspected_node,
            );
            continue;
        }

        let first_in_run = visible_index == 0
            || visible
                .get(visible_index.saturating_sub(1))
                .map(|(previous_card_index, _)| *previous_card_index == active_index)
                .unwrap_or(false);
        write_tab_top_border(
            lines,
            hit_regions,
            row,
            &tab_title_with_template_catalog(card, template_catalog),
            cols,
            first_in_run,
            card,
            theme,
            metadata_controls,
            inspected_node,
        );
        render_card_body(
            lines,
            hit_regions,
            visible_cards,
            card,
            row,
            cell_height,
            cols,
            controller_available,
            theme,
            terminal_cell_size,
            template_catalog,
        );
        row += cell_height;
        let run_ends = visible
            .get(visible_index + 1)
            .map(|(next_card_index, _)| *next_card_index == active_index)
            .unwrap_or(true);
        if run_ends {
            if let Some(last_line) = lines.get_mut(row) {
                *last_line = bottom_border_line(cols, false, theme);
            }
            row += 1;
        }
    }
}

fn render_box_per_tab(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    cards: &[RenderCard],
    available_rows: usize,
    cols: usize,
    controller_available: bool,
    sizing: RailSizingPreset,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) {
    let visible = visible_boxes(cards, available_rows, sizing);
    let mut row = 0;
    for (card_index, box_height) in visible {
        row = render_standalone_box(
            lines,
            hit_regions,
            visible_cards,
            &cards[card_index],
            row,
            box_height,
            cols,
            controller_available,
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
        );
    }
}

fn render_standalone_box(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    card: &RenderCard,
    row: usize,
    box_height: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) -> usize {
    if row >= lines.len() || box_height < 2 {
        return row;
    }
    write_tab_top_border(
        lines,
        hit_regions,
        row,
        &tab_title_with_template_catalog(card, template_catalog),
        cols,
        true,
        card,
        theme,
        metadata_controls,
        inspected_node,
    );
    let total_body_rows = box_height.saturating_sub(2);
    let meta_lines = card.meta_panel.as_ref();
    let meta_rows = meta_lines.map_or(0, |panel| panel.len());
    let base_body_rows = total_body_rows.saturating_sub(meta_rows);
    let status_body_lines = body_lines(
        card,
        row,
        base_body_rows,
        cols,
        terminal_cell_size,
        template_catalog,
    );
    for body_index in 0..total_body_rows {
        let text = if body_index < base_body_rows {
            status_body_lines
                .get(body_index)
                .map(String::as_str)
                .unwrap_or_default()
        } else {
            meta_lines
                .and_then(|panel| panel.get(body_index - base_body_rows))
                .map(String::as_str)
                .unwrap_or_default()
        };
        if let Some(line) = lines.get_mut(row + 1 + body_index) {
            *line = body_line(text, cols, card.active, theme);
        }
    }
    if let Some(line) = lines.get_mut(row + box_height - 1) {
        *line = bottom_border_line(cols, card.active, theme);
    }
    let body_rows = total_body_rows;
    add_card_metadata(
        hit_regions,
        visible_cards,
        card,
        row,
        box_height,
        cols,
        controller_available,
        body_rows,
        terminal_cell_size,
    );
    row + box_height
}

fn render_card_body(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    card: &RenderCard,
    row: usize,
    cell_height: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
) {
    let total_body_rows = cell_height.saturating_sub(1);
    let meta_lines = card.meta_panel.as_ref();
    let meta_rows = meta_lines.map_or(0, |panel| panel.len());
    let base_body_rows = total_body_rows.saturating_sub(meta_rows);
    let status_body_lines = body_lines(
        card,
        row,
        base_body_rows,
        cols,
        terminal_cell_size,
        template_catalog,
    );
    for body_index in 0..total_body_rows {
        let text = if body_index < base_body_rows {
            status_body_lines
                .get(body_index)
                .map(String::as_str)
                .unwrap_or_default()
        } else {
            meta_lines
                .and_then(|panel| panel.get(body_index - base_body_rows))
                .map(String::as_str)
                .unwrap_or_default()
        };
        if let Some(line) = lines.get_mut(row + 1 + body_index) {
            *line = body_line(text, cols, card.active, theme);
        }
    }
    add_card_metadata(
        hit_regions,
        visible_cards,
        card,
        row,
        cell_height,
        cols,
        controller_available,
        total_body_rows,
        terminal_cell_size,
    );
}

fn add_card_metadata(
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    card: &RenderCard,
    row: usize,
    height: usize,
    cols: usize,
    controller_available: bool,
    body_rows: usize,
    terminal_cell_size: Option<SizeInPixels>,
) {
    hit_regions.push(HitRegion {
        row_start: row,
        row_end: row + height.saturating_sub(1),
        col_start: 0,
        col_end: cols.saturating_sub(1),
        tab_id: card.tab_id,
        tab_position: card.position,
        group_path: None,
        inspect_target: None,
        action: HitAction::SwitchTab,
    });
    if controller_available {
        hit_regions.push(HitRegion {
            row_start: row + 1,
            row_end: row + 1,
            col_start: 2,
            col_end: 7.min(cols.saturating_sub(1)),
            tab_id: card.tab_id,
            tab_position: card.position,
            group_path: None,
            inspect_target: None,
            action: HitAction::TogglePin,
        });
    }

    let status_row = card
        .status
        .as_ref()
        .and_then(|_| (body_rows > 0).then_some(row + 1));
    let status_icon_rect = card
        .status
        .as_ref()
        .and_then(|status| status_icon_rect(status, row, body_rows, cols, terminal_cell_size));
    visible_cards.push(VisibleCard {
        tab_id: card.tab_id,
        tab_position: card.position,
        row_start: row,
        status_row,
        status_icon_rect,
        status_priority: card.status.as_ref().map(|status| status.priority),
        status_icon: card.status.as_ref().and_then(|status| status.icon.clone()),
    });
}

fn nodes_to_render(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    collapsed_groups: &[GroupPath],
) -> Vec<RenderNode> {
    let local_by_id: HashMap<u64, &LocalTab> = tabs.iter().map(|tab| (tab.tab_id, tab)).collect();
    let Some(model) = model else {
        let mut tabs = tabs.to_vec();
        tabs.sort_by_key(|tab| tab.position);
        return tabs
            .into_iter()
            .map(|tab| {
                let metadata = metadata_for_local_tab(&tab);
                RenderNode::Tab(RenderTab {
                    card: RenderCard {
                        tab_id: tab.tab_id,
                        position: tab.position,
                        name: tab.name,
                        active: tab.active,
                        pinned: false,
                        status: None,
                        metadata,
                        metadata_sources: RenderMetadataSources::new(),
                        reachable_identities: RenderReachableIdentities::new(),
                        templates: ResolvedTemplateSlots::default(),
                        meta_panel: None,
                    },
                    indent: 0,
                    grouping: None,
                    parent_path: None,
                })
            })
            .collect();
    };

    let pending_rows = if model.rows.is_empty() {
        model
            .tabs
            .iter()
            .cloned()
            .map(|tab| {
                PendingRenderNode::Tab(RenderTab {
                    card: render_card_from_model(&tab, &local_by_id),
                    indent: 0,
                    grouping: tab.grouping,
                    parent_path: None,
                })
            })
            .collect()
    } else {
        model
            .rows
            .iter()
            .filter_map(|row| match row {
                RailRow::GroupHeader {
                    path,
                    label,
                    full_label,
                    tab_count,
                    templates,
                    ..
                } => Some(PendingRenderNode::GroupHeader {
                    path: path.clone(),
                    label: label.clone(),
                    full_label: full_label.clone(),
                    tab_count: *tab_count,
                    templates: templates.clone(),
                }),
                RailRow::Tab {
                    indent,
                    parent_path,
                    ..
                } => model.tab_for_row(row).map(|tab| {
                    PendingRenderNode::Tab(RenderTab {
                        card: render_card_from_model(tab, &local_by_id),
                        indent: *indent,
                        grouping: tab.grouping.clone(),
                        parent_path: parent_path.clone(),
                    })
                }),
            })
            .collect()
    };
    let mut nodes = pending_nodes_to_render_nodes(pending_rows, collapsed_groups);
    merge_resolved_metadata(&mut nodes, &model.resolved_metadata);
    conflate_spindly_groups(&mut nodes);
    nodes
}

fn pending_nodes_to_render_nodes(
    pending_rows: Vec<PendingRenderNode>,
    collapsed_groups: &[GroupPath],
) -> Vec<RenderNode> {
    let mut nodes = vec![];
    let mut current_group: Option<CurrentGroupHeader> = None;
    for pending in pending_rows {
        match pending {
            PendingRenderNode::GroupHeader {
                path,
                label,
                full_label,
                tab_count: _,
                templates,
            } => {
                ensure_group_path(
                    &mut nodes,
                    &path,
                    &label,
                    &full_label,
                    &templates,
                    collapsed_groups,
                    true,
                );
                current_group = Some(CurrentGroupHeader {
                    path,
                    label,
                    full_label,
                    templates,
                });
            }
            PendingRenderNode::Tab(mut tab) if tab.parent_path.is_some() => {
                let parent_path = tab.parent_path.clone().expect("checked above");
                let (label, full_label, templates) = current_group
                    .as_ref()
                    .filter(|group_header| group_header.path == parent_path)
                    .map(|group_header| {
                        (
                            group_header.label.clone(),
                            group_header.full_label.clone(),
                            group_header.templates.clone(),
                        )
                    })
                    .unwrap_or_else(|| {
                        let label = parent_path
                            .0
                            .last()
                            .map(group_segment_label)
                            .unwrap_or_default();
                        let full_label = parent_path
                            .0
                            .iter()
                            .map(group_segment_label)
                            .collect::<Vec<_>>()
                            .join("/");
                        (label, full_label, ResolvedTemplateSlots::default())
                    });
                tab.indent = parent_path.0.len() * 2;
                let group = ensure_group_path(
                    &mut nodes,
                    &parent_path,
                    &label,
                    &full_label,
                    &templates,
                    collapsed_groups,
                    false,
                );
                group.children.push(RenderNode::Tab(tab));
            }
            PendingRenderNode::Tab(mut tab) if current_group.is_some() && tab.indent > 0 => {
                let group_header = current_group.as_ref().expect("checked above");
                tab.parent_path = Some(group_header.path.clone());
                tab.indent = group_header.path.0.len() * 2;
                let group = ensure_group_path(
                    &mut nodes,
                    &group_header.path,
                    &group_header.label,
                    &group_header.full_label,
                    &group_header.templates,
                    collapsed_groups,
                    false,
                );
                group.children.push(RenderNode::Tab(tab));
            }
            PendingRenderNode::Tab(tab) => {
                current_group = None;
                nodes.push(RenderNode::Tab(tab));
            }
        }
    }
    refresh_group_tab_counts(&mut nodes);
    normalize_render_node_order(&mut nodes);
    nodes
}

fn normalize_render_node_order(nodes: &mut [RenderNode]) {
    for node in nodes {
        let RenderNode::Group(group) = node else {
            continue;
        };
        normalize_render_node_order(&mut group.children);
        let mut tabs = Vec::new();
        let mut groups = Vec::new();
        for child in group.children.drain(..) {
            match child {
                RenderNode::Tab(_) => tabs.push(child),
                RenderNode::Group(_) => groups.push(child),
            }
        }
        group.children = tabs;
        group.children.extend(groups);
    }
}

fn ensure_group_path<'a>(
    nodes: &'a mut Vec<RenderNode>,
    path: &GroupPath,
    leaf_label: &str,
    leaf_full_label: &str,
    templates: &ResolvedTemplateSlots,
    collapsed_groups: &[GroupPath],
    update_existing_leaf: bool,
) -> &'a mut RenderGroup {
    if path.0.is_empty() {
        let group_index = nodes
            .iter()
            .position(|node| matches!(node, RenderNode::Group(group) if group.path == *path))
            .unwrap_or_else(|| {
                let collapsed = collapsed_groups.iter().any(|collapsed| collapsed == path);
                nodes.push(RenderNode::Group(RenderGroup {
                    metadata: metadata_for_group_header(path, leaf_label, leaf_full_label, 0),
                    metadata_sources: RenderMetadataSources::new(),
                    reachable_identities: RenderReachableIdentities::new(),
                    templates: templates.clone(),
                    path: path.clone(),
                    conflated_paths: vec![path.clone()],
                    label: leaf_label.to_owned(),
                    full_label: leaf_full_label.to_owned(),
                    tab_count: 0,
                    collapsed,
                    indent: 0,
                    children: vec![],
                }));
                nodes.len() - 1
            });
        let RenderNode::Group(group) = &mut nodes[group_index] else {
            unreachable!("group index should point at a group");
        };
        return group;
    }
    ensure_group_path_at(
        nodes,
        path,
        1,
        leaf_label,
        leaf_full_label,
        templates,
        collapsed_groups,
        update_existing_leaf,
    )
}

fn ensure_group_path_at<'a>(
    nodes: &'a mut Vec<RenderNode>,
    path: &GroupPath,
    depth: usize,
    leaf_label: &str,
    leaf_full_label: &str,
    templates: &ResolvedTemplateSlots,
    collapsed_groups: &[GroupPath],
    update_existing_leaf: bool,
) -> &'a mut RenderGroup {
    let segment = &path.0[depth - 1];
    let prefix = GroupPath(path.0[..depth].to_vec());
    let is_leaf = depth == path.0.len();
    let label = if is_leaf {
        leaf_label.to_owned()
    } else {
        group_segment_label(segment)
    };
    let full_label = if is_leaf {
        leaf_full_label.to_owned()
    } else {
        prefix
            .0
            .iter()
            .map(group_segment_label)
            .collect::<Vec<_>>()
            .join("/")
    };
    let group_index = nodes
        .iter()
        .position(|node| matches!(node, RenderNode::Group(group) if group.path == prefix))
        .unwrap_or_else(|| {
            let indent = prefix.0.len().saturating_sub(1) * 2;
            let collapsed = collapsed_groups
                .iter()
                .any(|collapsed| collapsed == &prefix);
            nodes.push(RenderNode::Group(RenderGroup {
                metadata: metadata_for_group_header(&prefix, &label, &full_label, 0),
                metadata_sources: RenderMetadataSources::new(),
                reachable_identities: RenderReachableIdentities::new(),
                templates: if is_leaf {
                    templates.clone()
                } else {
                    ResolvedTemplateSlots::default()
                },
                path: prefix.clone(),
                conflated_paths: vec![prefix.clone()],
                label: label.clone(),
                full_label: full_label.clone(),
                tab_count: 0,
                collapsed,
                indent,
                children: vec![],
            }));
            nodes.len() - 1
        });
    let RenderNode::Group(group) = &mut nodes[group_index] else {
        unreachable!("group index should point at a group");
    };
    if is_leaf {
        if !update_existing_leaf {
            return group;
        }
        group.label = label;
        group.full_label = full_label;
        group.templates = templates.clone();
        group.metadata.extend(metadata_for_group_header(
            &group.path,
            &group.label,
            &group.full_label,
            0,
        ));
        group
    } else {
        ensure_group_path_at(
            &mut group.children,
            path,
            depth + 1,
            leaf_label,
            leaf_full_label,
            templates,
            collapsed_groups,
            update_existing_leaf,
        )
    }
}

fn group_segment_label(segment: &GroupSegment) -> String {
    segment
        .label
        .clone()
        .unwrap_or_else(|| format_metadata_value(&segment.value))
}

fn refresh_group_tab_counts(nodes: &mut [RenderNode]) -> usize {
    nodes.iter_mut().fold(0, |total, node| {
        total
            + match node {
                RenderNode::Tab(_) => 1,
                RenderNode::Group(group) => {
                    let tab_count = refresh_group_tab_counts(&mut group.children);
                    group.tab_count = tab_count;
                    group.metadata.insert(
                        "group.tab_count".to_owned(),
                        MetadataValue::Integer(tab_count as i64),
                    );
                    tab_count
                }
            }
    })
}

fn merge_resolved_metadata(nodes: &mut [RenderNode], resolved_metadata: &[ResolvedMetadata]) {
    let by_target = resolved_metadata
        .iter()
        .map(|metadata| {
            (
                metadata.target.clone(),
                render_metadata_from_entries(&metadata.values),
            )
        })
        .collect::<HashMap<_, _>>();
    let sources_by_target = resolved_metadata
        .iter()
        .map(|metadata| (metadata.target.clone(), metadata.source_entries.clone()))
        .collect::<HashMap<_, _>>();
    let identities_by_target = resolved_metadata
        .iter()
        .map(|metadata| {
            (
                metadata.target.clone(),
                metadata.reachable_identities.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    merge_resolved_metadata_into_nodes(
        nodes,
        &by_target,
        &sources_by_target,
        &identities_by_target,
    );
}

fn conflate_spindly_groups(nodes: &mut Vec<RenderNode>) {
    for node in nodes {
        let RenderNode::Group(group) = node else {
            continue;
        };
        conflate_spindly_groups(&mut group.children);
        while can_conflate_group_with_only_child(group) {
            conflate_group_with_only_child(group);
            conflate_spindly_groups(&mut group.children);
        }
    }
}

fn can_conflate_group_with_only_child(group: &RenderGroup) -> bool {
    if group.collapsed || child_layout_from_metadata(&group.metadata).is_some() {
        return false;
    }
    let mut child_groups = 0;
    for child in &group.children {
        match child {
            RenderNode::Tab(_) => return false,
            RenderNode::Group(child_group) => {
                if child_group.collapsed
                    || child_layout_from_metadata(&child_group.metadata).is_some()
                {
                    return false;
                }
                child_groups += 1;
            }
        }
    }
    child_groups == 1
}

fn conflate_group_with_only_child(group: &mut RenderGroup) {
    let child_index = group
        .children
        .iter()
        .position(|child| matches!(child, RenderNode::Group(_)))
        .expect("checked by can_conflate_group_with_only_child");
    let RenderNode::Group(mut child) = group.children.remove(child_index) else {
        unreachable!("child index should point at a group");
    };
    let indent_delta = child.indent.saturating_sub(group.indent);
    let label = format!("{} / {}", group.label, child.label);
    let full_label = label.clone();

    let mut metadata = group.metadata.clone();
    metadata.extend(child.metadata.clone());
    metadata.insert("group.label".to_owned(), MetadataValue::Text(label.clone()));
    metadata.insert(
        "group.full_label".to_owned(),
        MetadataValue::Text(full_label.clone()),
    );

    group.path = child.path;
    group.conflated_paths.extend(child.conflated_paths);
    group.label = label;
    group.full_label = full_label;
    group.tab_count = child.tab_count;
    group.collapsed = child.collapsed;
    group.templates = child.templates;
    group.metadata = metadata;
    group
        .metadata_sources
        .extend(std::mem::take(&mut child.metadata_sources));
    group
        .reachable_identities
        .extend(std::mem::take(&mut child.reachable_identities));
    group.children = child.children;
    shift_render_node_indents(&mut group.children, indent_delta);
}

fn shift_render_node_indents(nodes: &mut [RenderNode], delta: usize) {
    for node in nodes {
        match node {
            RenderNode::Tab(tab) => {
                tab.indent = tab.indent.saturating_sub(delta);
            }
            RenderNode::Group(group) => {
                group.indent = group.indent.saturating_sub(delta);
                shift_render_node_indents(&mut group.children, delta);
            }
        }
    }
}

fn merge_resolved_metadata_into_nodes(
    nodes: &mut [RenderNode],
    by_target: &HashMap<MetadataTarget, RenderMetadata>,
    sources_by_target: &HashMap<MetadataTarget, RenderMetadataSources>,
    identities_by_target: &HashMap<MetadataTarget, RenderReachableIdentities>,
) {
    for node in nodes {
        match node {
            RenderNode::Tab(tab) => {
                let target = MetadataTarget::Tab(tab.card.tab_id);
                if let Some(metadata) = by_target.get(&target) {
                    tab.card.metadata.extend(metadata.clone());
                }
                if let Some(metadata_sources) = sources_by_target.get(&target) {
                    tab.card.metadata_sources.extend(metadata_sources.clone());
                }
                if let Some(reachable_identities) = identities_by_target.get(&target) {
                    tab.card
                        .reachable_identities
                        .extend(reachable_identities.clone());
                }
            }
            RenderNode::Group(group) => {
                let target = MetadataTarget::Group(group.path.clone());
                if let Some(metadata) = by_target.get(&target) {
                    group.metadata.extend(metadata.clone());
                }
                if let Some(metadata_sources) = sources_by_target.get(&target) {
                    group.metadata_sources.extend(metadata_sources.clone());
                }
                if let Some(reachable_identities) = identities_by_target.get(&target) {
                    group
                        .reachable_identities
                        .extend(reachable_identities.clone());
                }
                merge_resolved_metadata_into_nodes(
                    &mut group.children,
                    by_target,
                    sources_by_target,
                    identities_by_target,
                );
            }
        }
    }
}

fn render_metadata_from_entries(entries: &BTreeMap<String, MetadataEntry>) -> RenderMetadata {
    entries
        .iter()
        .map(|(key, entry)| (key.clone(), entry.value.clone()))
        .collect()
}

fn metadata_for_group_header(
    path: &GroupPath,
    label: &str,
    full_label: &str,
    tab_count: usize,
) -> RenderMetadata {
    let mut metadata = RenderMetadata::new();
    metadata.insert(
        "group.label".to_owned(),
        MetadataValue::Text(label.to_owned()),
    );
    metadata.insert(
        "group.full_label".to_owned(),
        MetadataValue::Text(full_label.to_owned()),
    );
    metadata.insert(
        "group.tab_count".to_owned(),
        MetadataValue::Integer(tab_count as i64),
    );
    for segment in &path.0 {
        metadata.insert(segment.key.clone(), segment.value.clone());
    }
    metadata
}

fn render_card_from_model(card: &TabCard, local_by_id: &HashMap<u64, &LocalTab>) -> RenderCard {
    let local = local_by_id.get(&card.tab_id).copied();
    let position = local.map(|tab| tab.position).unwrap_or(card.position);
    let name = local
        .map(|tab| tab.name.clone())
        .unwrap_or_else(|| card.name.clone());
    let active = local.map(|tab| tab.active).unwrap_or(card.active);
    let status = card.status.clone();
    RenderCard {
        tab_id: card.tab_id,
        position,
        name: name.clone(),
        active,
        pinned: card.pinned,
        status,
        metadata: metadata_for_tab_card(card, position, &name, active),
        metadata_sources: RenderMetadataSources::new(),
        reachable_identities: RenderReachableIdentities::new(),
        templates: card.templates.clone(),
        meta_panel: None,
    }
}

fn metadata_for_local_tab(tab: &LocalTab) -> RenderMetadata {
    let mut metadata = RenderMetadata::new();
    metadata.insert(
        "zellij.tab.id".to_owned(),
        MetadataValue::Integer(tab.tab_id as i64),
    );
    metadata.insert(
        "zellij.tab.position".to_owned(),
        MetadataValue::Integer(tab.position as i64),
    );
    metadata.insert(
        "zellij.tab.name".to_owned(),
        MetadataValue::Text(tab.name.clone()),
    );
    metadata.insert(
        "zellij.tab.active".to_owned(),
        MetadataValue::Bool(tab.active),
    );
    metadata.insert("rail.tab.pinned".to_owned(), MetadataValue::Bool(false));
    metadata
}

fn metadata_for_tab_card(
    card: &TabCard,
    position: usize,
    name: &str,
    active: bool,
) -> RenderMetadata {
    let mut metadata = RenderMetadata::new();
    metadata.insert(
        "zellij.tab.id".to_owned(),
        MetadataValue::Integer(card.tab_id as i64),
    );
    metadata.insert(
        "zellij.tab.position".to_owned(),
        MetadataValue::Integer(position as i64),
    );
    metadata.insert(
        "zellij.tab.name".to_owned(),
        MetadataValue::Text(name.to_owned()),
    );
    metadata.insert("zellij.tab.active".to_owned(), MetadataValue::Bool(active));
    metadata.insert(
        "rail.tab.pinned".to_owned(),
        MetadataValue::Bool(card.pinned),
    );
    if let Some(active_pane) = card.active_pane {
        metadata.insert(
            "zellij.tab.active_pane".to_owned(),
            MetadataValue::Text(format_pane_target(active_pane)),
        );
    }
    if let Some(grouping) = card.grouping.as_ref() {
        metadata.insert(
            "group.label".to_owned(),
            MetadataValue::Text(grouping.label.clone()),
        );
        metadata.insert(
            "group.full_label".to_owned(),
            MetadataValue::Text(grouping.full_label.clone()),
        );
    }
    if let Some(status) = card.status.as_ref() {
        metadata.extend(status_metadata(status));
    }
    metadata
}

fn visible_cells(
    cards: &[RenderCard],
    available_rows: usize,
    sizing: RailSizingPreset,
) -> Vec<(usize, usize)> {
    if cards.is_empty() || available_rows < COMPACT_CELL_HEIGHT + 1 {
        return vec![];
    }
    let active_index = cards.iter().position(|card| card.active).unwrap_or(0);
    let cell_height = |card: &RenderCard| cell_height(card, sizing, true);
    let all_rows = cards.iter().map(cell_height).sum::<usize>() + 1;
    if all_rows <= available_rows {
        return cards
            .iter()
            .enumerate()
            .map(|(index, card)| (index, cell_height(card)))
            .collect();
    }

    let mut selected = vec![(active_index, cell_height(&cards[active_index]))];
    let mut used_rows = selected[0].1 + 1;
    let mut before = active_index;
    let mut after = active_index + 1;
    loop {
        let before_height = before
            .checked_sub(1)
            .map(|index| (index, cell_height(&cards[index])));
        let after_height = (after < cards.len()).then(|| (after, cell_height(&cards[after])));
        match (before_height, after_height) {
            (Some((index, height)), _) if used_rows + height <= available_rows => {
                selected.insert(0, (index, height));
                used_rows += height;
                before = index;
            }
            (_, Some((index, height))) if used_rows + height <= available_rows => {
                selected.push((index, height));
                used_rows += height;
                after = index + 1;
            }
            _ => break,
        }
    }
    selected
}

fn visible_boxes(
    cards: &[RenderCard],
    available_rows: usize,
    sizing: RailSizingPreset,
) -> Vec<(usize, usize)> {
    if cards.is_empty() || available_rows < COMPACT_CELL_HEIGHT + 1 {
        return vec![];
    }
    let active_index = cards.iter().position(|card| card.active).unwrap_or(0);
    let box_height = |card: &RenderCard| cell_height(card, sizing, false).max(2);
    let mut selected = vec![(active_index, box_height(&cards[active_index]))];
    let mut used_rows = selected[0].1;
    let mut before = active_index;
    let mut after = active_index + 1;
    loop {
        let before_height = before
            .checked_sub(1)
            .map(|index| (index, box_height(&cards[index])));
        let after_height = (after < cards.len()).then(|| (after, box_height(&cards[after])));
        match (before_height, after_height) {
            (Some((index, height)), _) if used_rows + height <= available_rows => {
                selected.insert(0, (index, height));
                used_rows += height;
                before = index;
            }
            (_, Some((index, height))) if used_rows + height <= available_rows => {
                selected.push((index, height));
                used_rows += height;
                after = index + 1;
            }
            _ => break,
        }
    }
    selected
}

fn cell_height(card: &RenderCard, sizing: RailSizingPreset, joined_cell: bool) -> usize {
    let compact = if joined_cell {
        COMPACT_CELL_HEIGHT
    } else {
        COMPACT_CELL_HEIGHT + 1
    };
    let base = match sizing {
        RailSizingPreset::Compact => compact,
        RailSizingPreset::Large => ACTIVE_CELL_HEIGHT,
        RailSizingPreset::ActiveLarge if card.active => ACTIVE_CELL_HEIGHT,
        RailSizingPreset::PinnedLarge if card.active || card.pinned => ACTIVE_CELL_HEIGHT,
        RailSizingPreset::ActiveLarge | RailSizingPreset::PinnedLarge => compact,
    };
    // Meta panel sits inside the card body; grow the card by the number of
    // panel rows so the existing border + body layout absorbs them without
    // breaking joined-cells adjacency.
    base + card.meta_panel.as_ref().map_or(0, |panel| panel.len())
}

fn format_status_with_template_catalog(
    card: &RenderCard,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> String {
    let fields = card
        .templates
        .tab_status
        .as_ref()
        .map(template_fields_from_resolved_slot)
        .unwrap_or_else(|| {
            status_template_fields_with_template_catalog(&card.metadata, template_catalog)
        });
    join_template_fields(&fields, true)
}

#[cfg(test)]
fn status_template_fields(metadata: &RenderMetadata) -> Vec<TemplateField> {
    status_template_fields_with_template_catalog(metadata, None)
}

fn status_template_fields_with_template_catalog(
    metadata: &RenderMetadata,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> Vec<TemplateField> {
    let context = TemplateRenderContext {
        slot: TemplateSlot::TabStatus,
        node_kind: RenderNodeKind::Tab,
        metadata,
        collapsed: false,
        active_tab_name: None,
    };
    if let Some(fields) = external_template_fields(template_catalog, context) {
        return fields;
    }
    template_fields_for(TemplateRenderContext {
        slot: TemplateSlot::TabStatus,
        node_kind: RenderNodeKind::Tab,
        metadata,
        collapsed: false,
        active_tab_name: None,
    })
}

fn status_metadata(status: &TabStatusSummary) -> RenderMetadata {
    let mut metadata = RenderMetadata::new();
    metadata.insert(
        "status.priority".to_owned(),
        MetadataValue::Text(format!("{:?}", status.priority).to_ascii_lowercase()),
    );
    metadata.insert(
        "status.title".to_owned(),
        MetadataValue::Text(status.title.clone()),
    );
    if let Some(detail) = status.detail.as_ref() {
        metadata.insert(
            "status.detail".to_owned(),
            MetadataValue::Text(detail.clone()),
        );
    }
    metadata.insert(
        "status.source_pane".to_owned(),
        MetadataValue::Text(format_pane_target(status.source_pane)),
    );
    metadata
}

fn metadata_text<'a>(metadata: &'a RenderMetadata, key: &str) -> Option<&'a str> {
    match metadata.get(key) {
        Some(MetadataValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn metadata_display_value(metadata: &RenderMetadata, key: &str) -> Option<String> {
    metadata.get(key).map(format_metadata_value)
}

fn tab_title_with_template_catalog(
    card: &RenderCard,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> String {
    let fields = card
        .templates
        .tab_title
        .as_ref()
        .map(template_fields_from_resolved_slot)
        .unwrap_or_else(|| {
            tab_title_template_fields_with_template_catalog(&card.metadata, template_catalog)
        });
    join_template_fields(&fields, true)
}

#[cfg(test)]
fn tab_title_template_fields(metadata: &RenderMetadata) -> Vec<TemplateField> {
    tab_title_template_fields_with_template_catalog(metadata, None)
}

fn tab_title_template_fields_with_template_catalog(
    metadata: &RenderMetadata,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> Vec<TemplateField> {
    let context = TemplateRenderContext {
        slot: TemplateSlot::TabTitle,
        node_kind: RenderNodeKind::Tab,
        metadata,
        collapsed: false,
        active_tab_name: None,
    };
    if let Some(fields) = external_template_fields(template_catalog, context) {
        return fields;
    }
    template_fields_for(TemplateRenderContext {
        slot: TemplateSlot::TabTitle,
        node_kind: RenderNodeKind::Tab,
        metadata,
        collapsed: false,
        active_tab_name: None,
    })
}

fn body_lines(
    card: &RenderCard,
    row: usize,
    body_rows: usize,
    cols: usize,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> Vec<String> {
    let mut lines = vec![String::new(); body_rows];
    let Some(status) = &card.status else {
        return lines;
    };
    let inner_width = cols.saturating_sub(2);
    if body_rows == 0 || inner_width == 0 {
        return lines;
    }
    let icon_reserve = status_icon_rect(status, row, body_rows, cols, terminal_cell_size)
        .map(|rect| rect.columns + 1)
        .unwrap_or(0);
    let text_width = inner_width.saturating_sub(icon_reserve);
    let text_lines = wrap_to_width(
        &format_status_with_template_catalog(card, template_catalog),
        text_width,
        body_rows,
    );
    for (line, text_line) in lines.iter_mut().zip(text_lines) {
        if icon_reserve > 0 {
            line.push_str(&" ".repeat(icon_reserve));
        }
        line.push_str(&text_line);
    }
    lines
}

fn status_icon_rect(
    status: &TabStatusSummary,
    card_row: usize,
    body_rows: usize,
    cols: usize,
    terminal_cell_size: Option<SizeInPixels>,
) -> Option<VisibleIconRect> {
    let icon_columns = square_columns_for_rows(body_rows, terminal_cell_size);
    if body_rows == 0
        || cols.saturating_sub(2) < icon_columns
        || !status.icon.as_ref().is_some_and(status_icon_is_renderable)
    {
        return None;
    }
    Some(VisibleIconRect {
        x: 1,
        y: card_row + 1,
        columns: icon_columns,
        rows: body_rows,
    })
}

fn square_columns_for_rows(rows: usize, terminal_cell_size: Option<SizeInPixels>) -> usize {
    let Some(cell_size) = terminal_cell_size else {
        return rows;
    };
    if cell_size.width == 0 || cell_size.height == 0 {
        return rows;
    }
    (((rows as u64) * (cell_size.height as u64) + (cell_size.width as u64) - 1)
        / (cell_size.width as u64)) as usize
}

pub fn status_icon_is_renderable(icon: &StatusIcon) -> bool {
    matches!(icon, StatusIcon::PngFile(_))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BorderKind {
    Top { active: bool, first_cell: bool },
    Bottom { active: bool },
    Footer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellStyle {
    Border { active: bool },
    Title { active: bool },
    Body,
}

#[derive(Debug, Clone)]
struct BorderCell {
    glyph: char,
    style: CellStyle,
    owner: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct BorderHitPayload {
    tab_id: u64,
    tab_position: usize,
    group_path: Option<GroupPath>,
    inspect_target: Option<NodeKey>,
}

impl BorderHitPayload {
    fn none() -> Self {
        Self {
            tab_id: 0,
            tab_position: 0,
            group_path: None,
            inspect_target: None,
        }
    }
}

#[derive(Debug, Clone)]
struct BorderPendingHit {
    col_start: usize,
    col_end: usize,
    action: HitAction,
    payload: BorderHitPayload,
}

/// Single-row builder that tracks per-cell ownership so two widgets cannot
/// silently overpaint each other. Existing border/footer rendering routes
/// through this; new controls (tri-state, root toggle) will use the same
/// placement API.
struct BorderRow {
    cells: Vec<BorderCell>,
    width: usize,
    kind: BorderKind,
    hits: Vec<BorderPendingHit>,
}

impl BorderRow {
    fn new(width: usize, kind: BorderKind) -> Self {
        let (fill_glyph, fill_style) = match kind {
            BorderKind::Top { active, .. } | BorderKind::Bottom { active } => {
                ('─', CellStyle::Border { active })
            }
            BorderKind::Footer => (' ', CellStyle::Body),
        };
        let mut cells = (0..width)
            .map(|_| BorderCell {
                glyph: fill_glyph,
                style: fill_style,
                owner: None,
            })
            .collect::<Vec<_>>();
        if width >= 1 {
            if let Some((glyph, style)) = corner_left(kind) {
                cells[0] = BorderCell {
                    glyph,
                    style,
                    owner: Some("corner_left"),
                };
            }
        }
        if width >= 2 {
            if let Some((glyph, style)) = corner_right(kind) {
                cells[width - 1] = BorderCell {
                    glyph,
                    style,
                    owner: Some("corner_right"),
                };
            }
        }
        Self {
            cells,
            width,
            kind,
            hits: vec![],
        }
    }

    fn inner_range(&self) -> std::ops::Range<usize> {
        let start = match self.kind {
            BorderKind::Top { .. } | BorderKind::Bottom { .. } if self.width >= 1 => 1,
            _ => 0,
        };
        let end = match self.kind {
            BorderKind::Top { .. } | BorderKind::Bottom { .. } if self.width >= 2 => self.width - 1,
            _ => self.width,
        };
        start.min(end)..end
    }

    fn decoration_style(&self) -> CellStyle {
        match self.kind {
            BorderKind::Top { active, .. } | BorderKind::Bottom { active } => {
                CellStyle::Border { active }
            }
            BorderKind::Footer => CellStyle::Body,
        }
    }

    fn place_left(
        &mut self,
        offset: usize,
        glyph: char,
        hit: Option<(HitAction, BorderHitPayload)>,
        owner: &'static str,
    ) {
        let inner = self.inner_range();
        let col = inner.start + offset;
        self.place_at(col, glyph, hit, owner);
    }

    fn place_right(
        &mut self,
        offset: usize,
        glyph: char,
        hit: Option<(HitAction, BorderHitPayload)>,
        owner: &'static str,
    ) {
        let inner = self.inner_range();
        if inner.end == 0 {
            return;
        }
        let col = inner.end - 1 - offset;
        self.place_at(col, glyph, hit, owner);
    }

    fn place_at(
        &mut self,
        col: usize,
        glyph: char,
        hit: Option<(HitAction, BorderHitPayload)>,
        owner: &'static str,
    ) {
        let inner = self.inner_range();
        if col < inner.start || col >= inner.end {
            debug_assert!(
                false,
                "BorderRow::place_at out of inner range: col={} inner={:?} owner={}",
                col, inner, owner
            );
            return;
        }
        if let Some(existing) = self.cells[col].owner {
            debug_assert!(
                false,
                "BorderRow cell collision at col={} (claimed by {}, attempted by {})",
                col, existing, owner
            );
            return;
        }
        self.cells[col] = BorderCell {
            glyph,
            style: self.decoration_style(),
            owner: Some(owner),
        };
        if let Some((action, payload)) = hit {
            self.hits.push(BorderPendingHit {
                col_start: col,
                col_end: col,
                action,
                payload,
            });
        }
    }

    /// Place a title in the leftmost unclaimed inner cells. Pads the title
    /// with one space on each side and truncates to fit. Caller is responsible
    /// for placing decorations *before* calling `title` if those decorations
    /// should constrain title width.
    fn title(&mut self, title: &str) {
        let inner = self.inner_range();
        if inner.is_empty() {
            return;
        }
        // Find the leftmost unclaimed run starting at inner.start.
        let mut run_start = inner.start;
        while run_start < inner.end && self.cells[run_start].owner.is_some() {
            run_start += 1;
        }
        let mut run_end = run_start;
        while run_end < inner.end && self.cells[run_end].owner.is_none() {
            run_end += 1;
        }
        let run_width = run_end - run_start;
        if run_width == 0 {
            return;
        }
        let padded = format!(" {title} ");
        let label = truncate_to_width(&padded, run_width);
        let title_style = match self.kind {
            BorderKind::Top { active, .. } | BorderKind::Bottom { active } => {
                CellStyle::Title { active }
            }
            BorderKind::Footer => CellStyle::Body,
        };
        let mut col = run_start;
        for ch in label.chars() {
            if col >= run_end {
                break;
            }
            let ch_width = ch.width().unwrap_or(0).max(1);
            self.cells[col] = BorderCell {
                glyph: ch,
                style: title_style,
                owner: Some("title"),
            };
            col += ch_width;
        }
    }

    fn finish(self, row: usize, theme: Option<RenderTheme>) -> (String, Vec<HitRegion>) {
        let mut out = String::new();
        if !self.cells.is_empty() {
            let mut i = 0;
            while i < self.cells.len() {
                let style = self.cells[i].style;
                let mut j = i + 1;
                while j < self.cells.len() && self.cells[j].style == style {
                    j += 1;
                }
                let span: String = self.cells[i..j].iter().map(|c| c.glyph).collect();
                out.push_str(&style_span(span, style, theme));
                i = j;
            }
        }
        let hits = self
            .hits
            .into_iter()
            .map(|hit| HitRegion {
                row_start: row,
                row_end: row,
                col_start: hit.col_start,
                col_end: hit.col_end,
                tab_id: hit.payload.tab_id,
                tab_position: hit.payload.tab_position,
                group_path: hit.payload.group_path,
                inspect_target: hit.payload.inspect_target,
                action: hit.action,
            })
            .collect();
        (out, hits)
    }

    fn into_line(self, theme: Option<RenderTheme>) -> String {
        self.finish(0, theme).0
    }
}

fn corner_left(kind: BorderKind) -> Option<(char, CellStyle)> {
    match kind {
        BorderKind::Top { active, first_cell } => Some((
            if first_cell { '┌' } else { '├' },
            CellStyle::Border { active },
        )),
        BorderKind::Bottom { active } => Some(('└', CellStyle::Border { active })),
        BorderKind::Footer => None,
    }
}

fn corner_right(kind: BorderKind) -> Option<(char, CellStyle)> {
    match kind {
        BorderKind::Top { active, first_cell } => Some((
            if first_cell { '┐' } else { '┤' },
            CellStyle::Border { active },
        )),
        BorderKind::Bottom { active } => Some(('┘', CellStyle::Border { active })),
        BorderKind::Footer => None,
    }
}

fn style_span(text: String, style: CellStyle, theme: Option<RenderTheme>) -> String {
    match style {
        CellStyle::Border { active } => style_border_text(text, active, theme),
        CellStyle::Title { active } => style_title_text(text, active, theme),
        CellStyle::Body => style_body_text(text, theme),
    }
}

/// Effective glyph for a node given the explicit-state map. Step 3 treats
/// absent (no explicit override) the same as Clean visually; step 4 will add
/// the inherited-dim distinction.
fn inspect_node_glyph(inspected_node: Option<&NodeKey>, key: &NodeKey) -> char {
    if inspected_node == Some(key) {
        '●'
    } else {
        '○'
    }
}

/// Top border for a tab card. Places an inspect glyph on the right when
/// there is room. Hit region emitted at the glyph cell to select this tab in
/// the config inspector.
fn write_tab_top_border(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    row: usize,
    title: &str,
    width: usize,
    first_cell: bool,
    card: &RenderCard,
    theme: Option<RenderTheme>,
    _metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) {
    let mut border = BorderRow::new(
        width,
        BorderKind::Top {
            active: card.active,
            first_cell,
        },
    );
    if width >= 4 {
        let key = NodeKey::Tab(card.tab_id);
        let glyph = inspect_node_glyph(inspected_node, &key);
        let payload = BorderHitPayload {
            tab_id: card.tab_id,
            tab_position: card.position,
            group_path: None,
            inspect_target: Some(NodeKey::Tab(card.tab_id)),
        };
        border.place_right(
            0,
            glyph,
            Some((HitAction::InspectNode, payload)),
            "tab_inspect",
        );
    }
    border.title(title);
    let (line, hits) = border.finish(row, theme);
    if let Some(slot) = lines.get_mut(row) {
        *slot = line;
    }
    hit_regions.extend(hits);
}

fn bottom_border_line(width: usize, active: bool, theme: Option<RenderTheme>) -> String {
    BorderRow::new(width, BorderKind::Bottom { active }).into_line(theme)
}

fn body_line(text: &str, width: usize, active: bool, theme: Option<RenderTheme>) -> String {
    match width {
        0 => String::new(),
        1 => style_border_text("│".to_owned(), active, theme),
        _ => {
            let inner_width = width - 2;
            let text = truncate_to_width(text, inner_width);
            format!(
                "{}{}{}",
                style_border_text("│".to_owned(), active, theme),
                style_body_text(pad_to_width(&text, inner_width), theme),
                style_border_text("│".to_owned(), active, theme)
            )
        }
    }
}

fn render_footer(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    row: usize,
    width: usize,
    theme: Option<RenderTheme>,
    _metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
    rail_can_scroll: bool,
) {
    if width == 0 {
        return;
    }
    let mut footer = BorderRow::new(width, BorderKind::Footer);
    footer.place_left(
        0,
        '⚙',
        Some((HitAction::OpenConfig, BorderHitPayload::none())),
        "footer_gear",
    );
    if width >= 2 && rail_can_scroll {
        footer.place_right(
            1,
            '▲',
            Some((HitAction::ScrollRailUp, BorderHitPayload::none())),
            "footer_scroll_up",
        );
        footer.place_right(
            0,
            '▼',
            Some((HitAction::ScrollRailDown, BorderHitPayload::none())),
            "footer_scroll_down",
        );
    }
    if width >= 5 {
        let offset = if rail_can_scroll { 2 } else { 0 };
        footer.place_right(
            offset,
            inspect_node_glyph(inspected_node, &NodeKey::Root),
            Some((
                HitAction::InspectNode,
                BorderHitPayload {
                    inspect_target: Some(NodeKey::Root),
                    ..BorderHitPayload::none()
                },
            )),
            "footer_root_inspect",
        );
    }
    let (line, hits) = footer.finish(row, theme);
    lines[row] = line;
    hit_regions.extend(hits);
}

fn group_header_line(
    metadata: &RenderMetadata,
    collapsed: bool,
    contains_active_tab: bool,
    active_tab_name: Option<&str>,
    resolved_slot: Option<&ResolvedTemplateSlot>,
    width: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
    ancestor_template_fields: &BTreeSet<ResolvedTemplateFieldSource>,
) -> RenderedTemplateLine {
    let fields = match resolved_slot {
        Some(slot) => group_header_fields_from_resolved_slot(slot, collapsed, active_tab_name),
        None => group_header_template_fields_with_template_catalog(
            metadata,
            collapsed,
            active_tab_name,
            template_catalog,
        ),
    };
    let rendered_fields =
        render_template_fields_with_suppression(&fields, width, ancestor_template_fields);
    let label = rendered_fields.text;
    let remaining = width.saturating_sub(label.width());
    let text = if remaining >= 2 {
        format!("{label} {}", "─".repeat(remaining - 1))
    } else {
        label
    };
    let text = style_group_header_text(
        pad_to_width(&truncate_to_width(&text, width), width),
        contains_active_tab,
        theme,
    );
    RenderedTemplateLine {
        text,
        visible_sources: rendered_fields.visible_sources,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedTemplateLine {
    text: String,
    visible_sources: BTreeSet<ResolvedTemplateFieldSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplateField {
    Required(String),
    Optional(String),
    Priority(String),
    Prioritized {
        value: String,
        priority: i64,
        source: Option<ResolvedTemplateFieldSource>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateFieldClass {
    Required,
    Optional,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateFieldCondition {
    Always,
    Collapsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateValueSource {
    Literal(&'static str),
    MetadataText(&'static str),
    MetadataDisplay(&'static str),
    TabNumberFromPosition,
    ActiveTabName,
    CollapsedToggle {
        collapsed: &'static str,
        expanded: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TemplateFieldSpec {
    class: TemplateFieldClass,
    sources: &'static [TemplateValueSource],
    prefix: &'static str,
    suffix: &'static str,
    condition: TemplateFieldCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateSizingHint {
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderNodeKind {
    Group,
    Tab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateSlot {
    GroupHeader,
    TabTitle,
    TabStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetadataPredicate {
    Exists(&'static str),
    TextEquals {
        key: &'static str,
        value: &'static str,
    },
    TextPrefix {
        key: &'static str,
        prefix: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
struct TemplateDefinition<'a> {
    name: &'static str,
    slot: TemplateSlot,
    node_kind: RenderNodeKind,
    predicates: &'a [MetadataPredicate],
    fields: &'a [TemplateFieldSpec],
    sizing: TemplateSizingHint,
}

#[derive(Debug, Clone, Copy)]
struct TemplateRenderContext<'a> {
    slot: TemplateSlot,
    node_kind: RenderNodeKind,
    metadata: &'a RenderMetadata,
    collapsed: bool,
    active_tab_name: Option<&'a str>,
}

const STATUS_TEMPLATE_PREDICATES: &[MetadataPredicate] =
    &[MetadataPredicate::Exists("status.title")];
const WAITING_STATUS_TEMPLATE_PREDICATES: &[MetadataPredicate] = &[
    MetadataPredicate::Exists("status.title"),
    MetadataPredicate::TextEquals {
        key: "status.priority",
        value: "waiting",
    },
];
const TERMINAL_STATUS_TEMPLATE_PREDICATES: &[MetadataPredicate] = &[
    MetadataPredicate::Exists("status.title"),
    MetadataPredicate::TextPrefix {
        key: "status.source_pane",
        prefix: "terminal:",
    },
];
const GROUP_HEADER_TEMPLATE_FIELDS: &[TemplateFieldSpec] = &[
    TemplateFieldSpec {
        class: TemplateFieldClass::Required,
        sources: &[TemplateValueSource::CollapsedToggle {
            collapsed: "▶",
            expanded: "▼",
        }],
        prefix: "",
        suffix: "",
        condition: TemplateFieldCondition::Always,
    },
    TemplateFieldSpec {
        class: TemplateFieldClass::Required,
        sources: &[
            TemplateValueSource::MetadataText("group.label"),
            TemplateValueSource::Literal("group"),
        ],
        prefix: "",
        suffix: "",
        condition: TemplateFieldCondition::Always,
    },
    TemplateFieldSpec {
        class: TemplateFieldClass::Optional,
        sources: &[TemplateValueSource::MetadataDisplay("group.tab_count")],
        prefix: "(",
        suffix: ")",
        condition: TemplateFieldCondition::Always,
    },
    TemplateFieldSpec {
        class: TemplateFieldClass::Priority,
        sources: &[TemplateValueSource::ActiveTabName],
        prefix: ": ",
        suffix: "",
        condition: TemplateFieldCondition::Collapsed,
    },
];
const TAB_TITLE_TEMPLATE_FIELDS: &[TemplateFieldSpec] = &[TemplateFieldSpec {
    class: TemplateFieldClass::Required,
    sources: &[
        TemplateValueSource::MetadataText("zellij.tab.name"),
        TemplateValueSource::TabNumberFromPosition,
        TemplateValueSource::Literal("Tab"),
    ],
    prefix: "",
    suffix: "",
    condition: TemplateFieldCondition::Always,
}];
const STATUS_TEMPLATE_FIELDS: &[TemplateFieldSpec] = &[
    TemplateFieldSpec {
        class: TemplateFieldClass::Required,
        sources: &[TemplateValueSource::MetadataText("status.title")],
        prefix: "",
        suffix: "",
        condition: TemplateFieldCondition::Always,
    },
    TemplateFieldSpec {
        class: TemplateFieldClass::Priority,
        sources: &[TemplateValueSource::MetadataText("status.detail")],
        prefix: ": ",
        suffix: "",
        condition: TemplateFieldCondition::Always,
    },
];

const BUILTIN_TEMPLATES: &[TemplateDefinition<'static>] = &[
    TemplateDefinition {
        name: "builtin.group-header",
        slot: TemplateSlot::GroupHeader,
        node_kind: RenderNodeKind::Group,
        predicates: &[],
        fields: GROUP_HEADER_TEMPLATE_FIELDS,
        sizing: TemplateSizingHint::Auto,
    },
    TemplateDefinition {
        name: "builtin.tab-title",
        slot: TemplateSlot::TabTitle,
        node_kind: RenderNodeKind::Tab,
        predicates: &[],
        fields: TAB_TITLE_TEMPLATE_FIELDS,
        sizing: TemplateSizingHint::Auto,
    },
    TemplateDefinition {
        name: "builtin.tab-status",
        slot: TemplateSlot::TabStatus,
        node_kind: RenderNodeKind::Tab,
        predicates: STATUS_TEMPLATE_PREDICATES,
        fields: STATUS_TEMPLATE_FIELDS,
        sizing: TemplateSizingHint::Auto,
    },
    TemplateDefinition {
        name: "builtin.tab-status.waiting",
        slot: TemplateSlot::TabStatus,
        node_kind: RenderNodeKind::Tab,
        predicates: WAITING_STATUS_TEMPLATE_PREDICATES,
        fields: STATUS_TEMPLATE_FIELDS,
        sizing: TemplateSizingHint::Auto,
    },
    TemplateDefinition {
        name: "builtin.tab-status.terminal-source",
        slot: TemplateSlot::TabStatus,
        node_kind: RenderNodeKind::Tab,
        predicates: TERMINAL_STATUS_TEMPLATE_PREDICATES,
        fields: STATUS_TEMPLATE_FIELDS,
        sizing: TemplateSizingHint::Auto,
    },
];

fn template_fields_for(context: TemplateRenderContext<'_>) -> Vec<TemplateField> {
    resolve_template(BUILTIN_TEMPLATES, &context)
        .map(|template| template.build_fields(&context))
        .unwrap_or_default()
}

fn external_template_fields(
    catalog: Option<&TemplateConfigCatalog>,
    context: TemplateRenderContext<'_>,
) -> Option<Vec<TemplateField>> {
    let catalog = catalog?;
    let context = TemplateConfigMatchContext {
        slot: template_config_slot(context.slot),
        node_kind: template_config_node_kind(context.node_kind),
        metadata: context.metadata,
        collapsed: context.collapsed,
        active_tab_name: context.active_tab_name,
    };
    let resolved = catalog.resolve(context)?;
    let fields = resolved
        .template
        .render_fields(context)
        .into_iter()
        .map(|field| match field.class {
            _ if field.priority.is_some() => TemplateField::Prioritized {
                value: field.value,
                priority: field.priority.unwrap_or(100),
                source: field.source,
            },
            TemplateConfigFieldClass::Required => TemplateField::Required(field.value),
            TemplateConfigFieldClass::Optional => TemplateField::Optional(field.value),
            TemplateConfigFieldClass::Priority => TemplateField::Priority(field.value),
        })
        .collect::<Vec<_>>();
    (!fields.is_empty()).then_some(fields)
}

fn template_config_slot(slot: TemplateSlot) -> TemplateConfigSlot {
    match slot {
        TemplateSlot::GroupHeader => TemplateConfigSlot::GroupHeader,
        TemplateSlot::TabTitle => TemplateConfigSlot::TabTitle,
        TemplateSlot::TabStatus => TemplateConfigSlot::TabStatus,
    }
}

fn template_config_node_kind(node_kind: RenderNodeKind) -> TemplateConfigNodeKind {
    match node_kind {
        RenderNodeKind::Group => TemplateConfigNodeKind::Group,
        RenderNodeKind::Tab => TemplateConfigNodeKind::Tab,
    }
}

fn matched_template(
    context: TemplateRenderContext<'_>,
) -> Option<&'static TemplateDefinition<'static>> {
    resolve_template(BUILTIN_TEMPLATES, &context)
}

fn matched_template_candidates_text(context: TemplateRenderContext<'_>) -> String {
    let candidates = matching_templates(BUILTIN_TEMPLATES, &context)
        .into_iter()
        .map(|template| format!("{}({})", template.name, template.specificity()))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        "none".to_owned()
    } else {
        candidates.join(", ")
    }
}

fn resolve_template<'a>(
    templates: &'a [TemplateDefinition<'a>],
    context: &TemplateRenderContext<'_>,
) -> Option<&'a TemplateDefinition<'a>> {
    let mut best: Option<(&TemplateDefinition<'a>, usize)> = None;
    for template in templates {
        if !template.matches(context) {
            continue;
        }
        let specificity = template.specificity();
        if best.is_none_or(|(_, best_specificity)| specificity > best_specificity) {
            best = Some((template, specificity));
        }
    }
    best.map(|(template, _)| template)
}

fn matching_templates<'a>(
    templates: &'a [TemplateDefinition<'a>],
    context: &TemplateRenderContext<'_>,
) -> Vec<&'a TemplateDefinition<'a>> {
    templates
        .iter()
        .filter(|template| template.matches(context))
        .collect()
}

impl TemplateDefinition<'_> {
    fn build_fields(&self, context: &TemplateRenderContext<'_>) -> Vec<TemplateField> {
        self.fields
            .iter()
            .filter_map(|field| field.render(context))
            .collect()
    }

    fn matches(&self, context: &TemplateRenderContext<'_>) -> bool {
        self.slot == context.slot
            && self.node_kind == context.node_kind
            && self
                .predicates
                .iter()
                .all(|predicate| predicate.matches(context.metadata))
    }

    fn specificity(&self) -> usize {
        self.predicates
            .iter()
            .map(MetadataPredicate::specificity)
            .sum()
    }

    fn predicates_text(&self) -> String {
        if self.predicates.is_empty() {
            return "none".to_owned();
        }
        self.predicates
            .iter()
            .map(MetadataPredicate::as_text)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl TemplateFieldSpec {
    fn render(&self, context: &TemplateRenderContext<'_>) -> Option<TemplateField> {
        if !self.condition.matches(context) {
            return None;
        }
        let value = self
            .sources
            .iter()
            .find_map(|source| source.resolve(context))?;
        let value = format!("{}{}{}", self.prefix, value, self.suffix);
        Some(self.class.field(value))
    }
}

impl TemplateFieldCondition {
    fn matches(&self, context: &TemplateRenderContext<'_>) -> bool {
        match self {
            TemplateFieldCondition::Always => true,
            TemplateFieldCondition::Collapsed => context.collapsed,
        }
    }
}

impl TemplateValueSource {
    fn resolve(&self, context: &TemplateRenderContext<'_>) -> Option<String> {
        let value = match self {
            TemplateValueSource::Literal(value) => (*value).to_owned(),
            TemplateValueSource::MetadataText(key) => {
                metadata_text(context.metadata, key)?.to_owned()
            }
            TemplateValueSource::MetadataDisplay(key) => {
                metadata_display_value(context.metadata, key)?
            }
            TemplateValueSource::TabNumberFromPosition => {
                let MetadataValue::Integer(position) =
                    context.metadata.get("zellij.tab.position")?
                else {
                    return None;
                };
                format!("Tab {}", position + 1)
            }
            TemplateValueSource::ActiveTabName => context.active_tab_name?.to_owned(),
            TemplateValueSource::CollapsedToggle {
                collapsed,
                expanded,
            } => {
                if context.collapsed {
                    (*collapsed).to_owned()
                } else {
                    (*expanded).to_owned()
                }
            }
        };
        (!value.is_empty()).then_some(value)
    }
}

impl TemplateFieldClass {
    fn field(&self, value: String) -> TemplateField {
        match self {
            TemplateFieldClass::Required => TemplateField::Required(value),
            TemplateFieldClass::Optional => TemplateField::Optional(value),
            TemplateFieldClass::Priority => TemplateField::Priority(value),
        }
    }
}

impl TemplateSizingHint {
    fn as_text(&self) -> &'static str {
        match self {
            TemplateSizingHint::Auto => "auto",
        }
    }
}

impl MetadataPredicate {
    fn matches(&self, metadata: &RenderMetadata) -> bool {
        match self {
            MetadataPredicate::Exists(key) => metadata.contains_key(*key),
            MetadataPredicate::TextEquals { key, value } => {
                metadata_text(metadata, key) == Some(*value)
            }
            MetadataPredicate::TextPrefix { key, prefix } => {
                metadata_text(metadata, key).is_some_and(|value| value.starts_with(prefix))
            }
        }
    }

    fn specificity(&self) -> usize {
        match self {
            MetadataPredicate::Exists(_) => 1,
            MetadataPredicate::TextPrefix { .. } => 2,
            MetadataPredicate::TextEquals { .. } => 3,
        }
    }

    fn as_text(&self) -> String {
        match self {
            MetadataPredicate::Exists(key) => format!("exists({key})"),
            MetadataPredicate::TextEquals { key, value } => format!("{key} == {value}"),
            MetadataPredicate::TextPrefix { key, prefix } => format!("{key} starts_with {prefix}"),
        }
    }
}

#[cfg(test)]
fn group_header_template_fields(
    metadata: &RenderMetadata,
    collapsed: bool,
    active_tab_name: Option<&str>,
) -> Vec<TemplateField> {
    group_header_template_fields_with_template_catalog(metadata, collapsed, active_tab_name, None)
}

fn group_header_template_fields_with_template_catalog(
    metadata: &RenderMetadata,
    collapsed: bool,
    active_tab_name: Option<&str>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> Vec<TemplateField> {
    let context = TemplateRenderContext {
        slot: TemplateSlot::GroupHeader,
        node_kind: RenderNodeKind::Group,
        metadata,
        collapsed,
        active_tab_name,
    };
    if let Some(fields) = external_template_fields(template_catalog, context) {
        return fields;
    }
    template_fields_for(TemplateRenderContext {
        slot: TemplateSlot::GroupHeader,
        node_kind: RenderNodeKind::Group,
        metadata,
        collapsed,
        active_tab_name,
    })
}

fn group_header_fields_from_resolved_slot(
    slot: &ResolvedTemplateSlot,
    collapsed: bool,
    active_tab_name: Option<&str>,
) -> Vec<TemplateField> {
    let mut fields = vec![TemplateField::Required(
        if collapsed { "▶" } else { "▼" }.to_owned(),
    )];
    fields.extend(slot.fields.iter().map(|field| TemplateField::Prioritized {
        value: field.text.clone(),
        priority: field.priority,
        source: field.source.clone(),
    }));
    if collapsed {
        if let Some(active_tab_name) = active_tab_name {
            fields.push(TemplateField::Prioritized {
                value: format!(": {active_tab_name}"),
                priority: 80,
                source: None,
            });
        }
    }
    fields
}

fn template_fields_from_resolved_slot(slot: &ResolvedTemplateSlot) -> Vec<TemplateField> {
    slot.fields
        .iter()
        .map(|field| TemplateField::Prioritized {
            value: field.text.clone(),
            priority: field.priority,
            source: field.source.clone(),
        })
        .collect()
}

#[cfg(test)]
fn render_template_fields(fields: &[TemplateField], width: usize) -> String {
    render_template_fields_with_suppression(fields, width, &BTreeSet::new()).text
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedTemplateFields {
    text: String,
    visible_sources: BTreeSet<ResolvedTemplateFieldSource>,
}

fn render_template_fields_with_suppression(
    fields: &[TemplateField],
    width: usize,
    suppressed_sources: &BTreeSet<ResolvedTemplateFieldSource>,
) -> RenderedTemplateFields {
    let fields = fields
        .iter()
        .filter(|field| {
            template_field_source(field).is_none_or(|source| !suppressed_sources.contains(source))
        })
        .cloned()
        .collect::<Vec<_>>();
    let selected_fields = select_template_fields_for_width(&fields, width);
    let text = join_template_fields_by(&selected_fields, |_| true);
    let text = truncate_to_width(&text, width);
    let visible_sources = selected_fields
        .iter()
        .filter_map(template_field_source)
        .cloned()
        .collect();
    RenderedTemplateFields {
        text,
        visible_sources,
    }
}

fn select_template_fields_for_width(fields: &[TemplateField], width: usize) -> Vec<TemplateField> {
    if let Some(selected) = fit_template_fields_for_width(fields, width) {
        return selected;
    }
    for threshold in droppable_template_field_priorities(fields) {
        let selected = fields
            .iter()
            .filter(|field| template_field_priority(field) > threshold)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(selected) = fit_template_fields_for_width(&selected, width) {
            return selected;
        }
    }
    let Some(highest_priority) = fields.iter().map(template_field_priority).max() else {
        return vec![];
    };
    fields
        .iter()
        .filter(|field| template_field_priority(field) == highest_priority)
        .cloned()
        .collect()
}

fn fit_template_fields_for_width(
    fields: &[TemplateField],
    width: usize,
) -> Option<Vec<TemplateField>> {
    if join_template_fields(fields, true).width() <= width {
        return Some(fields.to_vec());
    }
    let mut fields = fields.to_vec();
    let lowest_priority = fields.iter().map(template_field_priority).min()?;
    let field_indexes = fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            (template_field_priority(field) == lowest_priority).then_some(index)
        })
        .collect::<Vec<_>>();
    for index in field_indexes {
        let current_width = join_template_fields_by(&fields, |_| true).width();
        if current_width <= width {
            return Some(fields);
        }
        let value_width = template_field_value(&fields[index]).width();
        if value_width <= 3 {
            continue;
        }
        let overflow = current_width - width;
        let max_reduction = value_width - 3;
        let new_value_width = value_width - overflow.min(max_reduction);
        truncate_template_field(&mut fields[index], new_value_width);
    }
    (join_template_fields_by(&fields, |_| true).width() <= width).then_some(fields)
}

fn join_template_fields(fields: &[TemplateField], include_optional: bool) -> String {
    join_template_fields_by(fields, |field| {
        include_optional || !matches!(field, TemplateField::Optional(_))
    })
}

fn join_template_fields_by(
    fields: &[TemplateField],
    include: impl Fn(&TemplateField) -> bool,
) -> String {
    let mut output = String::new();
    for field in fields {
        if !include(field) {
            continue;
        }
        let value = template_field_value(field);
        if output.is_empty() || value.starts_with(':') {
            output.push_str(value);
        } else {
            output.push(' ');
            output.push_str(value);
        }
    }
    output
}

fn droppable_template_field_priorities(fields: &[TemplateField]) -> Vec<i64> {
    let Some(highest_priority) = fields.iter().map(template_field_priority).max() else {
        return vec![];
    };
    let mut priorities = fields
        .iter()
        .map(template_field_priority)
        .filter(|priority| *priority < highest_priority)
        .collect::<Vec<_>>();
    priorities.sort_unstable();
    priorities.dedup();
    priorities
}

fn template_field_value(field: &TemplateField) -> &str {
    match field {
        TemplateField::Required(value)
        | TemplateField::Optional(value)
        | TemplateField::Priority(value) => value,
        TemplateField::Prioritized { value, .. } => value,
    }
}

fn truncate_template_field(field: &mut TemplateField, width: usize) {
    let value = truncate_to_width(template_field_value(field), width);
    match field {
        TemplateField::Required(existing)
        | TemplateField::Optional(existing)
        | TemplateField::Priority(existing) => *existing = value,
        TemplateField::Prioritized {
            value: existing, ..
        } => *existing = value,
    }
}

fn template_field_priority(field: &TemplateField) -> i64 {
    match field {
        TemplateField::Optional(_) => 0,
        TemplateField::Required(_) | TemplateField::Priority(_) => 100,
        TemplateField::Prioritized { priority, .. } => *priority,
    }
}

fn template_field_source(field: &TemplateField) -> Option<&ResolvedTemplateFieldSource> {
    match field {
        TemplateField::Prioritized { source, .. } => source.as_ref(),
        TemplateField::Required(_) | TemplateField::Optional(_) | TemplateField::Priority(_) => {
            None
        }
    }
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let mut current_width = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width + 3 > max_width {
            break;
        }
        out.push(ch);
        current_width += ch_width;
    }
    out.push_str("...");
    out
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

fn wrap_to_width(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::with_capacity(max_lines);
    if max_lines == 0 {
        return lines;
    }
    if width == 0 {
        lines.resize(max_lines, String::new());
        return lines;
    }

    let mut current = String::new();
    for word in text.split_whitespace() {
        let word = if word.width() > width {
            truncate_to_width(word, width)
        } else {
            word.to_owned()
        };
        let separator_width = usize::from(!current.is_empty());
        if !current.is_empty() && current.width() + separator_width + word.width() > width {
            lines.push(current);
            current = String::new();
            if lines.len() == max_lines {
                return lines;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&word);
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }
    lines.resize(max_lines, String::new());
    lines
}

fn blank(cols: usize) -> String {
    " ".repeat(cols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use andamento_shared::{
        GroupPath, GroupSegment, MetadataEntry, MetadataTarget, MetadataTriState, MetadataValue,
        PaneTarget, RailConfig, RailGroupingMode, RailRow, RailSizingPreset, RailStructure,
        ResolvedMetadata, SortMode, StatusIcon,
    };

    fn local_tab(tab_id: u64, position: usize, active: bool) -> LocalTab {
        LocalTab {
            tab_id,
            position,
            name: format!("tab-{tab_id}"),
            active,
        }
    }

    fn test_theme() -> RenderTheme {
        RenderTheme {
            active_border: PaletteColor::EightBit(2),
            inactive_border: PaletteColor::EightBit(8),
            body_foreground: PaletteColor::EightBit(7),
            segment_active_background: PaletteColor::EightBit(10),
            segment_active_foreground: PaletteColor::EightBit(11),
            segment_inactive_background: PaletteColor::EightBit(12),
            segment_inactive_foreground: PaletteColor::EightBit(13),
            segment_between_background: PaletteColor::EightBit(14),
        }
    }

    fn visible_width_without_ansi(line: &str) -> usize {
        let mut visible = String::new();
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' && chars.peek() == Some(&'[') {
                let _ = chars.next();
                for code_ch in chars.by_ref() {
                    if code_ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                visible.push(ch);
            }
        }
        visible.width()
    }

    fn model() -> ControllerViewModel {
        ControllerViewModel {
            sort_mode: SortMode::PinnedFirst,
            config: RailConfig::default(),
            template_config: andamento_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![
                TabCard {
                    tab_id: 2,
                    position: 1,
                    name: "tab-2".to_owned(),
                    active: true,
                    pinned: true,
                    status: Some(TabStatusSummary {
                        priority: Priority::Waiting,
                        title: "waiting".to_owned(),
                        detail: Some("input".to_owned()),
                        icon: None,
                        source_pane: PaneTarget::Terminal(9),
                    }),
                    grouping: None,
                    templates: ResolvedTemplateSlots::default(),
                    active_pane: None,
                },
                TabCard {
                    tab_id: 1,
                    position: 0,
                    name: "tab-1".to_owned(),
                    active: false,
                    pinned: false,
                    status: None,
                    grouping: None,
                    templates: ResolvedTemplateSlots::default(),
                    active_pane: None,
                },
            ],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
        }
    }

    fn grouped_model() -> ControllerViewModel {
        let group_path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
            label: None,
        }]);
        let tab_one = TabCard {
            tab_id: 1,
            position: 0,
            name: "server".to_owned(),
            active: false,
            pinned: false,
            status: None,
            grouping: None,
            templates: ResolvedTemplateSlots::default(),
            active_pane: None,
        };
        let tab_two = TabCard {
            tab_id: 2,
            position: 1,
            name: "tests".to_owned(),
            active: true,
            pinned: false,
            status: None,
            grouping: None,
            templates: ResolvedTemplateSlots::default(),
            active_pane: None,
        };
        ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig {
                grouping: RailGroupingMode::Directory,
                sizing: RailSizingPreset::Compact,
                ..RailConfig::default()
            },
            template_config: andamento_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![tab_one.clone(), tab_two.clone()],
            rows: vec![
                RailRow::GroupHeader {
                    group_id: "cwd:/Users/robert/dev/zellij".to_owned(),
                    path: group_path.clone(),
                    label: "zellij".to_owned(),
                    full_label: "/Users/robert/dev/zellij".to_owned(),
                    tab_count: 2,
                    templates: ResolvedTemplateSlots::default(),
                },
                RailRow::Tab {
                    tab_id: tab_one.tab_id,
                    indent: 2,
                    parent_path: Some(group_path.clone()),
                },
                RailRow::Tab {
                    tab_id: tab_two.tab_id,
                    indent: 2,
                    parent_path: Some(group_path),
                },
            ],
            resolved_metadata: vec![],
            observed_identities: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
        }
    }

    fn nested_group_model() -> ControllerViewModel {
        let mut model = ControllerViewModel {
            sort_mode: SortMode::PinnedFirst,
            config: RailConfig {
                grouping: RailGroupingMode::Directory,
                structure: RailStructure::JoinedCells,
                sizing: RailSizingPreset::Compact,
                segment_between_color: None,
            },
            template_config: andamento_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
        };
        for (tab_id, worktree) in [(1, "worktree-a"), (2, "worktree-b")] {
            let path = GroupPath(vec![
                GroupSegment {
                    key: "project".to_owned(),
                    value: MetadataValue::Text("project-a".to_owned()),
                    label: None,
                },
                GroupSegment {
                    key: "worktree".to_owned(),
                    value: MetadataValue::Text(worktree.to_owned()),
                    label: None,
                },
            ]);
            let tab = TabCard {
                tab_id,
                position: tab_id as usize - 1,
                name: format!("agent-{tab_id}"),
                active: tab_id == 2,
                pinned: false,
                status: None,
                grouping: None,
                templates: ResolvedTemplateSlots::default(),
                active_pane: None,
            };
            model.rows.push(RailRow::GroupHeader {
                group_id: format!("project-a/{worktree}"),
                path: path.clone(),
                label: worktree.to_owned(),
                full_label: format!("project-a/{worktree}"),
                tab_count: 1,
                templates: ResolvedTemplateSlots::default(),
            });
            let tab_id = tab.tab_id;
            model.tabs.push(tab);
            model.rows.push(RailRow::Tab {
                tab_id,
                indent: 2,
                parent_path: Some(path),
            });
        }
        model
    }

    fn mixed_child_group_model() -> ControllerViewModel {
        let parent_path = GroupPath(vec![GroupSegment {
            key: "git.repo".to_owned(),
            value: MetadataValue::Text("zellij-org/zellij".to_owned()),
            label: Some("zellij".to_owned()),
        }]);
        let child_path = GroupPath(vec![
            parent_path.0[0].clone(),
            GroupSegment {
                key: "git.branch".to_owned(),
                value: MetadataValue::Text("feat/kitty-image-plumbing".to_owned()),
                label: None,
            },
        ]);
        ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig {
                grouping: RailGroupingMode::Directory,
                structure: RailStructure::JoinedCells,
                sizing: RailSizingPreset::Compact,
                segment_between_color: None,
            },
            template_config: andamento_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![
                TabCard {
                    tab_id: 1,
                    position: 0,
                    name: "branch-agent".to_owned(),
                    active: false,
                    pinned: false,
                    status: None,
                    grouping: None,
                    templates: ResolvedTemplateSlots::default(),
                    active_pane: None,
                },
                TabCard {
                    tab_id: 2,
                    position: 1,
                    name: "repo-overview".to_owned(),
                    active: true,
                    pinned: false,
                    status: None,
                    grouping: None,
                    templates: ResolvedTemplateSlots::default(),
                    active_pane: None,
                },
            ],
            rows: vec![
                RailRow::GroupHeader {
                    group_id: "git.repo:zellij-org/zellij".to_owned(),
                    path: parent_path.clone(),
                    label: "zellij".to_owned(),
                    full_label: "zellij".to_owned(),
                    tab_count: 2,
                    templates: ResolvedTemplateSlots::default(),
                },
                RailRow::GroupHeader {
                    group_id: "git.repo:zellij-org/zellij/git.branch:feat".to_owned(),
                    path: child_path.clone(),
                    label: "feat/kitty-image-plumbing".to_owned(),
                    full_label: "zellij / feat/kitty-image-plumbing".to_owned(),
                    tab_count: 1,
                    templates: ResolvedTemplateSlots::default(),
                },
                RailRow::Tab {
                    tab_id: 1,
                    indent: 4,
                    parent_path: Some(child_path),
                },
                RailRow::Tab {
                    tab_id: 2,
                    indent: 2,
                    parent_path: Some(parent_path),
                },
            ],
            resolved_metadata: vec![],
            observed_identities: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
        }
    }

    fn flat_rows_model() -> ControllerViewModel {
        let mut model = model();
        model.config.structure = RailStructure::JoinedCells;
        model.config.sizing = RailSizingPreset::Compact;
        model.rows = model
            .tabs
            .iter()
            .map(|tab| RailRow::Tab {
                tab_id: tab.tab_id,
                indent: 0,
                parent_path: None,
            })
            .collect();
        model
    }

    #[test]
    fn flat_controller_rows_still_honor_joined_cells_structure() {
        let rendered = render_lines(Some(&flat_rows_model()), &[], 7, 24, true);

        assert!(rendered.lines[0].starts_with("┌ tab-2"));
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.starts_with("├ tab-1")),
            "flat controller rows should render joined separators, not standalone boxes: {:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .any(|line| line.starts_with("┌ tab-1")),
            "inactive tab should not start a standalone box when structure is joined-cells: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn grouped_rendering_draws_clickable_group_toggle() {
        let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

        assert!(rendered.lines[0].contains("zellij"));
        assert!(
            rendered.lines[0].contains("──"),
            "group header should read as a section divider: {:?}",
            rendered.lines[0]
        );
        let hit = hit_at(&rendered.hit_regions, 0, 0).expect("group toggle should be clickable");
        assert_eq!(hit.action, HitAction::ToggleGroup);
        assert!(hit.group_path.is_some());
        assert_eq!(hit_at(&rendered.hit_regions, 0, 2), None);
    }

    #[test]
    fn render_nodes_walk_nested_groups_recursively() {
        let mut config = RailConfig::default();
        config.structure = RailStructure::JoinedCells;
        config.sizing = RailSizingPreset::Compact;
        let nodes = vec![RenderNode::Group(RenderGroup {
            path: GroupPath::default(),
            conflated_paths: vec![GroupPath::default()],
            label: "typed-parent".to_owned(),
            full_label: "typed-parent".to_owned(),
            tab_count: 1,
            collapsed: false,
            indent: 0,
            metadata: metadata_for_group_header(&GroupPath::default(), "parent", "parent", 1),
            metadata_sources: RenderMetadataSources::new(),
            reachable_identities: RenderReachableIdentities::new(),
            templates: ResolvedTemplateSlots::default(),
            children: vec![RenderNode::Group(RenderGroup {
                path: GroupPath::default(),
                conflated_paths: vec![GroupPath::default()],
                label: "typed-child".to_owned(),
                full_label: "typed-child".to_owned(),
                tab_count: 1,
                collapsed: false,
                indent: 0,
                metadata: metadata_for_group_header(&GroupPath::default(), "child", "child", 1),
                metadata_sources: RenderMetadataSources::new(),
                reachable_identities: RenderReachableIdentities::new(),
                templates: ResolvedTemplateSlots::default(),
                children: vec![RenderNode::Tab(RenderTab {
                    card: RenderCard {
                        tab_id: 42,
                        position: 0,
                        name: "typed-leaf".to_owned(),
                        active: true,
                        pinned: false,
                        status: None,
                        metadata: metadata_for_local_tab(&LocalTab {
                            tab_id: 42,
                            position: 0,
                            name: "leaf".to_owned(),
                            active: true,
                        }),
                        metadata_sources: RenderMetadataSources::new(),
                        reachable_identities: RenderReachableIdentities::new(),
                        templates: ResolvedTemplateSlots::default(),
                        meta_panel: None,
                    },
                    indent: 4,
                    grouping: None,
                    parent_path: None,
                })],
            })],
        })];
        let mut lines = vec![blank(32); 8];
        let mut hit_regions = vec![];
        let mut visible_cards = vec![];

        render_nodes(
            &mut lines,
            &mut hit_regions,
            &mut visible_cards,
            &nodes,
            8,
            32,
            true,
            config,
            None,
            None,
            None,
            &MetadataControls::default(),
            None,
            InheritedRailSettings::default(),
            0,
        );

        assert!(lines[0].starts_with("▼ parent"));
        assert!(lines[1].starts_with("▼ child"));
        assert!(
            lines.iter().any(|line| line.starts_with("    ┌ leaf")),
            "nested tab should render through the recursive group walker: {:?}",
            lines
        );
        assert!(visible_cards.iter().any(|card| card.tab_id == 42));
    }

    #[test]
    fn multi_segment_group_path_renders_nested_group_headers() {
        let rendered = render_lines(Some(&nested_group_model()), &[], 12, 36, true);

        assert!(
            rendered.lines[0].starts_with("▼ project-a (2)"),
            "parent group should render from the first path segment: {:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[1].starts_with("  ▼ worktree-a (1)"),
            "leaf group should render below the parent: {:?}",
            rendered.lines
        );
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.starts_with("    ├ agent-1") || line.starts_with("    ┌ agent-1")),
            "tab should be indented under the leaf group: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn single_child_group_chain_conflates_into_one_visible_header() {
        let mut model = nested_group_model();
        let path = GroupPath(vec![
            GroupSegment {
                key: "project".to_owned(),
                value: MetadataValue::Text("project-a".to_owned()),
                label: None,
            },
            GroupSegment {
                key: "repo".to_owned(),
                value: MetadataValue::Text("repo-a".to_owned()),
                label: None,
            },
            GroupSegment {
                key: "branch".to_owned(),
                value: MetadataValue::Text("main".to_owned()),
                label: None,
            },
        ]);
        model.tabs = vec![TabCard {
            tab_id: 1,
            position: 0,
            name: "agent-1".to_owned(),
            active: true,
            pinned: false,
            status: None,
            grouping: None,
            templates: ResolvedTemplateSlots::default(),
            active_pane: None,
        }];
        model.rows = vec![
            RailRow::GroupHeader {
                group_id: "project-a/repo-a/main".to_owned(),
                path: path.clone(),
                label: "main".to_owned(),
                full_label: "project-a/repo-a/main".to_owned(),
                tab_count: 1,
                templates: ResolvedTemplateSlots::default(),
            },
            RailRow::Tab {
                tab_id: 1,
                indent: 2,
                parent_path: Some(path.clone()),
            },
        ];

        let nodes = nodes_to_render(Some(&model), &[], &[]);
        let [RenderNode::Group(group)] = nodes.as_slice() else {
            panic!("expected one conflated group, got {nodes:?}");
        };
        assert_eq!(group.path, path);
        assert_eq!(group.conflated_paths.len(), 3);
        assert_eq!(group.label, "project-a / repo-a / main");
        assert!(
            matches!(&group.children[0], RenderNode::Tab(tab) if tab.indent == 2),
            "child tab should remain directly under the visible conflated group: {:?}",
            group.children
        );

        let rendered = render_lines(Some(&model), &[], 6, 48, true);
        assert!(
            rendered.lines[0].starts_with("▼ project-a / repo-a / main"),
            "{:?}",
            rendered.lines
        );
        assert!(
            !rendered.lines.iter().any(|line| line.starts_with("  ▼")),
            "intermediate headers should not be rendered for a spindly chain: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn explicit_child_layout_setting_blocks_group_conflation_boundary() {
        let mut model = nested_group_model();
        let path = GroupPath(vec![
            GroupSegment {
                key: "project".to_owned(),
                value: MetadataValue::Text("project-a".to_owned()),
                label: None,
            },
            GroupSegment {
                key: "repo".to_owned(),
                value: MetadataValue::Text("repo-a".to_owned()),
                label: None,
            },
        ]);
        model.tabs = vec![TabCard {
            tab_id: 1,
            position: 0,
            name: "agent-1".to_owned(),
            active: true,
            pinned: false,
            status: None,
            grouping: None,
            templates: ResolvedTemplateSlots::default(),
            active_pane: None,
        }];
        model.rows = vec![
            RailRow::GroupHeader {
                group_id: "project-a/repo-a".to_owned(),
                path: path.clone(),
                label: "repo-a".to_owned(),
                full_label: "project-a/repo-a".to_owned(),
                tab_count: 1,
                templates: ResolvedTemplateSlots::default(),
            },
            RailRow::Tab {
                tab_id: 1,
                indent: 2,
                parent_path: Some(path),
            },
        ];
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Group(GroupPath(vec![GroupSegment {
                key: "project".to_owned(),
                value: MetadataValue::Text("project-a".to_owned()),
                label: None,
            }])),
            values: BTreeMap::from([(
                RAIL_CHILD_LAYOUT_METADATA_KEY.to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("compact-strip".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines(Some(&model), &[], 6, 48, true);

        assert!(
            rendered.lines[0].starts_with("▼ project-a"),
            "{:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[1].starts_with("  ▼ repo-a"),
            "explicit parent layout should keep the child group boundary visible: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn group_can_contain_tabs_and_child_groups() {
        let model = mixed_child_group_model();

        let nodes = nodes_to_render(Some(&model), &[], &[]);

        let [RenderNode::Group(parent)] = nodes.as_slice() else {
            panic!("expected one parent group, got {nodes:?}");
        };
        assert_eq!(parent.label, "zellij");
        assert!(
            matches!(&parent.children[0], RenderNode::Tab(tab) if tab.card.name == "repo-overview"),
            "direct tabs should be ordered before child groups: {:?}",
            parent.children
        );
        assert!(parent
            .children
            .iter()
            .any(|node| matches!(node, RenderNode::Tab(tab) if tab.card.name == "repo-overview")));
        let child = parent
            .children
            .iter()
            .find_map(|node| match node {
                RenderNode::Group(group) if group.label == "feat/kitty-image-plumbing" => {
                    Some(group)
                }
                _ => None,
            })
            .expect("child group");
        assert!(child
            .children
            .iter()
            .any(|node| matches!(node, RenderNode::Tab(tab) if tab.card.name == "branch-agent")));
    }

    #[test]
    fn group_child_layout_metadata_can_render_direct_tabs_as_compact_strip() {
        let mut model = mixed_child_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Group(parent_path),
            values: BTreeMap::from([(
                "rail.child_layout".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("compact-strip".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines(Some(&model), &[], 8, 48, true);

        assert!(
            rendered.lines[1].contains(" repo-overview "),
            "direct tab should render as a Zellij-style tab strip below the group header: {:?}",
            rendered.lines
        );
        let repo_hit = hit_at(&rendered.hit_regions, 1, 4).expect("repo strip hit");
        assert_eq!(repo_hit.action, HitAction::SwitchTab);
        assert_eq!(repo_hit.tab_id, 2);
        assert!(
            rendered
                .lines
                .iter()
                .position(|line| line.contains("repo-overview"))
                < rendered
                    .lines
                    .iter()
                    .position(|line| line.contains("feat/kitty-image-plumbing")),
            "direct tab strip should appear before child group headers: {:?}",
            rendered.lines
        );
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains(" branch-agent ")),
            "child groups should inherit compact-strip for their direct tabs: {:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .any(|line| line.contains("┌ branch-agent")),
            "inherited compact-strip should avoid full child tab cards: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn root_child_layout_metadata_is_inherited_by_groups() {
        let mut model = mixed_child_group_model();
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Root,
            values: BTreeMap::from([(
                "rail.child_layout".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("compact-strip".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines(Some(&model), &[], 8, 48, true);

        assert!(
            rendered.lines[1].contains(" repo-overview "),
            "root child layout should apply to direct tabs in descendant groups: {:?}",
            rendered.lines
        );
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains(" branch-agent ")),
            "root child layout should be inherited by nested groups: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn themed_compact_strip_uses_exact_separator_with_active_inactive_and_between_colors() {
        let mut model = mixed_child_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Group(parent_path),
            values: BTreeMap::from([(
                "rail.child_layout".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("compact-strip".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines_with_theme(Some(&model), &[], 8, 48, true, Some(test_theme()));

        assert!(
            rendered.lines[1].contains("\u{1b}[1;48;5;10;38;5;14m\u{1b}[0m"),
            "active left separator should use between foreground and active tab background: {:?}",
            rendered.lines[1]
        );
        assert!(
            rendered.lines[1].contains("\u{1b}[1;48;5;10;38;5;11m repo-overview \u{1b}[0m"),
            "active label should use active tab foreground/background: {:?}",
            rendered.lines[1]
        );
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains("\u{1b}[1;48;5;12;38;5;13m branch-agent \u{1b}[0m")),
            "inactive label should use inactive tab foreground/background: {:?}",
            rendered.lines
        );
        assert_eq!(visible_width_without_ansi(&rendered.lines[1]), 48);
    }

    #[test]
    fn themed_compact_segment_truncates_before_styling_when_narrow() {
        let item = SegmentItem {
            label: "repo-overview".to_owned(),
            active: true,
        };

        let (segment, visible_width) = render_compact_segment(&item, 3, Some(test_theme()));

        assert_eq!(visible_width, 3);
        assert_eq!(visible_width_without_ansi(&segment), 3);
    }

    #[test]
    fn rail_config_overrides_compact_segment_between_color() {
        let mut model = mixed_child_group_model();
        model.config.segment_between_color = Some(andamento_shared::RailRgbColor {
            red: 1,
            green: 2,
            blue: 3,
        });
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Group(parent_path),
            values: BTreeMap::from([(
                "rail.child_layout".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("compact-strip".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines_with_theme(Some(&model), &[], 8, 48, true, Some(test_theme()));

        assert!(
            rendered.lines[1].contains("\u{1b}[1;48;5;10;38;2;1;2;3m\u{1b}[0m"),
            "configured between color should become the active left separator foreground: {:?}",
            rendered.lines[1]
        );
    }

    #[test]
    fn descendant_group_header_elides_metadata_field_rendered_by_ancestor() {
        let mut model = mixed_child_group_model();
        let repo_source = ResolvedTemplateFieldSource {
            key: "git.repo".to_owned(),
            value: MetadataValue::Text("zellij-org/zellij".to_owned()),
        };
        let branch_source = ResolvedTemplateFieldSource {
            key: "git.branch".to_owned(),
            value: MetadataValue::Text("feat/kitty-image-plumbing".to_owned()),
        };
        if let RailRow::GroupHeader { templates, .. } = &mut model.rows[0] {
            templates.group_header = Some(ResolvedTemplateSlot {
                template_name: "repo.group-header".to_owned(),
                fields: vec![andamento_shared::ResolvedTemplateField {
                    text: "zellij-org/zellij".to_owned(),
                    priority: 100,
                    source: Some(repo_source.clone()),
                }],
            });
        }
        if let RailRow::GroupHeader { templates, .. } = &mut model.rows[1] {
            templates.group_header = Some(ResolvedTemplateSlot {
                template_name: "branch.group-header".to_owned(),
                fields: vec![
                    andamento_shared::ResolvedTemplateField {
                        text: "zellij-org/zellij".to_owned(),
                        priority: 100,
                        source: Some(repo_source),
                    },
                    andamento_shared::ResolvedTemplateField {
                        text: "feat/kitty-image-plumbing".to_owned(),
                        priority: 100,
                        source: Some(branch_source),
                    },
                ],
            });
        }

        let rendered = render_lines(Some(&model), &[], 10, 80, true);

        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains("▼ zellij-org/zellij")),
            "{:?}",
            rendered.lines
        );
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains("  ▼ feat/kitty-image-plumbing")),
            "{:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .any(|line| line.contains("  ▼ zellij-org/zellij feat/kitty-image-plumbing")),
            "{:?}",
            rendered.lines
        );
    }

    #[test]
    fn multi_segment_group_paths_share_common_parent() {
        let rendered = render_lines(Some(&nested_group_model()), &[], 14, 36, true);

        let parent_headers = rendered
            .lines
            .iter()
            .filter(|line| line.starts_with("▼ project-a"))
            .count();
        assert_eq!(
            parent_headers, 1,
            "sibling leaf groups should share their common parent: {:?}",
            rendered.lines
        );
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.starts_with("  ▼ worktree-b (1)")));
    }

    #[test]
    fn nested_group_toggle_hit_region_uses_group_indent() {
        let rendered = render_lines(Some(&nested_group_model()), &[], 12, 36, true);

        assert_eq!(hit_at(&rendered.hit_regions, 1, 0), None);
        let hit = hit_at(&rendered.hit_regions, 1, 2).expect("nested group toggle");
        assert_eq!(hit.action, HitAction::ToggleGroup);
        assert_eq!(
            hit.group_path,
            match &nested_group_model().rows[0] {
                RailRow::GroupHeader { path, .. } => Some(path.clone()),
                _ => None,
            }
        );
    }

    #[test]
    fn collapsing_nested_leaf_group_hides_only_that_subtree() {
        let model = nested_group_model();
        let leaf_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("expected first row to be a group header"),
        };

        let rendered =
            render_lines_with_collapsed_groups(Some(&model), &[], 12, 36, true, &[leaf_path]);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.starts_with("  ▶ worktree-a (1)")));
        assert!(!rendered.lines.iter().any(|line| line.contains("agent-1")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("worktree-b")));
        assert!(rendered.lines.iter().any(|line| line.contains("agent-2")));
    }

    #[test]
    fn collapsing_nested_parent_group_hides_all_descendants() {
        let model = nested_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => GroupPath(path.0[..1].to_vec()),
            _ => panic!("expected first row to be a group header"),
        };

        let rendered =
            render_lines_with_collapsed_groups(Some(&model), &[], 12, 36, true, &[parent_path]);

        assert!(rendered.lines[0].starts_with("▶ project-a (2): agent-2"));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.contains("worktree-a")));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.contains("worktree-b")));
        assert!(!rendered.lines.iter().any(|line| line.contains("agent-1")));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.starts_with("    ┌ agent-2") || line.starts_with("    ├ agent-2")));
    }

    #[test]
    fn group_header_containing_active_tab_uses_active_style() {
        let rendered =
            render_lines_with_theme(Some(&grouped_model()), &[], 8, 24, true, Some(test_theme()));

        assert!(
            rendered.lines[0].starts_with("\u{1b}[1;38;5;2m▼"),
            "group containing the active tab should use active styling: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn grouped_child_tabs_remain_clickable_and_indented() {
        let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.starts_with("  ┌ tests") || line.starts_with("  ├ tests")));
        assert_eq!(
            hit_at(&rendered.hit_regions, 3, 4).map(|hit| hit.action),
            Some(HitAction::SwitchTab)
        );
    }

    #[test]
    fn grouped_joined_cells_render_child_tabs_as_one_indented_run() {
        let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.starts_with("  ├ tests")),
            "joined grouped child tabs should use an indented separator, not a standalone box: {:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .any(|line| line.starts_with("  ┌ tests")),
            "inactive and active children in the same group should not each start their own box: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn grouped_box_per_tab_keeps_complete_child_boxes() {
        let mut model = grouped_model();
        model.config.structure = RailStructure::BoxPerTab;

        let rendered = render_lines(Some(&model), &[], 8, 24, true);

        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.starts_with("  ┌ tests")),
            "box-per-tab grouped children should keep standalone boxes: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn grouped_rendering_preserves_active_visible_card_metadata() {
        let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

        assert!(rendered.visible_cards.iter().any(|card| card.tab_id == 2));
    }

    #[test]
    fn collapsed_group_hides_child_tabs_but_keeps_toggle_header() {
        let model = grouped_model();
        let group_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("expected first row to be a group header"),
        };

        let rendered =
            render_lines_with_collapsed_groups(Some(&model), &[], 8, 24, true, &[group_path]);

        assert!(rendered.lines[0].starts_with("▶ zellij (2): tests"));
        assert!(
            rendered.lines[0].contains("──"),
            "collapsed group header should keep the same section treatment: {:?}",
            rendered.lines[0]
        );
        assert!(!rendered.lines.iter().any(|line| line.contains("server")));
        assert_eq!(
            rendered
                .lines
                .iter()
                .filter(|line| line.contains("tests"))
                .count(),
            1,
            "collapsed group should show the active tab only in the group header: {:?}",
            rendered.lines
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 0, 0).map(|hit| hit.action),
            Some(HitAction::ToggleGroup)
        );
    }

    #[test]
    fn collapsed_group_header_prefers_active_tab_over_count_when_narrow() {
        let model = grouped_model();
        let group_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("expected first row to be a group header"),
        };

        let rendered =
            render_lines_with_collapsed_groups(Some(&model), &[], 8, 17, true, &[group_path]);

        assert!(
            rendered.lines[0].starts_with("▶ zellij: tests"),
            "narrow collapsed header should retain the active tab field before the count: {:?}",
            rendered.lines[0]
        );
        assert!(
            !rendered.lines[0].contains("(2)"),
            "tab count should be dropped before active tab context on narrow collapsed groups: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn group_header_template_fields_render_in_priority_order() {
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "group.label".to_owned(),
            MetadataValue::Text("zellij".to_owned()),
        );
        metadata.insert("group.tab_count".to_owned(), MetadataValue::Integer(2));
        let fields = group_header_template_fields(&metadata, true, Some("tests"));

        assert_eq!(
            fields,
            vec![
                TemplateField::Required("▶".to_owned()),
                TemplateField::Required("zellij".to_owned()),
                TemplateField::Optional("(2)".to_owned()),
                TemplateField::Priority(": tests".to_owned()),
            ]
        );
    }

    #[test]
    fn render_template_fields_drops_low_numeric_priorities_first() {
        let fields = vec![
            TemplateField::Prioritized {
                value: "repo".to_owned(),
                priority: 100,
                source: None,
            },
            TemplateField::Prioritized {
                value: "main".to_owned(),
                priority: 10,
                source: None,
            },
            TemplateField::Prioritized {
                value: ": tests".to_owned(),
                priority: 80,
                source: None,
            },
        ];

        assert_eq!(render_template_fields(&fields, 12), "repo: tests");
    }

    #[test]
    fn render_template_fields_truncates_low_priority_fields_before_dropping_them() {
        let fields = vec![
            TemplateField::Prioritized {
                value: "zellij-org/zellij".to_owned(),
                priority: 100,
                source: None,
            },
            TemplateField::Prioritized {
                value: "feat/kitty-image-plumbing".to_owned(),
                priority: 60,
                source: None,
            },
        ];

        assert_eq!(
            render_template_fields(&fields, 30),
            "zellij-org/zellij feat/kitt..."
        );
    }

    #[test]
    fn template_matcher_prefers_more_specific_metadata_match() {
        const GENERIC_FIELDS: &[TemplateFieldSpec] = &[TemplateFieldSpec {
            class: TemplateFieldClass::Required,
            sources: &[TemplateValueSource::Literal("generic")],
            prefix: "",
            suffix: "",
            condition: TemplateFieldCondition::Always,
        }];
        const SPECIFIC_FIELDS: &[TemplateFieldSpec] = &[TemplateFieldSpec {
            class: TemplateFieldClass::Required,
            sources: &[TemplateValueSource::Literal("specific")],
            prefix: "",
            suffix: "",
            condition: TemplateFieldCondition::Always,
        }];
        let templates = [
            TemplateDefinition {
                name: "test.generic",
                slot: TemplateSlot::TabStatus,
                node_kind: RenderNodeKind::Tab,
                predicates: &[],
                fields: GENERIC_FIELDS,
                sizing: TemplateSizingHint::Auto,
            },
            TemplateDefinition {
                name: "test.specific",
                slot: TemplateSlot::TabStatus,
                node_kind: RenderNodeKind::Tab,
                predicates: &[MetadataPredicate::TextEquals {
                    key: "status.priority",
                    value: "waiting",
                }],
                fields: SPECIFIC_FIELDS,
                sizing: TemplateSizingHint::Auto,
            },
        ];
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "status.priority".to_owned(),
            MetadataValue::Text("waiting".to_owned()),
        );
        let context = TemplateRenderContext {
            slot: TemplateSlot::TabStatus,
            node_kind: RenderNodeKind::Tab,
            metadata: &metadata,
            collapsed: false,
            active_tab_name: None,
        };

        let template = resolve_template(&templates, &context).expect("matching template");

        assert_eq!(
            template.build_fields(&context),
            vec![TemplateField::Required("specific".to_owned())]
        );
    }

    #[test]
    fn template_matcher_supports_text_prefix_predicates() {
        const GENERIC_FIELDS: &[TemplateFieldSpec] = &[TemplateFieldSpec {
            class: TemplateFieldClass::Required,
            sources: &[TemplateValueSource::Literal("generic")],
            prefix: "",
            suffix: "",
            condition: TemplateFieldCondition::Always,
        }];
        const SPECIFIC_FIELDS: &[TemplateFieldSpec] = &[TemplateFieldSpec {
            class: TemplateFieldClass::Required,
            sources: &[TemplateValueSource::Literal("specific")],
            prefix: "",
            suffix: "",
            condition: TemplateFieldCondition::Always,
        }];
        let templates = [
            TemplateDefinition {
                name: "test.generic",
                slot: TemplateSlot::TabStatus,
                node_kind: RenderNodeKind::Tab,
                predicates: &[],
                fields: GENERIC_FIELDS,
                sizing: TemplateSizingHint::Auto,
            },
            TemplateDefinition {
                name: "test.specific",
                slot: TemplateSlot::TabStatus,
                node_kind: RenderNodeKind::Tab,
                predicates: &[MetadataPredicate::TextPrefix {
                    key: "status.source_pane",
                    prefix: "terminal:",
                }],
                fields: SPECIFIC_FIELDS,
                sizing: TemplateSizingHint::Auto,
            },
        ];
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "status.source_pane".to_owned(),
            MetadataValue::Text("terminal:42".to_owned()),
        );
        let context = TemplateRenderContext {
            slot: TemplateSlot::TabStatus,
            node_kind: RenderNodeKind::Tab,
            metadata: &metadata,
            collapsed: false,
            active_tab_name: None,
        };

        let template = resolve_template(&templates, &context).expect("matching template");

        assert_eq!(
            template.build_fields(&context),
            vec![TemplateField::Required("specific".to_owned())]
        );
    }

    #[test]
    fn template_field_specs_coalesce_sources_and_apply_wrappers() {
        let mut metadata = RenderMetadata::new();
        metadata.insert("zellij.tab.position".to_owned(), MetadataValue::Integer(6));
        let context = TemplateRenderContext {
            slot: TemplateSlot::TabTitle,
            node_kind: RenderNodeKind::Tab,
            metadata: &metadata,
            collapsed: false,
            active_tab_name: None,
        };
        let spec = TemplateFieldSpec {
            class: TemplateFieldClass::Optional,
            sources: &[
                TemplateValueSource::MetadataText("zellij.tab.name"),
                TemplateValueSource::TabNumberFromPosition,
            ],
            prefix: "[",
            suffix: "]",
            condition: TemplateFieldCondition::Always,
        };

        assert_eq!(
            spec.render(&context),
            Some(TemplateField::Optional("[Tab 7]".to_owned()))
        );
    }

    #[test]
    fn builtin_templates_carry_auto_sizing_hints() {
        assert!(BUILTIN_TEMPLATES
            .iter()
            .all(|template| template.sizing == TemplateSizingHint::Auto));
    }

    #[test]
    fn tab_status_fields_are_resolved_through_builtin_template() {
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "status.title".to_owned(),
            MetadataValue::Text("waiting".to_owned()),
        );
        metadata.insert(
            "status.detail".to_owned(),
            MetadataValue::Text("input".to_owned()),
        );

        let fields = template_fields_for(TemplateRenderContext {
            slot: TemplateSlot::TabStatus,
            node_kind: RenderNodeKind::Tab,
            metadata: &metadata,
            collapsed: false,
            active_tab_name: None,
        });

        assert_eq!(
            fields,
            vec![
                TemplateField::Required("waiting".to_owned()),
                TemplateField::Priority(": input".to_owned()),
            ]
        );
    }

    #[test]
    fn group_header_rendering_reads_label_and_count_from_metadata() {
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "group.label".to_owned(),
            MetadataValue::Text("metadata-label".to_owned()),
        );
        metadata.insert("group.tab_count".to_owned(), MetadataValue::Integer(3));
        let group = RenderGroup {
            path: GroupPath::default(),
            conflated_paths: vec![GroupPath::default()],
            label: "typed-label".to_owned(),
            full_label: "/typed".to_owned(),
            tab_count: 1,
            collapsed: false,
            indent: 0,
            metadata,
            metadata_sources: RenderMetadataSources::new(),
            reachable_identities: RenderReachableIdentities::new(),
            templates: ResolvedTemplateSlots::default(),
            children: vec![],
        };
        let mut lines = vec![];
        let mut hit_regions = vec![];

        append_group_header(
            &mut lines,
            &mut hit_regions,
            &group,
            32,
            None,
            None,
            &BTreeSet::new(),
            &MetadataControls::default(),
            None,
        );

        assert!(
            lines[0].starts_with("▼ metadata-label (3)"),
            "group header template should render from metadata: {:?}",
            lines[0]
        );
    }

    #[test]
    fn template_fields_drop_optional_before_priority_when_narrow() {
        let fields = vec![
            TemplateField::Required("▶".to_owned()),
            TemplateField::Required("zellij".to_owned()),
            TemplateField::Optional("(2)".to_owned()),
            TemplateField::Priority(": tests".to_owned()),
        ];

        assert_eq!(render_template_fields(&fields, 17), "▶ zellij: tests");
    }

    #[test]
    fn tab_title_template_fields_use_required_title() {
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "zellij.tab.name".to_owned(),
            MetadataValue::Text("agent".to_owned()),
        );

        assert_eq!(
            tab_title_template_fields(&metadata),
            vec![TemplateField::Required("agent".to_owned())]
        );
    }

    #[test]
    fn tab_title_template_fields_read_name_from_metadata() {
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "zellij.tab.name".to_owned(),
            MetadataValue::Text("metadata-agent".to_owned()),
        );
        let card = RenderCard {
            tab_id: 7,
            position: 6,
            name: "typed-agent".to_owned(),
            active: false,
            pinned: false,
            status: None,
            metadata,
            metadata_sources: RenderMetadataSources::new(),
            reachable_identities: RenderReachableIdentities::new(),
            templates: ResolvedTemplateSlots::default(),
            meta_panel: None,
        };

        assert_eq!(
            tab_title_template_fields(&card.metadata),
            vec![TemplateField::Required("metadata-agent".to_owned())]
        );
    }

    #[test]
    fn external_template_catalog_can_override_tab_title_rendering() {
        let config = andamento_shared::template_config::parse_template_config_json(
            r#"
            {
              "templates": [
                {
                  "name": "custom.tab-title",
                  "slot": "tab-title",
                  "node-kind": "tab",
                  "fields": [
                    { "class": "required", "sources": [{ "kind": "literal", "value": "External" }] }
                  ]
                }
              ]
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_shared::template_config::TemplateConfigCatalog::from_config(config);

        let rendered =
            render_lines_with_template_catalog(Some(&model()), &[], 7, 24, true, Some(&catalog));

        assert!(rendered.lines[0].starts_with("┌ External"));
    }

    #[test]
    fn external_kdl_template_catalog_can_render_numeric_priority_fields() {
        let config = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            template "git.group-header" slot="group-header" node-kind="group" {
              when exists="git.repo"

              field priority=100 {
                value source="collapsed-toggle" collapsed="▶" expanded="▼"
              }
              field key="git.repo" priority=100
              field key="git.branch" priority=10
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_shared::template_config::TemplateConfigCatalog::from_config(config);
        let metadata = RenderMetadata::from([
            (
                "git.repo".to_owned(),
                MetadataValue::Text("rjwittams/zellij-scratch".to_owned()),
            ),
            (
                "git.branch".to_owned(),
                MetadataValue::Text("main".to_owned()),
            ),
        ]);

        let fields = group_header_template_fields_with_template_catalog(
            &metadata,
            false,
            None,
            Some(&catalog),
        );

        assert_eq!(
            fields,
            vec![
                TemplateField::Prioritized {
                    value: "▼".to_owned(),
                    priority: 100,
                    source: None,
                },
                TemplateField::Prioritized {
                    value: "rjwittams/zellij-scratch".to_owned(),
                    priority: 100,
                    source: Some(ResolvedTemplateFieldSource {
                        key: "git.repo".to_owned(),
                        value: MetadataValue::Text("rjwittams/zellij-scratch".to_owned()),
                    }),
                },
                TemplateField::Prioritized {
                    value: "main".to_owned(),
                    priority: 10,
                    source: Some(ResolvedTemplateFieldSource {
                        key: "git.branch".to_owned(),
                        value: MetadataValue::Text("main".to_owned()),
                    }),
                },
            ]
        );
    }

    #[test]
    fn external_kdl_group_header_template_uses_resolved_group_metadata() {
        let config = andamento_shared::template_config::parse_template_config_kdl(
            r#"
            template "git.group-header" slot="group-header" node-kind="group" {
              when exists="git.repo"

              field priority=100 {
                value source="collapsed-toggle" collapsed="▶" expanded="▼"
              }
              field key="git.repo" priority=100
              field key="git.branch" priority=60
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_shared::template_config::TemplateConfigCatalog::from_config(config);
        let mut model = grouped_model();
        let group_path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
            label: None,
        }]);
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Group(group_path),
            values: BTreeMap::from([
                (
                    "git.repo".to_owned(),
                    MetadataEntry {
                        value: MetadataValue::Text("rjwittams/zellij-scratch".to_owned()),
                        updated_at: 1,
                        ttl_ms: None,
                        precedence: 0,
                        ordinal: 0,
                    },
                ),
                (
                    "git.branch".to_owned(),
                    MetadataEntry {
                        value: MetadataValue::Text("main".to_owned()),
                        updated_at: 1,
                        ttl_ms: None,
                        precedence: 0,
                        ordinal: 0,
                    },
                ),
            ]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered =
            render_lines_with_template_catalog(Some(&model), &[], 10, 80, true, Some(&catalog));

        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains("▼ rjwittams/zellij-scratch main")),
            "{:?}",
            rendered.lines
        );
    }

    #[test]
    fn status_template_fields_use_title_and_detail() {
        let mut metadata = RenderMetadata::new();
        metadata.insert(
            "status.title".to_owned(),
            MetadataValue::Text("waiting".to_owned()),
        );
        metadata.insert(
            "status.detail".to_owned(),
            MetadataValue::Text("input".to_owned()),
        );

        assert_eq!(
            status_template_fields(&metadata),
            vec![
                TemplateField::Required("waiting".to_owned()),
                TemplateField::Priority(": input".to_owned()),
            ]
        );
    }

    #[test]
    fn render_card_adapter_populates_status_metadata() {
        let model = model();
        let card = render_card_from_model(&model.tabs[0], &HashMap::new());

        assert_eq!(
            metadata_text(&card.metadata, "status.title"),
            Some("waiting")
        );
        assert_eq!(
            metadata_text(&card.metadata, "status.detail"),
            Some("input")
        );
    }

    #[test]
    fn active_tab_title_is_embedded_in_border_without_marker() {
        let rendered = render_lines(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            4,
            20,
            true,
        );

        assert!(rendered.lines[0].starts_with("┌ tab-2"));
        assert!(!rendered.lines[0].contains("*"));
    }

    #[test]
    fn pin_state_is_not_rendered_as_text() {
        let rendered = render_lines(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            9,
            24,
            true,
        );

        assert!(!rendered.lines.iter().any(|line| line.contains("pin:")));
    }

    #[test]
    fn themed_output_does_not_paint_background_colors() {
        let rendered = render_lines_with_theme(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            9,
            24,
            true,
            Some(test_theme()),
        );

        assert!(!rendered.lines.iter().any(|line| line.contains("[48;")));
    }

    #[test]
    fn themed_output_styles_body_borders_separately_from_body_text() {
        let rendered = render_lines_with_theme(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            9,
            24,
            true,
            Some(test_theme()),
        );

        assert!(
            rendered.lines[1].starts_with("\u{1b}[1;38;5;2m│"),
            "active body side border should use active border color: {:?}",
            rendered.lines[1]
        );
        assert!(
            rendered.lines[1].contains("\u{1b}[38;5;7mwaiting: input"),
            "active body text should use body text color: {:?}",
            rendered.lines[1]
        );
    }

    #[test]
    fn status_icon_reserves_image_space_without_rendering_as_text() {
        let mut model = model();
        model.tabs[0].status.as_mut().unwrap().icon =
            Some(StatusIcon::PngFile("/tmp/waiting.png".into()));

        let rendered = render_lines(Some(&model), &[], 9, 24, true);

        assert!(rendered.lines[1].contains("  waiting: input"));
        assert!(!rendered.lines[1].contains("Builtin"));
        assert!(!rendered.lines[1].contains("waiting waiting"));

        let active_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 2)
            .expect("active card should be visible");
        assert_eq!(active_card.status_row, Some(1));
        assert_eq!(
            active_card.status_icon_rect,
            Some(VisibleIconRect {
                x: 1,
                y: 1,
                columns: 4,
                rows: 4,
            })
        );
        assert_eq!(
            active_card.status_icon,
            Some(StatusIcon::PngFile("/tmp/waiting.png".into()))
        );
    }

    #[test]
    fn status_text_wraps_next_to_full_height_icon() {
        let mut model = model();
        let status = model.tabs[0].status.as_mut().unwrap();
        status.title = "Claude waiting".to_owned();
        status.detail = Some("needs a fairly long approval before continuing".to_owned());
        status.icon = Some(StatusIcon::PngFile("/tmp/waiting.png".into()));

        let rendered = render_lines(Some(&model), &[], 9, 24, true);

        assert!(rendered.lines[1].contains("     Claude waiting:"));
        assert!(rendered.lines[2].contains("     needs a fairly"));
        assert!(rendered.lines[3].contains("     long approval"));
        assert!(rendered.lines[4].contains("     before continuing"));
    }

    #[test]
    fn builtin_icon_key_does_not_reserve_space_until_an_asset_exists() {
        let mut model = model();
        model.tabs[0].status.as_mut().unwrap().icon =
            Some(StatusIcon::Builtin("waiting".to_owned()));

        let rendered = render_lines(Some(&model), &[], 9, 24, true);

        assert!(rendered.lines[1].contains("waiting: input"));
        assert!(!rendered.lines[1].contains("  waiting: input"));
    }

    #[test]
    fn status_icon_slot_uses_terminal_cell_aspect_ratio() {
        let mut model = model();
        model.tabs[0].status.as_mut().unwrap().icon =
            Some(StatusIcon::PngFile("/tmp/waiting.png".into()));

        let rendered = render_lines_with_options(
            Some(&model),
            &[],
            9,
            24,
            true,
            None,
            Some(SizeInPixels {
                width: 9,
                height: 18,
            }),
            &[],
            None,
        );

        let active_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 2)
            .expect("active card should be visible");
        assert_eq!(
            active_card.status_icon_rect,
            Some(VisibleIconRect {
                x: 1,
                y: 1,
                columns: 8,
                rows: 4,
            })
        );
        assert!(rendered.lines[1].contains("         waiting"));
    }

    #[test]
    fn themed_output_uses_active_border_on_active_cell_edges() {
        let rendered = render_lines_with_theme(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            9,
            24,
            true,
            Some(test_theme()),
        );

        assert!(
            rendered.lines[0].starts_with("\u{1b}[1;38;5;2m┌"),
            "active top border should use active border color: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn joined_separator_below_active_uses_cell_below_color() {
        let rendered = render_lines_with_theme(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            9,
            24,
            true,
            Some(test_theme()),
        );

        assert!(
            rendered.lines[5].starts_with("\u{1b}[1;38;5;8m├"),
            "separator introducing inactive cell should use inactive border color: {:?}",
            rendered.lines[5]
        );
        assert!(
            rendered.lines[5].contains("\u{1b}[1;38;5;8m tab-1 "),
            "title embedded in inactive separator should match inactive border color: {:?}",
            rendered.lines[5]
        );
    }

    #[test]
    fn split_around_active_omits_empty_runs_and_isolates_active_box() {
        let mut model = model();
        model.config.structure = RailStructure::SplitAroundActive;

        let rendered = render_lines(Some(&model), &[], 9, 24, true);

        assert!(rendered.lines[0].starts_with("┌ tab-2"));
        assert!(rendered.lines[5].starts_with("└"));
        assert!(rendered.lines[6].starts_with("┌ tab-1"));
    }

    #[test]
    fn box_per_tab_draws_a_complete_box_for_each_visible_tab() {
        let mut model = model();
        model.config.structure = RailStructure::BoxPerTab;
        model.config.sizing = RailSizingPreset::Compact;

        let rendered = render_lines(Some(&model), &[], 7, 24, true);

        assert!(rendered.lines[0].starts_with("┌ tab-2"));
        assert!(rendered.lines[2].starts_with("└"));
        assert!(rendered.lines[3].starts_with("┌ tab-1"));
        assert!(rendered.lines[5].starts_with("└"));
    }

    #[test]
    fn rail_uses_joined_pane_border_topology() {
        let rendered = render_lines(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            9,
            20,
            true,
        );

        assert!(rendered.lines[0].starts_with("┌"));
        assert!(rendered.lines.iter().any(|line| line.starts_with("├")));
        assert!(rendered.lines.iter().any(|line| line.starts_with("└")));
    }

    #[test]
    fn active_tab_gets_expanded_cell_when_space_allows() {
        let rendered = render_lines(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            9,
            24,
            true,
        );

        let active_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 2)
            .expect("active card should be visible");
        let next_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 1)
            .expect("inactive card should be visible");

        assert_eq!(next_card.row_start - active_card.row_start, 5);
    }

    #[test]
    fn controller_model_active_state_drives_rendering() {
        let rendered = render_lines(Some(&model()), &[], 9, 24, true);

        let active_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 2)
            .expect("controller active card should be visible");
        let next_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 1)
            .expect("inactive card should be visible");

        assert_eq!(next_card.row_start - active_card.row_start, 5);
    }

    #[test]
    fn local_active_state_overrides_controller_model_active_state() {
        let mut model = model();
        model.tabs.swap(0, 1);
        let rendered = render_lines(
            Some(&model),
            &[local_tab(1, 0, true), local_tab(2, 1, false)],
            9,
            24,
            true,
        );

        let local_active_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 1)
            .expect("local active card should be visible");
        let controller_active_card = rendered
            .visible_cards
            .iter()
            .find(|card| card.tab_id == 2)
            .expect("controller active card should be visible");

        assert_eq!(
            controller_active_card.row_start - local_active_card.row_start,
            5
        );
    }

    #[test]
    fn fallback_warning_uses_final_row() {
        let rendered = render_lines(None, &[local_tab(1, 0, true)], 3, 24, false);

        assert_eq!(
            rendered.lines.last().unwrap().trim(),
            "controller unavailable"
        );
    }

    #[test]
    fn truncation_does_not_exceed_width() {
        let rendered = render_lines(
            None,
            &[LocalTab {
                tab_id: 1,
                position: 0,
                name: "a very very very long tab name".to_owned(),
                active: true,
            }],
            2,
            10,
            false,
        );

        assert!(rendered.lines.iter().all(|line| line.width() <= 10));
    }

    #[test]
    fn hit_regions_map_rows_to_actions() {
        let rendered = render_lines(
            Some(&model()),
            &[local_tab(1, 0, false), local_tab(2, 1, true)],
            4,
            20,
            true,
        );

        assert_eq!(
            hit_at(&rendered.hit_regions, 1, 2).map(|hit| hit.action),
            Some(HitAction::TogglePin)
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 0, 8).map(|hit| hit.action),
            Some(HitAction::SwitchTab)
        );
    }

    #[test]
    fn final_row_opens_config() {
        let rendered = render_lines(Some(&model()), &[], 9, 24, true);

        assert_eq!(
            hit_at(&rendered.hit_regions, 8, 0).map(|hit| hit.action),
            Some(HitAction::OpenConfig)
        );
        assert!(rendered.lines[8].starts_with("⚙"));
        // The right edge is now the root inspect selector when scroll arrows
        // are absent.
        assert_eq!(
            hit_at(&rendered.hit_regions, 8, 23).map(|hit| hit.action),
            Some(HitAction::InspectNode)
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 8, 23).and_then(|hit| hit.inspect_target),
            Some(NodeKey::Root)
        );
    }

    #[test]
    fn border_row_zero_width_emits_empty_string() {
        let line = BorderRow::new(
            0,
            BorderKind::Top {
                active: true,
                first_cell: true,
            },
        )
        .into_line(None);
        assert_eq!(line, "");
        let line = BorderRow::new(0, BorderKind::Bottom { active: false }).into_line(None);
        assert_eq!(line, "");
        let line = BorderRow::new(0, BorderKind::Footer).into_line(None);
        assert_eq!(line, "");
    }

    #[test]
    fn border_row_width_one_emits_left_corner_only() {
        let line = BorderRow::new(
            1,
            BorderKind::Top {
                active: true,
                first_cell: true,
            },
        )
        .into_line(None);
        assert_eq!(line, "┌");
        let line = BorderRow::new(1, BorderKind::Bottom { active: true }).into_line(None);
        assert_eq!(line, "└");
    }

    #[test]
    fn border_row_top_with_title_matches_old_format() {
        let line = BorderRow::new(
            20,
            BorderKind::Top {
                active: true,
                first_cell: true,
            },
        )
        .also(|row| row.title("zellij"))
        .into_line(None);
        assert_eq!(line, "┌ zellij ──────────┐");
    }

    #[test]
    fn border_row_top_continuation_uses_t_corners() {
        let line = BorderRow::new(
            10,
            BorderKind::Top {
                active: true,
                first_cell: false,
            },
        )
        .also(|row| row.title("x"))
        .into_line(None);
        assert_eq!(line, "├ x ─────┤");
    }

    #[test]
    fn border_row_bottom_has_no_title_slot() {
        let line = BorderRow::new(10, BorderKind::Bottom { active: false }).into_line(None);
        assert_eq!(line, "└────────┘");
    }

    #[test]
    fn border_row_title_truncates_to_fit() {
        let line = BorderRow::new(
            8,
            BorderKind::Top {
                active: false,
                first_cell: true,
            },
        )
        .also(|row| row.title("a-very-long-title-here"))
        .into_line(None);
        assert_eq!(line.chars().count(), 8);
        assert!(line.starts_with('┌'));
        assert!(line.ends_with('┐'));
    }

    #[test]
    fn border_row_footer_places_gear_and_scroll_arrows() {
        let mut footer = BorderRow::new(10, BorderKind::Footer);
        footer.place_left(0, '⚙', None, "gear");
        footer.place_right(1, '▲', None, "up");
        footer.place_right(0, '▼', None, "down");
        let line = footer.into_line(None);
        assert_eq!(line, "⚙       ▲▼");
    }

    #[test]
    fn border_row_footer_emits_hit_regions_at_correct_columns() {
        let mut footer = BorderRow::new(10, BorderKind::Footer);
        footer.place_left(
            0,
            '⚙',
            Some((HitAction::OpenConfig, BorderHitPayload::none())),
            "gear",
        );
        footer.place_right(
            1,
            '▲',
            Some((HitAction::ScrollRailUp, BorderHitPayload::none())),
            "up",
        );
        footer.place_right(
            0,
            '▼',
            Some((HitAction::ScrollRailDown, BorderHitPayload::none())),
            "down",
        );
        let (_, hits) = footer.finish(7, None);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].row_start, 7);
        assert_eq!(hits[0].col_start, 0);
        assert_eq!(hits[0].action, HitAction::OpenConfig);
        assert_eq!(hits[1].col_start, 8);
        assert_eq!(hits[1].action, HitAction::ScrollRailUp);
        assert_eq!(hits[2].col_start, 9);
        assert_eq!(hits[2].action, HitAction::ScrollRailDown);
    }

    #[test]
    fn border_row_top_decoration_then_title_fills_around_it() {
        let mut row = BorderRow::new(
            20,
            BorderKind::Top {
                active: true,
                first_cell: true,
            },
        );
        row.place_right(0, '○', None, "tristate");
        row.title("tab");
        let line = row.into_line(None);
        assert_eq!(line, "┌ tab ────────────○┐");
    }

    #[test]
    fn border_row_title_skips_claimed_left_cells() {
        let mut row = BorderRow::new(
            20,
            BorderKind::Top {
                active: true,
                first_cell: true,
            },
        );
        row.place_left(0, '◇', None, "root_toggle");
        row.title("tab");
        let line = row.into_line(None);
        assert_eq!(line, "┌◇ tab ────────────┐");
    }

    #[test]
    #[should_panic(expected = "BorderRow cell collision")]
    fn border_row_collision_panics_in_debug() {
        let mut row = BorderRow::new(
            20,
            BorderKind::Top {
                active: true,
                first_cell: true,
            },
        );
        row.place_left(0, '◇', None, "first");
        row.place_left(0, '◆', None, "second");
    }

    #[test]
    #[should_panic(expected = "BorderRow::place_at out of inner range")]
    fn border_row_out_of_range_panics_in_debug() {
        let mut row = BorderRow::new(
            5,
            BorderKind::Top {
                active: true,
                first_cell: true,
            },
        );
        row.place_left(99, '◇', None, "way_off");
    }

    trait BorderRowExt {
        fn also(self, f: impl FnOnce(&mut Self)) -> Self;
    }

    impl BorderRowExt for BorderRow {
        fn also(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }

    #[test]
    fn append_group_header_returns_single_row_allocation() {
        let mut lines: Vec<String> = vec!["pre".into(), "pre".into()];
        let mut hits = vec![];
        let group = RenderGroup {
            path: GroupPath(vec![GroupSegment {
                key: "git.repo".into(),
                value: MetadataValue::Text("zellij".into()),
                label: Some("zellij".into()),
            }]),
            conflated_paths: vec![GroupPath(vec![GroupSegment {
                key: "git.repo".into(),
                value: MetadataValue::Text("zellij".into()),
                label: Some("zellij".into()),
            }])],
            label: "zellij".into(),
            full_label: "zellij".into(),
            tab_count: 1,
            collapsed: false,
            indent: 0,
            metadata: BTreeMap::new(),
            metadata_sources: BTreeMap::new(),
            reachable_identities: vec![],
            templates: ResolvedTemplateSlots::default(),
            children: vec![],
        };
        let (_sources, allocation) = append_group_header(
            &mut lines,
            &mut hits,
            &group,
            20,
            None,
            None,
            &BTreeSet::new(),
            &MetadataControls::default(),
            None,
        );
        assert_eq!(allocation.rows, 2..3);
        assert!(matches!(allocation.key, NodeKey::Group(ref p) if p == &group.path));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn metadata_controls_cycle_visits_each_state_then_clears() {
        let mut controls = MetadataControls::default();
        let key = NodeKey::Tab(42);
        // Absent → Meta
        controls.cycle(key.clone());
        assert_eq!(controls.per_node.get(&key), Some(&MetadataTriState::Meta));
        // Meta → MetaChildren
        controls.cycle(key.clone());
        assert_eq!(
            controls.per_node.get(&key),
            Some(&MetadataTriState::MetaChildren)
        );
        // MetaChildren → Clean
        controls.cycle(key.clone());
        assert_eq!(controls.per_node.get(&key), Some(&MetadataTriState::Clean));
        // Clean → Absent
        controls.cycle(key.clone());
        assert_eq!(controls.per_node.get(&key), None);
    }

    #[test]
    fn metadata_tristate_glyphs_are_distinct() {
        assert_eq!(MetadataTriState::Clean.glyph(), '○');
        assert_eq!(MetadataTriState::Meta.glyph(), '◐');
        assert_eq!(MetadataTriState::MetaChildren.glyph(), '●');
    }

    #[test]
    fn footer_shows_unselected_root_inspect_glyph() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let controls = MetadataControls::default();
        render_footer(&mut lines, &mut hits, 0, 20, None, &controls, None, false);
        assert!(lines[0].starts_with("⚙"), "got {:?}", lines[0]);
        assert!(lines[0].ends_with("○"), "got {:?}", lines[0]);
        let inspect_hit = hits
            .iter()
            .find(|h| h.inspect_target == Some(NodeKey::Root))
            .expect("inspect hit emitted");
        assert_eq!(inspect_hit.action, HitAction::InspectNode);
        assert_eq!(inspect_hit.col_start, 19);
    }

    #[test]
    fn footer_shows_selected_root_inspect_glyph() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let controls = MetadataControls::default();
        render_footer(
            &mut lines,
            &mut hits,
            0,
            20,
            None,
            &controls,
            Some(&NodeKey::Root),
            false,
        );
        assert!(lines[0].starts_with("⚙"), "got {:?}", lines[0]);
        assert!(lines[0].ends_with("●"), "got {:?}", lines[0]);
    }

    #[test]
    fn footer_places_root_inspect_next_to_scroll_arrows() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let controls = MetadataControls::default();
        render_footer(
            &mut lines,
            &mut hits,
            0,
            20,
            None,
            &controls,
            Some(&NodeKey::Root),
            true,
        );
        assert!(lines[0].ends_with("●▲▼"), "got {:?}", lines[0]);
        let cycle_hit = hits
            .iter()
            .find(|h| h.inspect_target == Some(NodeKey::Root))
            .expect("root inspect hit emitted");
        assert_eq!(cycle_hit.action, HitAction::InspectNode);
        assert_eq!(cycle_hit.col_start, 17);
    }

    #[test]
    fn footer_places_root_inspect_at_far_right_when_no_scroll_arrows() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let controls = MetadataControls::default();
        render_footer(
            &mut lines,
            &mut hits,
            0,
            20,
            None,
            &controls,
            Some(&NodeKey::Root),
            false,
        );
        assert!(lines[0].ends_with('●'), "got {:?}", lines[0]);
    }

    #[test]
    fn footer_always_keeps_root_inspect_selector() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let controls = MetadataControls::default();
        render_footer(&mut lines, &mut hits, 0, 20, None, &controls, None, false);
        assert!(
            hits.iter()
                .any(|h| h.action == HitAction::InspectNode
                    && h.inspect_target == Some(NodeKey::Root)),
            "root inspect hit should still be emitted"
        );
        assert!(lines[0].starts_with("⚙"), "got {:?}", lines[0]);
        assert!(lines[0].ends_with('○'), "got {:?}", lines[0]);
    }

    #[test]
    fn tab_top_border_places_selected_inspect_glyph() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let mut controls = MetadataControls::default();
        controls
            .per_node
            .insert(NodeKey::Tab(7), MetadataTriState::MetaChildren);
        let card = RenderCard {
            tab_id: 7,
            position: 0,
            name: "tab".into(),
            active: true,
            pinned: false,
            status: None,
            metadata: BTreeMap::new(),
            metadata_sources: BTreeMap::new(),
            reachable_identities: vec![],
            templates: ResolvedTemplateSlots::default(),
            meta_panel: None,
        };
        write_tab_top_border(
            &mut lines,
            &mut hits,
            0,
            "tab",
            20,
            true,
            &card,
            None,
            &controls,
            Some(&NodeKey::Tab(7)),
        );
        assert!(lines[0].ends_with("●┐"), "got {:?}", lines[0]);
        let cycle_hit = hits
            .iter()
            .find(|h| h.action == HitAction::InspectNode)
            .expect("inspect hit emitted");
        assert_eq!(cycle_hit.col_start, 18);
        assert_eq!(cycle_hit.tab_id, 7);
    }

    #[test]
    fn tab_top_border_places_unselected_inspect_glyph() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let controls = MetadataControls::default(); // root disabled
        let card = RenderCard {
            tab_id: 7,
            position: 0,
            name: "tab".into(),
            active: false,
            pinned: false,
            status: None,
            metadata: BTreeMap::new(),
            metadata_sources: BTreeMap::new(),
            reachable_identities: vec![],
            templates: ResolvedTemplateSlots::default(),
            meta_panel: None,
        };
        write_tab_top_border(
            &mut lines, &mut hits, 0, "tab", 20, true, &card, None, &controls, None,
        );
        assert!(lines[0].ends_with("○┐"), "got {:?}", lines[0]);
        assert!(hits.iter().any(|h| h.action == HitAction::InspectNode));
    }

    #[test]
    fn effective_show_uses_explicit_node_state() {
        let mut controls = MetadataControls::default();
        controls
            .per_node
            .insert(NodeKey::Tab(1), MetadataTriState::Meta);
        assert!(controls.effective_show(&NodeKey::Tab(1), false));
    }

    #[test]
    fn effective_show_explicit_clean_overrides_inheritance() {
        let mut controls = MetadataControls::default();
        controls
            .per_node
            .insert(NodeKey::Tab(1), MetadataTriState::Clean);
        // Even with ancestor MetaChildren cascading down, explicit Clean hides.
        assert!(!controls.effective_show(&NodeKey::Tab(1), true));
    }

    #[test]
    fn effective_show_inherits_from_ancestor_when_absent() {
        let controls = MetadataControls::default();
        // No entry for Tab(1); inherits.
        assert!(controls.effective_show(&NodeKey::Tab(1), true));
        assert!(!controls.effective_show(&NodeKey::Tab(1), false));
    }

    #[test]
    fn propagates_to_children_only_for_meta_children_or_inherited() {
        let mut controls = MetadataControls::default();
        let key = NodeKey::Tab(1);
        // Explicit Meta does NOT propagate.
        controls
            .per_node
            .insert(key.clone(), MetadataTriState::Meta);
        assert!(!controls.propagates_to_children(&key, false));
        assert!(!controls.propagates_to_children(&key, true));
        // Explicit MetaChildren propagates.
        controls
            .per_node
            .insert(key.clone(), MetadataTriState::MetaChildren);
        assert!(controls.propagates_to_children(&key, false));
        // Explicit Clean stops propagation even if ancestor was MetaChildren.
        controls
            .per_node
            .insert(key.clone(), MetadataTriState::Clean);
        assert!(!controls.propagates_to_children(&key, true));
        // Absent passes ancestor flag through.
        controls.per_node.remove(&key);
        assert!(controls.propagates_to_children(&key, true));
        assert!(!controls.propagates_to_children(&key, false));
    }

    #[test]
    fn hit_at_prefers_smaller_region_over_card_wide() {
        let hits = vec![
            HitRegion {
                row_start: 0,
                row_end: 5,
                col_start: 0,
                col_end: 19,
                tab_id: 7,
                tab_position: 0,
                group_path: None,
                inspect_target: None,
                action: HitAction::SwitchTab,
            },
            HitRegion {
                row_start: 0,
                row_end: 0,
                col_start: 18,
                col_end: 18,
                tab_id: 7,
                tab_position: 0,
                group_path: None,
                inspect_target: Some(NodeKey::Tab(7)),
                action: HitAction::InspectNode,
            },
        ];
        // Clicking the single-cell tristate glyph should NOT be shadowed
        // by the surrounding card-wide SwitchTab hit.
        let hit = hit_at(&hits, 0, 18).expect("hit found");
        assert_eq!(hit.action, HitAction::InspectNode);
        // Clicking elsewhere on the card still falls through to SwitchTab.
        let hit = hit_at(&hits, 2, 5).expect("hit found");
        assert_eq!(hit.action, HitAction::SwitchTab);
    }

    #[test]
    fn cell_height_grows_when_card_has_meta_panel() {
        let mut card = RenderCard {
            tab_id: 1,
            position: 0,
            name: "t".into(),
            active: false,
            pinned: false,
            status: None,
            metadata: BTreeMap::new(),
            metadata_sources: BTreeMap::new(),
            reachable_identities: vec![],
            templates: ResolvedTemplateSlots::default(),
            meta_panel: None,
        };
        let base = cell_height(&card, RailSizingPreset::Compact, false);
        card.meta_panel = Some(vec!["a".into(), "b".into(), "c".into()]);
        let grown = cell_height(&card, RailSizingPreset::Compact, false);
        assert_eq!(grown, base + 3);
    }

    #[test]
    fn append_meta_panel_emits_red_bar_and_content() {
        let mut lines: Vec<String> = vec![];
        let panel_lines = vec!["group: zellij".to_owned(), "  [metadata]".to_owned()];
        append_meta_panel(&mut lines, 2, 30, panel_lines, None);
        assert_eq!(lines.len(), 2);
        // Outer indent 2, then `│`, then space, then content.
        assert!(
            lines[0].starts_with("  │ group: zellij"),
            "got {:?}",
            lines[0]
        );
        assert!(
            lines[1].starts_with("  │   [metadata]"),
            "got {:?}",
            lines[1]
        );
    }

    #[test]
    fn group_header_appends_inspect_glyph() {
        let mut lines = vec![];
        let mut hits = vec![];
        let mut controls = MetadataControls::default();
        let path = GroupPath(vec![GroupSegment {
            key: "git.repo".into(),
            value: MetadataValue::Text("zellij".into()),
            label: Some("zellij".into()),
        }]);
        controls
            .per_node
            .insert(NodeKey::Group(path.clone()), MetadataTriState::Meta);
        let group = RenderGroup {
            path: path.clone(),
            conflated_paths: vec![path.clone()],
            label: "zellij".into(),
            full_label: "zellij".into(),
            tab_count: 1,
            collapsed: false,
            indent: 0,
            metadata: BTreeMap::new(),
            metadata_sources: BTreeMap::new(),
            reachable_identities: vec![],
            templates: ResolvedTemplateSlots::default(),
            children: vec![],
        };
        append_group_header(
            &mut lines,
            &mut hits,
            &group,
            30,
            None,
            None,
            &BTreeSet::new(),
            &controls,
            Some(&NodeKey::Group(path.clone())),
        );
        assert!(lines[0].ends_with("●─"), "got {:?}", lines[0]);
        let cycle_hit = hits
            .iter()
            .find(|h| h.action == HitAction::InspectNode)
            .expect("inspect hit emitted");
        assert_eq!(cycle_hit.col_start, 28);
        assert_eq!(cycle_hit.group_path.as_ref(), Some(&path));
    }

    #[test]
    fn append_tab_run_returns_per_tab_allocations() {
        let mut lines: Vec<String> = vec!["pre".into()];
        let mut hits = vec![];
        let mut visible = vec![];
        let make_tab = |id: u64, position: usize, active: bool| RenderTab {
            card: RenderCard {
                tab_id: id,
                position,
                name: format!("tab{id}"),
                active,
                pinned: false,
                status: None,
                metadata: BTreeMap::new(),
                metadata_sources: BTreeMap::new(),
                reachable_identities: vec![],
                templates: ResolvedTemplateSlots::default(),
                meta_panel: None,
            },
            indent: 0,
            grouping: None,
            parent_path: None,
        };
        let tabs = vec![make_tab(1, 0, true), make_tab(2, 1, false)];
        let allocations = append_tab_run(
            &mut lines,
            &mut hits,
            &mut visible,
            &tabs,
            30,
            true,
            RailConfig::default(),
            None,
            None,
            None,
            &MetadataControls::default(),
            None,
            false,
        );
        assert_eq!(allocations.len(), 2);
        for allocation in &allocations {
            assert!(matches!(allocation.key, NodeKey::Tab(_)));
            assert!(
                allocation.rows.start >= 1,
                "starts after the preexisting row"
            );
            assert!(allocation.rows.end > allocation.rows.start);
            assert!(allocation.rows.end <= lines.len());
        }
        let tab_ids: Vec<u64> = allocations
            .iter()
            .map(|a| match a.key {
                NodeKey::Tab(id) => id,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(tab_ids, vec![1, 2]);
    }
}
