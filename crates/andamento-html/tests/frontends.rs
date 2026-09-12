use andamento_core::{sidebar::Action, MetadataPatch, Sidebar};

#[test]
fn native_and_terminal_geometry_share_actions_and_placement_state() {
    let mut sidebar = Sidebar::new(include_str!(
        "../../andamento-core/tests/fixtures/sidebar.kdl"
    ))
    .unwrap();
    let patches = include_str!("../../andamento-core/tests/fixtures/sidebar.jsonl")
        .lines()
        .map(|l| serde_json::from_str::<MetadataPatch>(l).unwrap())
        .collect::<Vec<_>>();
    sidebar.apply(0, patches);
    let before = sidebar.snapshot();
    let terminal = andamento_terminal::surface::render(&before.surface, 32);
    let html = andamento_html::render(&before);
    assert!(terminal.lines.iter().any(|l| l.contains("Worker <one>")));
    assert!(html.contains("Worker &lt;one&gt;"));
    assert!(!html.contains("Worker <one>"));
    assert!(!html.contains('\u{1b}'));
    let action = terminal
        .hits
        .iter()
        .map(|hit| &hit.action)
        .find(|a| matches!(a, Action::TogglePlacement { .. }))
        .unwrap()
        .clone();
    assert!(html.contains(&andamento_html::escape(
        &serde_json::to_string(&action).unwrap()
    )));
    sidebar.dispatch(action).unwrap();
    let after = sidebar.snapshot();
    let collapsed = andamento_terminal::surface::render(&after.surface, 32);
    assert!(collapsed.lines.len() < terminal.lines.len());
    assert_eq!(
        after.surface.sections[1].nodes,
        before.surface.sections[1].nodes
    );
    assert!(andamento_html::render(&after).contains("<details  data-toggle="));
}
