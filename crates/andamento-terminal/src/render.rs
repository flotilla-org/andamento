use andamento_core::presentation::{Content, PlacementNode, PresentationState, SurfaceSnapshot};
use andamento_core::template_config::{
    TemplateConfigCatalog, TemplateConfigFieldClass, TemplateControlKind,
};
use andamento_core::{
    ControllerViewModel, MaterializeLatentRequest, MetadataControls, NodeKey, PlacementKey,
    Priority, StatusIcon,
};
use ansi_term::{Color, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
/// Terminal-owned colors; the host adapter translates its theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteColor {
    Rgb((u8, u8, u8)),
    EightBit(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeInPixels {
    pub height: usize,
    pub width: usize,
}

const DETAIL_PANEL_HEIGHT: usize = 4;

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
    ActivateEntity,
    Materialize,
    TogglePin,
    TogglePlacement,
    OpenConfig,
    ScrollRailUp,
    ScrollRailDown,
    InspectNode,
    ShowDetail,
    ToggleVariable(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitRegion {
    pub row_start: usize,
    pub row_end: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub tab_id: u64,
    pub tab_position: usize,
    pub inspect_target: Option<NodeKey>,
    pub materialize_request: Option<MaterializeLatentRequest>,
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
    /// A new shared absolute offset needed to reveal a newly active tab.
    pub ensure_visible_offset: Option<isize>,
    /// Whether an ensure-visible request found its renderable tab or group header.
    pub ensure_active_resolved: bool,
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

fn foreground_style(color: PaletteColor) -> Style {
    Style::new().fg(ansi_color(color))
}

fn ansi_color(color: PaletteColor) -> Color {
    match color {
        PaletteColor::Rgb((r, g, b)) => Color::RGB(r, g, b),
        PaletteColor::EightBit(color) => Color::Fixed(color),
    }
}

fn blank(cols: usize) -> String {
    " ".repeat(cols)
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

fn render_detail_card(
    content: &[String],
    height: usize,
    width: usize,
    theme: Option<RenderTheme>,
) -> Vec<String> {
    if height < 2 || width < 2 {
        let summary = content.first().map(String::as_str).unwrap_or_default();
        let line = pad_to_width(&truncate_to_width(summary, width), width);
        return (0..height)
            .map(|_| style_body_text(line.clone(), theme))
            .collect();
    }
    let inner_width = width - 2;
    let horizontal = "─".repeat(inner_width);
    let mut lines = Vec::with_capacity(height);
    lines.push(style_border_text(format!("┌{horizontal}┐"), false, theme));
    let content_rows = height.saturating_sub(2);
    for row in 0..content_rows {
        let mut text = content.get(row).cloned().unwrap_or_default();
        if row + 1 == content_rows && content.len() > content_rows && inner_width > 0 {
            text = format!("{}…", truncate_to_width(&text, inner_width - 1));
        }
        let text = pad_to_width(&truncate_to_width(&text, inner_width), inner_width);
        lines.push(format!(
            "{}{}{}",
            style_border_text("│".to_owned(), false, theme),
            style_body_text(text, theme),
            style_border_text("│".to_owned(), false, theme),
        ));
    }
    lines.push(style_border_text(format!("└{horizontal}┘"), false, theme));
    lines
}

pub fn status_icon_is_renderable(icon: &StatusIcon) -> bool {
    matches!(icon, StatusIcon::PngFile(_))
}

fn semantic_fields(content: &andamento_core::presentation::Content) -> Vec<TemplateField> {
    content
        .fields
        .iter()
        .cloned()
        .map(|field| match field.class {
            _ if field.priority.is_some() => TemplateField::Prioritized {
                value: field.value,
                priority: field.priority.unwrap_or(100),
            },
            TemplateConfigFieldClass::Required => TemplateField::Required(field.value),
            TemplateConfigFieldClass::Optional => TemplateField::Optional(field.value),
            TemplateConfigFieldClass::Priority => TemplateField::Priority(field.value),
        })
        .collect()
}

fn template_field_value(field: &TemplateField) -> &str {
    match field {
        TemplateField::Required(value)
        | TemplateField::Optional(value)
        | TemplateField::Priority(value) => value,
        TemplateField::Prioritized { value, .. } => value,
    }
}

enum TemplateField {
    Required(String),
    Optional(String),
    Priority(String),
    Prioritized { value: String, priority: i64 },
}
#[derive(Debug)]
struct PlacementRenderLine<'a> {
    text: String,
    entity: Option<&'a PlacementNode>,
    inline_hits: Vec<(&'a PlacementNode, std::ops::Range<usize>)>,
}

/// Render a placement tree while keeping each loop invocation as one layout
/// scope. The controller has already materialized the loop tree; the final
/// placement segment identifies sibling loop instances, and `placement_layout`
/// carries the declaration that selected their geometry.
fn render_placement_lines<'a>(
    root: String,
    entities: &'a [PlacementNode],
    cols: usize,
) -> Vec<PlacementRenderLine<'a>> {
    let mut lines = vec![PlacementRenderLine {
        text: root,
        entity: None,
        inline_hits: vec![],
    }];
    append_placement_loop_instance(&mut lines, 0, entities, 0, cols);
    lines
}

#[allow(clippy::too_many_arguments)]
fn append_placement_loop_instance<'a>(
    lines: &mut Vec<PlacementRenderLine<'a>>,
    parent_row: usize,
    entities: &'a [PlacementNode],
    depth: usize,
    cols: usize,
) {
    if entities.is_empty() {
        return;
    }
    // A template may declare several child loops. Their results are appended
    // in declaration order, so equal loop names form one contiguous instance.
    let mut start = 0;
    while start < entities.len() {
        let loop_key = &entities[start].loop_key;
        let mut end = start + 1;
        while end < entities.len() && &entities[end].loop_key == loop_key {
            end += 1;
        }
        let siblings = &entities[start..end];
        let item_texts = resolve_placement_loop_fields(siblings, cols);
        let layout = siblings[0].layout.as_deref();
        let inline = matches!(layout, Some("inline" | "row"));
        let indent = "  ".repeat(depth + 1);

        if inline {
            let run = item_texts.join("  ");
            let parent_width = lines[parent_row].text.width();
            let separator_width = usize::from(!run.is_empty() && parent_width > 0);
            if layout == Some("inline") && parent_width + separator_width + run.width() <= cols {
                let filler = cols.saturating_sub(parent_width + run.width() + 2);
                if filler > 0 {
                    lines[parent_row].text.push(' ');
                    lines[parent_row].text.push_str(&"─".repeat(filler));
                }
                if !run.is_empty() {
                    lines[parent_row].text.push(' ');
                    let mut column = lines[parent_row].text.width();
                    lines[parent_row].text.push_str(&run);
                    for (entity, text) in siblings.iter().zip(&item_texts) {
                        let end = column + text.width();
                        lines[parent_row].inline_hits.push((entity, column..end));
                        column = end + 2;
                    }
                }
                // Inline items share the parent's row. Nested loops are still
                // rendered, but each child instance makes its own fit decision.
                for entity in siblings {
                    if entity.collapsed {
                        continue;
                    }
                    append_placement_loop_instance(
                        lines,
                        parent_row,
                        &entity.children,
                        depth + 1,
                        cols,
                    );
                }
            } else {
                // Once the niche rejects the instance, every item moves. Wrap
                // complete items across dedicated rows; never use layout(),
                // whose width selection is intentionally allowed to suppress.
                let available = cols.saturating_sub(indent.width());
                let mut rows = Vec::new();
                let mut current = String::new();
                let mut current_hits = vec![];
                for (entity, item) in siblings.iter().zip(&item_texts) {
                    let item = truncate_to_width(item, available);
                    let separator = if current.is_empty() { "" } else { "  " };
                    if !current.is_empty()
                        && current.width() + separator.width() + item.width() > available
                    {
                        rows.push((
                            std::mem::take(&mut current),
                            std::mem::take(&mut current_hits),
                        ));
                    }
                    if !current.is_empty() {
                        current.push_str("  ");
                    }
                    let start = indent.width() + current.width();
                    current.push_str(&item);
                    current_hits.push((entity, start..start + item.width()));
                }
                if !current.is_empty() {
                    rows.push((current, current_hits));
                }
                let mut item_parent_rows = Vec::new();
                for (row, inline_hits) in rows {
                    let row_index = lines.len();
                    item_parent_rows
                        .extend(inline_hits.iter().map(|(entity, _)| (*entity, row_index)));
                    lines.push(PlacementRenderLine {
                        text: format!("{indent}{row}"),
                        entity: None,
                        inline_hits,
                    });
                }
                for entity in siblings {
                    if entity.collapsed {
                        continue;
                    }
                    let item_parent = item_parent_rows
                        .iter()
                        .find_map(|(candidate, row)| {
                            std::ptr::eq(*candidate, entity).then_some(*row)
                        })
                        .unwrap_or(parent_row);
                    append_placement_loop_instance(
                        lines,
                        item_parent,
                        &entity.children,
                        depth + 1,
                        cols,
                    );
                }
            }
        } else {
            for (entity, text) in siblings.iter().zip(item_texts) {
                let row = lines.len();
                lines.push(PlacementRenderLine {
                    text: format!("{indent}{text}"),
                    entity: Some(entity),
                    inline_hits: vec![],
                });
                if entity.collapsed {
                    continue;
                }
                append_placement_loop_instance(lines, row, &entity.children, depth + 1, cols);
            }
        }
        start = end;
    }
}

fn resolve_placement_loop_fields(entities: &[PlacementNode], cols: usize) -> Vec<String> {
    let rows = entities
        .iter()
        .map(|entity| {
            let mut fields = semantic_fields(&entity.content);
            if let Some((collapsed, expanded)) = entity.content.chrome.toggle() {
                let glyph = if entity.children.is_empty() {
                    " "
                } else if entity.collapsed {
                    collapsed
                } else {
                    expanded
                };
                fields.insert(0, TemplateField::Required(glyph.into()));
            }
            fields
        })
        .collect::<Vec<_>>();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or_default();
    let mut widths = (0..column_count)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|field| template_field_value(field).width())
                .max()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let mut visible = vec![true; column_count];
    let total_width = |visible: &[bool], widths: &[usize]| {
        let columns = visible.iter().filter(|visible| **visible).count();
        widths
            .iter()
            .zip(visible)
            .filter(|(_, visible)| **visible)
            .map(|(width, _)| *width)
            .sum::<usize>()
            .saturating_add(columns.saturating_sub(1))
    };

    if total_width(&visible, &widths) > cols {
        for column in 0..column_count {
            if rows
                .iter()
                .filter_map(|row| row.get(column))
                .all(|field| matches!(field, TemplateField::Optional(_)))
            {
                visible[column] = false;
            }
        }
    }
    let mut priorities = (0..column_count)
        .filter_map(|column| {
            let priority = rows
                .iter()
                .filter_map(|row| row.get(column))
                .filter_map(|field| match field {
                    TemplateField::Priority(_) => Some(100),
                    TemplateField::Prioritized { priority, .. } => Some(*priority),
                    _ => None,
                })
                .min()?;
            Some((priority, column))
        })
        .collect::<Vec<_>>();
    priorities.sort_unstable();
    for (_, column) in priorities {
        if total_width(&visible, &widths) <= cols {
            break;
        }
        visible[column] = false;
    }
    if total_width(&visible, &widths) > cols {
        if let Some(column) = (0..column_count).rev().find(|column| visible[*column]) {
            let overflow = total_width(&visible, &widths).saturating_sub(cols);
            widths[column] = widths[column].saturating_sub(overflow);
        }
    }

    rows.into_iter()
        .zip(entities)
        .map(|(row, entity)| {
            let selected = visible
                .iter()
                .enumerate()
                .filter(|(_, visible)| **visible)
                .map(|(column, _)| {
                    let value = row
                        .get(column)
                        .map(template_field_value)
                        .unwrap_or_default();
                    pad_to_width(&truncate_to_width(value, widths[column]), widths[column])
                })
                .collect::<Vec<_>>();
            let text = selected.join(" ").trim_end().to_owned();
            if let Some(error) = &entity.content.error {
                format!("{} [template error: {error}]", entity.label)
            } else if text.is_empty() && entity.content.template_name.is_none() {
                entity.label.clone()
            } else {
                text
            }
        })
        .collect()
}

fn node_hit(node: &PlacementNode, row: usize, cols: std::ops::Range<usize>) -> HitRegion {
    let (tab_id, action) = match node.state {
        PresentationState::Live { workspace_id, .. } => (workspace_id, HitAction::SwitchTab),
        _ => (0, HitAction::ActivateEntity),
    };
    HitRegion {
        row_start: row,
        row_end: row,
        col_start: cols.start,
        col_end: cols.end.saturating_sub(1),
        tab_id,
        tab_position: 0,
        inspect_target: Some(NodeKey::Placement(node.key.clone())),
        materialize_request: None,
        action,
    }
}

fn add_toggle_hit(hits: &mut Vec<HitRegion>, node: &PlacementNode, col: usize, end: usize) {
    if !node.children.is_empty() && node.content.chrome.toggle().is_some() && col < end {
        let mut hit = node_hit(node, 0, col..col + 1);
        hit.action = HitAction::TogglePlacement;
        hits.push(hit);
    }
}

fn control_line(
    content: &Content,
    surface: &SurfaceSnapshot,
    cols: usize,
    can_scroll: bool,
) -> (String, Vec<HitRegion>) {
    let mut cells = vec![' '; cols];
    let mut hits = Vec::new();
    let mut left = 0;
    let mut right = cols;
    for control in &content.controls {
        let (fallback, action, at_right) = match control.kind {
            TemplateControlKind::OpenConfig => ('⚙', HitAction::OpenConfig, false),
            TemplateControlKind::InspectRoot => ('◇', HitAction::InspectNode, true),
            TemplateControlKind::ScrollUp if can_scroll => ('▲', HitAction::ScrollRailUp, true),
            TemplateControlKind::ScrollDown if can_scroll => ('▼', HitAction::ScrollRailDown, true),
            TemplateControlKind::ScrollUp | TemplateControlKind::ScrollDown => continue,
            TemplateControlKind::DisplayVariable => {
                let Some(index) = surface
                    .display_variables
                    .iter()
                    .position(|v| Some(&v.name) == control.variable.as_ref())
                else {
                    continue;
                };
                let v = &surface.display_variables[index];
                let glyph = if surface.display_values.get(&v.name)
                    == Some(&andamento_core::DisplayVariableValue::Bool(false))
                {
                    '·'
                } else {
                    v.icon.chars().next().unwrap_or('·')
                };
                (glyph, HitAction::ToggleVariable(index), false)
            }
        };
        if left >= right {
            break;
        }
        let col = if at_right {
            right -= 1;
            right
        } else {
            let col = left;
            left += 1;
            col
        };
        cells[col] = control
            .glyph
            .as_deref()
            .and_then(|s| s.chars().next())
            .unwrap_or(fallback);
        hits.push(HitRegion {
            row_start: 0,
            row_end: 0,
            col_start: col,
            col_end: col,
            tab_id: 0,
            tab_position: 0,
            inspect_target: (action == HitAction::InspectNode).then_some(NodeKey::Root),
            materialize_request: None,
            action,
        });
    }
    (cells.into_iter().collect(), hits)
}

struct BufferedLine {
    text: String,
    hits: Vec<HitRegion>,
    active: bool,
    card: Option<VisibleCard>,
}

fn contains_selected(node: &PlacementNode) -> bool {
    matches!(node.state, PresentationState::Live { selected: true, .. })
        || node.children.iter().any(contains_selected)
}

fn show_metadata(model: &ControllerViewModel, node: &PlacementNode) -> bool {
    let controls = &model.metadata_controls;
    let mut inherited = controls.propagates_to_children(&NodeKey::Root, false);
    for depth in 1..node.key.0.len() {
        inherited = controls.propagates_to_children(
            &NodeKey::Placement(PlacementKey(node.key.0[..depth].to_vec())),
            inherited,
        );
    }
    let placement = NodeKey::Placement(node.key.clone());
    let target = if controls.per_node.contains_key(&placement) {
        placement
    } else {
        NodeKey::Entity(node.entity.clone())
    };
    controls.effective_show(&target, inherited)
}

fn buffer_section<'a>(
    model: &ControllerViewModel,
    section: &'a andamento_core::presentation::Section,
    cols: usize,
) -> Vec<BufferedLine> {
    let mut lines = render_placement_lines(section.content.text(), &section.nodes, cols);
    if lines
        .first()
        .is_some_and(|l| l.text.is_empty() && l.inline_hits.is_empty())
    {
        lines.remove(0);
    }
    let mut buffered = Vec::new();
    for line in lines {
        let nodes = line
            .entity
            .into_iter()
            .chain(line.inline_hits.iter().map(|(node, _)| *node))
            .collect::<Vec<_>>();
        let active = nodes.iter().any(|node| {
            matches!(node.state, PresentationState::Live { selected: true, .. })
                || (node.collapsed && contains_selected(node))
        });
        let mut hits = Vec::new();
        if let Some(node) = line.entity {
            hits.push(node_hit(node, 0, 0..cols));
            add_toggle_hit(
                &mut hits,
                node,
                line.text.chars().take_while(|c| *c == ' ').count(),
                cols,
            );
        }
        for (node, range) in line.inline_hits {
            if range.start < cols && range.end > range.start {
                hits.push(node_hit(node, 0, range.start..range.end.min(cols)));
                add_toggle_hit(&mut hits, node, range.start, range.end.min(cols));
            }
        }
        buffered.push(BufferedLine {
            text: line.text,
            hits,
            active,
            card: None,
        });
        for node in nodes {
            if let PresentationState::Live { workspace_id, .. } = node.state {
                if let Some(tab) = model.tab_by_id(workspace_id) {
                    let mut pin = node_hit(node, 0, cols.saturating_sub(1)..cols);
                    pin.action = HitAction::TogglePin;
                    let icon = tab.status.as_ref().and_then(|s| s.icon.clone());
                    let title = tab.status.as_ref().map(|s| s.title.as_str()).unwrap_or("");
                    let text = format!(
                        "{}{}",
                        pad_to_width(
                            &truncate_to_width(&format!("  {title}"), cols.saturating_sub(1)),
                            cols.saturating_sub(1)
                        ),
                        if tab.pinned { "◆" } else { "◇" }
                    );
                    buffered.push(BufferedLine {
                        text,
                        hits: vec![node_hit(node, 0, 0..cols), pin],
                        active: false,
                        card: Some(VisibleCard {
                            tab_id: tab.tab_id,
                            tab_position: tab.position,
                            row_start: 0,
                            status_row: Some(0),
                            status_icon_rect: icon.as_ref().filter(|_| cols > 2).map(|_| {
                                VisibleIconRect {
                                    x: 0,
                                    y: 0,
                                    columns: 1,
                                    rows: 1,
                                }
                            }),
                            status_priority: tab.status.as_ref().map(|s| s.priority),
                            status_icon: icon,
                        }),
                    });
                }
            }
            if show_metadata(model, node) {
                for (key, value) in &node.facts {
                    buffered.push(BufferedLine {
                        text: format!("    {key}: {value:?}"),
                        hits: vec![],
                        active: false,
                        card: None,
                    });
                }
            }
            if !node.content.controls.is_empty() {
                // Widgets are content, so they follow the entity they belong to.
                let surface = model.presentation.as_ref().unwrap();
                let (text, hits) = control_line(&node.content, surface, cols, false);
                buffered.push(BufferedLine {
                    text,
                    hits,
                    active: false,
                    card: None,
                });
            }
        }
    }
    buffered
}

#[allow(clippy::too_many_arguments)]
fn render_snapshot(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    theme: Option<RenderTheme>,
    offset: isize,
    ensure_active: bool,
    detail_target: Option<&NodeKey>,
    detail: bool,
) -> RenderedRail {
    let mut result = RenderedRail {
        lines: vec![blank(cols); rows],
        hit_regions: vec![],
        visible_cards: vec![],
        content_height: 0,
        available_rows: rows,
        ensure_visible_offset: None,
        ensure_active_resolved: false,
    };
    if cols == 0 || rows == 0 {
        return result;
    }
    let Some((model, surface)) =
        model.and_then(|model| model.presentation.as_ref().map(|surface| (model, surface)))
    else {
        for (row, tab) in tabs.iter().take(rows).enumerate() {
            result.lines[row] = pad_to_width(&truncate_to_width(&tab.name, cols), cols);
            result.hit_regions.push(HitRegion {
                row_start: row,
                row_end: row,
                col_start: 0,
                col_end: cols - 1,
                tab_id: tab.tab_id,
                tab_position: tab.position,
                inspect_target: Some(NodeKey::Tab(tab.tab_id)),
                materialize_request: None,
                action: HitAction::SwitchTab,
            });
        }
        return result;
    };
    let detail_rows = if detail {
        DETAIL_PANEL_HEIGHT.min(rows.saturating_sub(1))
    } else {
        0
    };
    // An empty workspace fallback renders nothing here: this frontend has no
    // workspace-creation affordance to anchor to its header.
    let mut sections = surface
        .sections
        .iter()
        .map(|section| {
            if section.is_empty_workspace_fallback() {
                Vec::new()
            } else {
                buffer_section(model, section, cols)
            }
        })
        .collect::<Vec<_>>();
    let height = |index: usize, lines: &[BufferedLine]| {
        lines.len() + usize::from(!surface.sections[index].content.controls.is_empty())
    };
    let fixed = sections
        .iter()
        .enumerate()
        .filter(|(i, _)| surface.sections[*i].pinned)
        .map(|(i, lines)| height(i, lines))
        .sum::<usize>();
    let available = rows.saturating_sub(fixed + detail_rows);
    result.available_rows = available;
    result.content_height = sections
        .iter()
        .enumerate()
        .filter(|(i, _)| !surface.sections[*i].pinned)
        .map(|(i, lines)| height(i, lines))
        .sum();
    let can_scroll = result.can_scroll();
    for (section, lines) in surface.sections.iter().zip(&mut sections) {
        if !section.content.controls.is_empty() {
            let (text, hits) = control_line(&section.content, surface, cols, can_scroll);
            lines.push(BufferedLine {
                text,
                hits,
                active: false,
                card: None,
            });
        }
    }
    let mut start = (offset.max(0) as usize).min(result.content_height.saturating_sub(available));
    if ensure_active && available > 0 {
        if let Some(index) = sections
            .iter()
            .enumerate()
            .filter(|(i, _)| !surface.sections[*i].pinned)
            .flat_map(|(_, lines)| lines)
            .position(|line| line.active)
        {
            result.ensure_active_resolved = true;
            let next = if index < start {
                index
            } else if index >= start + available {
                index + 1 - available
            } else {
                start
            };
            if next != start {
                result.ensure_visible_offset = Some(next as isize);
                start = next;
            }
        }
    }
    let suffix_start = surface
        .sections
        .iter()
        .rposition(|section| !section.pinned && !section.is_empty_workspace_fallback())
        .map(|i| i + 1)
        .unwrap_or(surface.sections.len());
    let suffix_rows = sections[suffix_start..]
        .iter()
        .map(Vec::len)
        .sum::<usize>()
        .min(rows.saturating_sub(detail_rows));
    let suffix_row = rows - suffix_rows;
    let body_end = suffix_row.saturating_sub(detail_rows);
    let mut row = 0;
    let mut ordinal = 0;
    for (index, lines) in sections.into_iter().enumerate() {
        if index == suffix_start {
            row = suffix_row;
        }
        for mut line in lines {
            if !surface.sections[index].pinned {
                let visible = ordinal >= start && ordinal < start + available;
                ordinal += 1;
                if !visible {
                    continue;
                }
            }
            let limit = if index >= suffix_start {
                rows
            } else {
                body_end
            };
            if row >= limit {
                continue;
            }
            let text = pad_to_width(&truncate_to_width(&line.text, cols), cols);
            result.lines[row] = if line.active {
                Style::new().bold().paint(text).to_string()
            } else {
                style_body_text(text, theme)
            };
            for hit in &mut line.hits {
                hit.row_start = row;
                hit.row_end = row;
                if let Some(tab) = model.tab_by_id(hit.tab_id) {
                    hit.tab_position = tab.position;
                }
            }
            if let Some(mut card) = line.card {
                card.row_start = row;
                card.status_row = Some(row);
                if let Some(rect) = &mut card.status_icon_rect {
                    rect.y = row;
                }
                result.visible_cards.push(card);
            }
            result.hit_regions.extend(line.hits);
            row += 1;
        }
    }
    if detail_rows > 0 {
        let node = match detail_target {
            Some(NodeKey::Placement(key)) => surface.node(key),
            _ => None,
        };
        let content = node
            .map(|node| {
                node.detail
                    .fields
                    .iter()
                    .map(|f| f.value.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (row, line) in render_detail_card(&content, detail_rows, cols, theme)
            .into_iter()
            .enumerate()
        {
            result.lines[body_end + row] = line;
        }
    }
    result
}
#[allow(clippy::too_many_arguments)]
pub fn render_lines(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
) -> RenderedRail {
    render_lines_with_theme(model, tabs, rows, cols, controller_available, None)
}
#[allow(clippy::too_many_arguments)]
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
#[allow(clippy::too_many_arguments)]
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
#[allow(clippy::too_many_arguments)]
pub fn render_lines_with_options(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_placements: &[PlacementKey],
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
        collapsed_placements,
        template_catalog,
        &MetadataControls::default(),
    )
}
#[allow(clippy::too_many_arguments)]
pub fn render_lines_with_metadata_controls(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_placements: &[PlacementKey],
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
        collapsed_placements,
        template_catalog,
        metadata_controls,
        0,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn render_lines_with_rail_scroll(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_placements: &[PlacementKey],
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    rail_scroll_offset: isize,
) -> RenderedRail {
    render_lines_with_rail_viewport(
        model,
        tabs,
        rows,
        cols,
        controller_available,
        theme,
        terminal_cell_size,
        collapsed_placements,
        template_catalog,
        metadata_controls,
        rail_scroll_offset,
        false,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn render_lines_with_detail_surface(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_placements: &[PlacementKey],
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    rail_scroll_offset: isize,
    ensure_active_visible: bool,
    detail_target: Option<&NodeKey>,
) -> RenderedRail {
    let _ = (
        controller_available,
        terminal_cell_size,
        collapsed_placements,
        template_catalog,
        metadata_controls,
    );
    render_snapshot(
        model,
        tabs,
        rows,
        cols,
        theme,
        rail_scroll_offset,
        ensure_active_visible,
        detail_target,
        true,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn render_lines_with_rail_viewport(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_placements: &[PlacementKey],
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    rail_scroll_offset: isize,
    ensure_active_visible: bool,
) -> RenderedRail {
    let _ = (
        controller_available,
        terminal_cell_size,
        collapsed_placements,
        template_catalog,
        metadata_controls,
    );
    render_snapshot(
        model,
        tabs,
        rows,
        cols,
        theme,
        rail_scroll_offset,
        ensure_active_visible,
        None,
        false,
    )
}
