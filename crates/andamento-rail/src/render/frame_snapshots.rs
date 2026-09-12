use super::test_fixtures::{entity, ControllerModelFixture, LocalTabsFixture, RailFrameFixture};

#[test]
fn sidebar_with_three_projects() {
    let frame = RailFrameFixture::new(16, 38)
        .with_model(ControllerModelFixture::project_sidebar(&[
            "andamento",
            "flotilla",
            "zellij",
        ]))
        .snapshot();

    insta::assert_snapshot!("sidebar_with_three_projects", frame);
}

#[test]
fn hover_state_renders_visible_detail_panel() {
    let hovered_issue = entity(
        "issue",
        "github/flotilla-org/andamento#49",
        "#49 Keep hover detail visible",
    );
    let frame = RailFrameFixture::new(12, 48)
        .with_model(
            ControllerModelFixture::project_sidebar(&["andamento"])
                .with_latent_entity(hovered_issue.clone()),
        )
        .hover_entity(&hovered_issue)
        .snapshot();

    assert!(
        frame.contains("[issue] #49 Keep hover detail visible"),
        "hovered entity detail must be present in the rendered frame"
    );
    insta::assert_snapshot!("hover_state_renders_visible_detail_panel", frame);
}

#[test]
fn empty_controller_and_unavailable_controller_states() {
    let connected = RailFrameFixture::new(6, 32)
        .with_model(ControllerModelFixture::empty())
        .snapshot();
    let unavailable = RailFrameFixture::new(6, 32)
        .with_tabs(LocalTabsFixture::named(&[]))
        .controller_available(false)
        .snapshot();

    insta::assert_snapshot!(
        "empty_controller_and_unavailable_controller_states",
        format!("connected\n{connected}\n\nunavailable\n{unavailable}")
    );
}

#[test]
fn size_edge_cases() {
    let model_names = ["andamento", "flotilla"];
    let one_row = RailFrameFixture::new(1, 16)
        .with_model(ControllerModelFixture::project_sidebar(&model_names))
        .snapshot();
    let narrow = RailFrameFixture::new(5, 8)
        .with_model(ControllerModelFixture::project_sidebar(&model_names))
        .snapshot();
    let zero_width = RailFrameFixture::new(3, 0)
        .with_model(ControllerModelFixture::project_sidebar(&model_names))
        .snapshot();
    let zero_width_one_row = RailFrameFixture::new(1, 0)
        .with_model(ControllerModelFixture::project_sidebar(&model_names))
        .snapshot();
    let zero_width_unavailable = RailFrameFixture::new(3, 0)
        .controller_available(false)
        .snapshot();
    let zero_width_regions = RailFrameFixture::new(3, 0)
        .with_model(ControllerModelFixture::empty().with_header_region())
        .snapshot();

    insta::assert_snapshot!(
        "size_edge_cases",
        format!(
            "one row\n{one_row}\n\nnarrow\n{narrow}\n\nzero width detail surface\n{zero_width}\n\nzero width one row\n{zero_width_one_row}\n\nzero width unavailable\n{zero_width_unavailable}\n\nzero width configured regions\n{zero_width_regions}"
        )
    );
}

#[test]
fn abbreviation_ladder_declared_tiers() {
    let medium = RailFrameFixture::new(4, 40)
        .with_model(ControllerModelFixture::abbreviation_ladder("medium"))
        .snapshot();
    let short = RailFrameFixture::new(4, 40)
        .with_model(ControllerModelFixture::abbreviation_ladder("short"))
        .snapshot();

    insta::assert_snapshot!(
        "abbreviation_ladder_declared_tiers",
        format!("medium\n{medium}\n\nshort\n{short}")
    );
}
