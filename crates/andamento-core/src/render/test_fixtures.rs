use super::{render_lines_with_detail_surface, LocalTab, RenderedRail};
use andamento_shared::{
    ControllerViewModel, DisplayEntity, DisplayRegion, EntityRef, GroupPath, GroupSegment,
    LatentMaterializationState, LatentTab, MetadataControls, MetadataValue, NodeKey, RailConfig,
    RailRow, ResolvedTemplateSlots, SortMode, TabCard, TemplateConfigDiagnostics,
};
use std::collections::BTreeMap;
use unicode_width::UnicodeWidthStr;

pub(super) struct ControllerModelFixture {
    model: ControllerViewModel,
}

impl ControllerModelFixture {
    pub(super) fn empty() -> Self {
        Self {
            model: ControllerViewModel {
                sort_mode: SortMode::Position,
                config: RailConfig::default(),
                template_config: TemplateConfigDiagnostics::default(),
                tabs: vec![],
                rows: vec![],
                resolved_metadata: vec![],
                observed_identities: vec![],
                grouping_diagnostics: vec![],
                metadata_controls: MetadataControls::default(),
                inspected_node: None,
                collapsed_groups: vec![],
                display_variables: vec![],
                display_variable_values: BTreeMap::new(),
                surface_regions: vec![],
            },
        }
    }

    pub(super) fn project_sidebar(project_names: &[&str]) -> Self {
        let mut fixture = Self::empty();
        for (position, project_name) in project_names.iter().enumerate() {
            let tab_id = position as u64 + 1;
            let path = GroupPath(vec![GroupSegment {
                key: "project".to_owned(),
                value: MetadataValue::Text((*project_name).to_owned()),
                label: Some((*project_name).to_owned()),
            }]);
            fixture.model.tabs.push(TabCard {
                tab_id,
                position,
                name: format!("{project_name}/work"),
                active: position == 0,
                pinned: false,
                status: None,
                grouping: None,
                templates: ResolvedTemplateSlots::default(),
                active_pane: None,
            });
            fixture.model.rows.extend([
                RailRow::GroupHeader {
                    group_id: format!("project:{project_name}"),
                    path: path.clone(),
                    label: (*project_name).to_owned(),
                    full_label: (*project_name).to_owned(),
                    tab_count: 1,
                    templates: ResolvedTemplateSlots::default(),
                },
                RailRow::Tab {
                    tab_id,
                    indent: 2,
                    parent_path: Some(path),
                },
            ]);
        }

        fixture
    }

    pub(super) fn with_latent_entity(mut self, entity: DisplayEntity) -> Self {
        self.model.rows.push(RailRow::Latent {
            latent: LatentTab {
                entity: entity.entity,
                action_target: "github/open-issue".to_owned(),
                path: GroupPath::default(),
                name: entity.label,
                materialization: LatentMaterializationState::Ready,
                status_state: None,
                summary: None,
                source: Some("github".to_owned()),
                materialize_recipe: None,
                checkout_path: None,
                templates: entity.templates,
            },
            indent: 0,
            parent_path: None,
        });
        self
    }

    pub(super) fn with_header_region(mut self) -> Self {
        self.model.surface_regions.push(DisplayRegion {
            definition: andamento_shared::template_config::SurfaceRegionDefinition {
                name: "header".to_owned(),
                source: andamento_shared::template_config::SurfaceRegionSource::Header,
                root_template: "region/header".to_owned(),
                form: "full".to_owned(),
                attention_key: None,
                pinned: false,
                promotions: vec![],
            },
            root: None,
            entities: vec![],
        });
        self
    }

    pub(super) fn build(self) -> ControllerViewModel {
        self.model
    }
}

pub(super) struct LocalTabsFixture;

impl LocalTabsFixture {
    pub(super) fn named(names: &[&str]) -> Vec<LocalTab> {
        names
            .iter()
            .enumerate()
            .map(|(position, name)| LocalTab {
                tab_id: position as u64 + 1,
                position,
                name: (*name).to_owned(),
                active: position == 0,
            })
            .collect()
    }
}

pub(super) struct DetailSurfaceFixture {
    target: Option<NodeKey>,
}

impl DetailSurfaceFixture {
    pub(super) fn hidden() -> Self {
        Self { target: None }
    }
}

pub(super) fn entity(kind: &str, id: &str, label: &str) -> DisplayEntity {
    DisplayEntity {
        entity: EntityRef {
            kind: kind.to_owned(),
            id: id.to_owned(),
        },
        label: label.to_owned(),
        form: "full".to_owned(),
        metadata: BTreeMap::new(),
        templates: ResolvedTemplateSlots::default(),
    }
}

pub(super) struct RailFrameFixture {
    model: Option<ControllerViewModel>,
    tabs: Vec<LocalTab>,
    rows: usize,
    cols: usize,
    controller_available: bool,
    detail_surface: DetailSurfaceFixture,
}

impl RailFrameFixture {
    pub(super) fn new(rows: usize, cols: usize) -> Self {
        Self {
            model: None,
            tabs: vec![],
            rows,
            cols,
            controller_available: true,
            detail_surface: DetailSurfaceFixture::hidden(),
        }
    }

    pub(super) fn with_model(mut self, model: ControllerModelFixture) -> Self {
        self.model = Some(model.build());
        self
    }

    pub(super) fn with_tabs(mut self, tabs: Vec<LocalTab>) -> Self {
        self.tabs = tabs;
        self
    }

    pub(super) fn hover_entity(mut self, entity: &DisplayEntity) -> Self {
        let initial = self.render();
        let target = NodeKey::Entity(entity.entity.clone());
        initial
            .hit_regions
            .iter()
            .find(|hit| hit.inspect_target.as_ref() == Some(&target))
            .unwrap_or_else(|| panic!("rendered frame has no hit region for {target:?}"));
        self.detail_surface.target = Some(target);
        self
    }

    pub(super) fn controller_available(mut self, available: bool) -> Self {
        self.controller_available = available;
        self
    }

    pub(super) fn render(&self) -> RenderedRail {
        let default_metadata_controls = MetadataControls::default();
        let metadata_controls = self
            .model
            .as_ref()
            .map(|model| &model.metadata_controls)
            .unwrap_or(&default_metadata_controls);
        render_lines_with_detail_surface(
            self.model.as_ref(),
            &self.tabs,
            self.rows,
            self.cols,
            self.controller_available,
            None,
            None,
            &[],
            None,
            metadata_controls,
            0,
            false,
            self.detail_surface.target.as_ref(),
        )
    }

    pub(super) fn snapshot(&self) -> String {
        let rendered = self.render();
        assert_eq!(
            rendered.lines.len(),
            self.rows,
            "rendered frame has {} rows, expected {}",
            rendered.lines.len(),
            self.rows
        );
        let mut snapshot = format!("frame {}x{}", self.cols, self.rows);
        for (index, line) in rendered.lines.iter().enumerate() {
            let plain = strip_ansi(line);
            let visible_width = UnicodeWidthStr::width(plain.as_str());
            assert_eq!(
                visible_width, self.cols,
                "rendered row {index} has width {visible_width}, expected {}",
                self.cols
            );
            let content = plain.trim_end();
            let trailing = self.cols.saturating_sub(UnicodeWidthStr::width(content));
            snapshot.push_str(&format!("\n{index:02} |{content}|"));
            if trailing > 0 {
                snapshot.push_str(&format!(" + {trailing} spaces"));
            }
        }
        snapshot
    }
}

fn strip_ansi(line: &str) -> String {
    let mut plain = String::new();
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
            plain.push(ch);
        }
    }
    plain
}
