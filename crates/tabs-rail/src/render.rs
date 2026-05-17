use std::collections::HashMap;

use ansi_term::{Color, Style};
use tabs_shared::{
    ControllerViewModel, GroupPath, MetadataValue, PaneTarget, Priority, RailConfig, RailRow,
    RailSizingPreset, RailStructure, RailViewMode, StatusIcon, TabCard, TabGroupingInfo,
    TabStatusSummary,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::{PaletteColor, SizeInPixels, Styling};

const ACTIVE_CELL_HEIGHT: usize = 5;
const COMPACT_CELL_HEIGHT: usize = 2;

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
    children: Vec<RenderTab>,
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
    },
    Tab(RenderTab),
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
    available_rows: usize,
    cols: usize,
    theme: Option<RenderTheme>,
    metadata_scroll_offset: usize,
) -> usize {
    let blocks = metadata_projection_blocks(nodes);
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
                blocks.extend(group.children.iter().map(tab_metadata_block));
            }
            RenderNode::Tab(tab) => blocks.push(tab_metadata_block(tab)),
        }
    }
    blocks
}

fn group_metadata_block(group: &RenderGroup) -> MetadataBlock {
    let mut lines = vec![];
    push_metadata_text_line(&mut lines, 0, "group", &group.label);
    push_metadata_text_line(&mut lines, 2, "group.full_label", &group.full_label);
    push_metadata_text_line(
        &mut lines,
        2,
        "group.tab_count",
        &group.tab_count.to_string(),
    );
    push_group_path_metadata(&mut lines, 2, &group.path);
    MetadataBlock { lines, hit: None }
}

fn tab_metadata_block(tab: &RenderTab) -> MetadataBlock {
    let mut lines = vec![];
    let indent = tab.indent;
    let card = &tab.card;
    push_metadata_text_line(&mut lines, indent, "tab", &card.name);
    push_metadata_text_line(
        &mut lines,
        indent + 2,
        "zellij.tab.id",
        &card.tab_id.to_string(),
    );
    push_metadata_text_line(
        &mut lines,
        indent + 2,
        "zellij.tab.position",
        &card.position.to_string(),
    );
    push_metadata_text_line(
        &mut lines,
        indent + 2,
        "zellij.tab.active",
        bool_text(card.active),
    );
    push_metadata_text_line(
        &mut lines,
        indent + 2,
        "rail.tab.pinned",
        bool_text(card.pinned),
    );
    if let Some(grouping) = tab.grouping.as_ref() {
        push_metadata_text_line(&mut lines, indent + 2, "group.label", &grouping.label);
        push_metadata_text_line(
            &mut lines,
            indent + 2,
            "group.full_label",
            &grouping.full_label,
        );
        push_group_path_metadata(&mut lines, indent + 2, &grouping.path);
    }
    if let Some(status) = card.status.as_ref() {
        push_metadata_text_line(
            &mut lines,
            indent + 2,
            "status.priority",
            &format!("{:?}", status.priority).to_ascii_lowercase(),
        );
        push_metadata_text_line(&mut lines, indent + 2, "status.title", &status.title);
        if let Some(detail) = status.detail.as_ref() {
            push_metadata_text_line(&mut lines, indent + 2, "status.detail", detail);
        }
        push_metadata_text_line(
            &mut lines,
            indent + 2,
            "status.source_pane",
            &format_pane_target(status.source_pane),
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

fn push_metadata_text_line(lines: &mut Vec<String>, indent: usize, key: &str, value: &str) {
    lines.push(format!("{}{}: {}", " ".repeat(indent), key, value));
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
                );
                pending_tabs.clear();
                append_group_header(lines, hit_regions, group, cols, theme);
                if !group.collapsed {
                    append_tab_run(
                        lines,
                        hit_regions,
                        visible_cards,
                        &group.children,
                        cols,
                        controller_available,
                        config,
                        theme,
                        terminal_cell_size,
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
    );
}

fn append_group_header(
    lines: &mut Vec<String>,
    hit_regions: &mut Vec<HitRegion>,
    group: &RenderGroup,
    cols: usize,
    theme: Option<RenderTheme>,
) {
    let row = lines.len();
    lines.push(group_header_line(
        &group.label,
        group.tab_count,
        group.collapsed,
        group.children.iter().any(|tab| tab.card.active),
        group
            .children
            .iter()
            .find(|tab| tab.card.active)
            .map(|tab| tab.card.name.as_str()),
        cols,
        theme,
    ));
    hit_regions.push(HitRegion {
        row_start: row,
        row_end: row,
        col_start: 0,
        col_end: 0,
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
        RenderNode::Group(group) => group
            .children
            .iter()
            .find(|tab| tab.card.active)
            .map(|tab| tab.card.tab_id),
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
            &tab_title(card),
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
            );
            continue;
        }

        let first_in_run = visible_index == 0
            || visible
                .get(visible_index.saturating_sub(1))
                .map(|(previous_card_index, _)| *previous_card_index == active_index)
                .unwrap_or(false);
        lines[row] = title_border_line(&tab_title(card), cols, first_in_run, false, theme);
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
) -> usize {
    if row >= lines.len() || box_height < 2 {
        return row;
    }
    lines[row] = title_border_line(&tab_title(card), cols, true, card.active, theme);
    let body_rows = box_height.saturating_sub(2);
    let body_lines = body_lines(card, row, body_rows, cols, terminal_cell_size);
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
) {
    let body_rows = cell_height.saturating_sub(1);
    let body_lines = body_lines(card, row, body_rows, cols, terminal_cell_size);
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
                RenderNode::Tab(RenderTab {
                    card: RenderCard {
                        tab_id: tab.tab_id,
                        position: tab.position,
                        name: tab.name,
                        active: tab.active,
                        pinned: false,
                        status: None,
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
            .map(|row| match row {
                RailRow::GroupHeader {
                    path,
                    label,
                    full_label,
                    tab_count,
                    ..
                } => PendingRenderNode::GroupHeader {
                    path: path.clone(),
                    label: label.clone(),
                    full_label: full_label.clone(),
                    tab_count: *tab_count,
                },
                RailRow::Tab { tab, indent } => PendingRenderNode::Tab(RenderTab {
                    card: render_card_from_model(tab, &local_by_id),
                    indent: *indent,
                    grouping: tab.grouping.clone(),
                }),
            })
            .collect()
    };
    pending_nodes_to_render_nodes(pending_rows, collapsed_groups)
}

fn pending_nodes_to_render_nodes(
    pending_rows: Vec<PendingRenderNode>,
    collapsed_groups: &[GroupPath],
) -> Vec<RenderNode> {
    let mut nodes = vec![];
    let mut pending_group: Option<RenderGroup> = None;
    for pending in pending_rows {
        match pending {
            PendingRenderNode::GroupHeader {
                path,
                label,
                full_label,
                tab_count,
            } => {
                if let Some(group) = pending_group.take() {
                    nodes.push(RenderNode::Group(group));
                }
                let collapsed = collapsed_groups.iter().any(|collapsed| collapsed == &path);
                pending_group = Some(RenderGroup {
                    path,
                    label,
                    full_label,
                    tab_count,
                    collapsed,
                    children: vec![],
                });
            }
            PendingRenderNode::Tab(tab) if pending_group.is_some() && tab.indent > 0 => {
                if let Some(group) = pending_group.as_mut() {
                    group.children.push(tab);
                }
            }
            PendingRenderNode::Tab(tab) => {
                if let Some(group) = pending_group.take() {
                    nodes.push(RenderNode::Group(group));
                }
                nodes.push(RenderNode::Tab(tab));
            }
        }
    }
    if let Some(group) = pending_group.take() {
        nodes.push(RenderNode::Group(group));
    }
    nodes
}

fn render_card_from_model(card: &TabCard, local_by_id: &HashMap<u64, &LocalTab>) -> RenderCard {
    let local = local_by_id.get(&card.tab_id).copied();
    RenderCard {
        tab_id: card.tab_id,
        position: local.map(|tab| tab.position).unwrap_or(card.position),
        name: local
            .map(|tab| tab.name.clone())
            .unwrap_or_else(|| card.name.clone()),
        active: local.map(|tab| tab.active).unwrap_or(card.active),
        pinned: card.pinned,
        status: card.status.clone(),
    }
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

fn format_status(status: &TabStatusSummary) -> String {
    match &status.detail {
        Some(detail) if !detail.is_empty() => format!("{}: {}", status.title, detail),
        _ => status.title.clone(),
    }
}

fn tab_title(card: &RenderCard) -> String {
    if card.name.is_empty() {
        format!("Tab {}", card.position + 1)
    } else {
        card.name.clone()
    }
}

fn body_lines(
    card: &RenderCard,
    row: usize,
    body_rows: usize,
    cols: usize,
    terminal_cell_size: Option<SizeInPixels>,
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
    let text_lines = wrap_to_width(&format_status(status), text_width, body_rows);
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
    label: &str,
    tab_count: usize,
    collapsed: bool,
    contains_active_tab: bool,
    active_tab_name: Option<&str>,
    width: usize,
    theme: Option<RenderTheme>,
) -> String {
    let fields = group_header_template_fields(label, tab_count, collapsed, active_tab_name);
    let label = render_group_header_fields(&fields, width);
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
enum GroupHeaderField {
    Required(String),
    Optional(String),
    Priority(String),
}

fn group_header_template_fields(
    label: &str,
    tab_count: usize,
    collapsed: bool,
    active_tab_name: Option<&str>,
) -> Vec<GroupHeaderField> {
    let mut fields = vec![
        GroupHeaderField::Required(if collapsed { "▶" } else { "▼" }.to_owned()),
        GroupHeaderField::Required(label.to_owned()),
        GroupHeaderField::Optional(format!("({tab_count})")),
    ];
    if let Some(active_tab_name) = collapsed.then_some(active_tab_name).flatten() {
        fields.push(GroupHeaderField::Priority(format!(": {active_tab_name}")));
    }
    fields
}

fn render_group_header_fields(fields: &[GroupHeaderField], width: usize) -> String {
    let full = join_group_header_fields(fields, true);
    if full.width() <= width {
        return full;
    }
    let without_optional = join_group_header_fields(fields, false);
    if without_optional.width() <= width {
        return without_optional;
    }
    truncate_to_width(&without_optional, width)
}

fn join_group_header_fields(fields: &[GroupHeaderField], include_optional: bool) -> String {
    let mut output = String::new();
    for field in fields {
        let value = match field {
            GroupHeaderField::Required(value) | GroupHeaderField::Priority(value) => value,
            GroupHeaderField::Optional(value) if include_optional => value,
            GroupHeaderField::Optional(_) => continue,
        };
        if output.is_empty() || value.starts_with(':') {
            output.push_str(value);
        } else {
            output.push(' ');
            output.push_str(value);
        }
    }
    output
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
        GroupPath, GroupSegment, MetadataValue, PaneTarget, RailConfig, RailGroupingMode, RailRow,
        RailSizingPreset, RailStructure, RailViewMode, SortMode, StatusIcon,
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
                },
                TabCard {
                    tab_id: 1,
                    position: 0,
                    name: "tab-1".to_owned(),
                    active: false,
                    pinned: false,
                    status: None,
                    grouping: None,
                },
            ],
            rows: vec![],
        }
    }

    fn grouped_model() -> ControllerViewModel {
        let group_path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
        }]);
        let tab_one = TabCard {
            tab_id: 1,
            position: 0,
            name: "server".to_owned(),
            active: false,
            pinned: false,
            status: None,
            grouping: None,
        };
        let tab_two = TabCard {
            tab_id: 2,
            position: 1,
            name: "tests".to_owned(),
            active: true,
            pinned: false,
            status: None,
            grouping: None,
        };
        ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig {
                grouping: RailGroupingMode::Directory,
                sizing: RailSizingPreset::Compact,
                ..RailConfig::default()
            },
            tabs: vec![tab_one.clone(), tab_two.clone()],
            rows: vec![
                RailRow::GroupHeader {
                    group_id: "cwd:/Users/robert/dev/zellij".to_owned(),
                    path: group_path,
                    label: "zellij".to_owned(),
                    full_label: "/Users/robert/dev/zellij".to_owned(),
                    tab_count: 2,
                },
                RailRow::Tab {
                    tab: tab_one,
                    indent: 2,
                },
                RailRow::Tab {
                    tab: tab_two,
                    indent: 2,
                },
            ],
        }
    }

    fn flat_rows_model() -> ControllerViewModel {
        let mut model = model();
        model.config.structure = RailStructure::JoinedCells;
        model.config.sizing = RailSizingPreset::Compact;
        model.rows = model
            .tabs
            .iter()
            .cloned()
            .map(|tab| RailRow::Tab { tab, indent: 0 })
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
        let fields = group_header_template_fields("zellij", 2, true, Some("tests"));

        assert_eq!(
            fields,
            vec![
                GroupHeaderField::Required("▶".to_owned()),
                GroupHeaderField::Required("zellij".to_owned()),
                GroupHeaderField::Optional("(2)".to_owned()),
                GroupHeaderField::Priority(": tests".to_owned()),
            ]
        );
    }

    #[test]
    fn metadata_view_renders_group_and_tab_key_values() {
        let mut model = grouped_model();
        model.config.view = RailViewMode::Metadata;

        let rendered = render_lines(Some(&model), &[], 18, 64, true);

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
            render_lines_with_theme_and_cell_size(Some(&model), &[], 5, 64, true, None, 2, None);
        let overscrolled =
            render_lines_with_theme_and_cell_size(Some(&model), &[], 5, 64, true, None, 999, None);

        assert!(top.lines[0].contains("group: zellij"));
        assert!(scrolled.lines[0].contains("group.tab_count"));
        assert!(overscrolled.metadata_scroll_offset < 999);
    }
}
