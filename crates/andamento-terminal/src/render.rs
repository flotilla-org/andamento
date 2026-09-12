use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use crate::inline_layout::{InlineHit, InlineItem, InlineRun};
use crate::segment_bar::{self, SegmentItem};
use andamento_core::template_config::{
    ChromeSpec, SurfaceRegionDefinition, SurfaceRegionSource, TemplateConfigCatalog,
    TemplateConfigFieldClass, TemplateConfigMatchContext, TemplateConfigNodeKind,
    TemplateConfigRenderReady, TemplateConfigSlot,
};
use andamento_core::{
    ControllerViewModel, DisplayRegion, GroupPath, GroupSegment, LatentMaterializationState,
    LatentTab, MaterializeLatentRequest, MetadataControls, MetadataEntry, MetadataSourceEntry,
    MetadataValue, NodeKey, PaneTarget, Priority, RailConfig, RailRgbColor, RailRow, RailStructure,
    ReachableMetadataIdentity, ResolvedMetadata, ResolvedMetadataTarget,
    ResolvedTemplateFieldSource, ResolvedTemplateSlot, ResolvedTemplateSlots, StatusIcon, TabCard,
    TabGroupingInfo, TabStatusSummary, DISPLAY_FORM_COMPACT,
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

const ACTIVE_CELL_HEIGHT: usize = 5;
const COMPACT_CELL_HEIGHT: usize = 2;
const CHILD_LAYOUT_VARIABLE_KEY: &str = "var.child-layout";

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
    ActivateEntity,
    Materialize,
    TogglePin,
    ToggleGroup,
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
    pub group_path: Option<GroupPath>,
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
    latent: bool,
    materialize_request: Option<MaterializeLatentRequest>,
    latent_summary: Option<String>,
    /// Metadata-panel content rendered inside the card body when set.
    /// Populated by `append_tab_run`/`render_nodes` based on
    /// `MetadataControls`; the card grows by `meta_panel.len()` rows.
    meta_panel: Option<Vec<String>>,
    entity: Option<andamento_core::EntityRef>,
    compact_only: bool,
    form: String,
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
    child_layout: ChildLayoutMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildLayoutMode {
    Cards,
    Strip,
}

impl Default for InheritedRailSettings {
    fn default() -> Self {
        Self {
            child_layout: ChildLayoutMode::Cards,
        }
    }
}

impl InheritedRailSettings {
    fn with_node_metadata(self, metadata: &RenderMetadata) -> Self {
        Self {
            child_layout: child_layout_from_variables(metadata).unwrap_or(self.child_layout),
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
    render_lines_with_rail_viewport(
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
        rail_scroll_offset,
        false,
    )
}

pub fn render_lines_with_rail_viewport(
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
    ensure_active_visible: bool,
) -> RenderedRail {
    let mut configured_regions = model
        .map(|model| model.surface_regions.clone())
        .filter(|regions| !regions.is_empty())
        .or_else(|| {
            template_catalog.map(|catalog| {
                catalog
                    .regions()
                    .iter()
                    .cloned()
                    .map(|definition| DisplayRegion {
                        root: resolve_region_root(&definition, catalog),
                        definition,
                        entities: vec![],
                    })
                    .collect()
            })
        })
        .unwrap_or_default();
    if let Some(catalog) = template_catalog {
        for region in &mut configured_regions {
            if region.root.is_none() {
                region.root = resolve_region_root(&region.definition, catalog);
            }
        }
    }
    if !configured_regions.is_empty() {
        return render_region_stack(
            model,
            tabs,
            rows,
            cols,
            controller_available,
            theme,
            terminal_cell_size,
            collapsed_groups,
            &configured_regions,
            template_catalog,
            metadata_controls,
            rail_scroll_offset,
            ensure_active_visible,
        );
    }
    render_legacy_rail_viewport(
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
        rail_scroll_offset,
        ensure_active_visible,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_legacy_rail_viewport(
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
    ensure_active_visible: bool,
    form_policy: Option<&SurfaceRegionDefinition>,
) -> RenderedRail {
    if rows == 0 || cols == 0 {
        return RenderedRail {
            lines: vec![blank(cols); rows],
            hit_regions: vec![],
            visible_cards: vec![],
            content_height: 0,
            available_rows: rows.saturating_sub(1),
            ensure_visible_offset: None,
            ensure_active_resolved: false,
        };
    }

    let footer_rows = 1.min(rows);
    let card_rows_available = rows.saturating_sub(footer_rows);
    let mut lines = vec![blank(cols); rows];
    let mut hit_regions = vec![];
    let mut visible_cards = vec![];
    let nodes = nodes_to_render(model, tabs, collapsed_groups, form_policy);
    let config = model.map(|model| model.config).unwrap_or_default();
    let theme = theme.map(|theme| theme.with_config(config));
    let inspected_node = model.and_then(|model| model.inspected_node.as_ref());
    let root_settings = root_inherited_settings_for_model(model);
    let mut content_height = 0;
    let mut viewport = Viewport::default();
    if nodes.is_empty() {
        lines[0] = pad_to_width("tabs: waiting for tab state", cols);
    } else {
        let rendered_nodes = render_nodes(
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
            ensure_active_visible,
        );
        content_height = rendered_nodes.content_height;
        viewport = rendered_nodes.viewport;
    }

    let rail_can_scroll = content_height > card_rows_available;
    if controller_available {
        let catalog = match template_catalog {
            Some(catalog) => catalog,
            None => bundled_template_catalog(),
        };
        let controls = catalog
            .regions()
            .iter()
            .find(|region| region.source == SurfaceRegionSource::Controls)
            .and_then(|region| resolve_region_root(region, catalog))
            .and_then(|root| root.render_ready)
            .map(|ready| ready.controls)
            .unwrap_or_default();
        render_control_template(
            &mut lines,
            &mut hit_regions,
            rows - 1,
            cols,
            theme,
            inspected_node,
            rail_can_scroll,
            &controls,
            model
                .map(|model| model.display_variables.as_slice())
                .unwrap_or_default(),
            model.map(|model| &model.display_variable_values),
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
        ensure_visible_offset: viewport.ensure_visible_offset,
        ensure_active_resolved: viewport.ensure_active_resolved,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_region_stack(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    rows: usize,
    cols: usize,
    controller_available: bool,
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    collapsed_groups: &[GroupPath],
    regions: &[DisplayRegion],
    catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    rail_scroll_offset: isize,
    ensure_active_visible: bool,
) -> RenderedRail {
    if rows == 0 || cols == 0 {
        return RenderedRail {
            lines: vec![blank(cols); rows],
            hit_regions: vec![],
            visible_cards: vec![],
            content_height: 0,
            available_rows: rows,
            ensure_visible_offset: None,
            ensure_active_resolved: false,
        };
    }

    let mut lines = Vec::with_capacity(rows);
    let mut hit_regions = vec![];
    let mut visible_cards = vec![];
    let mut content_height = 0;
    let mut ensure_visible_offset = None;
    let mut ensure_active_resolved = false;
    let mut region_can_scroll = false;
    let mut controls_footer_rows = vec![];

    for (region_index, display_region) in regions.iter().enumerate() {
        let region = &display_region.definition;
        if lines.len() >= rows {
            break;
        }
        let remaining = rows - lines.len();
        let later_pinned_rows = regions[region_index + 1..]
            .iter()
            .filter(|region| region.definition.pinned)
            .map(|region| pinned_region_rows(region, model))
            .sum::<usize>();
        match region.source {
            SurfaceRegionSource::Header => {
                lines.push(region_root_line(display_region, None, cols, theme, model));
                content_height += 1;
            }
            SurfaceRegionSource::Attention => {
                let key = region
                    .attention_key
                    .as_deref()
                    .unwrap_or("status.attention");
                // Catalog-derived view models carry every presence class here.
                // The row fallback keeps hand-built/legacy models renderable,
                // but can only see inline entities.
                let attention_entities = if display_region.entities.is_empty() {
                    model
                        .into_iter()
                        .flat_map(|model| model.rows.iter())
                        .filter_map(|row| match row {
                            RailRow::Entity { entity, .. } => Some(entity),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                } else {
                    display_region.entities.iter().collect()
                };
                let attention_entities = attention_entities
                    .into_iter()
                    .filter(|entity| {
                        region.placement.is_some()
                            || entity.metadata.get(key) == Some(&MetadataValue::Bool(true))
                    })
                    .collect::<Vec<_>>();
                if region.placement.is_some() && !display_region.entities.is_empty() {
                    let available = remaining.saturating_sub(later_pinned_rows);
                    let rendered = render_placement_lines(
                        region_root_line(display_region, None, cols, None, model)
                            .trim_end()
                            .to_owned(),
                        &display_region.entities,
                        region,
                        catalog,
                        cols,
                        model,
                    );
                    content_height += rendered.len();
                    for rendered_line in rendered.into_iter().take(available) {
                        let row = lines.len();
                        lines.push(style_body_text(
                            pad_to_width(&truncate_to_width(&rendered_line.text, cols), cols),
                            theme,
                        ));
                        if let Some(entity) = rendered_line.entity {
                            hit_regions.push(placement_hit_region(entity, row, cols));
                        }
                        for (entity, hit_cols) in rendered_line.inline_hits {
                            hit_regions
                                .push(placement_inline_hit_region(entity, row, hit_cols, cols));
                        }
                    }
                    continue;
                }
                let attention_entities = attention_entities
                    .into_iter()
                    .flat_map(|entity| {
                        placement_entity_tree(
                            entity,
                            0,
                            model
                                .map(|model| model.collapsed_placements.as_slice())
                                .unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>();
                let available_entity_rows =
                    rows.saturating_sub(lines.len() + 1 + later_pinned_rows);
                let hidden_count = attention_entities
                    .len()
                    .saturating_sub(available_entity_rows);
                let suffix = (hidden_count > 0).then(|| format!(" (+{hidden_count} more)"));
                lines.push(region_root_line(
                    display_region,
                    suffix.as_deref(),
                    cols,
                    theme,
                    model,
                ));
                content_height += 1;
                for (entity, depth) in attention_entities {
                    if lines.len() + later_pinned_rows < rows {
                        content_height += 1;
                        let row = lines.len();
                        let mut line =
                            region_entity_line(entity, region, catalog, cols, theme, model);
                        if depth > 0 {
                            let prefix = "  ".repeat(depth);
                            line = format!("{prefix}{line}");
                            line = truncate_to_width(&line, cols);
                            line = pad_to_width(&line, cols);
                        }
                        lines.push(line);
                        hit_regions.push(HitRegion {
                            row_start: row,
                            row_end: row,
                            col_start: 0,
                            col_end: cols.saturating_sub(1),
                            tab_id: 0,
                            tab_position: 0,
                            group_path: None,
                            inspect_target: entity
                                .placement
                                .clone()
                                .map(NodeKey::Placement)
                                .or_else(|| Some(NodeKey::Entity(entity.entity.clone()))),
                            materialize_request: None,
                            action: if entity.children.is_empty() {
                                HitAction::ActivateEntity
                            } else {
                                HitAction::TogglePlacement
                            },
                        });
                    }
                }
            }
            SurfaceRegionSource::Tree => {
                lines.push(region_root_line(display_region, None, cols, theme, model));
                content_height += 1;
                if remaining <= 1 {
                    continue;
                }
                if region.placement.is_some() {
                    let available = remaining.saturating_sub(1 + later_pinned_rows);
                    lines.pop();
                    content_height = content_height.saturating_sub(1);
                    let rendered = render_placement_lines(
                        region_root_line(display_region, None, cols, None, model)
                            .trim_end()
                            .to_owned(),
                        &display_region.entities,
                        region,
                        catalog,
                        cols,
                        model,
                    );
                    content_height += rendered.len();
                    for rendered_line in rendered.into_iter().take(available + 1) {
                        let row = lines.len();
                        lines.push(style_body_text(
                            pad_to_width(&truncate_to_width(&rendered_line.text, cols), cols),
                            theme,
                        ));
                        if let Some(entity) = rendered_line.entity {
                            hit_regions.push(placement_hit_region(entity, row, cols));
                        }
                        for (entity, hit_cols) in rendered_line.inline_hits {
                            hit_regions
                                .push(placement_inline_hit_region(entity, row, hit_cols, cols));
                        }
                    }
                    continue;
                }
                let mut region_model = model.cloned();
                if let Some(model) = &mut region_model {
                    apply_region_form(&mut model.rows, region, catalog);
                }
                let tree_rows = remaining.saturating_sub(1 + later_pinned_rows);
                if tree_rows == 0 {
                    continue;
                }
                let mut rendered = render_legacy_rail_viewport(
                    region_model.as_ref(),
                    tabs,
                    tree_rows + 1,
                    cols,
                    controller_available,
                    theme,
                    terminal_cell_size,
                    collapsed_groups,
                    catalog,
                    metadata_controls,
                    rail_scroll_offset,
                    ensure_active_visible,
                    Some(region),
                );
                region_can_scroll |= rendered.can_scroll();
                rendered.lines.truncate(tree_rows);
                rendered.hit_regions.retain(|hit| hit.row_start < tree_rows);
                rendered
                    .visible_cards
                    .retain(|card| card.row_start < tree_rows);
                let row_offset = lines.len();
                for hit in &mut rendered.hit_regions {
                    hit.row_start += row_offset;
                    hit.row_end += row_offset;
                }
                for card in &mut rendered.visible_cards {
                    card.row_start += row_offset;
                    card.status_row = card.status_row.map(|row| row + row_offset);
                }
                lines.extend(rendered.lines);
                hit_regions.extend(rendered.hit_regions);
                visible_cards.extend(rendered.visible_cards);
                content_height += rendered.content_height;
                ensure_visible_offset = rendered.ensure_visible_offset;
                ensure_active_resolved = rendered.ensure_active_resolved;
            }
            SurfaceRegionSource::Controls => {
                controls_footer_rows.push((lines.len(), display_region));
                lines.push(blank(cols));
                content_height += 1;
            }
        }
    }
    for (footer_row, controls_region) in controls_footer_rows {
        let mut footer = vec![blank(cols)];
        let mut footer_hits = vec![];
        render_control_template(
            &mut footer,
            &mut footer_hits,
            0,
            cols,
            theme,
            model.and_then(|model| model.inspected_node.as_ref()),
            region_can_scroll,
            controls_region
                .root
                .as_ref()
                .and_then(|root| root.render_ready.as_ref())
                .map(|ready| ready.controls.as_slice())
                .unwrap_or_default(),
            model
                .map(|model| model.display_variables.as_slice())
                .unwrap_or_default(),
            model.map(|model| &model.display_variable_values),
        );
        lines[footer_row] = footer.remove(0);
        for hit in &mut footer_hits {
            hit.row_start += footer_row;
            hit.row_end += footer_row;
        }
        hit_regions.extend(footer_hits);
    }
    lines.resize_with(rows, || blank(cols));
    RenderedRail {
        lines,
        hit_regions,
        visible_cards,
        content_height,
        available_rows: rows,
        ensure_visible_offset,
        ensure_active_resolved,
    }
}

fn placement_entity_tree<'a>(
    entity: &'a andamento_core::DisplayEntity,
    depth: usize,
    collapsed: &[andamento_core::PlacementKey],
) -> Vec<(&'a andamento_core::DisplayEntity, usize)> {
    let mut result = vec![(entity, depth)];
    if entity
        .placement
        .as_ref()
        .is_some_and(|key| collapsed.contains(key))
    {
        return result;
    }
    for child in &entity.children {
        result.extend(placement_entity_tree(child, depth + 1, collapsed));
    }
    result
}

#[derive(Debug)]
struct PlacementRenderLine<'a> {
    text: String,
    entity: Option<&'a andamento_core::DisplayEntity>,
    inline_hits: Vec<(&'a andamento_core::DisplayEntity, std::ops::Range<usize>)>,
}

/// Render a placement tree while keeping each loop invocation as one layout
/// scope. The controller has already materialized the loop tree; the final
/// placement segment identifies sibling loop instances, and `placement_layout`
/// carries the declaration that selected their geometry.
fn render_placement_lines<'a>(
    root: String,
    entities: &'a [andamento_core::DisplayEntity],
    region: &SurfaceRegionDefinition,
    catalog: Option<&TemplateConfigCatalog>,
    cols: usize,
    model: Option<&ControllerViewModel>,
) -> Vec<PlacementRenderLine<'a>> {
    let mut lines = vec![PlacementRenderLine {
        text: root,
        entity: None,
        inline_hits: vec![],
    }];
    append_placement_loop_instance(&mut lines, 0, entities, 0, region, catalog, cols, model);
    lines
}

#[allow(clippy::too_many_arguments)]
fn append_placement_loop_instance<'a>(
    lines: &mut Vec<PlacementRenderLine<'a>>,
    parent_row: usize,
    entities: &'a [andamento_core::DisplayEntity],
    depth: usize,
    region: &SurfaceRegionDefinition,
    catalog: Option<&TemplateConfigCatalog>,
    cols: usize,
    model: Option<&ControllerViewModel>,
) {
    if entities.is_empty() {
        return;
    }
    // A template may declare several child loops. Their results are appended
    // in declaration order, so equal loop names form one contiguous instance.
    let mut start = 0;
    while start < entities.len() {
        let loop_key = entities[start]
            .placement
            .as_ref()
            .and_then(|key| key.loop_key(&region.name));
        let mut end = start + 1;
        while end < entities.len()
            && loop_key.is_some()
            && entities[end]
                .placement
                .as_ref()
                .and_then(|key| key.loop_key(&region.name))
                == loop_key
        {
            end += 1;
        }
        let siblings = &entities[start..end];
        let item_texts = resolve_placement_loop_fields(siblings, region, catalog, cols, model);
        let inline = siblings[0].placement_layout.as_deref() == Some("inline");
        let indent = "  ".repeat(depth + 1);

        if inline {
            let run = item_texts.join("  ");
            let parent_width = lines[parent_row].text.width();
            let separator_width = usize::from(!run.is_empty() && parent_width > 0);
            if parent_width + separator_width + run.width() <= cols {
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
                    if placement_is_collapsed(entity, model) {
                        continue;
                    }
                    append_placement_loop_instance(
                        lines,
                        parent_row,
                        &entity.children,
                        depth + 1,
                        region,
                        catalog,
                        cols,
                        model,
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
                    if placement_is_collapsed(entity, model) {
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
                        region,
                        catalog,
                        cols,
                        model,
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
                if placement_is_collapsed(entity, model) {
                    continue;
                }
                append_placement_loop_instance(
                    lines,
                    row,
                    &entity.children,
                    depth + 1,
                    region,
                    catalog,
                    cols,
                    model,
                );
            }
        }
        start = end;
    }
}

fn placement_is_collapsed(
    entity: &andamento_core::DisplayEntity,
    model: Option<&ControllerViewModel>,
) -> bool {
    entity.placement.as_ref().is_some_and(|placement| {
        model.is_some_and(|model| model.collapsed_placements.contains(placement))
    })
}

fn placement_hit_region(
    entity: &andamento_core::DisplayEntity,
    row: usize,
    cols: usize,
) -> HitRegion {
    HitRegion {
        row_start: row,
        row_end: row,
        col_start: 0,
        col_end: cols.saturating_sub(1),
        tab_id: 0,
        tab_position: 0,
        group_path: None,
        inspect_target: entity
            .placement
            .clone()
            .map(NodeKey::Placement)
            .or_else(|| Some(NodeKey::Entity(entity.entity.clone()))),
        materialize_request: None,
        action: if entity.children.is_empty() {
            HitAction::ActivateEntity
        } else {
            HitAction::TogglePlacement
        },
    }
}

fn placement_inline_hit_region(
    entity: &andamento_core::DisplayEntity,
    row: usize,
    cols: std::ops::Range<usize>,
    width: usize,
) -> HitRegion {
    let mut hit = placement_hit_region(entity, row, width);
    hit.col_start = cols.start.min(width.saturating_sub(1));
    hit.col_end = cols.end.saturating_sub(1).min(width.saturating_sub(1));
    hit
}

fn resolve_placement_loop_fields(
    entities: &[andamento_core::DisplayEntity],
    region: &SurfaceRegionDefinition,
    catalog: Option<&TemplateConfigCatalog>,
    cols: usize,
    model: Option<&ControllerViewModel>,
) -> Vec<String> {
    let rows = entities
        .iter()
        .map(|entity| placement_entity_fields(entity, region, catalog, model))
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
            if text.is_empty() {
                entity.label.clone()
            } else {
                text
            }
        })
        .collect()
}

fn placement_entity_fields(
    entity: &andamento_core::DisplayEntity,
    region: &SurfaceRegionDefinition,
    catalog: Option<&TemplateConfigCatalog>,
    model: Option<&ControllerViewModel>,
) -> Vec<TemplateField> {
    if let Some(node) = entity.placement.as_ref().and_then(|key| {
        model
            .and_then(|m| m.presentation.as_ref())
            .and_then(|surface| surface.node(key))
    }) {
        return semantic_fields(&node.content);
    }
    let node = entity
        .placement
        .clone()
        .map(NodeKey::Placement)
        .unwrap_or_else(|| NodeKey::Entity(entity.entity.clone()));
    let mut metadata = metadata_with_effective_variables(entity.metadata.clone(), &node, model);
    andamento_core::presentation::apply_declared_abbreviation(&mut metadata);
    let form = form_for_region(region, &metadata);
    let slot = if form == DISPLAY_FORM_COMPACT {
        TemplateConfigSlot::Compact
    } else {
        TemplateConfigSlot::Detail
    };
    let context = TemplateConfigMatchContext {
        slot,
        node_kind: TemplateConfigNodeKind::Entity,
        metadata: &metadata,
        collapsed: false,
        collapsible: false,
        active_tab_name: None,
    };
    let resolved = if slot == TemplateConfigSlot::Compact {
        entity.templates.compact.as_ref()
    } else {
        entity.templates.detail.as_ref()
    };
    let fallback;
    let resolved = match resolved {
        Some(resolved) => Some(resolved),
        None => {
            fallback = catalog.and_then(|catalog| resolve_entity_slot(&metadata, slot, catalog));
            fallback.as_ref()
        }
    };
    resolved
        .map(|resolved| {
            resolved
                .render_ready
                .as_ref()
                .map(|ready| {
                    ready
                        .fields
                        .iter()
                        .map(|spec| {
                            let rendered = spec.render(context);
                            let value = rendered
                                .as_ref()
                                .map(|field| field.value.clone())
                                .unwrap_or_default();
                            let source = rendered.and_then(|field| field.source);
                            match spec.class {
                                _ if spec.priority.is_some() => TemplateField::Prioritized {
                                    value,
                                    priority: spec.priority.unwrap_or(100),
                                    source,
                                },
                                TemplateConfigFieldClass::Required => {
                                    TemplateField::Required(value)
                                }
                                TemplateConfigFieldClass::Optional => {
                                    TemplateField::Optional(value)
                                }
                                TemplateConfigFieldClass::Priority => {
                                    TemplateField::Priority(value)
                                }
                            }
                        })
                        .collect()
                })
                .unwrap_or_else(|| template_fields_from_resolved_slot(resolved, context))
        })
        .unwrap_or_else(|| {
            vec![TemplateField::Required(
                metadata_text(&metadata, "display.label")
                    .unwrap_or(&entity.label)
                    .to_owned(),
            )]
        })
}

fn pinned_region_rows(region: &DisplayRegion, model: Option<&ControllerViewModel>) -> usize {
    if matches!(
        region.definition.source,
        SurfaceRegionSource::Attention | SurfaceRegionSource::Tree
    ) && region.definition.placement.is_some()
        && !region.entities.is_empty()
    {
        return 1 + region
            .entities
            .iter()
            .map(|entity| {
                placement_entity_tree(
                    entity,
                    0,
                    model
                        .map(|model| model.collapsed_placements.as_slice())
                        .unwrap_or_default(),
                )
                .len()
            })
            .sum::<usize>();
    }
    match region.definition.source {
        SurfaceRegionSource::Controls => 1,
        SurfaceRegionSource::Attention => {
            let key = region
                .definition
                .attention_key
                .as_deref()
                .unwrap_or("status.attention");
            let entity_count = if region.entities.is_empty() {
                model
                    .into_iter()
                    .flat_map(|model| model.rows.iter())
                    .filter(|row| {
                        matches!(
                            row,
                            RailRow::Entity { entity, .. }
                                if entity.metadata.get(key) == Some(&MetadataValue::Bool(true))
                        )
                    })
                    .count()
            } else {
                region
                    .entities
                    .iter()
                    .filter(|entity| entity.metadata.get(key) == Some(&MetadataValue::Bool(true)))
                    .count()
            };
            1 + entity_count
        }
        SurfaceRegionSource::Header | SurfaceRegionSource::Tree => 1,
    }
}

fn region_root_line(
    region: &DisplayRegion,
    suffix: Option<&str>,
    cols: usize,
    theme: Option<RenderTheme>,
    model: Option<&ControllerViewModel>,
) -> String {
    let metadata = metadata_with_effective_variables(BTreeMap::new(), &NodeKey::Root, model);
    let text = region
        .root
        .as_ref()
        .map(|slot| {
            join_template_fields(
                &template_fields_from_resolved_slot(
                    slot,
                    TemplateConfigMatchContext {
                        slot: TemplateConfigSlot::Compact,
                        node_kind: TemplateConfigNodeKind::Entity,
                        metadata: &metadata,
                        collapsed: false,
                        collapsible: false,
                        active_tab_name: None,
                    },
                ),
                true,
            )
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| region.definition.name.clone());
    let text = format!("{text}{}", suffix.unwrap_or_default());
    style_body_text(pad_to_width(&truncate_to_width(&text, cols), cols), theme)
}

fn region_entity_line(
    entity: &andamento_core::DisplayEntity,
    region: &SurfaceRegionDefinition,
    catalog: Option<&TemplateConfigCatalog>,
    cols: usize,
    theme: Option<RenderTheme>,
    model: Option<&ControllerViewModel>,
) -> String {
    if let Some(node) = entity.placement.as_ref().and_then(|key| {
        model
            .and_then(|model| model.presentation.as_ref())
            .and_then(|surface| surface.node(key))
    }) {
        let text = join_template_fields(&semantic_fields(&node.content), true);
        let text = if text.is_empty() {
            node.label.clone()
        } else {
            text
        };
        return style_body_text(
            pad_to_width(&truncate_to_width(&format!("  {text}"), cols), cols),
            theme,
        );
    }
    let node = entity
        .placement
        .clone()
        .map(NodeKey::Placement)
        .unwrap_or_else(|| NodeKey::Entity(entity.entity.clone()));
    let mut metadata = metadata_with_effective_variables(entity.metadata.clone(), &node, model);
    apply_declared_abbreviation(&mut metadata, cols.saturating_sub(2));
    let form = form_for_region(region, &metadata);
    let slot = if form == DISPLAY_FORM_COMPACT {
        TemplateConfigSlot::Compact
    } else {
        TemplateConfigSlot::Detail
    };
    let context = TemplateConfigMatchContext {
        slot,
        node_kind: TemplateConfigNodeKind::Entity,
        metadata: &metadata,
        collapsed: false,
        collapsible: false,
        active_tab_name: None,
    };
    let resolved = if slot == TemplateConfigSlot::Compact {
        entity.templates.compact.as_ref()
    } else {
        entity.templates.detail.as_ref()
    };
    let fallback;
    let resolved = match resolved {
        Some(resolved) => Some(resolved),
        None => {
            fallback = catalog.and_then(|catalog| resolve_entity_slot(&metadata, slot, catalog));
            fallback.as_ref()
        }
    };
    let text = resolved
        .map(|resolved| {
            join_template_fields(&template_fields_from_resolved_slot(resolved, context), true)
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| {
            metadata_text(&metadata, "display.label")
                .unwrap_or(&entity.label)
                .to_owned()
        });
    style_body_text(
        pad_to_width(&truncate_to_width(&format!("  {text}"), cols), cols),
        theme,
    )
}

fn apply_declared_abbreviation(metadata: &mut RenderMetadata, available_width: usize) {
    if andamento_core::presentation::apply_declared_abbreviation(metadata) {
        if let Some(label) = metadata_text(metadata, "display.label") {
            let label = middle_elide_to_width(label, available_width);
            metadata.insert("display.label".to_owned(), MetadataValue::Text(label));
        }
    }
}

fn middle_elide_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let characters = text.chars().collect::<Vec<_>>();
    let content_width = width - 1;
    let left_budget = content_width.div_ceil(2);
    let right_budget = content_width / 2;
    let mut left = String::new();
    let mut used = 0;
    let mut left_count = 0;
    for character in &characters {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > left_budget {
            break;
        }
        left.push(*character);
        used += character_width;
        left_count += 1;
    }
    let mut right = Vec::new();
    used = 0;
    for character in characters[left_count..].iter().rev() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > right_budget {
            break;
        }
        right.push(*character);
        used += character_width;
    }
    right.reverse();
    format!("{left}…{}", right.into_iter().collect::<String>())
}

fn apply_region_form(
    rows: &mut [RailRow],
    region: &SurfaceRegionDefinition,
    catalog: Option<&TemplateConfigCatalog>,
) {
    for row in rows {
        let RailRow::Entity { entity, .. } = row else {
            continue;
        };
        entity.form = form_for_region(region, &entity.metadata).to_owned();
        let slot = if entity.form == DISPLAY_FORM_COMPACT {
            TemplateConfigSlot::Compact
        } else {
            TemplateConfigSlot::Detail
        };
        if slot == TemplateConfigSlot::Compact && entity.templates.compact.is_none() {
            entity.templates.compact =
                catalog.and_then(|catalog| resolve_entity_slot(&entity.metadata, slot, catalog));
        } else if slot == TemplateConfigSlot::Detail && entity.templates.detail.is_none() {
            entity.templates.detail =
                catalog.and_then(|catalog| resolve_entity_slot(&entity.metadata, slot, catalog));
        }
    }
}

fn metadata_with_effective_variables(
    mut metadata: RenderMetadata,
    node: &NodeKey,
    model: Option<&ControllerViewModel>,
) -> RenderMetadata {
    let variables = model
        .into_iter()
        .flat_map(|model| model.template_config.effective_variables.iter())
        .find(|variables| &variables.node == node);
    if let Some(variables) = variables {
        for (name, effective) in &variables.values {
            metadata.insert(
                format!("var.{name}"),
                MetadataValue::Text(effective.value.clone()),
            );
        }
    }
    metadata
}

fn form_for_region<'a>(region: &'a SurfaceRegionDefinition, metadata: &RenderMetadata) -> &'a str {
    region
        .promotions
        .iter()
        .find(|promotion| metadata.get(&promotion.when) == Some(&MetadataValue::Bool(true)))
        .map(|promotion| promotion.form.as_str())
        .unwrap_or(&region.form)
}

fn resolve_entity_slot(
    metadata: &RenderMetadata,
    slot: TemplateConfigSlot,
    catalog: &TemplateConfigCatalog,
) -> Option<ResolvedTemplateSlot> {
    let context = TemplateConfigMatchContext {
        slot,
        node_kind: TemplateConfigNodeKind::Entity,
        metadata,
        collapsed: false,
        collapsible: false,
        active_tab_name: None,
    };
    catalog
        .resolve(context)
        .ok()
        .flatten()
        .map(|template| ResolvedTemplateSlot {
            template_name: template.name.clone(),
            fields: vec![],
            render_ready: Some(template.render_ready()),
            setters: template.setters.clone(),
            effective_kdl: template.dump_kdl(),
            resolve_error: None,
        })
}

fn resolve_region_root(
    region: &SurfaceRegionDefinition,
    catalog: &TemplateConfigCatalog,
) -> Option<ResolvedTemplateSlot> {
    let metadata = BTreeMap::from([(
        "presentation.template".to_owned(),
        MetadataValue::Text(region.root_template.clone()),
    )]);
    let context = TemplateConfigMatchContext {
        slot: TemplateConfigSlot::Compact,
        node_kind: TemplateConfigNodeKind::Entity,
        metadata: &metadata,
        collapsed: false,
        collapsible: false,
        active_tab_name: None,
    };
    catalog
        .resolve(context)
        .ok()
        .flatten()
        .map(|template| ResolvedTemplateSlot {
            template_name: template.name.clone(),
            fields: vec![],
            render_ready: Some(template.render_ready()),
            setters: template.setters.clone(),
            effective_kdl: template.dump_kdl(),
            resolve_error: None,
        })
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
    collapsed_groups: &[GroupPath],
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    rail_scroll_offset: isize,
    ensure_active_visible: bool,
    detail_target: Option<&NodeKey>,
) -> RenderedRail {
    if rows < 2 || !controller_available {
        return render_lines_with_rail_viewport(
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
            rail_scroll_offset,
            ensure_active_visible,
        );
    }
    let mut rendered = render_lines_with_rail_viewport(
        model,
        tabs,
        rows - 1,
        cols,
        controller_available,
        theme,
        terminal_cell_size,
        collapsed_groups,
        template_catalog,
        metadata_controls,
        rail_scroll_offset,
        ensure_active_visible,
    );
    let footer_row = rows - 2;
    let footer = rendered.lines.pop().unwrap_or_else(|| blank(cols));
    for hit in &mut rendered.hit_regions {
        if hit.row_start == footer_row {
            hit.row_start += 1;
            hit.row_end += 1;
        }
    }
    rendered
        .lines
        .push(render_detail_surface(model, detail_target, cols, theme));
    rendered.lines.push(footer);
    rendered
}

fn render_detail_surface(
    model: Option<&ControllerViewModel>,
    target: Option<&NodeKey>,
    width: usize,
    theme: Option<RenderTheme>,
) -> String {
    let detail = match (model, target) {
        (Some(model), Some(NodeKey::Entity(target))) => display_entity_for_target(model, target)
            .map(display_entity_detail)
            .or_else(|| latent_entity_for_target(model, target).map(latent_entity_detail)),
        (Some(model), Some(NodeKey::Placement(key))) => key
            .0
            .last()
            .and_then(|_| display_entity_for_placement(model, key))
            .map(display_entity_detail),
        _ => None,
    }
    .unwrap_or_default();
    style_body_text(
        pad_to_width(&truncate_to_width(&detail, width), width),
        theme,
    )
}

fn display_entity_detail(entity: &andamento_core::DisplayEntity) -> String {
    entity_detail(
        &entity.entity,
        &entity.label,
        &entity.templates,
        &display_entity_metadata(entity),
    )
}

fn latent_entity_detail(latent: &andamento_core::LatentTab) -> String {
    let card = render_card_from_latent(latent);
    entity_detail(&latent.entity, &card.name, &card.templates, &card.metadata)
}

fn entity_detail(
    entity: &andamento_core::EntityRef,
    label: &str,
    templates: &ResolvedTemplateSlots,
    metadata: &RenderMetadata,
) -> String {
    let text = templates
        .detail
        .as_ref()
        .map(|slot| {
            let fields = template_fields_from_resolved_slot(
                slot,
                TemplateConfigMatchContext {
                    slot: TemplateConfigSlot::Detail,
                    node_kind: TemplateConfigNodeKind::Entity,
                    metadata,
                    collapsed: false,
                    collapsible: false,
                    active_tab_name: None,
                },
            );
            join_template_fields(&fields, true)
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| label.to_owned());
    format!("[{}] {text}", entity.kind)
}

fn display_entity_for_target<'a>(
    model: &'a ControllerViewModel,
    target: &andamento_core::EntityRef,
) -> Option<&'a andamento_core::DisplayEntity> {
    model
        .rows
        .iter()
        .filter_map(|row| match row {
            RailRow::Entity { entity, .. } => Some(entity),
            _ => None,
        })
        .chain(
            model
                .surface_regions
                .iter()
                .flat_map(|region| region.entities.iter()),
        )
        .find(|entity| &entity.entity == target)
}

fn display_entity_for_placement<'a>(
    model: &'a ControllerViewModel,
    target: &andamento_core::PlacementKey,
) -> Option<&'a andamento_core::DisplayEntity> {
    fn find<'a>(
        entities: &'a [andamento_core::DisplayEntity],
        target: &andamento_core::PlacementKey,
    ) -> Option<&'a andamento_core::DisplayEntity> {
        entities.iter().find_map(|entity| {
            (entity.placement.as_ref() == Some(target))
                .then_some(entity)
                .or_else(|| find(&entity.children, target))
        })
    }

    model
        .surface_regions
        .iter()
        .find_map(|region| find(&region.entities, target))
}

fn latent_entity_for_target<'a>(
    model: &'a ControllerViewModel,
    target: &andamento_core::EntityRef,
) -> Option<&'a andamento_core::LatentTab> {
    model.rows.iter().find_map(|row| match row {
        RailRow::Latent { latent, .. } if &latent.entity == target => Some(latent),
        _ => None,
    })
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
    let excluded_keys = [
        "group.label",
        "group.full_label",
        "group.tab_count",
        "group.key",
        "group.value",
    ]
    .into_iter()
    .chain(group.path.0.iter().map(|segment| segment.key.as_str()))
    .collect::<Vec<_>>();
    push_additional_metadata_lines(&mut lines, 4, &group.metadata, &excluded_keys);
    push_metadata_source_detail_lines(&mut lines, 2, &group.metadata_sources);
    push_reachable_identity_lines(&mut lines, 2, &group.reachable_identities);
    push_effective_template_section(
        &mut lines,
        2,
        [(
            "group_header",
            group.templates.group_header.as_ref(),
            TemplateRenderContext {
                slot: TemplateConfigSlot::GroupHeader,
                node_kind: TemplateConfigNodeKind::Group,
                metadata: &group.metadata,
                collapsed: group.collapsed,
                collapsible: true,
                active_tab_name: active_tab_name(&group.children),
            },
        )],
    );
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
    let template_contexts = if card.entity.is_some() {
        vec![
            (
                "compact",
                card.templates.compact.as_ref(),
                TemplateRenderContext {
                    slot: TemplateConfigSlot::Compact,
                    node_kind: TemplateConfigNodeKind::Entity,
                    metadata: &card.metadata,
                    collapsed: false,
                    collapsible: false,
                    active_tab_name: None,
                },
            ),
            (
                "detail",
                card.templates.detail.as_ref(),
                TemplateRenderContext {
                    slot: TemplateConfigSlot::Detail,
                    node_kind: TemplateConfigNodeKind::Entity,
                    metadata: &card.metadata,
                    collapsed: false,
                    collapsible: false,
                    active_tab_name: None,
                },
            ),
        ]
    } else {
        let mut contexts = vec![(
            "tab_title",
            card.templates.tab_title.as_ref(),
            TemplateRenderContext {
                slot: TemplateConfigSlot::TabTitle,
                node_kind: TemplateConfigNodeKind::Tab,
                metadata: &card.metadata,
                collapsed: false,
                collapsible: false,
                active_tab_name: None,
            },
        )];
        if card.status.is_some() {
            contexts.push((
                "tab_status",
                card.templates.tab_status.as_ref(),
                TemplateRenderContext {
                    slot: TemplateConfigSlot::TabStatus,
                    node_kind: TemplateConfigNodeKind::Tab,
                    metadata: &card.metadata,
                    collapsed: false,
                    collapsible: false,
                    active_tab_name: None,
                },
            ));
        }
        contexts
    };
    push_effective_template_section(&mut lines, indent + 2, template_contexts);
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

fn push_effective_template_section<'a>(
    lines: &mut Vec<String>,
    indent: usize,
    templates: impl IntoIterator<
        Item = (
            &'a str,
            Option<&'a ResolvedTemplateSlot>,
            TemplateRenderContext<'a>,
        ),
    >,
) {
    let dumps = templates
        .into_iter()
        .filter_map(|(slot_name, slot, context)| {
            let dump = if let Some(slot) = slot {
                if slot.effective_kdl.is_empty() {
                    format!("// resolved template: {}\n", slot.template_name)
                } else {
                    slot.effective_kdl.clone()
                }
            } else {
                match bundled_template_catalog().resolve(template_match_context(context)) {
                    Ok(Some(resolved)) => resolved.dump_kdl(),
                    Ok(None) => return None,
                    Err(error) => format!("// error: {error}\n"),
                }
            };
            Some((slot_name, dump))
        })
        .collect::<Vec<_>>();
    if dumps.is_empty() {
        return;
    }
    push_metadata_section_header(lines, indent, "resolved_template");
    for (slot_name, dump) in dumps {
        lines.push(format!("{}// slot: {slot_name}", " ".repeat(indent + 2)));
        lines.extend(
            dump.lines()
                .map(|line| format!("{}{}", " ".repeat(indent + 2), line)),
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

fn style_title_text(
    line: String,
    active: bool,
    dimmed: bool,
    theme: Option<RenderTheme>,
) -> String {
    let mut style = theme
        .map(|theme| {
            foreground_style(if active {
                theme.active_border
            } else {
                theme.inactive_border
            })
            .bold()
        })
        .unwrap_or_default();
    if dimmed {
        style = latent_tab_style(style);
    }
    style.paint(line).to_string()
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

fn latent_tab_style(style: Style) -> Style {
    style.dimmed()
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
    ensure_active_visible: bool,
) -> RenderedNodes {
    if nodes.is_empty() || available_rows == 0 {
        return RenderedNodes::default();
    }

    // Root's MetaChildren cascades to descendants.
    let root_meta_children = metadata_controls.propagates_to_children(&NodeKey::Root, false);

    if let Some(top_tabs) = top_level_tabs(nodes) {
        let cards = top_tabs
            .iter()
            .map(|tab| {
                let mut card = tab.card.clone();
                let key = card
                    .entity
                    .clone()
                    .map(NodeKey::Entity)
                    .unwrap_or(NodeKey::Tab(card.tab_id));
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
        return RenderedNodes {
            viewport: Viewport {
                ensure_active_resolved: ensure_active_visible && controller_available,
                ..Viewport::default()
            },
            ..RenderedNodes::default()
        };
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
    let viewport = copy_visible_buffer(
        lines,
        hit_regions,
        visible_cards,
        buffered_lines,
        buffered_hits,
        buffered_cards,
        active_ensure_target(nodes),
        available_rows,
        rail_scroll_offset,
        ensure_active_visible,
    );
    RenderedNodes {
        content_height,
        viewport,
    }
}

fn root_inherited_settings_for_model(model: Option<&ControllerViewModel>) -> InheritedRailSettings {
    let Some(model) = model else {
        return InheritedRailSettings::default();
    };
    let Some(variables) = model
        .template_config
        .effective_variables
        .iter()
        .find(|variables| variables.node == NodeKey::Root)
    else {
        return InheritedRailSettings::default();
    };
    variables
        .values
        .get("child-layout")
        .and_then(|value| child_layout_from_text(&value.value))
        .map(|child_layout| InheritedRailSettings { child_layout })
        .unwrap_or_default()
}

fn top_level_tabs(nodes: &[RenderNode]) -> Option<Vec<&RenderTab>> {
    nodes
        .iter()
        .map(|node| match node {
            RenderNode::Tab(tab) if tab.indent == 0 && !tab.card.compact_only => Some(tab),
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
                flush_render_tabs(
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
                let group_key = NodeKey::Group(group.path.clone());
                let child_settings = inherited_settings.with_node_metadata(&group.metadata);
                let (direct_tabs, child_group_nodes) =
                    if child_settings.child_layout == ChildLayoutMode::Strip {
                        direct_tabs_and_child_groups(&group.children)
                    } else {
                        (vec![], vec![])
                    };
                let child_groups = child_group_nodes
                    .iter()
                    .filter_map(|node| match node {
                        RenderNode::Group(group) => Some(group.clone()),
                        RenderNode::Tab(_) => None,
                    })
                    .collect::<Vec<_>>();
                let (visible_header_sources, _group_allocation, niche_consumption) =
                    append_group_header(
                        lines,
                        hit_regions,
                        group,
                        cols,
                        theme,
                        template_catalog,
                        ancestor_template_fields,
                        metadata_controls,
                        inspected_node,
                        &direct_tabs,
                        &child_groups,
                        child_settings.child_layout,
                    );
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
                    if child_settings.child_layout == ChildLayoutMode::Strip {
                        let remaining_children =
                            children_after_niche_consumption(&group.children, &niche_consumption);
                        let (remaining_direct_tabs, remaining_child_groups) =
                            direct_tabs_and_child_groups(&remaining_children);
                        append_tab_strip(
                            lines,
                            hit_regions,
                            &remaining_direct_tabs,
                            cols,
                            theme,
                            template_catalog,
                        );
                        render_nodes_to_buffer(
                            lines,
                            hit_regions,
                            visible_cards,
                            &remaining_child_groups,
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
    flush_render_tabs(
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

#[allow(clippy::too_many_arguments)]
fn flush_render_tabs(
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
) {
    let (compact, full): (Vec<_>, Vec<_>) =
        tabs.iter().cloned().partition(|tab| tab.card.compact_only);
    let _ = append_tab_run(
        lines,
        hit_regions,
        visible_cards,
        &full,
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
    append_tab_strip(lines, hit_regions, &compact, cols, theme, template_catalog);
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
    niche_tabs: &[RenderTab],
    niche_child_groups: &[RenderGroup],
    child_layout: ChildLayoutMode,
) -> (
    BTreeSet<ResolvedTemplateFieldSource>,
    NodeRowAllocation,
    HeaderNicheConsumption,
) {
    let row = lines.len();
    let chrome = group_header_chrome(
        &group.metadata,
        group.collapsed,
        true,
        active_tab_name(&group.children),
        group.templates.group_header.as_ref(),
        template_catalog,
    );
    let hierarchy_depth = group.indent / 2;
    let indent = chrome
        .indent_width()
        .map(|width| hierarchy_depth.saturating_mul(width))
        .unwrap_or_default()
        .min(cols);
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
    let mut rendered = group_header_line(
        &group.metadata,
        group.collapsed,
        true,
        contains_active_tab(&group.children),
        active_tab_name(&group.children),
        group.templates.group_header.as_ref(),
        header_width,
        theme,
        template_catalog,
        ancestor_template_fields,
        niche_tabs,
        niche_child_groups,
    );
    if !group_header_has_revealable_body(group, child_layout, &rendered.niche_consumption) {
        rendered = group_header_line(
            &group.metadata,
            group.collapsed,
            false,
            contains_active_tab(&group.children),
            active_tab_name(&group.children),
            group.templates.group_header.as_ref(),
            header_width,
            theme,
            template_catalog,
            ancestor_template_fields,
            niche_tabs,
            niche_child_groups,
        );
    }
    line.push_str(&rendered.text);
    for hit in rendered.niche_hits {
        hit_regions.push(HitRegion {
            row_start: row,
            row_end: row,
            col_start: indent + hit.col_start,
            col_end: indent + hit.col_end,
            tab_id: hit.tab_id,
            tab_position: hit.tab_position,
            group_path: hit.group_path,
            inspect_target: hit.inspect_target,
            materialize_request: hit.materialize_request,
            action: hit.action,
        });
    }
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
            materialize_request: None,
            action: HitAction::InspectNode,
        });
    }
    lines.push(line);
    let collapse_visible =
        group_header_has_revealable_body(group, child_layout, &rendered.niche_consumption);
    if collapse_visible {
        hit_regions.push(HitRegion {
            row_start: row,
            row_end: row,
            col_start: indent,
            col_end: indent,
            tab_id: 0,
            tab_position: 0,
            group_path: Some(group.path.clone()),
            inspect_target: None,
            materialize_request: None,
            action: HitAction::ToggleGroup,
        });
    }
    let allocation = NodeRowAllocation {
        key: NodeKey::Group(group.path.clone()),
        rows: row..row + 1,
    };
    (
        rendered.visible_sources,
        allocation,
        rendered.niche_consumption,
    )
}

fn group_header_has_revealable_body(
    group: &RenderGroup,
    child_layout: ChildLayoutMode,
    niche_consumption: &HeaderNicheConsumption,
) -> bool {
    if group.children.is_empty() {
        return false;
    }
    if child_layout != ChildLayoutMode::Strip {
        return true;
    }
    !children_after_niche_consumption(&group.children, niche_consumption).is_empty()
}

fn child_layout_from_variables(metadata: &RenderMetadata) -> Option<ChildLayoutMode> {
    let MetadataValue::Text(value) = metadata.get(CHILD_LAYOUT_VARIABLE_KEY)? else {
        return None;
    };
    child_layout_from_text(value)
}

fn child_layout_from_text(value: &str) -> Option<ChildLayoutMode> {
    match value {
        "cards" => Some(ChildLayoutMode::Cards),
        "strip" => Some(ChildLayoutMode::Strip),
        _ => None,
    }
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

fn children_after_niche_consumption(
    children: &[RenderNode],
    consumption: &HeaderNicheConsumption,
) -> Vec<RenderNode> {
    let mut remaining = vec![];
    let mut skipped_tabs = 0usize;
    let mut seen_groups = 0usize;
    for child in children {
        match child {
            RenderNode::Tab(_) if skipped_tabs < consumption.direct_tabs => {
                skipped_tabs += 1;
            }
            RenderNode::Group(group) => {
                if let Some(group_consumption) = consumption.child_group.as_deref() {
                    if seen_groups == group_consumption.child_group_index {
                        let mut group = group.clone();
                        group.children = children_after_niche_consumption(
                            &group.children,
                            &group_consumption.child_consumption,
                        );
                        if !group.collapsed {
                            remaining.extend(group.children);
                        }
                        seen_groups += 1;
                        continue;
                    }
                }
                seen_groups += 1;
                remaining.push(child.clone());
            }
            RenderNode::Tab(_) => remaining.push(child.clone()),
        }
    }
    remaining
}

fn append_tab_strip(
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
        .map(|tab| {
            let hierarchy_depth = tab.indent / 2;
            card_chrome(&tab.card, template_catalog)
                .indent_width()
                .map(|width| hierarchy_depth.saturating_mul(width))
                .unwrap_or_default()
        })
        .min()
        .unwrap_or(0)
        .min(cols.saturating_sub(1));
    let inner_width = cols.saturating_sub(indent);
    if inner_width == 0 {
        return;
    }

    let items = tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let label = tab_title_with_template_catalog(&tab.card, template_catalog);
            InlineItem::segment(
                format!("tab:{index}"),
                label.clone(),
                strip_segment_width(&label),
            )
            .hit(InlineHit::SwitchTab { index })
        })
        .collect::<Vec<_>>();
    let rendered = render_strip_segment_run(&items, tabs, inner_width, theme);
    let base_row = lines.len();
    for line in rendered.lines {
        lines.push(format!("{}{}", " ".repeat(indent), line));
    }
    for hit in rendered.hits {
        let Some(tab) = tabs.get(hit.index) else {
            continue;
        };
        if tab.card.latent && tab.card.materialize_request.is_none() && tab.card.entity.is_none() {
            continue;
        }
        hit_regions.push(HitRegion {
            row_start: base_row + hit.row,
            row_end: base_row + hit.row,
            col_start: indent + hit.col_start,
            col_end: indent + hit.col_end,
            tab_id: tab.card.tab_id,
            tab_position: tab.card.position,
            group_path: tab.parent_path.clone(),
            inspect_target: tab.card.entity.clone().map(NodeKey::Entity),
            materialize_request: tab.card.materialize_request.clone(),
            action: if tab.card.latent && tab.card.materialize_request.is_some() {
                HitAction::Materialize
            } else if tab.card.entity.is_some() {
                HitAction::ShowDetail
            } else {
                HitAction::SwitchTab
            },
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StripSegmentRun {
    lines: Vec<String>,
    hits: Vec<StripSegmentHitBox>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StripSegmentHitBox {
    index: usize,
    row: usize,
    col_start: usize,
    col_end: usize,
}

fn render_strip_segment_run(
    items: &[InlineItem],
    tabs: &[RenderTab],
    width: usize,
    theme: Option<RenderTheme>,
) -> StripSegmentRun {
    if width == 0 || items.is_empty() {
        return StripSegmentRun {
            lines: vec![],
            hits: vec![],
        };
    }
    if theme.is_none() {
        let segment_items = tabs
            .iter()
            .zip(items.iter())
            .map(|tab| SegmentItem {
                label: tab.1.text.clone(),
                active: tab.0.card.active,
            })
            .collect::<Vec<_>>();
        let rendered =
            segment_bar::render_wrapped(&segment_items, &segment_bar::ZellijRibbonStyle, width);
        return StripSegmentRun {
            lines: rendered.lines,
            hits: rendered
                .hits
                .into_iter()
                .map(|hit| StripSegmentHitBox {
                    index: hit.index,
                    row: hit.row,
                    col_start: hit.col_start,
                    col_end: hit.col_end,
                })
                .collect(),
        };
    }

    let mut lines = vec![];
    let mut hits = vec![];
    for (row, placed) in InlineRun::new(items.to_vec())
        .layout_wrapped(width)
        .into_iter()
        .enumerate()
    {
        let mut line = String::new();
        let mut col = 0usize;
        for item in placed.items {
            let Some(index) = inline_switch_tab_index(item.hit) else {
                continue;
            };
            let Some(tab) = tabs.get(index) else {
                continue;
            };
            if item.cols.start > col {
                line.push_str(&" ".repeat(item.cols.start - col));
                col = item.cols.start;
            }
            let label = items
                .get(index)
                .map(|item| item.text.clone())
                .unwrap_or_else(|| tab_title_with_template_catalog(&tab.card, None));
            let segment_item = SegmentItem {
                label,
                active: tab.card.active,
            };
            let max_width = item.cols.end.saturating_sub(item.cols.start);
            let (segment, visible_width) = render_strip_segment(&segment_item, max_width, theme);
            if visible_width == 0 {
                continue;
            }
            line.push_str(&segment);
            col = col.saturating_add(visible_width);
            hits.push(StripSegmentHitBox {
                index,
                row,
                col_start: item.cols.start,
                col_end: item.cols.end.saturating_sub(1),
            });
        }
        if line.is_empty() {
            continue;
        }
        lines.push(pad_styled_line_to_width(&line, col, width));
    }
    StripSegmentRun { lines, hits }
}

fn inline_switch_tab_index(hit: Option<InlineHit>) -> Option<usize> {
    match hit {
        Some(InlineHit::SwitchTab { index }) => Some(index),
        Some(InlineHit::GroupToggle | InlineHit::InspectNode) | None => None,
    }
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

fn strip_segment_width(label: &str) -> usize {
    label.width().saturating_add(4)
}

fn render_strip_segment(
    item: &SegmentItem,
    max_width: usize,
    theme: Option<RenderTheme>,
) -> (String, usize) {
    if max_width == 0 {
        return (String::new(), 0);
    }
    let visible_width = strip_segment_width(&item.label).min(max_width);
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
            let key = card
                .entity
                .clone()
                .map(NodeKey::Entity)
                .unwrap_or(NodeKey::Tab(card.tab_id));
            if metadata_controls.effective_show(&key, ancestor_meta_children) {
                card.meta_panel = Some(tab_metadata_block(tab));
            }
            card
        })
        .collect::<Vec<_>>();
    let run_height = generous_card_run_height(&cards);
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

fn generous_card_run_height(cards: &[RenderCard]) -> usize {
    cards
        .iter()
        .map(|card| cell_height(card, false).max(ACTIVE_CELL_HEIGHT) + 1)
        .sum::<usize>()
        + 1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Viewport {
    visible_start: usize,
    ensure_visible_offset: Option<isize>,
    ensure_active_resolved: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RenderedNodes {
    content_height: usize,
    viewport: Viewport,
}

fn absolute_viewport_start(scroll_offset: isize, max_start: usize) -> usize {
    scroll_offset.clamp(0, max_start as isize) as usize
}

fn ensure_visible_start(
    visible_start: usize,
    target_start: usize,
    target_end: usize,
    available_rows: usize,
    max_start: usize,
) -> usize {
    // Any overlap counts as visible so selecting an already clickable card never shifts the rail.
    let visible_end = visible_start.saturating_add(available_rows);
    if target_end < visible_start {
        target_start.min(max_start)
    } else if target_start >= visible_end {
        target_end
            .saturating_add(1)
            .saturating_sub(available_rows)
            .min(max_start)
    } else {
        visible_start
    }
}

fn copy_visible_buffer(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    visible_cards: &mut Vec<VisibleCard>,
    buffered_lines: Vec<String>,
    buffered_hits: Vec<HitRegion>,
    buffered_cards: Vec<VisibleCard>,
    active_ensure_target: Option<NodeKey>,
    available_rows: usize,
    user_scroll_offset: isize,
    ensure_active_visible: bool,
) -> Viewport {
    if buffered_lines.is_empty() || available_rows == 0 {
        return Viewport::default();
    }
    let max_start = buffered_lines.len().saturating_sub(available_rows);
    let base_start = absolute_viewport_start(user_scroll_offset, max_start);
    let active_hit = ensure_active_visible
        .then_some(active_ensure_target.as_ref())
        .flatten()
        .and_then(|target| {
            buffered_hits
                .iter()
                .find(|hit| hit_matches_node(hit, target))
        });
    let ensure_active_resolved = active_hit.is_some();
    let visible_start = if ensure_active_visible {
        active_hit
            .map(|hit| {
                ensure_visible_start(
                    base_start,
                    hit.row_start,
                    hit.row_end,
                    available_rows,
                    max_start,
                )
            })
            .unwrap_or(base_start)
    } else {
        base_start
    };
    let ensure_visible_offset = (visible_start != base_start).then_some(visible_start as isize);
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
    Viewport {
        visible_start,
        ensure_visible_offset,
        ensure_active_resolved,
    }
}

fn hit_matches_node(hit: &HitRegion, node: &NodeKey) -> bool {
    match node {
        NodeKey::Tab(tab_id) => hit.action == HitAction::SwitchTab && hit.tab_id == *tab_id,
        NodeKey::Group(path) => {
            hit.action == HitAction::ToggleGroup && hit.group_path.as_ref() == Some(path)
        }
        NodeKey::Root => false,
        NodeKey::Entity(entity) => {
            hit.action == HitAction::ShowDetail
                && hit.inspect_target.as_ref() == Some(&NodeKey::Entity(entity.clone()))
        }
        NodeKey::Placement(key) => {
            hit.inspect_target.as_ref() == Some(&NodeKey::Placement(key.clone()))
        }
    }
}

fn active_ensure_target(nodes: &[RenderNode]) -> Option<NodeKey> {
    nodes.iter().find_map(|node| match node {
        RenderNode::Tab(tab) if tab.card.active => Some(NodeKey::Tab(tab.card.tab_id)),
        RenderNode::Tab(_) => None,
        RenderNode::Group(group) if group.collapsed && contains_active_tab(&group.children) => {
            Some(NodeKey::Group(group.path.clone()))
        }
        RenderNode::Group(group) => active_ensure_target(&group.children),
    })
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
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) {
    let visible = visible_cells(cards, available_rows);
    for (visible_index, (card_index, cell_height)) in visible.iter().copied().enumerate() {
        let card = &cards[card_index];
        let chrome = card_chrome(card, template_catalog);
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
            &chrome,
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
            &chrome,
        );
    }
    if let Some(last_line) = lines.get_mut(visible.iter().map(|(_, height)| *height).sum::<usize>())
    {
        let bottom_border_active = visible
            .last()
            .map(|(card_index, _)| cards[*card_index].active)
            .unwrap_or(false);
        let chrome = visible
            .last()
            .map(|(card_index, _)| card_chrome(&cards[*card_index], template_catalog))
            .unwrap_or_default();
        *last_line = bottom_border_line(cols, bottom_border_active, theme, &chrome);
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
            theme,
            terminal_cell_size,
            template_catalog,
            metadata_controls,
            inspected_node,
        );
    };
    let visible = visible_cells(cards, available_rows);
    let mut row = 0;
    for (visible_index, (card_index, cell_height)) in visible.iter().copied().enumerate() {
        let card = &cards[card_index];
        let chrome = card_chrome(card, template_catalog);
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
            &chrome,
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
            &chrome,
        );
        row += cell_height;
        let run_ends = visible
            .get(visible_index + 1)
            .map(|(next_card_index, _)| *next_card_index == active_index)
            .unwrap_or(true);
        if run_ends {
            if let Some(last_line) = lines.get_mut(row) {
                *last_line = bottom_border_line(cols, false, theme, &chrome);
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
    theme: Option<RenderTheme>,
    terminal_cell_size: Option<SizeInPixels>,
    template_catalog: Option<&TemplateConfigCatalog>,
    metadata_controls: &MetadataControls,
    inspected_node: Option<&NodeKey>,
) {
    let visible = visible_boxes(cards, available_rows);
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
    let chrome = card_chrome(card, template_catalog);
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
        &chrome,
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
            *line = body_line(
                text,
                cols,
                card.active,
                card_dimmed(card, &chrome),
                theme,
                &chrome,
            );
        }
    }
    if let Some(line) = lines.get_mut(row + box_height - 1) {
        *line = bottom_border_line(cols, card.active, theme, &chrome);
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
    chrome: &ChromeSpec,
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
            *line = body_line(
                text,
                cols,
                card.active,
                card_dimmed(card, chrome),
                theme,
                chrome,
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
    if card.entity.is_some() || !card.latent || card.materialize_request.is_some() {
        hit_regions.push(HitRegion {
            row_start: row,
            row_end: row + height.saturating_sub(1),
            col_start: 0,
            col_end: cols.saturating_sub(1),
            tab_id: card.tab_id,
            tab_position: card.position,
            group_path: None,
            inspect_target: card.entity.clone().map(NodeKey::Entity),
            materialize_request: card.materialize_request.clone(),
            action: if card.latent && card.materialize_request.is_some() {
                HitAction::Materialize
            } else if card.entity.is_some() {
                HitAction::ShowDetail
            } else {
                HitAction::SwitchTab
            },
        });
    }
    if controller_available && !card.latent {
        hit_regions.push(HitRegion {
            row_start: row + 1,
            row_end: row + 1,
            col_start: 2,
            col_end: 7.min(cols.saturating_sub(1)),
            tab_id: card.tab_id,
            tab_position: card.position,
            group_path: None,
            inspect_target: None,
            materialize_request: None,
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
    if !card.latent {
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
}

fn nodes_to_render(
    model: Option<&ControllerViewModel>,
    tabs: &[LocalTab],
    collapsed_groups: &[GroupPath],
    form_policy: Option<&SurfaceRegionDefinition>,
) -> Vec<RenderNode> {
    let form_policy = match form_policy {
        Some(policy) => Some(policy),
        None => bundled_template_catalog()
            .regions()
            .iter()
            .find(|region| region.source == SurfaceRegionSource::Tree),
    };
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
                        latent: false,
                        materialize_request: None,
                        latent_summary: None,
                        meta_panel: None,
                        entity: None,
                        compact_only: false,
                        form: "full".to_owned(),
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
                RailRow::Latent {
                    latent,
                    indent,
                    parent_path,
                } => Some(PendingRenderNode::Tab(RenderTab {
                    card: render_card_from_latent(latent),
                    indent: *indent,
                    grouping: None,
                    parent_path: parent_path.clone(),
                })),
                RailRow::Entity {
                    entity,
                    indent,
                    parent_path,
                } => Some(PendingRenderNode::Tab(RenderTab {
                    card: render_card_from_entity(entity),
                    indent: *indent,
                    grouping: None,
                    parent_path: parent_path.clone(),
                })),
            })
            .collect()
    };
    let mut nodes = pending_nodes_to_render_nodes(pending_rows, collapsed_groups);
    merge_resolved_metadata(&mut nodes, &model.resolved_metadata);
    merge_effective_variables(&mut nodes, &model.template_config.effective_variables);
    if let Some(region) = form_policy {
        apply_region_form_to_nodes(&mut nodes, region);
    }
    conflate_spindly_groups(&mut nodes, root_inherited_settings_for_model(Some(model)));
    nodes
}

fn apply_region_form_to_nodes(nodes: &mut [RenderNode], region: &SurfaceRegionDefinition) {
    for node in nodes {
        match node {
            RenderNode::Tab(tab) => {
                tab.card.form = form_for_region(region, &tab.card.metadata).to_owned();
            }
            RenderNode::Group(group) => {
                apply_region_form_to_nodes(&mut group.children, region);
            }
        }
    }
}

fn merge_effective_variables(
    nodes: &mut [RenderNode],
    resolved: &[andamento_core::EffectiveNodeVariables],
) {
    let by_node = resolved
        .iter()
        .map(|variables| (&variables.node, &variables.values))
        .collect::<HashMap<_, _>>();
    merge_effective_variables_into_nodes(nodes, &by_node);
}

fn merge_effective_variables_into_nodes(
    nodes: &mut [RenderNode],
    by_node: &HashMap<&NodeKey, &BTreeMap<String, andamento_core::EffectiveVariableValue>>,
) {
    for node in nodes {
        let (key, metadata, children) = match node {
            RenderNode::Tab(tab) => (
                tab.card
                    .entity
                    .clone()
                    .map(NodeKey::Entity)
                    .unwrap_or(NodeKey::Tab(tab.card.tab_id)),
                &mut tab.card.metadata,
                None,
            ),
            RenderNode::Group(group) => (
                NodeKey::Group(group.path.clone()),
                &mut group.metadata,
                Some(group.children.as_mut_slice()),
            ),
        };
        if let Some(variables) = by_node.get(&key) {
            metadata.extend(variables.iter().map(|(name, value)| {
                (
                    format!("var.{name}"),
                    MetadataValue::Text(value.value.clone()),
                )
            }));
        }
        if let Some(children) = children {
            merge_effective_variables_into_nodes(children, by_node);
        }
    }
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

fn conflate_spindly_groups(nodes: &mut Vec<RenderNode>, inherited_settings: InheritedRailSettings) {
    for node in nodes {
        let RenderNode::Group(group) = node else {
            continue;
        };
        let child_settings = inherited_settings.with_node_metadata(&group.metadata);
        conflate_spindly_groups(&mut group.children, child_settings);
        while can_conflate_group_with_only_child(group, child_settings) {
            conflate_group_with_only_child(group);
            conflate_spindly_groups(&mut group.children, child_settings);
        }
    }
}

fn can_conflate_group_with_only_child(
    group: &RenderGroup,
    effective_settings: InheritedRailSettings,
) -> bool {
    if group.collapsed || effective_settings.child_layout != ChildLayoutMode::Cards {
        return false;
    }
    let mut child_groups = 0;
    for child in &group.children {
        match child {
            RenderNode::Tab(_) => return false,
            RenderNode::Group(child_group) => {
                if child_group.collapsed
                    || child_layout_from_variables(&child_group.metadata)
                        .is_some_and(|layout| layout != effective_settings.child_layout)
                {
                    return false;
                }
                child_groups += 1;
            }
        }
    }
    child_groups == 1
}

fn conflated_group_label(parent: &str, child: &str) -> String {
    format!("{parent}─{child}")
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
    let label = conflated_group_label(&group.label, &child.label);
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
    by_target: &HashMap<ResolvedMetadataTarget, RenderMetadata>,
    sources_by_target: &HashMap<ResolvedMetadataTarget, RenderMetadataSources>,
    identities_by_target: &HashMap<ResolvedMetadataTarget, RenderReachableIdentities>,
) {
    for node in nodes {
        match node {
            RenderNode::Tab(tab) => {
                let target = tab
                    .card
                    .entity
                    .clone()
                    .map(ResolvedMetadataTarget::Entity)
                    .unwrap_or(ResolvedMetadataTarget::Tab(tab.card.tab_id));
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
                let target = ResolvedMetadataTarget::Group(group.path.clone());
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
    if let Some(segment) = path.0.last() {
        metadata.insert(
            "group.key".to_owned(),
            MetadataValue::Text(segment.key.clone()),
        );
        metadata.insert("group.value".to_owned(), segment.value.clone());
        if let Some(label) = segment.label.as_ref() {
            metadata.insert(
                "group.segment.label".to_owned(),
                MetadataValue::Text(label.clone()),
            );
        }
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
        latent: false,
        materialize_request: None,
        latent_summary: None,
        meta_panel: None,
        entity: None,
        compact_only: false,
        form: "full".to_owned(),
    }
}

fn render_card_from_latent(latent: &LatentTab) -> RenderCard {
    let materialize_request = latent.materialize_request();
    let (marker, status_state, summary, materialize_state) = match latent.materialization {
        LatentMaterializationState::Opening => ("…", Some("opening"), None, Some("opening")),
        LatentMaterializationState::Ready => (
            if materialize_request.is_some() {
                "↗"
            } else {
                "○"
            },
            latent.status_state.as_deref(),
            latent.summary.as_deref(),
            None,
        ),
    };
    let name = latent.name.clone();
    let mut metadata = RenderMetadata::from([
        (
            "action.primary.target".to_owned(),
            MetadataValue::Text(latent.action_target.clone()),
        ),
        ("rail.tab.latent".to_owned(), MetadataValue::Bool(true)),
        (
            "zellij.tab.name".to_owned(),
            MetadataValue::Text(name.clone()),
        ),
        (
            "materialize.glyph".to_owned(),
            MetadataValue::Text(marker.to_owned()),
        ),
        (
            "entity.kind".to_owned(),
            MetadataValue::Text(latent.entity.kind.clone()),
        ),
        (
            "entity.id".to_owned(),
            MetadataValue::Text(latent.entity.id.clone()),
        ),
    ]);
    if let Some(materialize_state) = materialize_state {
        metadata.insert(
            "materialize.state".to_owned(),
            MetadataValue::Text(materialize_state.to_owned()),
        );
    }
    if let Some(state) = status_state {
        metadata.insert(
            "status.state".to_owned(),
            MetadataValue::Text(state.to_owned()),
        );
    }
    if let Some(summary) = summary {
        metadata.insert(
            "summary.text".to_owned(),
            MetadataValue::Text(summary.to_owned()),
        );
    }
    if let Some(source) = latent.source.as_ref() {
        metadata.insert("source".to_owned(), MetadataValue::Text(source.clone()));
    }
    if let Some(recipe) = latent.materialize_recipe.as_ref() {
        metadata.insert(
            "action.primary.recipe".to_owned(),
            MetadataValue::Text(recipe.clone()),
        );
    }
    let latent_summary = match (status_state, summary) {
        (Some(state), Some(summary)) => Some(format!("{state} · {summary}")),
        (Some(state), None) => Some(state.to_owned()),
        (None, Some(summary)) => Some(summary.to_owned()),
        (None, None) => None,
    };
    let status = status_state.map(|state| TabStatusSummary {
        priority: match state {
            "failed" => Priority::Error,
            "waiting" => Priority::Waiting,
            "opening" => Priority::Waiting,
            "active" => Priority::Info,
            _ => Priority::Idle,
        },
        title: state.to_owned(),
        detail: summary.map(str::to_owned),
        icon: Some(StatusIcon::Builtin(state.to_owned())),
        source_pane: PaneTarget::Plugin(0),
    });
    if let Some(status) = status.as_ref() {
        metadata.extend(status_metadata(status));
    }
    RenderCard {
        tab_id: 0,
        position: 0,
        name,
        active: false,
        pinned: false,
        status,
        metadata,
        metadata_sources: RenderMetadataSources::new(),
        reachable_identities: RenderReachableIdentities::new(),
        templates: latent.templates.clone(),
        latent: true,
        materialize_request,
        latent_summary,
        meta_panel: None,
        entity: Some(latent.entity.clone()),
        compact_only: false,
        form: "full".to_owned(),
    }
}

fn render_card_from_entity(entity: &andamento_core::DisplayEntity) -> RenderCard {
    RenderCard {
        tab_id: 0,
        position: 0,
        name: entity.label.clone(),
        active: false,
        pinned: false,
        status: None,
        metadata: display_entity_metadata(entity),
        metadata_sources: RenderMetadataSources::new(),
        reachable_identities: RenderReachableIdentities::new(),
        templates: entity.templates.clone(),
        latent: true,
        materialize_request: None,
        latent_summary: None,
        meta_panel: None,
        entity: Some(entity.entity.clone()),
        compact_only: entity.form == DISPLAY_FORM_COMPACT,
        form: entity.form.clone(),
    }
}

fn display_entity_metadata(entity: &andamento_core::DisplayEntity) -> RenderMetadata {
    let mut metadata = entity.metadata.clone();
    metadata.insert(
        "entity.kind".to_owned(),
        MetadataValue::Text(entity.entity.kind.clone()),
    );
    metadata.insert(
        "entity.id".to_owned(),
        MetadataValue::Text(entity.entity.id.clone()),
    );
    metadata
        .entry("display.label".to_owned())
        .or_insert_with(|| MetadataValue::Text(entity.label.clone()));
    metadata
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

fn visible_cells(cards: &[RenderCard], available_rows: usize) -> Vec<(usize, usize)> {
    if cards.is_empty() || available_rows < COMPACT_CELL_HEIGHT + 1 {
        return vec![];
    }
    let active_index = cards.iter().position(|card| card.active).unwrap_or(0);
    let cell_height = |card: &RenderCard| cell_height(card, true);
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

fn visible_boxes(cards: &[RenderCard], available_rows: usize) -> Vec<(usize, usize)> {
    if cards.is_empty() || available_rows < COMPACT_CELL_HEIGHT + 1 {
        return vec![];
    }
    let active_index = cards.iter().position(|card| card.active).unwrap_or(0);
    let box_height = |card: &RenderCard| cell_height(card, false).max(2);
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

fn cell_height(card: &RenderCard, joined_cell: bool) -> usize {
    let compact = if joined_cell {
        COMPACT_CELL_HEIGHT
    } else {
        COMPACT_CELL_HEIGHT + 1
    };
    let base = if card.form == DISPLAY_FORM_COMPACT {
        compact
    } else {
        ACTIVE_CELL_HEIGHT
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
        .map(|slot| {
            template_fields_from_resolved_slot(
                slot,
                TemplateConfigMatchContext {
                    slot: TemplateConfigSlot::TabStatus,
                    node_kind: TemplateConfigNodeKind::Tab,
                    metadata: &card.metadata,
                    collapsed: false,
                    collapsible: false,
                    active_tab_name: None,
                },
            )
        })
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
        slot: TemplateConfigSlot::TabStatus,
        node_kind: TemplateConfigNodeKind::Tab,
        metadata,
        collapsed: false,
        collapsible: false,
        active_tab_name: None,
    };
    if let Some(fields) = external_template_fields(template_catalog, context) {
        return fields;
    }
    template_fields_for(TemplateRenderContext {
        slot: TemplateConfigSlot::TabStatus,
        node_kind: TemplateConfigNodeKind::Tab,
        metadata,
        collapsed: false,
        collapsible: false,
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
    let fields = (if card.compact_only {
        card.templates.compact.as_ref()
    } else {
        card.templates.tab_title.as_ref()
    })
    .map(|slot| {
        template_fields_from_resolved_slot(
            slot,
            TemplateConfigMatchContext {
                slot: if card.compact_only {
                    TemplateConfigSlot::Compact
                } else {
                    TemplateConfigSlot::TabTitle
                },
                node_kind: if card.compact_only {
                    TemplateConfigNodeKind::Entity
                } else {
                    TemplateConfigNodeKind::Tab
                },
                metadata: &card.metadata,
                collapsed: false,
                collapsible: false,
                active_tab_name: None,
            },
        )
    })
    .unwrap_or_else(|| {
        tab_title_template_fields_with_template_catalog(&card.metadata, template_catalog)
    });
    let rendered = join_template_fields(&fields, true);
    if rendered.trim().is_empty() {
        format!("{} (tab)", card.name)
    } else {
        rendered
    }
}

fn card_chrome(card: &RenderCard, template_catalog: Option<&TemplateConfigCatalog>) -> ChromeSpec {
    let (resolved_slot, slot, node_kind) = if card.compact_only {
        (
            card.templates.compact.as_ref(),
            TemplateConfigSlot::Compact,
            TemplateConfigNodeKind::Entity,
        )
    } else {
        (
            card.templates.tab_title.as_ref(),
            TemplateConfigSlot::TabTitle,
            TemplateConfigNodeKind::Tab,
        )
    };
    if let Some(chrome) = resolved_slot
        .and_then(|slot| slot.render_ready.as_ref())
        .map(|render_ready| render_ready.chrome.clone())
    {
        return chrome;
    }
    resolved_render_template(
        template_catalog,
        TemplateRenderContext {
            slot,
            node_kind,
            metadata: &card.metadata,
            collapsed: false,
            collapsible: false,
            active_tab_name: None,
        },
    )
    .map(|template| template.chrome)
    .unwrap_or_default()
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
        slot: TemplateConfigSlot::TabTitle,
        node_kind: TemplateConfigNodeKind::Tab,
        metadata,
        collapsed: false,
        collapsible: false,
        active_tab_name: None,
    };
    if let Some(fields) = external_template_fields(template_catalog, context) {
        return fields;
    }
    template_fields_for(TemplateRenderContext {
        slot: TemplateConfigSlot::TabTitle,
        node_kind: TemplateConfigNodeKind::Tab,
        metadata,
        collapsed: false,
        collapsible: false,
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
        if let Some(summary) = card.latent_summary.as_ref() {
            let text_width = cols.saturating_sub(2);
            for (line, text) in lines
                .iter_mut()
                .zip(wrap_to_width(summary, text_width, body_rows))
            {
                *line = text;
            }
        }
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
    Title { active: bool, dimmed: bool },
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

    fn left_cell_is_available(&self, offset: usize) -> bool {
        let inner = self.inner_range();
        offset < inner.len() && self.cells[inner.start + offset].owner.is_none()
    }

    fn right_cell_is_available(&self, offset: usize) -> bool {
        let inner = self.inner_range();
        if offset >= inner.len() {
            return false;
        }
        self.cells[inner.end - 1 - offset].owner.is_none()
    }

    fn place_right(
        &mut self,
        offset: usize,
        glyph: char,
        hit: Option<(HitAction, BorderHitPayload)>,
        owner: &'static str,
    ) {
        let inner = self.inner_range();
        if offset >= inner.len() {
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
        self.title_with_dim(title, false);
    }

    fn title_with_dim(&mut self, title: &str, dimmed: bool) {
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
                CellStyle::Title { active, dimmed }
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
                materialize_request: None,
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
        CellStyle::Title { active, dimmed } => style_title_text(text, active, dimmed, theme),
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
    chrome: &ChromeSpec,
) {
    let dimmed = card_dimmed(card, chrome);
    if !chrome.boxed() {
        if let Some(slot) = lines.get_mut(row) {
            let show_inspect = width > 0 && !card.latent;
            let title_width = width.saturating_sub(usize::from(show_inspect));
            let title = pad_to_width(&truncate_to_width(title, title_width), title_width);
            let mut line = if dimmed {
                latent_tab_style(Style::new()).paint(title).to_string()
            } else {
                style_body_text(title, theme)
            };
            if show_inspect {
                let key = NodeKey::Tab(card.tab_id);
                line.push_str(&style_body_text(
                    inspect_node_glyph(inspected_node, &key).to_string(),
                    theme,
                ));
                hit_regions.push(HitRegion {
                    row_start: row,
                    row_end: row,
                    col_start: width - 1,
                    col_end: width - 1,
                    tab_id: card.tab_id,
                    tab_position: card.position,
                    group_path: None,
                    inspect_target: Some(key),
                    materialize_request: None,
                    action: HitAction::InspectNode,
                });
            }
            *slot = line;
        }
        return;
    }
    let mut border = BorderRow::new(
        width,
        BorderKind::Top {
            active: card.active,
            first_cell,
        },
    );
    if width >= 4 && !card.latent {
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
    if dimmed {
        border.title_with_dim(title, true);
    } else {
        border.title(title);
    }
    let (line, hits) = border.finish(row, theme);
    if let Some(slot) = lines.get_mut(row) {
        *slot = line;
    }
    hit_regions.extend(hits);
}

fn bottom_border_line(
    width: usize,
    active: bool,
    theme: Option<RenderTheme>,
    chrome: &ChromeSpec,
) -> String {
    if chrome.boxed() {
        BorderRow::new(width, BorderKind::Bottom { active }).into_line(theme)
    } else {
        blank(width)
    }
}

fn body_line(
    text: &str,
    width: usize,
    active: bool,
    dimmed: bool,
    theme: Option<RenderTheme>,
    chrome: &ChromeSpec,
) -> String {
    if !chrome.boxed() {
        let text = pad_to_width(&truncate_to_width(text, width), width);
        return if dimmed {
            latent_tab_style(Style::new()).paint(text).to_string()
        } else {
            style_body_text(text, theme)
        };
    }
    match width {
        0 => String::new(),
        1 => style_border_text("│".to_owned(), active, theme),
        _ => {
            let inner_width = width - 2;
            let text = truncate_to_width(text, inner_width);
            let body = if dimmed {
                let style = theme
                    .map(|theme| foreground_style(theme.body_foreground))
                    .unwrap_or_default();
                latent_tab_style(style)
                    .paint(pad_to_width(&text, inner_width))
                    .to_string()
            } else {
                style_body_text(pad_to_width(&text, inner_width), theme)
            };
            format!(
                "{}{}{}",
                style_border_text("│".to_owned(), active, theme),
                body,
                style_border_text("│".to_owned(), active, theme)
            )
        }
    }
}

fn card_dimmed(card: &RenderCard, chrome: &ChromeSpec) -> bool {
    chrome.dimmed(&card.metadata)
}

#[cfg(test)]
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
    let controls = vec![
        andamento_core::template_config::TemplateControlSpec {
            kind: andamento_core::template_config::TemplateControlKind::OpenConfig,
            variable: None,
            glyph: None,
        },
        andamento_core::template_config::TemplateControlSpec {
            kind: andamento_core::template_config::TemplateControlKind::ScrollDown,
            variable: None,
            glyph: None,
        },
        andamento_core::template_config::TemplateControlSpec {
            kind: andamento_core::template_config::TemplateControlKind::ScrollUp,
            variable: None,
            glyph: None,
        },
        andamento_core::template_config::TemplateControlSpec {
            kind: andamento_core::template_config::TemplateControlKind::InspectRoot,
            variable: None,
            glyph: None,
        },
    ];
    render_control_template(
        lines,
        hit_regions,
        row,
        width,
        theme,
        inspected_node,
        rail_can_scroll,
        &controls,
        &[],
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_control_template(
    lines: &mut [String],
    hit_regions: &mut Vec<HitRegion>,
    row: usize,
    width: usize,
    theme: Option<RenderTheme>,
    inspected_node: Option<&NodeKey>,
    rail_can_scroll: bool,
    controls: &[andamento_core::template_config::TemplateControlSpec],
    variables: &[andamento_core::template_config::TemplateVariableDefinition],
    values: Option<&BTreeMap<String, andamento_core::DisplayVariableValue>>,
) {
    if width == 0 {
        return;
    }
    let mut footer = BorderRow::new(width, BorderKind::Footer);
    let mut left_offset = 0;
    let mut right_offset = 0;
    for control in controls {
        use andamento_core::template_config::TemplateControlKind;
        let declared_glyph = control
            .glyph
            .as_deref()
            .and_then(|glyph| glyph.chars().next());
        match control.kind {
            TemplateControlKind::OpenConfig => {
                if !footer.left_cell_is_available(left_offset) {
                    continue;
                }
                footer.place_left(
                    left_offset,
                    declared_glyph.unwrap_or('⚙'),
                    Some((HitAction::OpenConfig, BorderHitPayload::none())),
                    "control_open_config",
                );
                left_offset += 1;
            }
            TemplateControlKind::DisplayVariable => {
                let Some(index) = control
                    .variable
                    .as_deref()
                    .and_then(|name| variables.iter().position(|variable| variable.name == name))
                else {
                    continue;
                };
                let variable = &variables[index];
                let Some(mut icon) = declared_glyph.or_else(|| variable.icon.chars().next()) else {
                    continue;
                };
                if matches!(
                    values.and_then(|values| values.get(&variable.name)),
                    Some(andamento_core::DisplayVariableValue::Bool(false))
                ) {
                    icon = '·';
                }
                if !footer.left_cell_is_available(left_offset) {
                    continue;
                }
                footer.place_left(
                    left_offset,
                    icon,
                    Some((HitAction::ToggleVariable(index), BorderHitPayload::none())),
                    "control_display_variable",
                );
                left_offset += 1;
            }
            TemplateControlKind::ScrollDown | TemplateControlKind::ScrollUp if rail_can_scroll => {
                if !footer.right_cell_is_available(right_offset) {
                    continue;
                }
                let (glyph, action, label) = if control.kind == TemplateControlKind::ScrollDown {
                    (
                        declared_glyph.unwrap_or('▼'),
                        HitAction::ScrollRailDown,
                        "control_scroll_down",
                    )
                } else {
                    (
                        declared_glyph.unwrap_or('▲'),
                        HitAction::ScrollRailUp,
                        "control_scroll_up",
                    )
                };
                footer.place_right(
                    right_offset,
                    glyph,
                    Some((action, BorderHitPayload::none())),
                    label,
                );
                right_offset += 1;
            }
            TemplateControlKind::InspectRoot => {
                if !footer.right_cell_is_available(right_offset) {
                    continue;
                }
                footer.place_right(
                    right_offset,
                    declared_glyph
                        .unwrap_or_else(|| inspect_node_glyph(inspected_node, &NodeKey::Root)),
                    Some((
                        HitAction::InspectNode,
                        BorderHitPayload {
                            inspect_target: Some(NodeKey::Root),
                            ..BorderHitPayload::none()
                        },
                    )),
                    "control_inspect_root",
                );
                right_offset += 1;
            }
            // Scroll controls are intentionally absent when the viewport already fits.
            TemplateControlKind::ScrollDown | TemplateControlKind::ScrollUp => {}
        }
    }
    let (line, hits) = footer.finish(row, theme);
    lines[row] = line;
    hit_regions.extend(hits);
}

fn group_header_line(
    metadata: &RenderMetadata,
    collapsed: bool,
    collapsible: bool,
    contains_active_tab: bool,
    active_tab_name: Option<&str>,
    resolved_slot: Option<&ResolvedTemplateSlot>,
    width: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
    ancestor_template_fields: &BTreeSet<ResolvedTemplateFieldSource>,
    niche_tabs: &[RenderTab],
    niche_child_groups: &[RenderGroup],
) -> RenderedTemplateLine {
    let (mut fields, text_style) = match resolved_slot {
        Some(slot) => {
            let fields = group_header_fields_from_resolved_slot(
                slot,
                metadata,
                collapsed,
                collapsible,
                active_tab_name,
            );
            if fields
                .iter()
                .any(|field| !template_field_value(field).trim().is_empty())
            {
                (fields, GroupHeaderTextStyle::Themed)
            } else if uses_bundled_group_header_fallback(metadata) {
                (
                    group_label_fallback_fields(metadata, collapsed, collapsible),
                    GroupHeaderTextStyle::Plain,
                )
            } else {
                (
                    group_identity_fallback_fields(metadata, collapsed, collapsible),
                    GroupHeaderTextStyle::Plain,
                )
            }
        }
        None => resolve_group_header_template_fields(
            metadata,
            collapsed,
            collapsible,
            active_tab_name,
            template_catalog,
        ),
    };
    let chrome = group_header_chrome(
        metadata,
        collapsed,
        collapsible,
        active_tab_name,
        resolved_slot,
        template_catalog,
    );
    if let Some((collapsed_glyph, expanded_glyph)) = chrome.toggle() {
        fields.insert(
            0,
            TemplateField::Required(
                if collapsible {
                    if collapsed {
                        collapsed_glyph
                    } else {
                        expanded_glyph
                    }
                } else {
                    " "
                }
                .to_owned(),
            ),
        );
    }
    let rendered_fields =
        render_template_fields_inline_with_suppression(&fields, width, ancestor_template_fields);
    let mut text = rendered_fields.text;
    let mut visible_width = text.width();
    let max_niche_width = width
        .saturating_sub(visible_width)
        .saturating_sub(" ─ ".width());
    let niche_projection = project_header_niche(
        niche_tabs,
        niche_child_groups,
        max_niche_width,
        theme,
        template_catalog,
    );
    let separator = header_niche_separator(&niche_projection.text);
    let niche_start = visible_width + separator.width();
    let niche_hits = niche_projection
        .hits
        .iter()
        .map(|hit| HeaderNicheHit {
            col_start: niche_start + hit.col_start,
            col_end: niche_start + hit.col_end,
            tab_id: hit.tab_id,
            tab_position: hit.tab_position,
            group_path: hit.group_path.clone(),
            inspect_target: hit.inspect_target.clone(),
            materialize_request: hit.materialize_request.clone(),
            action: hit.action,
        })
        .collect::<Vec<_>>();
    let mut prefix = text.clone();
    if niche_projection.visible_width > 0 {
        prefix.push_str(separator);
        text = prefix.clone();
        text.push_str(&niche_projection.text);
        visible_width += separator.width() + niche_projection.visible_width;
    }
    let remaining = width.saturating_sub(visible_width);
    let suffix = chrome.fill().map_or_else(String::new, |glyph| {
        if remaining >= 2 {
            format!(" {}", glyph.to_string().repeat(remaining - 1))
        } else if remaining == 1 {
            " ".to_owned()
        } else {
            String::new()
        }
    });
    let style_header = |text| match text_style {
        GroupHeaderTextStyle::Themed => style_group_header_text(text, contains_active_tab, theme),
        GroupHeaderTextStyle::Plain => text,
    };
    let text = if niche_projection.visible_width > 0 {
        let mut styled = style_header(prefix);
        styled.push_str(&niche_projection.text);
        styled.push_str(&style_header(suffix));
        styled
    } else {
        style_header(format!("{text}{suffix}"))
    };
    RenderedTemplateLine {
        text,
        visible_sources: rendered_fields.visible_sources,
        niche_hits,
        niche_consumption: niche_projection.consumption,
    }
}

fn group_header_chrome(
    metadata: &RenderMetadata,
    collapsed: bool,
    collapsible: bool,
    active_tab_name: Option<&str>,
    resolved_slot: Option<&ResolvedTemplateSlot>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> ChromeSpec {
    if let Some(chrome) = resolved_slot
        .and_then(|slot| slot.render_ready.as_ref())
        .map(|render_ready| render_ready.chrome.clone())
    {
        return chrome;
    }
    resolved_render_template(
        template_catalog,
        TemplateRenderContext {
            slot: TemplateConfigSlot::GroupHeader,
            node_kind: TemplateConfigNodeKind::Group,
            metadata,
            collapsed,
            collapsible,
            active_tab_name,
        },
    )
    .map(|template| template.chrome)
    .unwrap_or_default()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HeaderNicheProjection {
    text: String,
    visible_width: usize,
    hits: Vec<HeaderNicheHit>,
    consumption: HeaderNicheConsumption,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeaderNicheHit {
    col_start: usize,
    col_end: usize,
    tab_id: u64,
    tab_position: usize,
    group_path: Option<GroupPath>,
    inspect_target: Option<NodeKey>,
    materialize_request: Option<MaterializeLatentRequest>,
    action: HitAction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HeaderNicheConsumption {
    direct_tabs: usize,
    child_group: Option<Box<HeaderNicheGroupConsumption>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeaderNicheGroupConsumption {
    child_group_index: usize,
    child_consumption: HeaderNicheConsumption,
}

fn project_header_niche(
    tabs: &[RenderTab],
    child_groups: &[RenderGroup],
    width: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> HeaderNicheProjection {
    if !tabs.is_empty() {
        return project_direct_tab_header_niche(tabs, width, theme, template_catalog);
    }
    project_child_group_header_niche(child_groups, width, theme, template_catalog)
}

fn project_direct_tab_header_niche(
    tabs: &[RenderTab],
    width: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> HeaderNicheProjection {
    let mut text = String::new();
    let mut visible_width = 0usize;
    let mut hits = vec![];
    let mut consumption = HeaderNicheConsumption::default();
    for (index, tab) in tabs.iter().enumerate() {
        let label = tab_title_with_template_catalog(&tab.card, template_catalog);
        let segment_item = SegmentItem {
            label,
            active: tab.card.active,
        };
        let segment_width = strip_segment_width(&segment_item.label);
        if segment_width > width.saturating_sub(visible_width) {
            break;
        }
        let start = visible_width;
        let (segment, rendered_width) = render_strip_segment(&segment_item, segment_width, theme);
        if rendered_width == 0 || rendered_width > width.saturating_sub(visible_width) {
            break;
        }
        text.push_str(&segment);
        visible_width += rendered_width;
        if !tab.card.latent || tab.card.materialize_request.is_some() {
            hits.push(HeaderNicheHit {
                col_start: start,
                col_end: visible_width.saturating_sub(1),
                tab_id: tab.card.tab_id,
                tab_position: tab.card.position,
                group_path: tab.parent_path.clone(),
                inspect_target: None,
                materialize_request: tab.card.materialize_request.clone(),
                action: if tab.card.latent {
                    HitAction::Materialize
                } else {
                    HitAction::SwitchTab
                },
            });
        }
        consumption.direct_tabs = index + 1;
    }
    let leading_width = width.saturating_sub(visible_width);
    if visible_width > 0 && leading_width > 0 {
        let leading = if leading_width == 1 {
            " ".to_owned()
        } else {
            format!("{} ", "─".repeat(leading_width - 1))
        };
        text = format!("{leading}{text}");
        for hit in &mut hits {
            hit.col_start += leading_width;
            hit.col_end += leading_width;
        }
        visible_width += leading_width;
    }
    HeaderNicheProjection {
        text,
        visible_width,
        hits,
        consumption,
    }
}

fn project_child_group_header_niche(
    child_groups: &[RenderGroup],
    width: usize,
    theme: Option<RenderTheme>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> HeaderNicheProjection {
    let candidate_group_count = child_groups.len();
    let Some(group) = child_groups.first() else {
        return HeaderNicheProjection::default();
    };
    let group_label = group.label.clone();
    let group_width = group_label.width();
    if group_width == 0 || group_width > width {
        return HeaderNicheProjection::default();
    }

    let mut text = group_label;
    let mut visible_width = group_width;
    let mut hits = vec![];
    let mut child_consumption = HeaderNicheConsumption::default();

    let (direct_tabs, child_group_nodes) = direct_tabs_and_child_groups(&group.children);
    let child_groups = child_group_nodes
        .iter()
        .filter_map(|node| match node {
            RenderNode::Group(group) => Some(group.clone()),
            RenderNode::Tab(_) => None,
        })
        .collect::<Vec<_>>();
    let child_width = width
        .saturating_sub(visible_width)
        .saturating_sub(" ─ ".width());
    let child_projection = project_header_niche(
        &direct_tabs,
        &child_groups,
        child_width,
        theme,
        template_catalog,
    );
    if child_projection.visible_width > 0 {
        if candidate_group_count == 1
            && !children_after_niche_consumption(&group.children, &child_projection.consumption)
                .is_empty()
        {
            return HeaderNicheProjection::default();
        }
        let separator = header_niche_separator(&child_projection.text);
        let child_start = visible_width + separator.width();
        text.push_str(separator);
        text.push_str(&child_projection.text);
        hits.extend(child_projection.hits.into_iter().map(|hit| HeaderNicheHit {
            col_start: child_start + hit.col_start,
            col_end: child_start + hit.col_end,
            tab_id: hit.tab_id,
            tab_position: hit.tab_position,
            group_path: hit.group_path,
            inspect_target: hit.inspect_target,
            materialize_request: hit.materialize_request,
            action: hit.action,
        }));
        visible_width += separator.width() + child_projection.visible_width;
        child_consumption = child_projection.consumption;
    } else if !group.children.is_empty() {
        return HeaderNicheProjection::default();
    }

    HeaderNicheProjection {
        text,
        visible_width,
        hits,
        consumption: HeaderNicheConsumption {
            direct_tabs: 0,
            child_group: Some(Box::new(HeaderNicheGroupConsumption {
                child_group_index: 0,
                child_consumption,
            })),
        },
    }
}

fn header_niche_separator(niche_text: &str) -> &'static str {
    if niche_text.starts_with('─') {
        " "
    } else {
        " ─ "
    }
}

fn render_template_fields_inline_with_suppression(
    fields: &[TemplateField],
    width: usize,
    suppressed_sources: &BTreeSet<ResolvedTemplateFieldSource>,
) -> RenderedTemplateFields {
    let inline_items = fields
        .iter()
        .enumerate()
        .filter(|(_, field)| {
            template_field_source(field).is_none_or(|source| !suppressed_sources.contains(source))
        })
        .map(|(index, field)| inline_item_from_template_field(index, field))
        .collect::<Vec<_>>();
    let placed = InlineRun::new(inline_items).layout(width);
    let visible_sources = placed
        .items
        .iter()
        .filter_map(|item| {
            item.id
                .strip_prefix("field:")
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| fields.get(index))
                .and_then(template_field_source)
                .cloned()
        })
        .collect();
    RenderedTemplateFields {
        text: placed.text,
        visible_sources,
    }
}

fn inline_item_from_template_field(index: usize, field: &TemplateField) -> InlineItem {
    let item = InlineItem::text(format!("field:{index}"), template_field_value(field));
    match field {
        TemplateField::Required(_) => item.required(),
        TemplateField::Optional(_) => item.optional(),
        TemplateField::Priority(_) => item.priority(100),
        TemplateField::Prioritized { priority, .. } => item.priority(*priority),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedTemplateLine {
    text: String,
    visible_sources: BTreeSet<ResolvedTemplateFieldSource>,
    niche_hits: Vec<HeaderNicheHit>,
    niche_consumption: HeaderNicheConsumption,
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
enum GroupHeaderTextStyle {
    Themed,
    Plain,
}

#[derive(Debug, Clone, Copy)]
struct TemplateRenderContext<'a> {
    slot: TemplateConfigSlot,
    node_kind: TemplateConfigNodeKind,
    metadata: &'a RenderMetadata,
    collapsed: bool,
    collapsible: bool,
    active_tab_name: Option<&'a str>,
}

fn template_fields_for(context: TemplateRenderContext<'_>) -> Vec<TemplateField> {
    resolved_render_template(None, context)
        .map(|template| template.fields)
        .unwrap_or_default()
}

fn external_template_fields(
    catalog: Option<&TemplateConfigCatalog>,
    context: TemplateRenderContext<'_>,
) -> Option<Vec<TemplateField>> {
    resolved_render_template(catalog, context).map(|template| template.fields)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedRenderTemplate {
    name: String,
    fields: Vec<TemplateField>,
    chrome: ChromeSpec,
}

fn bundled_template_catalog() -> &'static TemplateConfigCatalog {
    static CATALOG: OnceLock<TemplateConfigCatalog> = OnceLock::new();
    CATALOG.get_or_init(TemplateConfigCatalog::default)
}

fn template_match_context<'a>(
    context: TemplateRenderContext<'a>,
) -> TemplateConfigMatchContext<'a> {
    TemplateConfigMatchContext {
        slot: context.slot,
        node_kind: context.node_kind,
        metadata: context.metadata,
        collapsed: context.collapsed,
        collapsible: true,
        active_tab_name: context.active_tab_name,
    }
}

fn resolved_render_template(
    catalog: Option<&TemplateConfigCatalog>,
    context: TemplateRenderContext<'_>,
) -> Option<ResolvedRenderTemplate> {
    if let Some(template) =
        catalog.and_then(|catalog| render_template_from_catalog(catalog, context))
    {
        return Some(template);
    }
    render_template_from_catalog(bundled_template_catalog(), context)
}

fn render_template_from_catalog(
    catalog: &TemplateConfigCatalog,
    context: TemplateRenderContext<'_>,
) -> Option<ResolvedRenderTemplate> {
    let match_context = template_match_context(context);
    let resolved = catalog.resolve(match_context).ok()??;
    let render_context = if resolved.is_bundled {
        TemplateConfigMatchContext {
            collapsed: context.collapsed && context.collapsible,
            collapsible: context.collapsible,
            ..match_context
        }
    } else {
        match_context
    };
    let render_ready = resolved.render_ready();
    let fields = render_ready
        .render_fields(render_context)
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
    Some(ResolvedRenderTemplate {
        name: resolved.name.clone(),
        fields,
        chrome: render_ready.chrome,
    })
}

#[cfg(test)]
fn group_header_template_fields(
    metadata: &RenderMetadata,
    collapsed: bool,
    active_tab_name: Option<&str>,
) -> Vec<TemplateField> {
    group_header_template_fields_with_template_catalog(
        metadata,
        collapsed,
        true,
        active_tab_name,
        None,
    )
}

#[cfg(test)]
fn group_header_template_fields_with_template_catalog(
    metadata: &RenderMetadata,
    collapsed: bool,
    collapsible: bool,
    active_tab_name: Option<&str>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> Vec<TemplateField> {
    resolve_group_header_template_fields(
        metadata,
        collapsed,
        collapsible,
        active_tab_name,
        template_catalog,
    )
    .0
}

fn resolve_group_header_template_fields(
    metadata: &RenderMetadata,
    collapsed: bool,
    collapsible: bool,
    active_tab_name: Option<&str>,
    template_catalog: Option<&TemplateConfigCatalog>,
) -> (Vec<TemplateField>, GroupHeaderTextStyle) {
    let context = TemplateRenderContext {
        slot: TemplateConfigSlot::GroupHeader,
        node_kind: TemplateConfigNodeKind::Group,
        metadata,
        collapsed,
        collapsible,
        active_tab_name,
    };
    let Some(template) = resolved_render_template(template_catalog, context) else {
        return (
            group_identity_fallback_fields(metadata, collapsed, collapsible),
            GroupHeaderTextStyle::Plain,
        );
    };
    let text_style = if template.name == "flotilla/group-header/fallback" {
        GroupHeaderTextStyle::Plain
    } else {
        GroupHeaderTextStyle::Themed
    };
    (template.fields, text_style)
}

fn group_identity_fallback_fields(
    metadata: &RenderMetadata,
    _collapsed: bool,
    _collapsible: bool,
) -> Vec<TemplateField> {
    let mut fields = Vec::new();
    if let (Some(key), Some(value)) = (
        metadata_text(metadata, "group.key"),
        metadata_display_value(metadata, "group.value"),
    ) {
        fields.push(TemplateField::Required(format!("{key}:")));
        fields.push(TemplateField::Required(format!("{value} (group)")));
        return fields;
    }
    let label = metadata_text(metadata, "group.label")
        .filter(|label| !label.trim().is_empty())
        .unwrap_or("group");
    fields.push(TemplateField::Required(format!("{label} (group)")));
    fields
}

fn uses_bundled_group_header_fallback(metadata: &RenderMetadata) -> bool {
    let context = TemplateRenderContext {
        slot: TemplateConfigSlot::GroupHeader,
        node_kind: TemplateConfigNodeKind::Group,
        metadata,
        collapsed: false,
        collapsible: true,
        active_tab_name: None,
    };
    matches!(
        bundled_template_catalog().resolve(template_match_context(context)),
        Ok(Some(resolved)) if resolved.name == "group/full"
    )
}

fn group_label_fallback_fields(
    metadata: &RenderMetadata,
    _collapsed: bool,
    _collapsible: bool,
) -> Vec<TemplateField> {
    let mut fields = Vec::new();
    let label = metadata_text(metadata, "group.label")
        .filter(|label| !label.trim().is_empty())
        .unwrap_or("group");
    fields.push(TemplateField::Required(format!("{label} (group)")));
    fields
}

fn group_header_fields_from_resolved_slot(
    slot: &ResolvedTemplateSlot,
    metadata: &RenderMetadata,
    collapsed: bool,
    collapsible: bool,
    active_tab_name: Option<&str>,
) -> Vec<TemplateField> {
    template_fields_from_resolved_slot(
        slot,
        TemplateConfigMatchContext {
            slot: TemplateConfigSlot::GroupHeader,
            node_kind: TemplateConfigNodeKind::Group,
            metadata,
            collapsed,
            collapsible,
            active_tab_name,
        },
    )
}

fn render_ready_fields(
    render_ready: &TemplateConfigRenderReady,
    context: TemplateConfigMatchContext<'_>,
) -> Vec<TemplateField> {
    render_ready
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
        .collect()
}

fn template_fields_from_resolved_slot(
    slot: &ResolvedTemplateSlot,
    context: TemplateConfigMatchContext<'_>,
) -> Vec<TemplateField> {
    slot.render_ready
        .as_ref()
        .map(|render_ready| render_ready_fields(render_ready, context))
        .unwrap_or_else(|| {
            slot.fields
                .iter()
                .map(|field| TemplateField::Prioritized {
                    value: field.text.clone(),
                    priority: field.priority,
                    source: field.source.clone(),
                })
                .collect()
        })
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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
    use andamento_core::{
        DisplayEntity, EntityRef, GroupPath, GroupSegment, LatentTab, MetadataEntry,
        MetadataTriState, MetadataValue, PaneTarget, RailConfig, RailRow, RailStructure,
        ResolvedMetadata, ResolvedMetadataTarget, SortMode, StatusIcon,
    };
    use andamento_core::{PLACEMENT_LOOP_BINDING_KEY, PLACEMENT_LOOP_TIER_KEY};

    #[test]
    fn real_presentation_uses_shared_tiers_and_normal_terminal_clipping() {
        use andamento_core::state::ControllerState;
        let config = include_str!("../../andamento-core/tests/fixtures/abbreviation.kdl")
            .replace("kind=\"vessel\"", "kind=\"vessel\" tier=\"short\"");
        for (medium, expected) in [(Some("worker"), "  worker"), (None, "  grouping-...")] {
            let mut state = ControllerState::default();
            state.set_grouping_catalog(None);
            state.set_template_catalog(Some(TemplateConfigCatalog::with_bundled_defaults(
                andamento_core::template_config::parse_template_config_kdl(&config).unwrap(),
            )));
            let mut patch: andamento_core::MetadataPatch = serde_json::from_str(r#"{"target":{"kind":"entity","value":{"kind":"vessel","id":"worker"}},"source_id":"test","set":{"flotilla.vessel":{"value":{"type":"text","value":"worker"}},"display.label":{"value":{"type":"text","value":"grouping-live-session"}}},"unset":[]}"#).unwrap();
            if let Some(medium) = medium {
                patch.set.insert(
                    "display.label.medium".into(),
                    andamento_core::MetadataValueUpdate {
                        value: MetadataValue::Text(medium.into()),
                        ttl_ms: None,
                        precedence: None,
                        ordinal: None,
                    },
                );
            }
            state.apply_metadata_patch(patch);
            let model = state.view_model();
            let region = &model.surface_regions[0];
            let entity = &region.entities[0];
            let node = model
                .presentation
                .as_ref()
                .unwrap()
                .node(entity.placement.as_ref().unwrap())
                .unwrap();
            assert_eq!(node.label, "grouping-live-session");
            assert_eq!(node.detail.text(), "grouping-live-session");
            assert_eq!(
                region_entity_line(entity, &region.definition, None, 14, None, Some(&model))
                    .trim_end(),
                expected
            );
        }
    }

    #[test]
    fn abbreviation_tier_uses_loop_variable_and_falls_back_longer() {
        let mut metadata = BTreeMap::from([
            (
                PLACEMENT_LOOP_BINDING_KEY.to_owned(),
                MetadataValue::Text("issue".to_owned()),
            ),
            (
                "var.issue.tier".to_owned(),
                MetadataValue::Text("short".to_owned()),
            ),
            (
                "display.label".to_owned(),
                MetadataValue::Text("grouping-live-session".to_owned()),
            ),
            (
                "display.label.medium".to_owned(),
                MetadataValue::Text("grouping-session".to_owned()),
            ),
        ]);

        apply_declared_abbreviation(&mut metadata, 40);

        assert_eq!(
            metadata.get("display.label"),
            Some(&MetadataValue::Text("grouping-session".to_owned())),
            "a missing short form falls back to the next longer producer form"
        );
    }

    #[test]
    fn abbreviation_tier_middle_elides_only_the_degraded_full_fallback() {
        let mut metadata = BTreeMap::from([
            (
                PLACEMENT_LOOP_BINDING_KEY.to_owned(),
                MetadataValue::Text("issue".to_owned()),
            ),
            (
                PLACEMENT_LOOP_TIER_KEY.to_owned(),
                MetadataValue::Text("short".to_owned()),
            ),
            (
                "display.label".to_owned(),
                MetadataValue::Text("grouping-live-session".to_owned()),
            ),
        ]);

        apply_declared_abbreviation(&mut metadata, 12);

        assert_eq!(
            metadata.get("display.label"),
            Some(&MetadataValue::Text("groupi…ssion".to_owned()))
        );
    }

    fn set_child_layout_variable(model: &mut ControllerViewModel, node: NodeKey, value: &str) {
        model
            .template_config
            .effective_variables
            .push(andamento_core::EffectiveNodeVariables {
                declarations: vec![],
                node: node.clone(),
                values: BTreeMap::from([(
                    "child-layout".to_owned(),
                    andamento_core::EffectiveVariableValue {
                        value: value.to_owned(),
                        provenance: andamento_core::VariableSetterProvenance {
                            setter: "test template".to_owned(),
                            ancestor: node,
                            origin: andamento_core::template_config::TemplateConfigOrigin {
                                layer:
                                    andamento_core::template_config::TemplateConfigLayerKind::User,
                                membership: None,
                                source: "test.kdl".to_owned(),
                            },
                        },
                        overridden: vec![],
                    },
                )]),
            });
    }

    fn local_tab(tab_id: u64, position: usize, active: bool) -> LocalTab {
        LocalTab {
            tab_id,
            position,
            name: format!("tab-{tab_id}"),
            active,
        }
    }

    #[test]
    fn absolute_viewport_base_clamps_to_content_bounds() {
        assert_eq!(absolute_viewport_start(-4, 12), 0);
        assert_eq!(absolute_viewport_start(7, 12), 7);
        assert_eq!(absolute_viewport_start(40, 12), 12);
    }

    #[test]
    fn ensure_visible_moves_only_far_enough_to_reveal_target() {
        assert_eq!(ensure_visible_start(10, 5, 7, 6, 30), 5);
        assert_eq!(ensure_visible_start(10, 18, 20, 6, 30), 15);
        assert_eq!(ensure_visible_start(10, 9, 10, 6, 30), 10);
        assert_eq!(ensure_visible_start(10, 15, 17, 6, 30), 10);
    }

    #[test]
    fn active_and_status_rows_do_not_move_the_absolute_viewport() {
        let mut lines = vec![String::new(); 3];
        let mut visible_hits = vec![];
        let mut visible_cards = vec![];
        let active_hit = HitRegion {
            row_start: 8,
            row_end: 10,
            col_start: 0,
            col_end: 9,
            tab_id: 2,
            tab_position: 1,
            group_path: None,
            inspect_target: None,
            materialize_request: None,
            action: HitAction::SwitchTab,
        };
        let status_card = VisibleCard {
            tab_id: 1,
            tab_position: 0,
            row_start: 0,
            status_row: Some(1),
            status_icon_rect: None,
            status_priority: Some(Priority::Waiting),
            status_icon: None,
        };

        let viewport = copy_visible_buffer(
            &mut lines,
            &mut visible_hits,
            &mut visible_cards,
            (0..12).map(|row| format!("row {row}")).collect(),
            vec![active_hit],
            vec![status_card],
            Some(NodeKey::Tab(2)),
            3,
            4,
            false,
        );

        assert_eq!(viewport.visible_start, 4);
        assert_eq!(viewport.ensure_visible_offset, None);
        assert!(!viewport.ensure_active_resolved);
    }

    #[test]
    fn activation_ensure_visible_requests_a_minimal_absolute_offset() {
        let mut lines = vec![String::new(); 4];
        let mut visible_hits = vec![];
        let mut visible_cards = vec![];
        let active_hit = HitRegion {
            row_start: 10,
            row_end: 12,
            col_start: 0,
            col_end: 9,
            tab_id: 2,
            tab_position: 1,
            group_path: None,
            inspect_target: None,
            materialize_request: None,
            action: HitAction::SwitchTab,
        };

        let viewport = copy_visible_buffer(
            &mut lines,
            &mut visible_hits,
            &mut visible_cards,
            (0..20).map(|row| format!("row {row}")).collect(),
            vec![active_hit],
            vec![],
            Some(NodeKey::Tab(2)),
            4,
            3,
            true,
        );

        assert_eq!(viewport.visible_start, 9);
        assert_eq!(viewport.ensure_visible_offset, Some(9));
        assert!(viewport.ensure_active_resolved);
    }

    #[test]
    fn collapsed_active_tab_ensures_its_visible_group_header() {
        let model = grouped_model();
        let group_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("expected group header"),
        };
        let nodes = nodes_to_render(Some(&model), &[], std::slice::from_ref(&group_path), None);
        let ensure_target = active_ensure_target(&nodes);
        assert_eq!(ensure_target, Some(NodeKey::Group(group_path.clone())));

        let mut lines = vec![String::new(); 4];
        let mut visible_hits = vec![];
        let mut visible_cards = vec![];
        let header_hit = HitRegion {
            row_start: 10,
            row_end: 10,
            col_start: 0,
            col_end: 9,
            tab_id: 0,
            tab_position: 0,
            group_path: Some(group_path),
            inspect_target: None,
            materialize_request: None,
            action: HitAction::ToggleGroup,
        };
        let viewport = copy_visible_buffer(
            &mut lines,
            &mut visible_hits,
            &mut visible_cards,
            (0..20).map(|row| format!("row {row}")).collect(),
            vec![header_hit],
            vec![],
            ensure_target,
            4,
            3,
            true,
        );

        assert_eq!(viewport.visible_start, 7);
        assert_eq!(viewport.ensure_visible_offset, Some(7));
        assert!(viewport.ensure_active_resolved);
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

    fn latent_model(recipe: Option<&str>) -> ControllerViewModel {
        let path = GroupPath(vec![GroupSegment {
            key: "flotilla.convoy".to_owned(),
            value: MetadataValue::Text("dev/latent-tabs".to_owned()),
            label: Some("latent tabs".to_owned()),
        }]);
        let latent = LatentTab {
            entity: andamento_core::EntityRef {
                kind: "convoy".to_owned(),
                id: "dev/latent-tabs@fleet".to_owned(),
            },
            action_target: "flotilla:convoys/dev/latent-tabs".to_owned(),
            path: path.clone(),
            name: "latent tabs".to_owned(),
            materialization: LatentMaterializationState::Ready,
            status_state: Some("waiting".to_owned()),
            summary: Some("1 vessel ready".to_owned()),
            source: Some("flotilla".to_owned()),
            materialize_recipe: recipe.map(str::to_owned),
            checkout_path: Some("/work/andamento".to_owned()),
            templates: ResolvedTemplateSlots::default(),
        };
        ControllerViewModel {
            sort_mode: SortMode::Position,
            config: RailConfig::default(),
            template_config: andamento_core::TemplateConfigDiagnostics::default(),
            tabs: vec![],
            rows: vec![
                RailRow::GroupHeader {
                    group_id: "convoy".to_owned(),
                    path: path.clone(),
                    label: "latent tabs".to_owned(),
                    full_label: "latent tabs".to_owned(),
                    tab_count: 1,
                    templates: ResolvedTemplateSlots::default(),
                },
                RailRow::Latent {
                    latent,
                    indent: 2,
                    parent_path: Some(path),
                },
            ],
            resolved_metadata: vec![],
            observed_identities: vec![],
            grouping_diagnostics: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
            collapsed_groups: vec![],
            collapsed_placements: vec![],
            display_variables: vec![],
            display_variable_values: BTreeMap::new(),
            surface_regions: vec![],
            presentation: None,
        }
    }

    #[test]
    fn openable_latent_tab_renders_metadata_and_materialize_hit() {
        let rendered = render_lines_with_theme(
            Some(&latent_model(Some("flotilla attach latent-tabs"))),
            &[],
            12,
            48,
            true,
            Some(test_theme()),
        );

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("↗ latent tabs")));
        assert!(
            !rendered.lines.iter().any(|line| line.contains("[flotilla]")),
            "the source fact must not render as a suffix badge: names are variable width, so anything tightly bound after them is visually unmoored"
        );
        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("\u{1b}[2;38;5;7mwaiting")));
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains("waiting: 1 vessel ready")),
            "{:?}",
            rendered.lines
        );
        assert!(rendered.hit_regions.iter().any(|hit| {
            let Some(request) = hit.materialize_request.as_ref() else {
                return false;
            };
            hit.action == HitAction::Materialize
                && request.action_target == "flotilla:convoys/dev/latent-tabs"
                && request.name == "latent tabs"
                && request.recipe == "flotilla attach latent-tabs"
                && request.path.0[0].key == "flotilla.convoy"
                && request.path.0[0].value == MetadataValue::Text("dev/latent-tabs".to_owned())
        }));
    }

    #[test]
    fn recipe_less_latent_tab_has_no_open_affordance_or_materialize_hit() {
        let rendered = render_lines(Some(&latent_model(None)), &[], 12, 48, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("○ latent tabs")));
        assert!(!rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == HitAction::Materialize));
    }

    #[test]
    fn opening_latent_stays_visible_without_a_duplicate_materialize_hit() {
        let mut model = latent_model(Some("flotilla attach latent-tabs"));
        let RailRow::Latent { latent, .. } = &mut model.rows[1] else {
            panic!("expected latent row");
        };
        latent.materialization = LatentMaterializationState::Opening;

        let rendered = render_lines(Some(&model), &[], 12, 48, true);

        assert!(rendered
            .lines
            .iter()
            .any(|line| line.contains("… latent tabs")));
        assert!(rendered.lines.iter().any(|line| line.contains("opening")));
        assert!(!rendered
            .hit_regions
            .iter()
            .any(|hit| hit.action == HitAction::Materialize));
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
            template_config: andamento_core::TemplateConfigDiagnostics::default(),
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
            grouping_diagnostics: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
            collapsed_groups: vec![],
            collapsed_placements: vec![],
            display_variables: vec![],
            display_variable_values: BTreeMap::new(),
            surface_regions: vec![],
            presentation: None,
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
                ..RailConfig::default()
            },
            template_config: andamento_core::TemplateConfigDiagnostics::default(),
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
            grouping_diagnostics: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
            collapsed_groups: vec![],
            collapsed_placements: vec![],
            display_variables: vec![],
            display_variable_values: BTreeMap::new(),
            surface_regions: vec![],
            presentation: None,
        }
    }

    fn nested_group_model() -> ControllerViewModel {
        let mut model = ControllerViewModel {
            sort_mode: SortMode::PinnedFirst,
            config: RailConfig {
                structure: RailStructure::JoinedCells,
                segment_between_color: None,
            },
            template_config: andamento_core::TemplateConfigDiagnostics::default(),
            tabs: vec![],
            rows: vec![],
            resolved_metadata: vec![],
            observed_identities: vec![],
            grouping_diagnostics: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
            collapsed_groups: vec![],
            collapsed_placements: vec![],
            display_variables: vec![],
            display_variable_values: BTreeMap::new(),
            surface_regions: vec![],
            presentation: None,
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
                structure: RailStructure::JoinedCells,
                segment_between_color: None,
            },
            template_config: andamento_core::TemplateConfigDiagnostics::default(),
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
            grouping_diagnostics: vec![],
            metadata_controls: MetadataControls::default(),
            inspected_node: None,
            collapsed_groups: vec![],
            collapsed_placements: vec![],
            display_variables: vec![],
            display_variable_values: BTreeMap::new(),
            surface_regions: vec![],
            presentation: None,
        }
    }

    fn flat_rows_model() -> ControllerViewModel {
        let mut model = model();
        model.config.structure = RailStructure::JoinedCells;
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
        let rendered = render_lines(Some(&flat_rows_model()), &[], 9, 24, true);

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
    fn configured_regions_drive_order_attention_promotion_and_tree_form() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            region "attention" source="attention" root-template="region/attention" form="detail" attention-key="status.attention"
            region "tree" source="tree" root-template="region/tree" form="compact" pinned=true
            template "region/attention" slot="compact" node-kind="entity" {
              field "label" source="literal" value="ATTENTION"
            }
            template "region/tree" slot="compact" node-kind="entity" {
              field "label" source="literal" value="TREE"
            }
            template "issue/detail" slot="detail" node-kind="entity" {
              field "label" source="metadata-text" key="display.label"
              field "summary" source="metadata-text" key="summary.text"
            }
            template "issue/compact" slot="compact" node-kind="entity" {
              field "label" source="metadata-text" key="display.label"
            }
            "#,
        )
        .expect("region config");
        let catalog = TemplateConfigCatalog::from_config(config);
        let mut model = model();
        model.tabs.clear();
        model.rows = vec![RailRow::Entity {
            entity: DisplayEntity {
                placement: None,
                placement_layout: None,
                entity: EntityRef {
                    kind: "issue".to_owned(),
                    id: "1060".to_owned(),
                },
                label: "Issue 1060".to_owned(),
                form: "full".to_owned(),
                metadata: BTreeMap::from([
                    (
                        "entity.kind".to_owned(),
                        MetadataValue::Text("issue".to_owned()),
                    ),
                    (
                        "display.label".to_owned(),
                        MetadataValue::Text("Issue 1060".to_owned()),
                    ),
                    (
                        "summary.text".to_owned(),
                        MetadataValue::Text("region stack".to_owned()),
                    ),
                    ("status.attention".to_owned(), MetadataValue::Bool(true)),
                ]),
                templates: ResolvedTemplateSlots::default(),
                children: vec![],
            },
            indent: 0,
            parent_path: None,
        }];

        let rendered =
            render_lines_with_template_catalog(Some(&model), &[], 8, 40, true, Some(&catalog));

        assert!(
            rendered.lines[0].contains("ATTENTION"),
            "{:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[1].contains("Issue 1060 region stack"),
            "{:?}",
            rendered.lines
        );
        assert!(rendered.lines[2].contains("TREE"), "{:?}", rendered.lines);
        assert!(
            rendered.lines[3..]
                .iter()
                .any(|line| line.contains("Issue 1060") && !line.contains("region stack")),
            "tree should use its compact form: {:?}",
            rendered.lines
        );
        assert!(rendered.hit_regions.iter().any(|hit| {
            hit.row_start == 1
                && hit.action == HitAction::ActivateEntity
                && hit.inspect_target
                    == Some(NodeKey::Entity(EntityRef {
                        kind: "issue".to_owned(),
                        id: "1060".to_owned(),
                    }))
        }));
    }

    #[test]
    fn bundled_tree_form_promotes_active_and_pinned_tabs_to_full() {
        let catalog = TemplateConfigCatalog::default();
        let tree = catalog
            .regions()
            .iter()
            .find(|region| region.source == SurfaceRegionSource::Tree)
            .expect("bundled tree region");

        let inactive = RenderMetadata::from([
            ("zellij.tab.active".to_owned(), MetadataValue::Bool(false)),
            ("rail.tab.pinned".to_owned(), MetadataValue::Bool(false)),
        ]);
        let active = RenderMetadata::from([
            ("zellij.tab.active".to_owned(), MetadataValue::Bool(true)),
            ("rail.tab.pinned".to_owned(), MetadataValue::Bool(false)),
        ]);
        let pinned = RenderMetadata::from([
            ("zellij.tab.active".to_owned(), MetadataValue::Bool(false)),
            ("rail.tab.pinned".to_owned(), MetadataValue::Bool(true)),
        ]);

        assert_eq!(form_for_region(tree, &inactive), DISPLAY_FORM_COMPACT);
        assert_eq!(form_for_region(tree, &active), "full");
        assert_eq!(form_for_region(tree, &pinned), "full");
    }

    #[test]
    fn detail_surface_resolves_entity_from_region_catalog() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            region "attention" source="attention" root-template="region/attention" form="detail" attention-key="status.attention"
            "#,
        )
        .expect("region config");
        let catalog = TemplateConfigCatalog::from_config(config);
        let entity_ref = EntityRef {
            kind: "issue".to_owned(),
            id: "1095".to_owned(),
        };
        let entity = DisplayEntity {
            placement: None,
            placement_layout: None,
            entity: entity_ref.clone(),
            label: "Issue 1095 hover detail".to_owned(),
            form: "detail".to_owned(),
            metadata: BTreeMap::from([("status.attention".to_owned(), MetadataValue::Bool(true))]),
            templates: ResolvedTemplateSlots::default(),
            children: vec![],
        };
        let mut model = model();
        model.tabs.clear();
        model.rows.clear();
        model.surface_regions = vec![andamento_core::DisplayRegion {
            definition: catalog.regions()[0].clone(),
            root: None,
            entities: vec![entity],
        }];

        let rendered = render_lines_with_detail_surface(
            Some(&model),
            &[],
            5,
            40,
            true,
            None,
            None,
            &[],
            Some(&catalog),
            &MetadataControls::default(),
            0,
            false,
            Some(&NodeKey::Entity(entity_ref)),
        );

        assert!(rendered.lines[3].contains("[issue] Issue 1095 hover detail"));
    }

    #[test]
    fn detail_surface_resolves_nested_entity_by_full_placement_key() {
        let parent_ref = EntityRef {
            kind: "project".to_owned(),
            id: "andamento".to_owned(),
        };
        let child_ref = EntityRef {
            kind: "issue".to_owned(),
            id: "89".to_owned(),
        };
        let parent_key = andamento_core::PlacementKey(vec![andamento_core::PlacementSegment {
            loop_name: "projects".to_owned(),
            entity: parent_ref.clone(),
        }]);
        let child_key = andamento_core::PlacementKey(vec![
            parent_key.0[0].clone(),
            andamento_core::PlacementSegment {
                loop_name: "issues".to_owned(),
                entity: child_ref.clone(),
            },
        ]);
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"region "attention" source="attention" root-template="region/attention" form="detail" attention-key="status.attention""#,
        )
        .expect("region config");
        let catalog = TemplateConfigCatalog::from_config(config);
        let mut model = model();
        model.rows.clear();
        model.surface_regions = vec![andamento_core::DisplayRegion {
            definition: catalog.regions()[0].clone(),
            root: None,
            entities: vec![DisplayEntity {
                entity: parent_ref,
                placement: Some(parent_key),
                placement_layout: None,
                label: "Andamento".to_owned(),
                form: "detail".to_owned(),
                metadata: BTreeMap::new(),
                templates: ResolvedTemplateSlots::default(),
                children: vec![DisplayEntity {
                    entity: child_ref,
                    placement: Some(child_key.clone()),
                    placement_layout: None,
                    label: "Nested issue 89".to_owned(),
                    form: "detail".to_owned(),
                    metadata: BTreeMap::new(),
                    templates: ResolvedTemplateSlots::default(),
                    children: vec![],
                }],
            }],
        }];

        let rendered =
            render_detail_surface(Some(&model), Some(&NodeKey::Placement(child_key)), 40, None);

        assert!(rendered.contains("[issue] Nested issue 89"));
    }

    #[test]
    fn pinned_placement_regions_reserve_every_nested_entity_row() {
        let child = DisplayEntity {
            placement: None,
            placement_layout: None,
            entity: EntityRef {
                kind: "convoy".to_owned(),
                id: "c".to_owned(),
            },
            label: "Convoy".to_owned(),
            form: "full".to_owned(),
            metadata: BTreeMap::new(),
            templates: ResolvedTemplateSlots::default(),
            children: vec![DisplayEntity {
                placement: None,
                placement_layout: None,
                entity: EntityRef {
                    kind: "vessel".to_owned(),
                    id: "v".to_owned(),
                },
                label: "Vessel".to_owned(),
                form: "full".to_owned(),
                metadata: BTreeMap::new(),
                templates: ResolvedTemplateSlots::default(),
                children: vec![],
            }],
        };
        let region = andamento_core::DisplayRegion {
            definition: SurfaceRegionDefinition {
                name: "attention".to_owned(),
                source: SurfaceRegionSource::Attention,
                root_template: "flotilla/region/attention".to_owned(),
                form: "full".to_owned(),
                attention_key: None,
                placement: Some("tree".to_owned()),
                pinned: true,
                promotions: vec![],
            },
            root: None,
            entities: vec![DisplayEntity {
                placement: None,
                placement_layout: None,
                entity: EntityRef {
                    kind: "project".to_owned(),
                    id: "p".to_owned(),
                },
                label: "Project".to_owned(),
                form: "full".to_owned(),
                metadata: BTreeMap::new(),
                templates: ResolvedTemplateSlots::default(),
                children: vec![child],
            }],
        };

        assert_eq!(pinned_region_rows(&region, None), 4);
        let mut tree_region = region;
        tree_region.definition.source = SurfaceRegionSource::Tree;
        assert_eq!(pinned_region_rows(&tree_region, None), 4);
    }

    #[test]
    fn wrapped_inline_siblings_keep_their_children_and_hits_on_their_own_rows() {
        fn item(
            id: &str,
            binding: &str,
            parent: &[andamento_core::PlacementSegment],
        ) -> DisplayEntity {
            let entity = EntityRef {
                kind: "vessel".into(),
                id: id.into(),
            };
            let mut key = parent.to_vec();
            key.push(andamento_core::PlacementSegment {
                loop_name: binding.into(),
                entity: entity.clone(),
            });
            DisplayEntity {
                entity,
                placement: Some(andamento_core::PlacementKey(key)),
                placement_layout: Some("inline".into()),
                label: id.into(),
                form: "compact".into(),
                metadata: BTreeMap::from([(
                    "display.label".into(),
                    MetadataValue::Text(id.into()),
                )]),
                templates: ResolvedTemplateSlots::default(),
                children: vec![],
            }
        }
        let mut first = item("firstxxxx", "item", &[]);
        let mut second = item("secondxxx", "item", &[]);
        first
            .children
            .push(item("one", "child", &first.placement.as_ref().unwrap().0));
        second
            .children
            .push(item("two", "child", &second.placement.as_ref().unwrap().0));
        let entities = vec![first, second];
        let region = SurfaceRegionDefinition {
            name: "tree".into(),
            source: SurfaceRegionSource::Tree,
            root_template: "root".into(),
            form: "compact".into(),
            attention_key: None,
            placement: Some("tree".into()),
            pinned: false,
            promotions: vec![],
        };
        let lines = render_placement_lines("root".into(), &entities, &region, None, 20, None);
        assert_eq!(lines.len(), 3);
        for (row, parent, child) in [(1, "firstxxxx", "one"), (2, "secondxxx", "two")] {
            assert!(
                lines[row].text.contains(parent) && lines[row].text.contains(child),
                "{:?}",
                lines[row]
            );
            assert!(lines[row].text.width() <= 20);
            assert_eq!(lines[row].inline_hits.len(), 2);
            assert_eq!(lines[row].inline_hits[0].0.entity.id, parent);
            assert_eq!(lines[row].inline_hits[1].0.entity.id, child);
            assert!(lines[row]
                .inline_hits
                .iter()
                .all(|(_, columns)| columns.start < columns.end && columns.end <= 20));
        }
    }

    #[test]
    fn placement_inline_loop_snapshots_fit_and_all_item_overflow() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r##"
            template "item/line" slot="compact" node-kind="entity" {
              field "label" class="required" source="metadata-text" key="display.label"
              field "change" class="required" source="metadata-text" key="change.number" prefix="#"
              field "owner" class="required" source="metadata-text" key="owner"
            }
            "##,
        )
        .expect("placement item template");
        let catalog = TemplateConfigCatalog::from_config(config);
        let region = SurfaceRegionDefinition {
            name: "tree".to_owned(),
            source: SurfaceRegionSource::Tree,
            root_template: "root".to_owned(),
            form: DISPLAY_FORM_COMPACT.to_owned(),
            attention_key: None,
            placement: Some("tree".to_owned()),
            pinned: false,
            promotions: vec![],
        };
        let entity = |id: &str, change: Option<&str>, owner: &str| {
            let metadata = BTreeMap::from_iter(
                [
                    Some((
                        "display.label".to_owned(),
                        MetadataValue::Text(id.to_owned()),
                    )),
                    change.map(|change| {
                        (
                            "change.number".to_owned(),
                            MetadataValue::Text(change.to_owned()),
                        )
                    }),
                    Some(("owner".to_owned(), MetadataValue::Text(owner.to_owned()))),
                ]
                .into_iter()
                .flatten(),
            );
            let resolved = catalog
                .resolve_placement_template("item/line", &metadata)
                .expect("resolve placement template")
                .expect("item line template");
            let slot = ResolvedTemplateSlot {
                template_name: resolved.name.clone(),
                fields: vec![],
                render_ready: Some(resolved.render_ready()),
                setters: vec![],
                effective_kdl: String::new(),
                resolve_error: None,
            };
            DisplayEntity {
                entity: EntityRef {
                    kind: "convoy".to_owned(),
                    id: id.to_owned(),
                },
                placement: Some(andamento_core::PlacementKey(vec![
                    andamento_core::PlacementSegment {
                        loop_name: "convoy".to_owned(),
                        entity: EntityRef {
                            kind: "convoy".to_owned(),
                            id: id.to_owned(),
                        },
                    },
                ])),
                placement_layout: Some("inline".to_owned()),
                label: id.to_owned(),
                form: DISPLAY_FORM_COMPACT.to_owned(),
                metadata,
                templates: ResolvedTemplateSlots {
                    compact: Some(slot),
                    ..ResolvedTemplateSlots::default()
                },
                children: vec![],
            }
        };
        let entities = vec![
            entity("tui", Some("62"), "alice"),
            entity("governor", None, "bob"),
        ];
        let snapshot = |width| {
            let lines = render_placement_lines(
                "▾ flotilla".to_owned(),
                &entities,
                &region,
                Some(&catalog),
                width,
                None,
            )
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>();
            assert!(lines.iter().all(|line| line.width() <= width));
            lines.join("\n")
        };

        insta::assert_snapshot!(
            format!("FIT\n{}\n\nOVERFLOW\n{}", snapshot(50), snapshot(30)),
            @r###"
        FIT
        ▾ flotilla ── tui      #62 alice  governor     bob

        OVERFLOW
        ▾ flotilla
          tui      #62 alice
          governor     bob
        "###
        );

        let mut themed_model = model();
        themed_model.rows.clear();
        themed_model.surface_regions = vec![DisplayRegion {
            definition: region,
            root: None,
            entities,
        }];
        let themed = render_lines_with_options(
            Some(&themed_model),
            &[],
            3,
            50,
            true,
            Some(test_theme()),
            None,
            &[],
            Some(&catalog),
        );
        assert!(
            themed.lines[0].contains("governor")
                && visible_width_without_ansi(&themed.lines[0]) == 50,
            "themed root layout must measure plain text before styling: {:?}",
            themed.lines[0]
        );
    }

    #[test]
    fn pinned_controls_keep_tree_footer_hits_and_scroll_state_on_visible_footer() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            region "controls-before" source="controls" root-template="region/controls" form="compact" pinned=true
            region "tree" source="tree" root-template="region/tree" form="compact"
            region "controls-after" source="controls" root-template="region/controls" form="compact" pinned=true
            template "region/tree" slot="compact" node-kind="entity" {
              field "label" source="literal" value="TREE"
            }
            template "region/controls" slot="compact" node-kind="entity" {
              control "open-config"
              control "scroll-down"
              control "scroll-up"
            }
            "#,
        )
        .expect("region config");
        let catalog = TemplateConfigCatalog::from_config(config);

        let rendered = render_lines_with_template_catalog(
            Some(&grouped_model()),
            &[],
            8,
            30,
            true,
            Some(&catalog),
        );
        let controls_rows = rendered
            .hit_regions
            .iter()
            .filter_map(|hit| (hit.action == HitAction::OpenConfig).then_some(hit.row_start))
            .collect::<Vec<_>>();
        assert_eq!(controls_rows.len(), 2, "{:?}", rendered.lines);

        for controls_row in controls_rows {
            assert!(rendered.hit_regions.iter().any(|hit| {
                hit.row_start == controls_row
                    && matches!(
                        hit.action,
                        HitAction::ScrollRailUp | HitAction::ScrollRailDown
                    )
            }));
        }
    }

    #[test]
    fn attention_overflow_is_reported_on_the_region_root() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            region "attention" source="attention" root-template="region/attention" form="detail" attention-key="status.attention"
            region "controls" source="controls" root-template="region/controls" form="compact" pinned=true
            template "region/attention" slot="compact" node-kind="entity" {
              field "label" source="literal" value="ATTENTION"
            }
            template "region/controls" slot="compact" node-kind="entity" {
              field "label" source="literal" value="CONTROLS"
            }
            template "issue/detail" slot="detail" node-kind="entity" {
              field "label" source="metadata-text" key="display.label"
            }
            "#,
        )
        .expect("region config");
        let catalog = TemplateConfigCatalog::from_config(config);
        let mut model = model();
        model.tabs.clear();
        model.rows = (1..=3)
            .map(|id| RailRow::Entity {
                entity: DisplayEntity {
                    placement: None,
                    placement_layout: None,
                    entity: EntityRef {
                        kind: "issue".to_owned(),
                        id: id.to_string(),
                    },
                    label: format!("Issue {id}"),
                    form: "full".to_owned(),
                    metadata: BTreeMap::from([
                        (
                            "entity.kind".to_owned(),
                            MetadataValue::Text("issue".to_owned()),
                        ),
                        (
                            "display.label".to_owned(),
                            MetadataValue::Text(format!("Issue {id}")),
                        ),
                        ("status.attention".to_owned(), MetadataValue::Bool(true)),
                    ]),
                    templates: ResolvedTemplateSlots::default(),
                    children: vec![],
                },
                indent: 0,
                parent_path: None,
            })
            .collect();

        let rendered =
            render_lines_with_template_catalog(Some(&model), &[], 4, 30, true, Some(&catalog));

        assert!(
            rendered.lines[0].contains("ATTENTION (+1 more)"),
            "{:?}",
            rendered.lines
        );
        assert!(
            !rendered.can_scroll(),
            "fixed attention truncation cannot be revealed by scrolling"
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
                        latent: false,
                        materialize_request: None,
                        latent_summary: None,
                        meta_panel: None,
                        entity: None,
                        compact_only: false,
                        form: "full".to_owned(),
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
            false,
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

        let nodes = nodes_to_render(Some(&model), &[], &[], None);
        let [RenderNode::Group(group)] = nodes.as_slice() else {
            panic!("expected one conflated group, got {nodes:?}");
        };
        assert_eq!(group.path, path);
        assert_eq!(group.conflated_paths.len(), 3);
        assert_eq!(group.label, "project-a─repo-a─main");
        assert!(
            matches!(&group.children[0], RenderNode::Tab(tab) if tab.indent == 2),
            "child tab should remain directly under the visible conflated group: {:?}",
            group.children
        );

        let rendered = render_lines(Some(&model), &[], 6, 48, true);
        assert!(
            rendered.lines[0].starts_with("▼ project-a─repo-a─main"),
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
    fn child_layout_variable_blocks_group_conflation_boundary() {
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
        model.template_config.effective_variables = vec![andamento_core::EffectiveNodeVariables {
            declarations: vec![],
            node: NodeKey::Group(GroupPath(vec![GroupSegment {
                key: "project".to_owned(),
                value: MetadataValue::Text("project-a".to_owned()),
                label: None,
            }])),
            values: BTreeMap::from([(
                "child-layout".to_owned(),
                andamento_core::EffectiveVariableValue {
                    value: "strip".to_owned(),
                    provenance: andamento_core::VariableSetterProvenance {
                        setter: "test".to_owned(),
                        ancestor: NodeKey::Root,
                        origin: andamento_core::template_config::TemplateConfigOrigin {
                            layer: andamento_core::template_config::TemplateConfigLayerKind::User,
                            membership: None,
                            source: "test".to_owned(),
                        },
                    },
                    overridden: vec![],
                },
            )]),
        }];

        let rendered = render_lines(Some(&model), &[], 6, 48, true);

        assert!(
            rendered.lines[0].trim_start().starts_with("project-a"),
            "{:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[0].contains("─ repo-a"),
            "parent layout should keep the child group boundary visible without structural conflation: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn group_can_contain_tabs_and_child_groups() {
        let model = mixed_child_group_model();

        let nodes = nodes_to_render(Some(&model), &[], &[], None);

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
    fn group_child_layout_variable_can_render_direct_tabs_as_strip() {
        let mut model = mixed_child_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        set_child_layout_variable(&mut model, NodeKey::Group(parent_path), "strip");

        let rendered = render_lines(Some(&model), &[], 8, 48, true);

        assert!(
            rendered.lines[0].contains(" repo-overview "),
            "direct tab should render as a Zellij-style tab strip in the group header niche: {:?}",
            rendered.lines
        );
        let repo_col = rendered.lines[0]
            .split("repo-overview")
            .next()
            .expect("repo strip should be visible")
            .width();
        let repo_hit = hit_at(&rendered.hit_regions, 0, repo_col).expect("repo strip hit");
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
            "child groups should inherit strip layout for their direct tabs: {:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .any(|line| line.contains("┌ branch-agent")),
            "inherited strip layout should avoid full child tab cards: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn compact_entity_uses_ribbon_form_and_detail_surface() {
        let mut model = grouped_model();
        let path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("expected group"),
        };
        let entity_ref = andamento_core::EntityRef {
            kind: "issue".to_owned(),
            id: "github/flotilla-org/flotilla#982".to_owned(),
        };
        let metadata = RenderMetadata::from([
            (
                "entity.kind".to_owned(),
                MetadataValue::Text(entity_ref.kind.clone()),
            ),
            (
                "entity.id".to_owned(),
                MetadataValue::Text(entity_ref.id.clone()),
            ),
            (
                "display.label".to_owned(),
                MetadataValue::Text("#982 entities-only cutover".to_owned()),
            ),
            (
                "summary.text".to_owned(),
                MetadataValue::Text("Cached entity metadata survives".to_owned()),
            ),
        ]);
        let resolve_slot = |slot| {
            let resolved = bundled_template_catalog()
                .resolve(TemplateConfigMatchContext {
                    slot,
                    node_kind: TemplateConfigNodeKind::Entity,
                    metadata: &metadata,
                    collapsed: false,
                    collapsible: false,
                    active_tab_name: None,
                })
                .expect("template resolution succeeds")
                .expect("bundled entity template");
            ResolvedTemplateSlot {
                template_name: resolved.name.clone(),
                fields: vec![],
                render_ready: Some(resolved.render_ready()),
                setters: resolved.setters.clone(),
                effective_kdl: resolved.dump_kdl(),
                resolve_error: None,
            }
        };
        model.tabs.clear();
        model.rows.truncate(1);
        model.rows.push(RailRow::Entity {
            entity: andamento_core::DisplayEntity {
                placement: None,
                placement_layout: None,
                entity: entity_ref.clone(),
                label: "#982 entities-only cutover".to_owned(),
                form: "compact".to_owned(),
                metadata: metadata.clone(),
                templates: ResolvedTemplateSlots {
                    compact: Some(resolve_slot(TemplateConfigSlot::Compact)),
                    detail: Some(resolve_slot(TemplateConfigSlot::Detail)),
                    ..Default::default()
                },
                children: vec![],
            },
            indent: 2,
            parent_path: Some(path),
        });

        let target = NodeKey::Entity(entity_ref.clone());
        let rendered = render_lines_with_detail_surface(
            Some(&model),
            &[],
            7,
            80,
            true,
            None,
            None,
            &[],
            None,
            &MetadataControls::default(),
            0,
            false,
            Some(&target),
        );

        assert!(rendered.lines.iter().any(|line| line.contains("#982")));
        assert!(
            !rendered
                .lines
                .iter()
                .any(|line| line.contains("#982 (tab)")),
            "compact template should resolve display.label from carried metadata: {:?}",
            rendered.lines
        );
        assert!(rendered.lines[5]
            .contains("[issue] #982 entities-only cutover Cached entity metadata survives"));
        assert!(rendered.hit_regions.iter().any(|hit| {
            hit.action == HitAction::ShowDetail
                && hit.inspect_target == Some(NodeKey::Entity(entity_ref.clone()))
        }));
    }

    #[test]
    fn full_entity_row_exposes_its_detail_target() {
        let entity_ref = andamento_core::EntityRef {
            kind: "issue".to_owned(),
            id: "github/flotilla-org/flotilla#1095".to_owned(),
        };
        let mut model = model();
        model.tabs.clear();
        model.rows = vec![RailRow::Entity {
            entity: andamento_core::DisplayEntity {
                placement: None,
                placement_layout: None,
                entity: entity_ref.clone(),
                label: "#1095 hover detail".to_owned(),
                form: "full".to_owned(),
                metadata: RenderMetadata::new(),
                templates: ResolvedTemplateSlots::default(),
                children: vec![],
            },
            indent: 0,
            parent_path: None,
        }];

        let rendered = render_lines_with_detail_surface(
            Some(&model),
            &[],
            8,
            40,
            true,
            None,
            None,
            &[],
            None,
            &MetadataControls::default(),
            0,
            false,
            None,
        );

        assert!(rendered.hit_regions.iter().any(|hit| {
            hit.action == HitAction::ShowDetail
                && hit.inspect_target == Some(NodeKey::Entity(entity_ref.clone()))
        }));
    }

    #[test]
    fn compact_chips_expose_their_individual_detail_targets() {
        let first = EntityRef {
            kind: "action".to_owned(),
            id: "tui".to_owned(),
        };
        let second = EntityRef {
            kind: "action".to_owned(),
            id: "governor".to_owned(),
        };
        let compact_row = |entity: EntityRef, label: &str| RailRow::Entity {
            entity: DisplayEntity {
                placement: None,
                placement_layout: None,
                entity,
                label: label.to_owned(),
                form: "compact".to_owned(),
                metadata: RenderMetadata::new(),
                templates: ResolvedTemplateSlots::default(),
                children: vec![],
            },
            indent: 0,
            parent_path: None,
        };
        let mut model = model();
        model.tabs.clear();
        model.rows = vec![
            compact_row(first.clone(), "tui"),
            compact_row(second.clone(), "gov"),
        ];

        let rendered = render_lines(Some(&model), &[], 4, 40, true);
        let hits = rendered
            .hit_regions
            .iter()
            .filter(|hit| hit.action == HitAction::ShowDetail)
            .collect::<Vec<_>>();

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].row_start, hits[1].row_start);
        assert_eq!(
            hits.iter()
                .map(|hit| hit.inspect_target.clone())
                .collect::<Vec<_>>(),
            vec![Some(NodeKey::Entity(first)), Some(NodeKey::Entity(second))]
        );
    }

    #[test]
    fn group_header_absorbs_direct_tab_prefix_into_header_niche() {
        let mut model = mixed_child_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        set_child_layout_variable(&mut model, NodeKey::Group(parent_path), "strip");

        let rendered = render_lines_with_theme(Some(&model), &[], 8, 72, true, Some(test_theme()));

        assert!(
            rendered.lines[0].contains("repo-overview"),
            "first direct tab should be rendered in the parent header niche: {:?}",
            rendered.lines
        );
        assert!(
            !rendered.lines[1].contains("repo-overview"),
            "absorbed direct tab should not be repeated below the header: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn group_header_absorbs_one_child_group_path_into_niche() {
        let mut model = nested_group_model();
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered = render_lines(Some(&model), &[], 10, 72, true);

        assert!(
            rendered.lines[0].contains("worktree-a"),
            "first child group should be absorbed into the parent header niche: {:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[0].contains("agent-1"),
            "first child group's direct tab should use the remaining niche: {:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .skip(1)
                .any(|line| line.contains("worktree-a")),
            "fully absorbed child group should not be repeated below: {:?}",
            rendered.lines
        );
        assert!(
            rendered
                .lines
                .iter()
                .skip(1)
                .any(|line| line.contains("worktree-b")),
            "unabsorbed sibling child group should still render below: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn absorbed_child_group_label_is_not_a_toggle_hit() {
        let mut model = nested_group_model();
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered = render_lines(Some(&model), &[], 10, 72, true);
        let group_label_col = rendered.lines[0]
            .split("worktree-a")
            .next()
            .expect("absorbed group label should be visible")
            .width();
        let tab_col = rendered.lines[0]
            .split("agent-1")
            .next()
            .expect("absorbed tab should be visible")
            .width();

        assert!(
            !matches!(
                hit_at(&rendered.hit_regions, 0, group_label_col),
                Some(HitRegion {
                    action: HitAction::ToggleGroup,
                    ..
                })
            ),
            "absorbed group label should not toggle the projected group's body: {:?}",
            rendered.hit_regions
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 0, tab_col)
                .expect("absorbed tab should remain clickable")
                .action,
            HitAction::SwitchTab
        );
    }

    #[test]
    fn absorbed_direct_tabs_are_right_aligned_in_header_niche() {
        let mut model = mixed_child_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        set_child_layout_variable(&mut model, NodeKey::Group(parent_path), "strip");

        let rendered = render_lines(Some(&model), &[], 8, 48, true);
        let tab_col = rendered.lines[0]
            .split("repo-overview")
            .next()
            .expect("absorbed tab should be visible")
            .width();

        assert!(
            tab_col >= 28,
            "absorbed tab should sit near the right edge of the available header niche: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn right_aligned_absorbed_tabs_do_not_leave_separator_artifact_after_group_label() {
        let mut model = nested_group_model();
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered = render_lines(Some(&model), &[], 10, 72, true);

        assert!(
            !rendered.lines[0].contains("worktree-a ─ ─"),
            "absorbed group label should join directly into border fill before right-aligned tabs: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn partially_absorbed_child_group_renders_overflow_without_duplicate_group_toggle() {
        let mut model = nested_group_model();
        let child_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be child group"),
        };
        for tab_id in [3_u64, 4] {
            model.tabs.push(TabCard {
                tab_id,
                position: tab_id as usize - 1,
                name: format!("agent-{tab_id}"),
                active: false,
                pinned: false,
                status: None,
                grouping: None,
                templates: ResolvedTemplateSlots::default(),
                active_pane: None,
            });
            model.rows.insert(
                tab_id as usize - 1,
                RailRow::Tab {
                    tab_id,
                    indent: 2,
                    parent_path: Some(child_path.clone()),
                },
            );
        }
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered = render_lines(Some(&model), &[], 10, 44, true);

        assert!(
            rendered.lines[0].contains("worktree-a"),
            "child group should still be absorbed into the parent header: {:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[0].contains("agent-1"),
            "at least one child tab should be absorbed into the parent header: {:?}",
            rendered.lines
        );
        assert!(
            rendered
                .lines
                .iter()
                .skip(1)
                .any(|line| line.contains("agent-")),
            "overflow child tabs should still render below the header: {:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .skip(1)
                .any(|line| line.contains("worktree-a")),
            "partially absorbed child group should not render a duplicate togglable group header below: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn fully_absorbed_group_body_does_not_show_a_collapse_toggle() {
        let mut model = grouped_model();
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered = render_lines(Some(&model), &[], 8, 72, true);

        assert!(
            rendered.lines[0].contains("server") && rendered.lines[0].contains("tests"),
            "both child tabs should be absorbed into the header niche: {:?}",
            rendered.lines
        );
        assert!(
            !rendered.lines[0].contains('▼') && !rendered.lines[0].contains('▶'),
            "a group with no remaining body rows should not show a collapse glyph: {:?}",
            rendered.lines
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 0, 0).map(|hit| hit.action),
            None,
            "a group with no remaining body rows should not register a collapse hit: {:?}",
            rendered.hit_regions
        );
        assert!(
            !rendered
                .lines
                .iter()
                .skip(1)
                .any(|line| line.contains("server") || line.contains("tests")),
            "fully absorbed tabs should not repeat below the header: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn empty_group_reserves_the_toggle_column_without_becoming_collapsible() {
        let mut model = grouped_model();
        let empty_path = GroupPath(vec![GroupSegment {
            key: "project".to_owned(),
            value: MetadataValue::Text("empty".to_owned()),
            label: None,
        }]);
        model.rows.push(RailRow::GroupHeader {
            group_id: "project:empty".to_owned(),
            path: empty_path,
            label: "empty".to_owned(),
            full_label: "empty".to_owned(),
            tab_count: 0,
            templates: ResolvedTemplateSlots::default(),
        });

        let rendered = render_lines(Some(&model), &[], 12, 24, true);
        let (row, line) = rendered
            .lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains("empty"))
            .expect("empty group should render");

        assert_eq!(line.find("empty"), Some(2), "got {line:?}");
        assert!(!line.contains('▼') && !line.contains('▶'), "got {line:?}");
        assert_eq!(
            hit_at(&rendered.hit_regions, row, 0).map(|hit| hit.action),
            None,
            "an empty group must not expose a collapse action"
        );
    }

    #[test]
    fn single_child_group_falls_back_to_grouped_rendering_when_too_narrow_to_absorb_all_children() {
        let mut model = nested_group_model();
        let child_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be child group"),
        };
        model.tabs.truncate(1);
        model.rows.truncate(2);
        for tab_id in [3_u64, 4] {
            model.tabs.push(TabCard {
                tab_id,
                position: tab_id as usize - 1,
                name: format!("agent-{tab_id}"),
                active: false,
                pinned: false,
                status: None,
                grouping: None,
                templates: ResolvedTemplateSlots::default(),
                active_pane: None,
            });
            model.rows.push(RailRow::Tab {
                tab_id,
                indent: 2,
                parent_path: Some(child_path.clone()),
            });
        }
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered = render_lines(Some(&model), &[], 10, 44, true);

        assert!(
            !rendered.lines[0].contains("worktree-a"),
            "single child group should not be partially absorbed when narrow: {:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[1].contains("▼ worktree-a"),
            "single child group should return to normal grouped rendering when narrow: {:?}",
            rendered.lines
        );
        assert!(
            rendered
                .lines
                .iter()
                .skip(2)
                .any(|line| line.contains("agent-")),
            "child tabs should remain visible under the normal group: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn collapsed_absorbed_child_group_keeps_descendants_already_projected_into_parent_niche() {
        let mut model = nested_group_model();
        let collapsed_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be child group"),
        };
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered =
            render_lines_with_collapsed_groups(Some(&model), &[], 10, 72, true, &[collapsed_path]);

        assert!(
            rendered.lines[0].contains("worktree-a"),
            "collapsed child group should still be absorbable as a group fragment: {:?}",
            rendered.lines
        );
        assert!(
            rendered.lines[0].contains("agent-1"),
            "collapse should not hide descendants already projected into an ancestor header niche: {:?}",
            rendered.lines
        );
        assert!(
            !rendered
                .lines
                .iter()
                .skip(1)
                .any(|line| line.contains("worktree-a")),
            "absorbed collapsed child group should not be repeated below: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn root_child_layout_variable_is_inherited_by_groups() {
        let mut model = mixed_child_group_model();
        set_child_layout_variable(&mut model, NodeKey::Root, "strip");

        let rendered = render_lines(Some(&model), &[], 8, 48, true);

        assert!(
            rendered.lines[0].contains(" repo-overview "),
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
    fn themed_strip_uses_exact_separator_with_active_inactive_and_between_colors() {
        let mut model = mixed_child_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        set_child_layout_variable(&mut model, NodeKey::Group(parent_path), "strip");

        let rendered = render_lines_with_theme(Some(&model), &[], 8, 48, true, Some(test_theme()));

        assert!(
            rendered.lines[0].contains("\u{1b}[1;48;5;10;38;5;14m\u{1b}[0m"),
            "active left separator should use between foreground and active tab background: {:?}",
            rendered.lines[0]
        );
        assert!(
            rendered.lines[0].contains("\u{1b}[1;48;5;10;38;5;11m repo-overview \u{1b}[0m"),
            "active label should use active tab foreground/background: {:?}",
            rendered.lines[0]
        );
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line.contains("\u{1b}[1;48;5;12;38;5;13m branch-agent \u{1b}[0m")),
            "inactive label should use inactive tab foreground/background: {:?}",
            rendered.lines
        );
        assert_eq!(visible_width_without_ansi(&rendered.lines[0]), 48);
    }

    #[test]
    fn themed_strip_uses_template_catalog_tab_titles() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            template "tab/title" slot="tab-title" node-kind="tab" {
              field "title" class="required" source="literal" value="External"
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_core::template_config::TemplateConfigCatalog::from_config(config);
        let mut model = mixed_child_group_model();
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        set_child_layout_variable(&mut model, NodeKey::Group(parent_path), "strip");

        let rendered = render_lines_with_options(
            Some(&model),
            &[],
            8,
            48,
            true,
            Some(test_theme()),
            None,
            &[],
            Some(&catalog),
        );

        assert!(
            rendered.lines[0].contains(" External "),
            "themed strip should keep template-resolved tab title labels: {:?}",
            rendered.lines
        );
        assert!(
            !rendered.lines[0].contains(" repo-overview "),
            "themed strip should not fall back to built-in tab title labels: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn themed_strip_segment_truncates_before_styling_when_narrow() {
        let item = SegmentItem {
            label: "repo-overview".to_owned(),
            active: true,
        };

        let (segment, visible_width) = render_strip_segment(&item, 3, Some(test_theme()));

        assert_eq!(visible_width, 3);
        assert_eq!(visible_width_without_ansi(&segment), 3);
    }

    #[test]
    fn rail_config_overrides_strip_segment_between_color() {
        let mut model = mixed_child_group_model();
        model.config.segment_between_color = Some(andamento_core::RailRgbColor {
            red: 1,
            green: 2,
            blue: 3,
        });
        let parent_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("first row should be parent group"),
        };
        set_child_layout_variable(&mut model, NodeKey::Group(parent_path), "strip");

        let rendered = render_lines_with_theme(Some(&model), &[], 8, 48, true, Some(test_theme()));

        assert!(
            rendered.lines[0].contains("\u{1b}[1;48;5;10;38;2;1;2;3m\u{1b}[0m"),
            "configured between color should become the active left separator foreground: {:?}",
            rendered.lines[0]
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
                fields: vec![andamento_core::ResolvedTemplateField {
                    text: "zellij-org/zellij".to_owned(),
                    priority: 100,
                    source: Some(repo_source.clone()),
                }],
                render_ready: None,
                setters: vec![],
                effective_kdl: String::new(),
                resolve_error: None,
            });
        }
        if let RailRow::GroupHeader { templates, .. } = &mut model.rows[1] {
            templates.group_header = Some(ResolvedTemplateSlot {
                template_name: "branch.group-header".to_owned(),
                fields: vec![
                    andamento_core::ResolvedTemplateField {
                        text: "zellij-org/zellij".to_owned(),
                        priority: 100,
                        source: Some(repo_source),
                    },
                    andamento_core::ResolvedTemplateField {
                        text: "feat/kitty-image-plumbing".to_owned(),
                        priority: 100,
                        source: Some(branch_source),
                    },
                ],
                render_ready: None,
                setters: vec![],
                effective_kdl: String::new(),
                resolve_error: None,
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
    fn tab_status_fields_are_resolved_through_bundled_template() {
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
            slot: TemplateConfigSlot::TabStatus,
            node_kind: TemplateConfigNodeKind::Tab,
            metadata: &metadata,
            collapsed: false,
            collapsible: false,
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
            &[],
            &[],
            ChildLayoutMode::Cards,
        );

        assert!(
            lines[0].trim_start().starts_with("metadata-label (3)"),
            "group header template should render from metadata: {:?}",
            lines[0]
        );
    }

    #[test]
    fn group_header_inline_layout_preserves_toggle_label_count_and_filler() {
        let rendered = render_lines(Some(&grouped_model()), &[], 8, 24, true);

        assert!(rendered.lines[0].starts_with("▼ zellij (2)"));
        assert!(
            rendered.lines[0].contains("──"),
            "group header should retain horizontal filler: {:?}",
            rendered.lines[0]
        );
        assert_eq!(
            hit_at(&rendered.hit_regions, 0, 0).map(|hit| hit.action),
            Some(HitAction::ToggleGroup)
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
            latent: false,
            materialize_request: None,
            latent_summary: None,
            meta_panel: None,
            entity: None,
            compact_only: false,
            form: "full".to_owned(),
        };

        assert_eq!(
            tab_title_template_fields(&card.metadata),
            vec![TemplateField::Required("metadata-agent".to_owned())]
        );
    }

    #[test]
    fn external_template_catalog_can_override_tab_title_rendering() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            template "tab/title" slot="tab-title" node-kind="tab" {
              field "title" class="required" source="literal" value="External"
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_core::template_config::TemplateConfigCatalog::from_config(config);

        let rendered =
            render_lines_with_template_catalog(Some(&model()), &[], 7, 24, true, Some(&catalog));

        assert!(rendered.lines[0].starts_with("External"));
    }

    #[test]
    fn external_template_fields_read_effective_node_variables() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            template "group/full" slot="group-header" node-kind="group" {
              field "layout" class="required" key="var.child-layout"
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_core::template_config::TemplateConfigCatalog::from_config(config);
        let mut model = grouped_model();
        let group_path = match &model.rows[0] {
            RailRow::GroupHeader { path, .. } => path.clone(),
            _ => panic!("expected group header"),
        };
        set_child_layout_variable(&mut model, NodeKey::Group(group_path), "strip");

        let rendered =
            render_lines_with_template_catalog(Some(&model), &[], 7, 24, true, Some(&catalog));

        assert!(
            rendered.lines[0].starts_with("strip"),
            "{:?}",
            rendered.lines
        );
    }

    #[test]
    fn entry_with_no_recognized_fields_renders_label_and_kind() {
        let mut model = model();
        model.tabs[0].templates.tab_title = Some(ResolvedTemplateSlot {
            template_name: "future.entry".to_owned(),
            fields: vec![],
            render_ready: None,
            setters: vec![],
            effective_kdl: String::new(),
            resolve_error: None,
        });

        let rendered = render_lines(Some(&model), &[], 7, 24, true);

        assert!(
            rendered.lines[0].starts_with("┌ tab-2 (tab)"),
            "entry fallback should retain its label and kind: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn unknown_grouping_key_renders_plain_key_value_fallback() {
        for group_header in [
            None,
            Some(ResolvedTemplateSlot {
                template_name: "future.group".to_owned(),
                fields: vec![],
                render_ready: None,
                setters: vec![],
                effective_kdl: String::new(),
                resolve_error: None,
            }),
        ] {
            let mut model = grouped_model();
            let path = GroupPath(vec![GroupSegment {
                key: "experimental.scope".to_owned(),
                value: MetadataValue::Text("andamento-total-fallback".to_owned()),
                label: Some("total fallback".to_owned()),
            }]);
            model.rows = vec![
                RailRow::GroupHeader {
                    group_id: "experimental.scope:andamento-total-fallback".to_owned(),
                    path: path.clone(),
                    label: "total fallback".to_owned(),
                    full_label: "total fallback".to_owned(),
                    tab_count: 2,
                    templates: ResolvedTemplateSlots {
                        group_header,
                        ..ResolvedTemplateSlots::default()
                    },
                },
                RailRow::Tab {
                    tab_id: 1,
                    indent: 2,
                    parent_path: Some(path.clone()),
                },
                RailRow::Tab {
                    tab_id: 2,
                    indent: 2,
                    parent_path: Some(path),
                },
            ];

            let rendered =
                render_lines_with_theme(Some(&model), &[], 8, 56, true, Some(test_theme()));

            assert!(
                rendered.lines[0]
                    .starts_with("experimental.scope: andamento-total-fallback (group)"),
                "unknown grouping key should render plain key, value, and kind: {:?}",
                rendered.lines[0]
            );
        }
    }

    #[test]
    fn vcs_repo_grouping_key_uses_themed_bundled_header() {
        let path = GroupPath(vec![GroupSegment {
            key: "vcs.repo".to_owned(),
            value: MetadataValue::Text("flotilla-org/flotilla".to_owned()),
            label: Some("flotilla".to_owned()),
        }]);
        let metadata = metadata_for_group_header(&path, "flotilla", "flotilla", 2);

        let (_, text_style) =
            resolve_group_header_template_fields(&metadata, false, true, None, None);

        assert_eq!(text_style, GroupHeaderTextStyle::Themed);
    }

    #[test]
    fn vcs_repo_group_renders_the_producer_label_only() {
        let mut model = grouped_model();
        let path = GroupPath(vec![GroupSegment {
            key: "vcs.repo".to_owned(),
            value: MetadataValue::Text("ghostty-org/ghostty".to_owned()),
            label: Some("ghostty".to_owned()),
        }]);
        let collapsed_path = path.clone();
        model.rows = vec![
            RailRow::GroupHeader {
                group_id: "vcs.repo:ghostty-org/ghostty".to_owned(),
                path: path.clone(),
                label: "ghostty".to_owned(),
                full_label: "ghostty".to_owned(),
                tab_count: 2,
                templates: ResolvedTemplateSlots::default(),
            },
            RailRow::Tab {
                tab_id: 1,
                indent: 2,
                parent_path: Some(path.clone()),
            },
            RailRow::Tab {
                tab_id: 2,
                indent: 2,
                parent_path: Some(path),
            },
        ];

        let rendered = render_lines(Some(&model), &[], 8, 56, true);

        assert!(
            rendered.lines[0].starts_with("▼ ghostty ─"),
            "repo group should render only its short label before rail chrome: {:?}",
            rendered.lines[0]
        );

        let collapsed =
            render_lines_with_collapsed_groups(Some(&model), &[], 8, 56, true, &[collapsed_path]);

        assert!(
            collapsed.lines[0].starts_with("▶ ghostty ─"),
            "collapsed repo group should still render only its short label: {:?}",
            collapsed.lines[0]
        );
        assert!(
            !collapsed.lines[0].contains("tests"),
            "repo template should not add generic collapsed-group context: {:?}",
            collapsed.lines[0]
        );
    }

    #[test]
    fn vcs_repo_group_derives_short_label_when_producer_omits_it() {
        let mut model = grouped_model();
        let path = GroupPath(vec![GroupSegment {
            key: "vcs.repo".to_owned(),
            value: MetadataValue::Text("ghostty-org/ghostty".to_owned()),
            label: None,
        }]);
        model.rows = vec![
            RailRow::GroupHeader {
                group_id: "vcs.repo:ghostty-org/ghostty".to_owned(),
                path: path.clone(),
                label: "ghostty-org/ghostty".to_owned(),
                full_label: "ghostty-org/ghostty".to_owned(),
                tab_count: 2,
                templates: ResolvedTemplateSlots::default(),
            },
            RailRow::Tab {
                tab_id: 1,
                indent: 2,
                parent_path: Some(path.clone()),
            },
            RailRow::Tab {
                tab_id: 2,
                indent: 2,
                parent_path: Some(path),
            },
        ];

        let rendered = render_lines(Some(&model), &[], 8, 56, true);

        assert!(
            rendered.lines[0].starts_with("▼ ghostty ─"),
            "repo group should derive a short label from its slug: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn vcs_repo_group_falls_back_to_group_label_for_an_empty_basename() {
        let mut model = grouped_model();
        let path = GroupPath(vec![GroupSegment {
            key: "vcs.repo".to_owned(),
            value: MetadataValue::Text("/".to_owned()),
            label: None,
        }]);
        model.rows = vec![
            RailRow::GroupHeader {
                group_id: "vcs.repo:/".to_owned(),
                path: path.clone(),
                label: "/".to_owned(),
                full_label: "/".to_owned(),
                tab_count: 2,
                templates: ResolvedTemplateSlots::default(),
            },
            RailRow::Tab {
                tab_id: 1,
                indent: 2,
                parent_path: Some(path.clone()),
            },
            RailRow::Tab {
                tab_id: 2,
                indent: 2,
                parent_path: Some(path),
            },
        ];

        let rendered = render_lines(Some(&model), &[], 8, 56, true);

        assert!(
            rendered.lines[0].starts_with("▼ / ─"),
            "repo group should retain a visible fallback label: {:?}",
            rendered.lines[0]
        );
    }

    fn assert_flotilla_group_renders_designed_label(
        key: &str,
        value: &str,
        segment_label: Option<&str>,
        expected: &str,
    ) {
        let mut model = grouped_model();
        let path = GroupPath(vec![GroupSegment {
            key: key.to_owned(),
            value: MetadataValue::Text(value.to_owned()),
            label: segment_label.map(str::to_owned),
        }]);
        model.rows = vec![RailRow::GroupHeader {
            group_id: format!("{key}:{value}"),
            path,
            label: segment_label.unwrap_or(value).to_owned(),
            full_label: segment_label.unwrap_or(value).to_owned(),
            tab_count: 2,
            templates: ResolvedTemplateSlots::default(),
        }];

        let rendered = render_lines(Some(&model), &[], 8, 80, true);

        assert!(
            rendered.lines[0]
                .trim_start()
                .starts_with(&format!("{expected} ─")),
            "{key} should render its designed label without conformance fallback: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn flotilla_project_group_renders_project_name_only() {
        assert_flotilla_group_renders_designed_label(
            "flotilla.project",
            "dev/andamento@feta",
            Some("andamento"),
            "andamento",
        );
    }

    #[test]
    fn flotilla_group_does_not_render_source_fact_as_a_badge() {
        let mut model = grouped_model();
        let path = GroupPath(vec![GroupSegment {
            key: "flotilla.convoy".to_owned(),
            value: MetadataValue::Text("dev/cutover@feta".to_owned()),
            label: Some("cutover".to_owned()),
        }]);
        model.rows = vec![RailRow::GroupHeader {
            group_id: "flotilla.convoy:dev/cutover@feta".to_owned(),
            path: path.clone(),
            label: "cutover".to_owned(),
            full_label: "cutover".to_owned(),
            tab_count: 1,
            templates: ResolvedTemplateSlots::default(),
        }];
        model.resolved_metadata = vec![ResolvedMetadata {
            target: ResolvedMetadataTarget::Group(path),
            values: BTreeMap::from([(
                "source".to_owned(),
                MetadataEntry {
                    value: MetadataValue::Text("flotilla".to_owned()),
                    updated_at: 1,
                    ttl_ms: None,
                    precedence: 0,
                    ordinal: 0,
                },
            )]),
            source_entries: BTreeMap::new(),
            reachable_identities: vec![],
        }];

        let rendered = render_lines(Some(&model), &[], 8, 80, true);

        assert!(
            rendered.lines[0].trim_start().starts_with("cutover")
                && !rendered.lines[0].contains("[flotilla]"),
            "{:?}",
            rendered.lines
        );
    }

    #[test]
    fn flotilla_convoy_group_renders_segment_label() {
        assert_flotilla_group_renders_designed_label(
            "flotilla.convoy",
            "flotilla-template-keys/issue-28",
            Some("issue-28"),
            "issue-28",
        );
    }

    #[test]
    fn flotilla_vessel_group_renders_vessel_name() {
        assert_flotilla_group_renders_designed_label("flotilla.vessel", "work", None, "work");
    }

    #[test]
    fn flotilla_independent_group_renders_session_short_name() {
        assert_flotilla_group_renders_designed_label(
            "flotilla.independent",
            "session-28",
            Some("codex"),
            "codex",
        );
    }

    #[test]
    fn flotilla_session_group_renders_session_label() {
        assert_flotilla_group_renders_designed_label(
            "flotilla.session",
            "feta/dev/terminal-coder",
            Some("coder"),
            "coder",
        );
    }

    #[test]
    fn flotilla_checkout_group_renders_checkout_short_name() {
        assert_flotilla_group_renders_designed_label(
            "flotilla.checkout",
            "checkout/flotilla-org/andamento/issue-28-template-keys",
            Some("issue-28-template-keys"),
            "issue-28-template-keys",
        );
    }

    #[test]
    fn flotilla_checkout_group_derives_short_name_when_segment_label_is_missing() {
        assert_flotilla_group_renders_designed_label(
            "flotilla.checkout",
            "checkout/flotilla-org/andamento/issue-28-template-keys",
            None,
            "issue-28-template-keys",
        );
    }

    #[test]
    fn flotilla_issue_group_renders_number_and_title() {
        assert_flotilla_group_renders_designed_label(
            "flotilla.issue",
            "issue/flotilla-org/andamento/28",
            Some("#28 Designed templates for remaining keys"),
            "#28 Designed templates for remaining keys",
        );
    }

    #[test]
    fn group_with_no_resolved_fields_uses_its_bundled_fallback() {
        let mut model = grouped_model();
        let RailRow::GroupHeader { templates, .. } = &mut model.rows[0] else {
            panic!("expected group header");
        };
        templates.group_header = Some(ResolvedTemplateSlot {
            template_name: "future.group".to_owned(),
            fields: vec![],
            render_ready: None,
            setters: vec![],
            effective_kdl: String::new(),
            resolve_error: None,
        });

        let rendered = render_lines(Some(&model), &[], 8, 24, true);

        assert!(
            rendered.lines[0].starts_with("▼ zellij (group)"),
            "empty resolved group fields should retain its label and kind: {:?}",
            rendered.lines[0]
        );
    }

    #[test]
    fn cached_group_template_renders_in_the_live_collapse_context_without_reparsing_kdl() {
        let mut model = grouped_model();
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            template "group/full" slot="group-header" node-kind="group" {
              toggle collapsed="▶" expanded="▼"
              fill glyph="─"
              field "label" class="required" source="literal" value="effective"
            }
            "#,
        )
        .expect("template parses");
        let catalog = TemplateConfigCatalog::from_config(config);
        let metadata = BTreeMap::from([(
            "presentation.template".to_owned(),
            MetadataValue::Text("group/full".to_owned()),
        )]);
        let resolved = catalog
            .resolve(TemplateConfigMatchContext {
                slot: TemplateConfigSlot::GroupHeader,
                node_kind: TemplateConfigNodeKind::Group,
                metadata: &metadata,
                collapsed: false,
                collapsible: true,
                active_tab_name: None,
            })
            .expect("resolution succeeds")
            .expect("template resolves");
        let RailRow::GroupHeader {
            path, templates, ..
        } = &mut model.rows[0]
        else {
            panic!("expected group header");
        };
        let collapsed_path = path.clone();
        templates.group_header = Some(ResolvedTemplateSlot {
            template_name: "group/full".to_owned(),
            fields: vec![
                andamento_core::ResolvedTemplateField {
                    text: "▼".to_owned(),
                    priority: 100,
                    source: None,
                },
                andamento_core::ResolvedTemplateField {
                    text: "stale".to_owned(),
                    priority: 100,
                    source: None,
                },
            ],
            render_ready: Some(resolved.render_ready()),
            setters: resolved.setters.clone(),
            effective_kdl: "// deliberately not parseable; inspect evidence only".to_owned(),
            resolve_error: None,
        });

        let rendered =
            render_lines_with_collapsed_groups(Some(&model), &[], 8, 24, true, &[collapsed_path]);

        assert!(
            rendered.lines[0].starts_with("▶ effective"),
            "the flattened template should render with live collapse state: {:?}",
            rendered.lines[0]
        );
        assert_eq!(rendered.lines[0].matches('▶').count(), 1);
        assert!(!rendered.lines[0].contains("stale"));
    }

    #[test]
    fn external_kdl_template_catalog_can_render_numeric_priority_fields() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            template "group/full" slot="group-header" node-kind="group" {
              field "toggle" priority=100 {
                value source="collapsed-toggle" collapsed="▶" expanded="▼"
              }
              field "repo" key="git.repo" priority=100
              field "branch" key="git.branch" priority=10
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_core::template_config::TemplateConfigCatalog::from_config(config);
        let metadata = RenderMetadata::from([
            (
                "git.repo".to_owned(),
                MetadataValue::Text("rjwittams/zellij-scratch".to_owned()),
            ),
            (
                "git.branch".to_owned(),
                MetadataValue::Text("main".to_owned()),
            ),
            (
                "group.key".to_owned(),
                MetadataValue::Text("zellij.pane.cwd".to_owned()),
            ),
        ]);

        let fields = group_header_template_fields_with_template_catalog(
            &metadata,
            false,
            true,
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
    fn repo_template_extends_the_bundled_header_without_losing_its_label() {
        let config = andamento_core::template_config::parse_template_config_kdl(include_str!(
            "../../../templates/andamento-git.kdl"
        ))
        .expect("example template config");
        let catalog =
            andamento_core::template_config::TemplateConfigCatalog::with_bundled_defaults(config);
        let metadata = RenderMetadata::from([
            (
                "group.key".to_owned(),
                MetadataValue::Text("vcs.repo".to_owned()),
            ),
            (
                "group.value".to_owned(),
                MetadataValue::Text("flotilla-org/andamento".to_owned()),
            ),
            (
                "group.label".to_owned(),
                MetadataValue::Text("andamento".to_owned()),
            ),
            (
                "vcs.repo".to_owned(),
                MetadataValue::Text("flotilla-org/andamento".to_owned()),
            ),
        ]);

        let fields = group_header_template_fields_with_template_catalog(
            &metadata,
            false,
            true,
            None,
            Some(&catalog),
        );

        assert_eq!(
            fields,
            vec![TemplateField::Prioritized {
                value: "flotilla-org/andamento".to_owned(),
                priority: 100,
                source: Some(ResolvedTemplateFieldSource {
                    key: "vcs.repo".to_owned(),
                    value: MetadataValue::Text("flotilla-org/andamento".to_owned()),
                }),
            },]
        );
    }

    #[test]
    fn external_kdl_group_header_template_uses_resolved_group_metadata() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"
            template "group/full" slot="group-header" node-kind="group" {
              field "toggle" priority=100 {
                value source="collapsed-toggle" collapsed="▶" expanded="▼"
              }
              field "repo" key="git.repo" priority=100
              field "branch" key="git.branch" priority=60
            }
            "#,
        )
        .expect("valid template config");
        let catalog = andamento_core::template_config::TemplateConfigCatalog::from_config(config);
        let mut model = grouped_model();
        let group_path = GroupPath(vec![GroupSegment {
            key: "zellij.pane.cwd".to_owned(),
            value: MetadataValue::Text("/Users/robert/dev/zellij".to_owned()),
            label: None,
        }]);
        model.resolved_metadata = vec![ResolvedMetadata {
            target: ResolvedMetadataTarget::Group(group_path),
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

        let rendered = render_lines(Some(&model), &[], 10, 24, true);

        assert!(rendered.lines[0].starts_with("┌ tab-2"));
        assert!(rendered.lines[4].starts_with("└"));
        assert!(rendered.lines[5].starts_with("┌ tab-1"));
        assert!(rendered.lines[7].starts_with("└"));
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
            12,
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
        let (_sources, allocation, _absorbed_tabs) = append_group_header(
            &mut lines,
            &mut hits,
            &group,
            20,
            None,
            None,
            &BTreeSet::new(),
            &MetadataControls::default(),
            None,
            &[],
            &[],
            ChildLayoutMode::Cards,
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
    fn footer_projects_declared_variable_control_and_false_state() {
        let config = andamento_core::template_config::parse_template_config_kdl(
            r#"display-variable "show-issues" type="bool" default=true label="Issues" icon="I""#,
        )
        .unwrap();
        let mut lines = vec![blank(20)];
        let mut hits = vec![];
        let values = BTreeMap::from([(
            "show-issues".to_owned(),
            andamento_core::DisplayVariableValue::Bool(false),
        )]);

        let controls = [andamento_core::template_config::TemplateControlSpec {
            kind: andamento_core::template_config::TemplateControlKind::DisplayVariable,
            variable: Some("show-issues".to_owned()),
            glyph: None,
        }];
        render_control_template(
            &mut lines,
            &mut hits,
            0,
            20,
            None,
            None,
            false,
            &controls,
            &config.display_variables,
            Some(&values),
        );

        assert_eq!(lines[0].chars().next(), Some('·'));
        assert!(hits
            .iter()
            .any(|hit| hit.action == HitAction::ToggleVariable(0)));
    }

    #[test]
    fn control_footer_drops_controls_that_do_not_fit_narrow_widths() {
        use andamento_core::template_config::{TemplateControlKind, TemplateControlSpec};

        let controls = [
            TemplateControlSpec {
                kind: TemplateControlKind::OpenConfig,
                variable: None,
                glyph: None,
            },
            TemplateControlSpec {
                kind: TemplateControlKind::ScrollDown,
                variable: None,
                glyph: None,
            },
            TemplateControlSpec {
                kind: TemplateControlKind::ScrollUp,
                variable: None,
                glyph: None,
            },
            TemplateControlSpec {
                kind: TemplateControlKind::InspectRoot,
                variable: None,
                glyph: None,
            },
        ];

        for width in 1..=3 {
            let mut lines = vec![blank(width)];
            let mut hits = vec![];
            render_control_template(
                &mut lines,
                &mut hits,
                0,
                width,
                None,
                None,
                true,
                &controls,
                &[],
                None,
            );

            assert_eq!(lines[0].chars().count(), width);
            assert!(hits.iter().all(|hit| hit.col_start < width));
            assert_eq!(
                hits.iter()
                    .map(|hit| hit.col_start)
                    .collect::<BTreeSet<_>>()
                    .len(),
                hits.len()
            );
        }
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
            latent: false,
            materialize_request: None,
            latent_summary: None,
            meta_panel: None,
            entity: None,
            compact_only: false,
            form: "full".to_owned(),
        };
        let chrome = card_chrome(&card, None);
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
            &chrome,
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
            latent: false,
            materialize_request: None,
            latent_summary: None,
            meta_panel: None,
            entity: None,
            compact_only: false,
            form: "full".to_owned(),
        };
        let chrome = card_chrome(&card, None);
        write_tab_top_border(
            &mut lines, &mut hits, 0, "tab", 20, true, &card, None, &controls, None, &chrome,
        );
        assert!(lines[0].ends_with("○┐"), "got {:?}", lines[0]);
        assert!(hits.iter().any(|h| h.action == HitAction::InspectNode));
    }

    #[test]
    fn unboxed_tab_title_keeps_the_inspect_affordance() {
        let mut lines = vec![blank(20); 1];
        let mut hits = vec![];
        let card = RenderCard {
            tab_id: 7,
            position: 3,
            name: "tab".into(),
            active: false,
            pinned: false,
            status: None,
            metadata: BTreeMap::new(),
            metadata_sources: BTreeMap::new(),
            reachable_identities: vec![],
            templates: ResolvedTemplateSlots::default(),
            latent: false,
            materialize_request: None,
            latent_summary: None,
            meta_panel: None,
            entity: None,
            compact_only: false,
            form: "full".to_owned(),
        };

        write_tab_top_border(
            &mut lines,
            &mut hits,
            0,
            "flat tab",
            20,
            true,
            &card,
            None,
            &MetadataControls::default(),
            None,
            &ChromeSpec::default(),
        );

        assert!(lines[0].ends_with('○'), "got {:?}", lines[0]);
        let inspect = hits
            .iter()
            .find(|hit| hit.action == HitAction::InspectNode)
            .expect("unboxed title retains inspect hit");
        assert_eq!(inspect.col_start, 19);
        assert_eq!(inspect.tab_id, 7);
        assert_eq!(inspect.tab_position, 3);
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
                materialize_request: None,
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
                materialize_request: None,
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
            latent: false,
            materialize_request: None,
            latent_summary: None,
            meta_panel: None,
            entity: None,
            compact_only: false,
            form: "full".to_owned(),
        };
        let base = cell_height(&card, false);
        card.meta_panel = Some(vec!["a".into(), "b".into(), "c".into()]);
        let grown = cell_height(&card, false);
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
    fn inspect_blocks_show_origin_annotated_effective_templates_for_groups_and_entities() {
        let group = RenderGroup {
            path: GroupPath(vec![GroupSegment {
                key: "vcs.repo".into(),
                value: MetadataValue::Text("flotilla-org/andamento".into()),
                label: Some("andamento".into()),
            }]),
            conflated_paths: vec![],
            label: "andamento".into(),
            full_label: "andamento".into(),
            tab_count: 1,
            collapsed: false,
            indent: 0,
            metadata: metadata_for_group_header(
                &GroupPath(vec![GroupSegment {
                    key: "vcs.repo".into(),
                    value: MetadataValue::Text("flotilla-org/andamento".into()),
                    label: Some("andamento".into()),
                }]),
                "andamento",
                "andamento",
                1,
            ),
            metadata_sources: BTreeMap::new(),
            reachable_identities: vec![],
            templates: ResolvedTemplateSlots::default(),
            children: vec![],
        };
        let group_lines = group_metadata_block(&group);
        assert!(group_lines
            .iter()
            .any(|line| line.trim() == "[resolved_template]"));
        assert!(group_lines
            .iter()
            .any(|line| line.contains("// origin: bundled")));

        let metadata = BTreeMap::from([(
            "entity.kind".to_owned(),
            MetadataValue::Text("issue".to_owned()),
        )]);
        let context = TemplateConfigMatchContext {
            slot: TemplateConfigSlot::Compact,
            node_kind: TemplateConfigNodeKind::Entity,
            metadata: &metadata,
            collapsed: false,
            collapsible: false,
            active_tab_name: None,
        };
        let resolved = bundled_template_catalog()
            .resolve(context)
            .expect("resolution succeeds")
            .expect("issue compact template");
        let entity = andamento_core::DisplayEntity {
            placement: None,
            placement_layout: None,
            entity: andamento_core::EntityRef {
                kind: "issue".to_owned(),
                id: "flotilla#1058".into(),
            },
            label: "#1058".into(),
            form: "compact".to_owned(),
            metadata: BTreeMap::new(),
            templates: ResolvedTemplateSlots {
                compact: Some(ResolvedTemplateSlot {
                    template_name: resolved.name.clone(),
                    fields: vec![],
                    render_ready: Some(resolved.render_ready()),
                    setters: resolved.setters.clone(),
                    effective_kdl: resolved.dump_kdl(),
                    resolve_error: None,
                }),
                ..ResolvedTemplateSlots::default()
            },
            children: vec![],
        };
        let entity_lines = tab_metadata_block(&RenderTab {
            card: render_card_from_entity(&entity),
            indent: 0,
            grouping: None,
            parent_path: None,
        });
        assert!(entity_lines
            .iter()
            .any(|line| line.trim() == "[resolved_template]"));
        assert!(entity_lines
            .iter()
            .any(|line| line.contains("flotilla/issue/compact")));
        assert!(entity_lines
            .iter()
            .any(|line| line.contains("// origin: bundled")));
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
            &[],
            &[],
            ChildLayoutMode::Cards,
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
                latent: false,
                materialize_request: None,
                latent_summary: None,
                meta_panel: None,
                entity: None,
                compact_only: false,
                form: "full".to_owned(),
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

fn semantic_fields(content: &andamento_core::presentation::Content) -> Vec<TemplateField> {
    content
        .fields
        .iter()
        .cloned()
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
        .collect()
}
