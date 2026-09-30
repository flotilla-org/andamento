use super::render::*;
use andamento_core::{state::ControllerState, MetadataControls, NodeKey, RailUiAction};
use unicode_width::UnicodeWidthStr;

fn state() -> ControllerState {
    let mut state = ControllerState::default();
    for line in include_str!("../../andamento-core/tests/fixtures/default.jsonl").lines() {
        state.apply_metadata_patch(serde_json::from_str(line).unwrap());
    }
    state
}
fn frame(state: &ControllerState, cols: usize) -> RenderedRail {
    render_lines(Some(&state.view_model()), &[], 16, cols, true)
}
fn snapshot(frame: &RenderedRail) -> String {
    frame.lines.iter().map(|line| format!("|{line}|\n")).collect()
}

#[test]
fn shipped_defaults_render_project_pills_nested_convoys_and_one_issue_row() {
    let frame = frame(&state(), 64);
    insta::assert_snapshot!("default_placement_64", snapshot(&frame));
    let project = frame.lines.iter().find(|l| l.contains("Andamento")).unwrap();
    assert!(project.contains("governor") && project.contains("tui"));
    let issues = frame.lines.iter().find(|l| l.contains("#71")).unwrap();
    assert!(issues.contains("#72"));
    assert!(!project.contains("#71"));
    assert_eq!(frame.lines.iter().filter(|l| l.contains("Placement cutover")).count(), 1);
    assert!(frame.lines.iter().any(|l| l.contains("coder")));
}

#[test]
fn an_inline_loop_moves_every_pill_when_the_parent_line_is_too_narrow() {
    let frame = frame(&state(), 24);
    insta::assert_snapshot!("default_placement_24", snapshot(&frame));
    let project = frame.lines.iter().find(|l| l.contains("Andamento")).unwrap();
    assert!(!project.contains("governor") && !project.contains("tui"));
    for label in ["governor", "tui"] {
        assert!(frame.lines.iter().any(|l| l.contains(label)));
    }
    assert!(frame.hit_regions.iter().all(|hit| hit.col_end < 24 && hit.row_end < 16));
}

#[test]
fn collapsed_placements_hide_all_descendants_and_keep_the_toggle() {
    let mut state = state();
    let key = state.view_model().presentation.unwrap().sections[1].nodes[0].key.clone();
    state.apply_rail_ui_action(RailUiAction::TogglePlacement { key });
    let frame = frame(&state, 64);
    assert!(frame.lines.iter().any(|line| line.contains("▶ Andamento")));
    for label in ["governor", "tui", "Placement cutover", "#71", "coder"] {
        assert!(!frame.lines.iter().any(|l| l.contains(label)), "{label}");
    }
}

#[test]
fn narrow_and_empty_viewports_keep_text_and_hits_in_bounds() {
    let state = state();
    for cols in [0, 1, 2, 4, 12, 24, 64] {
        for rows in [0, 1, 2, 5, 20] {
            let model = state.view_model();
            let frame = render_lines(Some(&model), &[], rows, cols, true);
            assert_eq!(frame.lines.len(), rows);
            assert!(frame.lines.iter().all(|l| l.width() <= cols), "{rows}x{cols}");
            assert!(frame.hit_regions.iter().all(|h| h.row_end < rows && h.col_end < cols), "{rows}x{cols}");
        }
    }
}

#[test]
fn detail_panel_is_reserved_and_controls_remain_clickable_below_it() {
    let state = state();
    let model = state.view_model();
    let project = &model.presentation.as_ref().unwrap().sections[1].nodes[0];
    let key = NodeKey::Placement(project.children[3].key.clone());
    let render = |target| render_lines_with_detail_surface(Some(&model), &[], 18, 48, true,
        None, None, &[], None, &MetadataControls::default(), 0, false, target);
    let empty = render(None);
    let hovered = render(Some(&key));
    assert_eq!(empty.available_rows, hovered.available_rows);
    assert!(hovered.lines.iter().any(|line| line.contains("Replace the push projection")));
    assert!(hovered.hit_regions.iter().any(|h| h.row_start == 17 && h.action == HitAction::OpenConfig));
}

#[test]
fn plain_observed_workspaces_remain_reachable_without_catalog_entities() {
    let mut state = ControllerState::default();
    state.observe_workspaces(vec![andamento_core::state::ControllerTab {tab_id:42,position:3,name:"Scratch".into(),active:true}]);
    let frame = frame(&state, 40);
    let hit = frame.hit_regions.iter().find(|h| h.tab_id == 42).unwrap();
    assert_eq!(hit.action, HitAction::SwitchTab);
    assert_eq!(hit.tab_position, 3);
    assert!(frame.lines.iter().any(|line| line.contains("Scratch")));
}

#[test]
fn viewport_scroll_preserves_pinned_controls_and_reveals_the_remaining_rows() {
    let state = state();
    let model = state.view_model();
    let render = |offset| render_lines_with_rail_scroll(Some(&model), &[], 5, 32, true,
        None, None, &[], None, &MetadataControls::default(), offset);
    let first = render(0);
    let last = render(100);
    assert!(first.can_scroll());
    assert_ne!(first.lines, last.lines);
    assert_eq!(first.lines[4], last.lines[4]);
    assert!(last.lines.iter().any(|line| line.contains("Empty project")));
    assert!(last.hit_regions.iter().any(|hit| hit.row_start == 4 && hit.action == HitAction::ScrollRailUp));
}
