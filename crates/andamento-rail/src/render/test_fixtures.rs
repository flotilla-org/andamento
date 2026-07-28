use super::{render_lines_with_detail_surface, LocalTab, RenderedRail};
use andamento_shared::{
    ControllerViewModel, DisplayEntity, EntityRef, GroupPath, GroupSegment, MetadataControls,
    MetadataValue, NodeKey, RailConfig, RailGroupingMode, RailRow, RailSizingPreset,
    ResolvedTemplateSlots, SortMode, TabCard, TemplateConfigDiagnostics,
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
        fixture.model.config.grouping = RailGroupingMode::Directory;
        fixture.model.config.sizing = RailSizingPreset::Compact;

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

    pub(super) fn with_entity(mut self, entity: DisplayEntity) -> Self {
        self.model.rows.push(RailRow::Entity {
            entity,
            indent: 0,
            parent_path: None,
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

    pub(super) fn hovering(entity: &DisplayEntity) -> Self {
        Self {
            target: Some(NodeKey::Entity(entity.entity.clone())),
        }
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

    pub(super) fn with_detail_surface(mut self, detail_surface: DetailSurfaceFixture) -> Self {
        self.detail_surface = detail_surface;
        self
    }

    pub(super) fn controller_available(mut self, available: bool) -> Self {
        self.controller_available = available;
        self
    }

    pub(super) fn render(&self) -> RenderedRail {
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
            self.model
                .as_ref()
                .map(|model| &model.metadata_controls)
                .unwrap_or_else(|| empty_metadata_controls()),
            0,
            false,
            self.detail_surface.target.as_ref(),
        )
    }

    pub(super) fn snapshot(&self) -> String {
        let rendered = self.render();
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

fn empty_metadata_controls() -> &'static MetadataControls {
    static EMPTY: std::sync::OnceLock<MetadataControls> = std::sync::OnceLock::new();
    EMPTY.get_or_init(MetadataControls::default)
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
