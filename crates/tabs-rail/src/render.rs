use std::collections::{BTreeMap, HashMap};

use ansi_term::{Color, Style};
use tabs_shared::template_config::{
    TemplateConfigCatalog, TemplateConfigFieldClass, TemplateConfigMatchContext,
    TemplateConfigNodeKind, TemplateConfigSlot,
};
use tabs_shared::{
    ControllerViewModel, GroupPath, GroupSegment, MetadataEntry, MetadataSourceEntry,
    MetadataTarget, MetadataValue, ObservedMetadataIdentity, PaneTarget, Priority, RailConfig,
    RailRow, RailSizingPreset, RailStructure, RailViewMode, ReachableMetadataIdentity,
    ResolvedMetadata, ResolvedTemplateSlot, ResolvedTemplateSlots, StatusIcon, TabCard,
    TabGroupingInfo, TabStatusSummary,
};
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
    ScrollMetadataUp,
    ScrollMetadataDown,
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
    pub action: HitAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedRail {
    pub lines: Vec<String>,
    pub hit_regions: Vec<HitRegion>,
    pub visible_cards: Vec<VisibleCard>,
    pub metadata_scroll_offset: usize,
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
}

impl From<Styling> for RenderTheme {
    fn from(colors: Styling) -> Self {
        Self {
            // Match the normal tab bar's selected/unselected foreground choices,
            // but do not paint a background over the terminal default.
            active_border: colors.ribbon_selected.background,
            inactive_border: colors.ribbon_unselected.background,
            body_foreground: colors.text_unselected.base,
        }
    }
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RenderNode {
    Group(RenderGroup),
    Tab(RenderTab),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderGroup {
    path: GroupPath,
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
    render_lines_with_theme_and_cell_size(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        theme,
        0,
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
        0,
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
        0,
        None,
        &[],
        template_catalog,
    )
}

pub fn render_lines_with_theme_and_cell_size(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    metadata_scroll_offset: usize,
    terminal_cell_size: Option<SizeInPixels>,
) -> RenderedRail {
    render_lines_with_options(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        theme,
        metadata_scroll_offset,
        terminal_cell_size,
        &[],
        None,
    )
}

pub fn render_lines_with_options(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    metadata_scroll_offset: usize,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_groups: &[GroupPath],
    template_catalog: Option<&TemplateConfigCatalog>,
) -> RenderedRail {
    if rows == 0 || cols == 0 {
        return RenderedRail {
            lines: vec![],
            hit_regions: vec![],
            visible_cards: vec![],
            metadata_scroll_offset: 0,
        };
    }

    let footer_rows = 1.min(rows);
    let card_rows_available = rows.saturating_sub(footer_rows);
    let mut lines = vec![blank(cols); rows];
    let mut hit_regions = vec![];
    let mut visible_cards = vec![];
    let nodes = nodes_to_render(model, tabs, collapsed_groups);
    let config = model.map(|model| model.config).unwrap_or_default();
    let mut effective_metadata_scroll_offset = 0;
    if config.view == RailViewMode::Metadata {
        effective_metadata_scroll_offset = render_metadata_projection(
            &mut lines,
            &mut hit_regions,
            &nodes,
            model
                .map(|model| model.observed_identities.as_slice())
                .unwrap_or(&[]),
            card_rows_available,
            cols,
            theme,
            metadata_scroll_offset,
        );
    } else if nodes.is_empty() {
        lines[0] = pad_to_width("tabs: waiting for tab state", cols);
    } else {
        render_nodes(
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
        );
    }

    if controller_available {
        render_footer(&mut lines, &mut hit_regions, rows - 1, cols, theme);
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
        metadata_scroll_offset: effective_metadata_scroll_offset,
    }
}

fn render_metadata_projection(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    nodes: &[RenderNode],
    observed_identities: &[ObservedMetadataIdentity],
    available_rows: usize,
    cols: usize,
    theme: Option<RenderTheme>,
    metadata_scroll_offset: usize,
) -> usize {
    let mut blocks = observed_identity_metadata_blocks(observed_identities);
    blocks.extend(metadata_projection_blocks(nodes));
    let content_lines = blocks
        .iter()
        .flat_map(|block| block.lines.iter().cloned())
        .collect::<Vec<_>>();
    let scroll_offset =
        clamp_scroll_offset(metadata_scroll_offset, content_lines.len(), available_rows);
    for (output_row, line) in content_lines
        .iter()
        .skip(scroll_offset)
        .take(available_rows)
        .enumerate()
    {
        lines[output_row] =
            style_body_text(pad_to_width(&truncate_to_width(line, cols), cols), theme);
    }
    let mut source_row = 0;
    for block in blocks {
        if source_row >= scroll_offset.saturating_add(available_rows) {
            break;
        }
        let block_start = source_row;
        source_row += block.lines.len();
        if let Some(hit) = block.hit {
            let visible_start = block_start.max(scroll_offset);
            let visible_end = source_row.min(scroll_offset.saturating_add(available_rows));
            if visible_start < visible_end {
                hit_regions.push(HitRegion {
                    row_start: visible_start - scroll_offset,
                    row_end: visible_end - scroll_offset - 1,
                    col_start: hit.indent.min(cols.saturating_sub(1)),
                    col_end: cols.saturating_sub(1),
                    tab_id: hit.tab_id,
                    tab_position: hit.tab_position,
                    group_path: None,
                    action: HitAction::SwitchTab,
                });
            }
        }
    }
    scroll_offset
}

fn observed_identity_metadata_blocks(
    observed_identities: &[ObservedMetadataIdentity],
) -> Vec<MetadataBlock> {
    if observed_identities.is_empty() {
        return vec![];
    }
    let mut lines = vec!["observed_identities".to_owned()];
    for observed in observed_identities {
        push_metadata_text_line(
            &mut lines,
            2,
            &format!("observed_identity.{}", observed.identity.key),
            &format!(
                "{} target_count={} nearest_distance={}",
                format_metadata_value(&observed.identity.value),
                observed.target_count,
                observed.nearest_distance
            ),
        );
    }
    vec![MetadataBlock { lines, hit: None }]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetadataBlock {
    lines: Vec<String>,
    hit: Option<MetadataHit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MetadataHit {
    tab_id: u64,
    tab_position: usize,
    indent: usize,
}

fn metadata_projection_blocks(nodes: &[RenderNode]) -> Vec<MetadataBlock> {
    let mut blocks = vec![];
    for node in nodes {
        match node {
            RenderNode::Group(group) => {
                blocks.push(group_metadata_block(group));
                blocks.extend(metadata_projection_blocks(&group.children));
            }
            RenderNode::Tab(tab) => blocks.push(tab_metadata_block(tab)),
        }
    }
    blocks
}

fn group_metadata_block(group: &RenderGroup) -> MetadataBlock {
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
    push_metadata_section_header(&mut lines, 2, "templates");
    push_template_or_builtin_diagnostic_line(
        &mut lines,
        4,
        "template.group_header",
        group.templates.group_header.as_ref(),
        TemplateRenderContext {
            slot: TemplateSlot::GroupHeader,
            node_kind: RenderNodeKind::Group,
            metadata: &group.metadata,
            collapsed: group.collapsed,
            active_tab_name: active_tab_name(&group.children),
        },
    );
    push_metadata_section_header(&mut lines, 2, "group_path");
    push_group_path_metadata(&mut lines, 4, &group.path);
    MetadataBlock { lines, hit: None }
}

fn tab_metadata_block(tab: &RenderTab) -> MetadataBlock {
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
    push_metadata_section_header(&mut lines, indent + 2, "templates");
    push_template_or_builtin_diagnostic_line(
        &mut lines,
        indent + 4,
        "template.tab_title",
        card.templates.tab_title.as_ref(),
        TemplateRenderContext {
            slot: TemplateSlot::TabTitle,
            node_kind: RenderNodeKind::Tab,
            metadata: &card.metadata,
            collapsed: false,
            active_tab_name: None,
        },
    );
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
        push_template_or_builtin_diagnostic_line(
            &mut lines,
            indent + 4,
            "template.tab_status",
            card.templates.tab_status.as_ref(),
            TemplateRenderContext {
                slot: TemplateSlot::TabStatus,
                node_kind: RenderNodeKind::Tab,
                metadata: &card.metadata,
                collapsed: false,
                active_tab_name: None,
            },
        );
    }
    MetadataBlock {
        lines,
        hit: Some(MetadataHit {
            tab_id: card.tab_id,
            tab_position: card.position,
            indent,
        }),
    }
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
    push_metadata_section_header(lines, indent, "sources");
    for (key, entries) in source_entries {
        if entries.is_empty() {
            continue;
        }
        push_metadata_text_line(lines, indent + 2, "key", key);
        push_metadata_source_table(lines, indent + 2, entries);
    }
}

fn push_metadata_source_table(
    lines: &mut Vec<String>,
    indent: usize,
    entries: &[MetadataSourceEntry],
) {
    let rows = entries
        .iter()
        .map(|entry| {
            (
                entry.source_id.as_str(),
                format_metadata_value(&entry.entry.value),
                entry,
            )
        })
        .collect::<Vec<_>>();
    let source_width = rows
        .iter()
        .map(|(source_id, _, _)| source_id.len())
        .max()
        .unwrap_or(0)
        .max("source".len());
    let value_width = rows
        .iter()
        .map(|(_, value, _)| value.len())
        .max()
        .unwrap_or(0)
        .max("value".len());

    lines.push(format!(
        "{}{:<source_width$}  {:<value_width$}  updated  ttl  prec  ord",
        " ".repeat(indent),
        "src",
        "value",
    ));
    for (source_id, value, entry) in rows {
        lines.push(format!(
            "{}{source_id:<source_width$}  {value:<value_width$}  {:>7}  {:>3}  {:>4}  {:>3}",
            " ".repeat(indent),
            entry.entry.updated_at,
            entry
                .entry
                .ttl_ms
                .map(|ttl_ms| ttl_ms.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            entry.entry.precedence,
            entry.entry.ordinal,
        ));
    }
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
    for reachable in reachable_identities {
        push_metadata_text_line(
            lines,
            indent + 2,
            &format!("identity.{}", reachable.identity.key),
            &format!(
                "{} distance={}",
                format_metadata_value(&reachable.identity.value),
                reachable.distance
            ),
        );
    }
}

fn push_metadata_text_line(lines: &mut Vec<String>, indent: usize, key: &str, value: &str) {
    lines.push(format!("{}{}: {}", " ".repeat(indent), key, value));
}

fn push_metadata_section_header(lines: &mut Vec<String>, indent: usize, label: &str) {
    lines.push(format!("{}[{}]", " ".repeat(indent), label));
}

fn clamp_scroll_offset(offset: usize, content_rows: usize, available_rows: usize) -> usize {
    offset.min(content_rows.saturating_sub(available_rows))
}

fn format_metadata_value(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Text(value) => value.clone(),
        MetadataValue::Bool(value) => bool_text(*value).to_owned(),
        MetadataValue::Integer(value) => value.to_string(),
        MetadataValue::StringList(values) => values.join(", "),
    }
}

fn push_template_or_builtin_diagnostic_line(
    lines: &mut Vec<String>,
    indent: usize,
    key: &str,
    resolved_slot: Option<&ResolvedTemplateSlot>,
    context: TemplateRenderContext<'_>,
) {
    if let Some(slot) = resolved_slot {
        push_metadata_text_line(lines, indent, key, &slot.template_name);
        let fields = slot
            .fields
            .iter()
            .map(|field| format!("{}({})", field.text, field.priority))
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            push_metadata_text_line(lines, indent, &format!("{key}.fields"), &fields.join(", "));
        }
        return;
    }
    if let Some(template) = matched_template(context) {
        push_metadata_text_line(lines, indent, key, template.name);
        push_metadata_text_line(
            lines,
            indent,
            &format!("{key}.sizing"),
            template.sizing.as_text(),
        );
        push_metadata_text_line(
            lines,
            indent,
            &format!("{key}.specificity"),
            &template.specificity().to_string(),
        );
        push_metadata_text_line(
            lines,
            indent,
            &format!("{key}.predicates"),
            &template.predicates_text(),
        );
        push_metadata_text_line(
            lines,
            indent,
            &format!("{key}.candidates"),
            &matched_template_candidates_text(context),
        );
    }
}

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
    Style::new().fg(match color {
        PaletteColor::Rgb((r, g, b)) => Color::RGB(r, g, b),
        PaletteColor::EightBit(color) => Color::Fixed(color),
    })
}

pub fn hit_at(hit_regions: &[HitRegion], row: usize, col: usize) -> Option<HitRegion> {
    hit_regions
        .iter()
        .rev()
        .find(|region| {
            row >= region.row_start
                && row <= region.row_end
                && col >= region.col_start
                && col <= region.col_end
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
) {
    if nodes.is_empty() || available_rows == 0 {
        return;
    }

    if let Some(cards) = top_level_cards(nodes) {
        return render_cards(
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
        );
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
    );
    copy_visible_buffer(
        lines,
        hit_regions,
        visible_cards,
        buffered_lines,
        buffered_hits,
        buffered_cards,
        active_tab_id(nodes),
        available_rows,
    );
}

fn top_level_cards(nodes: &[RenderNode]) -> Option<Vec<RenderCard>> {
    nodes
        .iter()
        .map(|node| match node {
            RenderNode::Tab(tab) if tab.indent == 0 => Some(tab.card.clone()),
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
) {
    let mut pending_tabs = vec![];
    for node in nodes {
        match node {
            RenderNode::Tab(tab) => pending_tabs.push(tab.clone()),
            RenderNode::Group(group) => {
                append_tab_run(
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
                );
                pending_tabs.clear();
                append_group_header(lines, hit_regions, group, cols, theme, template_catalog);
                if !group.collapsed {
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
                    );
                }
            }
        }
    }
    append_tab_run(
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
    );
}

fn append_group_header(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    group: &RenderGroup,
    cols: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
) {
    let row = lines.len();
    let indent = group.indent.min(cols);
    let inner_width = cols.saturating_sub(indent);
    let mut line = " ".repeat(indent);
    line.push_str(&group_header_line(
        &group.metadata,
        group.collapsed,
        contains_active_tab(&group.children),
        active_tab_name(&group.children),
        group.templates.group_header.as_ref(),
        inner_width,
        theme,
        template_catalog,
    ));
    lines.push(line);
    hit_regions.push(HitRegion {
        row_start: row,
        row_end: row,
        col_start: indent,
        col_end: indent,
        tab_id: 0,
        tab_position: 0,
        group_path: Some(group.path.clone()),
        action: HitAction::ToggleGroup,
    });
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
    let inner_cols = cols.saturating_sub(indent);
    let cards = tabs.iter().map(|tab| tab.card.clone()).collect::<Vec<_>>();
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
    );
    let used_rows = local_lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map(|index| index + 1)
        .unwrap_or(0);
    let row_offset = lines.len();
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
    let visible_start = active_row
        .saturating_sub(available_rows / 2)
        .min(buffered_lines.len().saturating_sub(available_rows));
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
) {
    let visible = visible_cells(cards, available_rows, sizing);
    for (visible_index, (card_index, cell_height)) in visible.iter().copied().enumerate() {
        let card = &cards[card_index];
        let row = visible
            .iter()
            .take(visible_index)
            .map(|(_, height)| *height)
            .sum();
        lines[row] = title_border_line(
            &tab_title_with_template_catalog(card, template_catalog),
            cols,
            visible_index == 0,
            card.active,
            theme,
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
            );
            continue;
        }

        let first_in_run = visible_index == 0
            || visible
                .get(visible_index.saturating_sub(1))
                .map(|(previous_card_index, _)| *previous_card_index == active_index)
                .unwrap_or(false);
        lines[row] = title_border_line(
            &tab_title_with_template_catalog(card, template_catalog),
            cols,
            first_in_run,
            false,
            theme,
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
) -> usize {
    if row >= lines.len() || box_height < 2 {
        return row;
    }
    lines[row] = title_border_line(
        &tab_title_with_template_catalog(card, template_catalog),
        cols,
        true,
        card.active,
        theme,
    );
    let body_rows = box_height.saturating_sub(2);
    let body_lines = body_lines(
        card,
        row,
        body_rows,
        cols,
        terminal_cell_size,
        template_catalog,
    );
    for body_index in 0..body_rows {
        if let Some(line) = lines.get_mut(row + 1 + body_index) {
            *line = body_line(
                body_lines
                    .get(body_index)
                    .map(String::as_str)
                    .unwrap_or_default(),
                cols,
                card.active,
                theme,
            );
        }
    }
    if let Some(line) = lines.get_mut(row + box_height - 1) {
        *line = bottom_border_line(cols, card.active, theme);
    }
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
    let body_rows = cell_height.saturating_sub(1);
    let body_lines = body_lines(
        card,
        row,
        body_rows,
        cols,
        terminal_cell_size,
        template_catalog,
    );
    for body_index in 0..body_rows {
        if let Some(line) = lines.get_mut(row + 1 + body_index) {
            *line = body_line(
                body_lines
                    .get(body_index)
                    .map(String::as_str)
                    .unwrap_or_default(),
                cols,
                card.active,
                theme,
            );
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
        body_rows,
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
                    },
                    indent: 0,
                    grouping: None,
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
                RailRow::Tab { indent, .. } => model.tab_for_row(row).map(|tab| {
                    PendingRenderNode::Tab(RenderTab {
                        card: render_card_from_model(tab, &local_by_id),
                        indent: *indent,
                        grouping: tab.grouping.clone(),
                    })
                }),
            })
            .collect()
    };
    let mut nodes = pending_nodes_to_render_nodes(pending_rows, collapsed_groups);
    merge_resolved_metadata(&mut nodes, &model.resolved_metadata);
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
                );
                current_group = Some(CurrentGroupHeader {
                    path,
                    label,
                    full_label,
                    templates,
                });
            }
            PendingRenderNode::Tab(mut tab) if current_group.is_some() && tab.indent > 0 => {
                let group_header = current_group.as_ref().expect("checked above");
                tab.indent = group_header.path.0.len() * 2;
                let group = ensure_group_path(
                    &mut nodes,
                    &group_header.path,
                    &group_header.label,
                    &group_header.full_label,
                    &group_header.templates,
                    collapsed_groups,
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
    nodes
}

fn ensure_group_path<'a>(
    nodes: &'a mut Vec<RenderNode>,
    path: &GroupPath,
    leaf_label: &str,
    leaf_full_label: &str,
    templates: &ResolvedTemplateSlots,
    collapsed_groups: &[GroupPath],
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
    match sizing {
        RailSizingPreset::Compact => compact,
        RailSizingPreset::Large => ACTIVE_CELL_HEIGHT,
        RailSizingPreset::ActiveLarge if card.active => ACTIVE_CELL_HEIGHT,
        RailSizingPreset::PinnedLarge if card.active || card.pinned => ACTIVE_CELL_HEIGHT,
        RailSizingPreset::ActiveLarge | RailSizingPreset::PinnedLarge => compact,
    }
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

fn title_border_line(
    title: &str,
    width: usize,
    first_cell: bool,
    active: bool,
    theme: Option<RenderTheme>,
) -> String {
    let left = if first_cell { "┌" } else { "├" };
    let right = if first_cell { "┐" } else { "┤" };
    border_line(left, right, &format!(" {title} "), width, active, theme)
}

fn bottom_border_line(width: usize, active: bool, theme: Option<RenderTheme>) -> String {
    border_line("└", "┘", "", width, active, theme)
}

fn border_line(
    left: &str,
    right: &str,
    label: &str,
    width: usize,
    active: bool,
    theme: Option<RenderTheme>,
) -> String {
    match width {
        0 => String::new(),
        1 => style_border_text(left.to_owned(), active, theme),
        _ => {
            let inner_width = width - 2;
            let label = truncate_to_width(label, inner_width);
            let fill_width = inner_width.saturating_sub(label.width());
            format!(
                "{}{}{}{}",
                style_border_text(left.to_owned(), active, theme),
                style_title_text(label, active, theme),
                style_border_text("─".repeat(fill_width), active, theme),
                style_border_text(right.to_owned(), active, theme)
            )
        }
    }
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
) {
    if width == 0 {
        return;
    }
    let mut chars = vec![' '; width];
    chars[0] = '⚙';
    if width >= 2 {
        chars[width - 2] = '▲';
        chars[width - 1] = '▼';
    } else {
        chars[0] = '⚙';
    }
    lines[row] = style_body_text(chars.into_iter().collect(), theme);
    hit_regions.push(HitRegion {
        row_start: row,
        row_end: row,
        col_start: 0,
        col_end: 0,
        tab_id: 0,
        tab_position: 0,
        group_path: None,
        action: HitAction::OpenConfig,
    });
    if width >= 2 {
        hit_regions.push(HitRegion {
            row_start: row,
            row_end: row,
            col_start: width - 2,
            col_end: width - 2,
            tab_id: 0,
            tab_position: 0,
            group_path: None,
            action: HitAction::ScrollMetadataUp,
        });
        hit_regions.push(HitRegion {
            row_start: row,
            row_end: row,
            col_start: width - 1,
            col_end: width - 1,
            tab_id: 0,
            tab_position: 0,
            group_path: None,
            action: HitAction::ScrollMetadataDown,
        });
    }
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
) -> String {
    let fields = match resolved_slot {
        Some(slot) => group_header_fields_from_resolved_slot(slot, collapsed, active_tab_name),
        None => group_header_template_fields_with_template_catalog(
            metadata,
            collapsed,
            active_tab_name,
            template_catalog,
        ),
    };
    let label = render_template_fields(&fields, width);
    let remaining = width.saturating_sub(label.width());
    let text = if remaining >= 2 {
        format!("{label} {}", "─".repeat(remaining - 1))
    } else {
        label
    };
    style_group_header_text(
        pad_to_width(&truncate_to_width(&text, width), width),
        contains_active_tab,
        theme,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplateField {
    Required(String),
    Optional(String),
    Priority(String),
    Prioritized { value: String, priority: i64 },
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
    }));
    if collapsed {
        if let Some(active_tab_name) = active_tab_name {
            fields.push(TemplateField::Prioritized {
                value: format!(": {active_tab_name}"),
                priority: 80,
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
        })
        .collect()
}

fn render_template_fields(fields: &[TemplateField], width: usize) -> String {
    let full = join_template_fields(fields, true);
    if full.width() <= width {
        return full;
    }
    for threshold in droppable_template_field_priorities(fields) {
        let candidate = join_template_fields_above_priority(fields, threshold);
        if candidate.width() <= width {
            return candidate;
        }
    }
    truncate_to_width(&join_highest_priority_template_fields(fields), width)
}

fn join_template_fields(fields: &[TemplateField], include_optional: bool) -> String {
    join_template_fields_by(fields, |field| {
        include_optional || !matches!(field, TemplateField::Optional(_))
    })
}

fn join_template_fields_above_priority(fields: &[TemplateField], threshold: i64) -> String {
    join_template_fields_by(fields, |field| template_field_priority(field) > threshold)
}

fn join_highest_priority_template_fields(fields: &[TemplateField]) -> String {
    let Some(highest_priority) = fields.iter().map(template_field_priority).max() else {
        return String::new();
    };
    join_template_fields_by(fields, |field| {
        template_field_priority(field) == highest_priority
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

fn template_field_priority(field: &TemplateField) -> i64 {
    match field {
        TemplateField::Optional(_) => 0,
        TemplateField::Required(_) | TemplateField::Priority(_) => 100,
        TemplateField::Prioritized { priority, .. } => *priority,
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
    use tabs_shared::{
        GroupPath, GroupSegment, MetadataEntry, MetadataTarget, MetadataValue, PaneTarget,
        RailConfig, RailGroupingMode, RailRow, RailSizingPreset, RailStructure, RailViewMode,
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

    fn model() -> ControllerViewModel {
        ControllerViewModel {
            sort_mode: SortMode::PinnedFirst,
            config: RailConfig::default(),
            template_config: tabs_shared::TemplateConfigDiagnostics::default(),
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
                },
            ],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
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
        };
        ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig {
                grouping: RailGroupingMode::Directory,
                sizing: RailSizingPreset::Compact,
                ..RailConfig::default()
            },
            template_config: tabs_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![tab_one.clone(), tab_two.clone()],
            rows: vec![
                RailRow::GroupHeader {
                    group_id: "cwd:/Users/robert/dev/zellij".to_owned(),
                    path: group_path,
                    label: "zellij".to_owned(),
                    full_label: "/Users/robert/dev/zellij".to_owned(),
                    tab_count: 2,
                    templates: ResolvedTemplateSlots::default(),
                },
                RailRow::Tab {
                    tab_id: tab_one.tab_id,
                    indent: 2,
                },
                RailRow::Tab {
                    tab_id: tab_two.tab_id,
                    indent: 2,
                },
            ],
            resolved_metadata: vec![],
            observed_identities: vec![],
        }
    }

    fn nested_group_model() -> ControllerViewModel {
        let mut model = ControllerViewModel {
            sort_mode: SortMode::PinnedFirst,
            config: RailConfig {
                grouping: RailGroupingMode::Directory,
                structure: RailStructure::JoinedCells,
                sizing: RailSizingPreset::Compact,
                view: RailViewMode::Normal,
            },
            template_config: tabs_shared::TemplateConfigDiagnostics::default(),
            tabs: vec![],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
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
            };
            model.rows.push(RailRow::GroupHeader {
                group_id: format!("project-a/{worktree}"),
                path,
                label: worktree.to_owned(),
                full_label: format!("project-a/{worktree}"),
                tab_count: 1,
                templates: ResolvedTemplateSlots::default(),
            });
            let tab_id = tab.tab_id;
            model.tabs.push(tab);
            model.rows.push(RailRow::Tab { tab_id, indent: 2 });
        }
        model
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
                    },
                    indent: 4,
                    grouping: None,
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
        let rendered = render_lines_with_theme(
            Some(&grouped_model()),
            &[],
            8,
            24,
            true,
            Some(RenderTheme {
                active_border: PaletteColor::EightBit(2),
                inactive_border: PaletteColor::EightBit(8),
                body_foreground: PaletteColor::EightBit(7),
            }),
        );

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
            },
            TemplateField::Prioritized {
                value: "main".to_owned(),
                priority: 10,
            },
            TemplateField::Prioritized {
                value: ": tests".to_owned(),
                priority: 80,
            },
        ];

        assert_eq!(render_template_fields(&fields, 12), "repo: tests");
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

        append_group_header(&mut lines, &mut hit_regions, &group, 32, None, None);

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
        };

        assert_eq!(
            tab_title_template_fields(&card.metadata),
            vec![TemplateField::Required("metadata-agent".to_owned())]
        );
    }

    #[test]
    fn external_template_catalog_can_override_tab_title_rendering() {
        let config = tabs_shared::template_config::parse_template_config_json(
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
        let catalog = tabs_shared::template_config::TemplateConfigCatalog::from_config(config);

        let rendered =
            render_lines_with_template_catalog(Some(&model()), &[], 7, 24, true, Some(&catalog));

        assert!(rendered.lines[0].starts_with("┌ External"));
    }

    #[test]
    fn external_kdl_template_catalog_can_render_numeric_priority_fields() {
        let config = tabs_shared::template_config::parse_template_config_kdl(
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
        let catalog = tabs_shared::template_config::TemplateConfigCatalog::from_config(config);
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
                },
                TemplateField::Prioritized {
                    value: "rjwittams/zellij-scratch".to_owned(),
                    priority: 100,
                },
                TemplateField::Prioritized {
                    value: "main".to_owned(),
                    priority: 10,
                },
            ]
        );
    }

    #[test]
    fn external_kdl_group_header_template_uses_resolved_group_metadata() {
        let config = tabs_shared::template_config::parse_template_config_kdl(
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
        let catalog = tabs_shared::template_config::TemplateConfigCatalog::from_config(config);
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
    fn metadata_view_renders_group_and_tab_key_values() {
        let mut model = grouped_model();
        model.config.view = RailViewMode::Metadata;

        let rendered = render_lines(Some(&model), &[], 52, 64, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("group: zellij")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("zellij.pane.cwd: /Users/robert/dev/zellij")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("tab: tests")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("zellij.tab.active: true")));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.starts_with("┌ tests")));
    }

    #[test]
    fn metadata_view_renders_matched_template_names() {
        let mut model = grouped_model();
        model.config.view = RailViewMode::Metadata;
        model.tabs[1].status = Some(TabStatusSummary {
            priority: Priority::Waiting,
            title: "waiting".to_owned(),
            detail: Some("input".to_owned()),
            icon: None,
            source_pane: PaneTarget::Terminal(9),
        });

        let rendered = render_lines(Some(&model), &[], 80, 180, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("template.group_header: builtin.group-header")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("template.tab_title: builtin.tab-title")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("template.tab_status: builtin.tab-status.waiting")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("template.tab_status.sizing: auto")));
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("template.tab_status.specificity: 4")));
        assert!(rendered.lines.iter().any(|line| line.contains(
            "template.tab_status.predicates: exists(status.title), status.priority == waiting"
        )));
        assert!(rendered.lines.iter().any(|line| line.contains(
            "template.tab_status.candidates: builtin.tab-status(1), builtin.tab-status.waiting(4), builtin.tab-status.terminal-source(3)"
        )));
    }

    #[test]
    fn metadata_view_keeps_resolved_group_templates_after_child_rows() {
        let mut model = grouped_model();
        model.config.view = RailViewMode::Metadata;
        if let RailRow::GroupHeader { templates, .. } = &mut model.rows[0] {
            templates.group_header = Some(ResolvedTemplateSlot {
                template_name: "andamento.git.group-header".to_owned(),
                fields: vec![
                    tabs_shared::ResolvedTemplateField {
                        text: "zellij-org/zellij".to_owned(),
                        priority: 100,
                    },
                    tabs_shared::ResolvedTemplateField {
                        text: " feat/kitty-image-plumbing".to_owned(),
                        priority: 60,
                    },
                ],
            });
        }

        let rendered = render_lines(Some(&model), &[], 80, 180, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| { line.contains("template.group_header: andamento.git.group-header") }));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.contains("template.group_header: builtin.group-header")));
    }

    #[test]
    fn metadata_view_uses_local_tab_name_over_controller_name() {
        let mut model = model();
        model.config.view = RailViewMode::Metadata;

        let rendered = render_lines(
            Some(&model),
            &[LocalTab {
                tab_id: 2,
                position: 1,
                name: "local-title".to_owned(),
                active: true,
            }],
            10,
            40,
            true,
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("tab: local-title")));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.contains("tab: tab-2")));
    }

    #[test]
    fn metadata_view_renders_resolved_model_metadata_values() {
        let mut model = model();
        model.config.view = RailViewMode::Metadata;
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Tab(2),
            values: BTreeMap::from([(
                "tab.subject".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("checkout".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines(Some(&model), &[], 14, 48, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("tab.subject: checkout")));
    }

    #[test]
    fn metadata_view_renders_resolved_metadata_source_details() {
        let mut model = model();
        model.config.view = RailViewMode::Metadata;
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Tab(2),
            values: BTreeMap::from([(
                "tab.subject".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("checkout".to_owned()),
                    updated_at: 12,
                    ttl_ms: Some(100),
                    precedence: 4,
                    ordinal: 2,
                },
            )]),
            source_entries: BTreeMap::from([(
                "tab.subject".to_owned(),
                vec![tabs_shared::MetadataSourceEntry {
                    source_id: "flotilla".to_owned(),
                    entry: MetadataEntry {
                        value: MetadataValue::Text("checkout".to_owned()),
                        updated_at: 12,
                        ttl_ms: Some(100),
                        precedence: 4,
                        ordinal: 2,
                    },
                }],
            )]),
            reachable_identities: vec![],
        }];

        let rendered = render_lines(Some(&model), &[], 36, 120, true);
        let source_header_line = rendered
            .lines
            .iter()
            .find(|line| line.contains("key: tab.subject"))
            .expect("source key line");
        let table_header_line = rendered
            .lines
            .iter()
            .find(|line| line.contains("src") && line.contains("updated"))
            .expect("source table header");
        let source_value_line = rendered
            .lines
            .iter()
            .find(|line| line.contains("flotilla") && line.contains("checkout"))
            .expect("source value line");

        assert!(source_header_line.contains("tab.subject"));
        assert!(table_header_line.contains("ttl"));
        assert!(table_header_line.contains("prec"));
        assert!(table_header_line.contains("ord"));
        assert!(source_value_line.contains("12"));
        assert!(source_value_line.contains("100"));
        assert!(source_value_line.contains("4"));
        assert!(source_value_line.contains("2"));
        assert!(!rendered
            .lines
            .iter()
            .any(|line| line.contains("tab.subject.source.flotilla")));
    }

    #[test]
    fn metadata_view_renders_reachable_identity_details() {
        let mut model = model();
        model.config.view = RailViewMode::Metadata;
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Tab(2),
            values: BTreeMap::new(),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![tabs_shared::ReachableMetadataIdentity {
                identity: tabs_shared::MetadataIdentity {
                    key: "git.repo".to_owned(),
                    value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
                },
                distance: 1,
            }],
        }];

        let rendered = render_lines(Some(&model), &[], 12, 120, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("identity.git.repo: rjwittams/katzensteg distance=1")));
    }

    #[test]
    fn metadata_view_renders_observed_identity_index() {
        let mut model = model();
        model.config.view = RailViewMode::Metadata;
        model.observed_identities = vec![tabs_shared::ObservedMetadataIdentity {
            identity: tabs_shared::MetadataIdentity {
                key: "git.repo".to_owned(),
                value: MetadataValue::Text("rjwittams/katzensteg".to_owned()),
            },
            target_count: 2,
            nearest_distance: 1,
        }];

        let rendered = render_lines(Some(&model), &[], 12, 120, true);

        assert!(rendered.lines.iter().any(|line| line.contains(
            "observed_identity.git.repo: rjwittams/katzensteg target_count=2 nearest_distance=1"
        )));
    }

    #[test]
    fn metadata_view_renders_resolved_group_metadata_values() {
        let mut model = grouped_model();
        model.config.view = RailViewMode::Metadata;
        let group_path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
            label: None,
        }]);
        model.resolved_metadata = vec![ResolvedMetadata {
            target: MetadataTarget::Group(group_path),
            values: BTreeMap::from([(
                "group.summary".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("running tests".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines(Some(&model), &[], 14, 48, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("group.summary: running tests")));
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
            Some(RenderTheme {
                active_border: PaletteColor::EightBit(2),
                inactive_border: PaletteColor::EightBit(8),
                body_foreground: PaletteColor::EightBit(7),
            }),
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
            Some(RenderTheme {
                active_border: PaletteColor::EightBit(2),
                inactive_border: PaletteColor::EightBit(8),
                body_foreground: PaletteColor::EightBit(7),
            }),
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

        let rendered = render_lines_with_theme_and_cell_size(
            Some(&model),
            &[],
            9,
            24,
            true,
            None,
            0,
            Some(SizeInPixels {
                width: 9,
                height: 18,
            }),
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
            Some(RenderTheme {
                active_border: PaletteColor::EightBit(2),
                inactive_border: PaletteColor::EightBit(8),
                body_foreground: PaletteColor::EightBit(7),
            }),
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
            Some(RenderTheme {
                active_border: PaletteColor::EightBit(2),
                inactive_border: PaletteColor::EightBit(8),
                body_foreground: PaletteColor::EightBit(7),
            }),
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
            hit_at(&rendered.hit_regions, 8, 2).map(|hit| hit.action),
            None
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 8, 0).map(|hit| hit.action),
            Some(HitAction::OpenConfig)
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 8, 22).map(|hit| hit.action),
            Some(HitAction::ScrollMetadataUp)
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 8, 23).map(|hit| hit.action),
            Some(HitAction::ScrollMetadataDown)
        );
        assert!(rendered.lines[8].starts_with("⚙"));
        assert!(rendered.lines[8].ends_with("▲▼"));
    }

    #[test]
    fn metadata_view_scroll_offset_moves_visible_lines_and_clamps() {
        let mut model = grouped_model();
        model.config.view = RailViewMode::Metadata;

        let top =
            render_lines_with_theme_and_cell_size(Some(&model), &[], 5, 64, true, None, 0, None);
        let scrolled =
            render_lines_with_theme_and_cell_size(Some(&model), &[], 5, 64, true, None, 3, None);
        let overscrolled =
            render_lines_with_theme_and_cell_size(Some(&model), &[], 5, 64, true, None, 999, None);

        assert!(top.lines[0].contains("group: zellij"));
        assert!(scrolled.lines[0].contains("group.tab_count"));
        assert!(overscrolled.metadata_scroll_offset < 999);
    }
}
