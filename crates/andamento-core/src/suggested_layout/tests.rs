use super::*;
use crate::FactMap;
use std::rc::Rc;

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
}
fn text(value: &str) -> MetadataValue {
    MetadataValue::Text(value.into())
}
fn list(values: &[&str]) -> MetadataValue {
    MetadataValue::StringList(values.iter().map(|v| v.to_string()).collect())
}
fn facts(pairs: impl IntoIterator<Item = (&'static str, MetadataValue)>) -> FactMap {
    pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
}
fn convoy() -> EntityRef {
    entity("convoy", "c")
}
fn parse_ok(facts: &FactMap) -> SuggestedLayout {
    parse(&convoy(), facts, PrimarySlot::IfPublished).unwrap().unwrap()
}
fn parse_err(facts: &FactMap) -> LayoutError {
    parse(&convoy(), facts, PrimarySlot::IfPublished).unwrap_err()
}
fn primary_facts() -> FactMap {
    facts([
        ("workspace.primary.state", text("ready")),
        ("workspace.primary.target", text("vessel:v1")),
        ("action.primary.recipe", text("flotilla attach v1")),
        ("git.root", text("/work")),
    ])
}
/// A minimal valid two-slot layout: one local command and one facet.
fn two_slots() -> FactMap {
    facts([
        ("layout.version", text("7")),
        ("layout.slots", list(&["shell", "pr"])),
        ("layout.slot.shell.kind", text("command")),
        ("layout.slot.shell.argv.0", text("git")),
        ("layout.slot.shell.argv.1", text("status")),
        ("layout.slot.shell.cwd", text("/work")),
        ("layout.slot.pr.entity", MetadataValue::EntityRefs(vec![entity("change_request", "gh/o/r!1")])),
        ("layout.slot.pr.facet", text("review")),
        ("layout.slot.pr.state", text("ready")),
        ("layout.slot.pr.target", text("cr:1")),
        ("layout.slot.pr.kind", text("url")),
        ("layout.slot.pr.url", text("https://example.test/1")),
    ])
}
fn with<K: Into<Rc<str>>>(mut base: FactMap, pairs: impl IntoIterator<Item = (K, MetadataValue)>) -> FactMap {
    base.extend(pairs.into_iter().map(|(k, v)| (k.into(), v)));
    base
}
fn without(mut base: FactMap, keys: &[&str]) -> FactMap {
    for key in keys {
        base.remove(*key);
    }
    base
}

#[test]
fn no_layout_or_primary_facts_is_no_layout() {
    let facts = facts([("display.label", text("x"))]);
    assert_eq!(parse(&convoy(), &facts, PrimarySlot::IfPublished), Ok(None));
}

#[test]
fn primary_facts_alone_are_a_one_slot_layout() {
    let layout = parse_ok(&primary_facts());
    assert_eq!(layout.version, BaselineVersion::PrimaryOnly);
    assert_eq!(layout.arrangement, None);
    assert_eq!(
        layout.slots,
        vec![Slot {
            key: "primary".into(),
            spec: ViewSpec {
                content: Content::ProviderFacet { entity: convoy(), facet: "primary".into() },
                presentation: None,
                role: Some("agent".into())
            },
            rebind: RebindPolicy::Replace,
            resolution: SlotResolution::Ready {
                target: Some("vessel:v1".into()),
                recipe: LocalRecipe::Command { line: CommandLine::Shell("flotilla attach v1".into()), cwd: Some("/work".into()) },
            },
        }]
    );
}

// The same reading as managed primary content: held, and every incomplete or
// malformed ready resolution, keep the slot but make it unresolved.
#[test]
fn primary_resolution_follows_managed_content_rules() {
    let held = with(primary_facts(), [("workspace.primary.state", text("held"))]);
    assert_eq!(parse_ok(&held).slots[0].resolution, SlotResolution::Held);
    for broken in [
        with(primary_facts(), [("workspace.primary.state", text("bogus"))]),
        with(primary_facts(), [("workspace.primary.target", text(""))]),
        without(primary_facts(), &["action.primary.recipe"]),
        with(primary_facts(), [("action.primary.recipe", MetadataValue::Bool(true))]),
    ] {
        let layout = parse_ok(&broken);
        assert_eq!(layout.slots.len(), 1);
        assert_eq!(layout.slots[0].resolution, SlotResolution::Unavailable);
    }
    let no_cwd = without(primary_facts(), &["git.root"]);
    assert!(matches!(&parse_ok(&no_cwd).slots[0].resolution, SlotResolution::Ready { recipe: LocalRecipe::Command { cwd: None, .. }, .. }));
    // Without a state there is no primary slot, as there is no desired content.
    let stateless = without(primary_facts(), &["workspace.primary.state"]);
    assert_eq!(parse(&convoy(), &stateless, PrimarySlot::IfPublished), Ok(None));
}

#[test]
fn local_and_facet_slots_parse_without_an_arrangement() {
    let layout = parse_ok(&two_slots());
    assert_eq!(layout.version, BaselineVersion::Published("7".into()));
    assert_eq!(layout.arrangement, None);
    let shell = layout.slot("shell").unwrap();
    let command = LocalRecipe::Command { line: CommandLine::Argv(vec!["git".into(), "status".into()]), cwd: Some("/work".into()) };
    assert_eq!(shell.spec.content, Content::Local(command.clone()));
    assert_eq!(shell.resolution, SlotResolution::Ready { target: None, recipe: command });
    let pr = layout.slot("pr").unwrap();
    assert_eq!(pr.spec.content, Content::ProviderFacet { entity: entity("change_request", "gh/o/r!1"), facet: "review".into() });
    assert_eq!(
        pr.resolution,
        SlotResolution::Ready { target: Some("cr:1".into()), recipe: LocalRecipe::Url { url: "https://example.test/1".into() } }
    );
}

#[test]
fn every_local_recipe_kind_parses() {
    let layout = parse_ok(&facts([
        ("layout.version", text("1")),
        ("layout.slots", list(&["sh", "doc", "web", "game"])),
        ("layout.slot.sh.kind", text("command")),
        ("layout.slot.sh.command", text("make test")),
        ("layout.slot.doc.kind", text("file")),
        ("layout.slot.doc.path", text("/r/notes.md")),
        ("layout.slot.web.kind", text("url")),
        ("layout.slot.web.url", text("https://luchs.test/page")),
        ("layout.slot.game.kind", text("jackstay")),
        ("layout.slot.game.launcher", text("retroarch")),
        ("layout.slot.game.endpoint", text("lab:4711")),
        ("layout.slot.game.presentation", text("stream")),
        ("layout.slot.game.rebind", text("ask")),
    ]));
    let recipes: Vec<_> = layout.slots.iter().map(|s| s.spec.content.clone()).collect();
    assert_eq!(
        recipes,
        vec![
            Content::Local(LocalRecipe::Command { line: CommandLine::Shell("make test".into()), cwd: None }),
            Content::Local(LocalRecipe::File { path: "/r/notes.md".into() }),
            Content::Local(LocalRecipe::Url { url: "https://luchs.test/page".into() }),
            Content::Local(LocalRecipe::Jackstay { launcher: "retroarch".into(), endpoint: "lab:4711".into() }),
        ]
    );
    let game = layout.slot("game").unwrap();
    assert_eq!(game.spec.presentation.as_deref(), Some("stream"));
    assert_eq!(game.rebind, RebindPolicy::Ask);
}

#[test]
fn argv_is_preferred_over_shell_text() {
    let both = with(two_slots(), [("layout.slot.shell.command", text("legacy"))]);
    assert!(matches!(
        &parse_ok(&both).slot("shell").unwrap().spec.content,
        Content::Local(LocalRecipe::Command { line: CommandLine::Argv(_), .. })
    ));
}

// Incomplete facet resolution never invalidates the layout's spec.
#[test]
fn facet_resolution_problems_make_the_slot_unavailable() {
    for broken in [
        without(two_slots(), &["layout.slot.pr.state"]),
        with(two_slots(), [("layout.slot.pr.state", text("bogus"))]),
        without(two_slots(), &["layout.slot.pr.target"]),
        without(two_slots(), &["layout.slot.pr.url"]),
        with(two_slots(), [("layout.slot.pr.kind", text("teleport"))]),
        with(two_slots(), [("layout.slot.pr.state", MetadataValue::Integer(1))]),
    ] {
        let layout = parse_ok(&broken);
        assert_eq!(layout.slot("pr").unwrap().resolution, SlotResolution::Unavailable);
        assert!(matches!(layout.slot("shell").unwrap().resolution, SlotResolution::Ready { .. }));
    }
    let held = without(
        with(two_slots(), [("layout.slot.pr.state", text("held"))]),
        &["layout.slot.pr.kind", "layout.slot.pr.url", "layout.slot.pr.target"],
    );
    assert_eq!(parse_ok(&held).slot("pr").unwrap().resolution, SlotResolution::Held);
}

#[test]
fn local_slots_may_be_held() {
    let held = with(two_slots(), [("layout.slot.shell.state", text("held"))]);
    assert_eq!(parse_ok(&held).slot("shell").unwrap().resolution, SlotResolution::Held);
}

#[test]
fn primary_joins_a_published_layout() {
    // Unlisted, it leads the slot order.
    let joined = parse_ok(&with(two_slots(), primary_facts()));
    let keys: Vec<_> = joined.slots.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["primary", "shell", "pr"]);
    assert_eq!(joined.version, BaselineVersion::Published("7".into()));
    // Listed, it takes its listed position.
    let listed = with(with(two_slots(), primary_facts()), [("layout.slots", list(&["shell", "primary", "pr"]))]);
    let keys: Vec<_> = parse_ok(&listed).slots.iter().map(|s| s.key.clone()).collect();
    assert_eq!(keys, ["shell", "primary", "pr"]);
    // Listed without its facts is a torn publication.
    let torn = with(two_slots(), [("layout.slots", list(&["shell", "primary", "pr"]))]);
    assert_eq!(parse_err(&torn), LayoutError::MissingPrimary);
    let reserved = with(with(two_slots(), primary_facts()), [("layout.slot.primary.kind", text("url"))]);
    assert!(matches!(parse_err(&reserved), LayoutError::ReservedPrimary { .. }));
}

#[test]
fn unknown_slot_fields_and_presentations_are_carried_or_ignored() {
    let layout = parse_ok(&with(
        two_slots(),
        [
            ("layout.slot.shell.future", MetadataValue::Bool(true)),
            ("layout.slot.pr.presentation", text("hologram")),
            ("layout.future", text("x")),
        ],
    ));
    assert_eq!(layout.slot("pr").unwrap().spec.presentation.as_deref(), Some("hologram"));
}

#[test]
fn slots_carry_their_role_and_the_primary_defaults_by_subject_kind() {
    let layout = parse_ok(&with(
        two_slots(),
        [("layout.slot.pr.role", text("review")), ("layout.slot.shell.role", text("")), ("layout.slot.pr.future", text("x"))],
    ));
    assert_eq!(layout.slot("pr").unwrap().spec.role.as_deref(), Some("review"));
    assert_eq!(layout.slot("shell").unwrap().spec.role, None, "an empty role is none");
    assert!(matches!(parse_err(&with(two_slots(), [("layout.slot.pr.role", list(&["a"]))])), LayoutError::WrongType { .. }));
    let primary = |kind: &str| {
        let layout = parse(&entity(kind, "x"), &FactMap::new(), PrimarySlot::Always).unwrap().unwrap();
        layout.slots[0].spec.role.clone()
    };
    assert_eq!(primary("vessel").as_deref(), Some("agent"));
    assert_eq!(primary("convoy").as_deref(), Some("agent"));
    assert_eq!(primary("project").as_deref(), Some("tui"));
    assert_eq!(primary("role").as_deref(), Some("tui"));
    assert_eq!(primary("issue"), None);
}

#[test]
fn a_view_spec_recorded_before_roles_reads_without_one() {
    let spec: ViewSpec = serde_json::from_str(r#"{"content":{"Local":{"Url":{"url":"https://example.test"}}},"presentation":null}"#)
        .expect("an old spec reads");
    assert_eq!(spec.role, None);
}

#[test]
fn malformed_specs_are_errors() {
    use LayoutError::*;
    let cases: Vec<(FactMap, LayoutError)> = vec![
        (without(two_slots(), &["layout.version"]), MissingFact { key: "layout.version".into() }),
        (without(two_slots(), &["layout.slots"]), MissingFact { key: "layout.slots".into() }),
        (with(two_slots(), [("layout.version", MetadataValue::Integer(7))]), WrongType { key: "layout.version".into(), expected: "text" }),
        (with(two_slots(), [("layout.slots", list(&["shell", "pr", "shell"]))]), DuplicateSlot { slot: "shell".into() }),
        (
            with(two_slots(), [("layout.slots", list(&["shell", "pr", "Bad.Key"]))]),
            InvalidId { key: "layout.slots".into(), id: "Bad.Key".into() },
        ),
        (with(two_slots(), [("layout.slots", list(&["shell"]))]), UnlistedSlot { key: "layout.slot.pr.entity".into() }),
        (without(two_slots(), &["layout.slot.pr.facet"]), IncompleteFacet { slot: "pr".into() }),
        (
            with(two_slots(), [("layout.slot.pr.entity", MetadataValue::EntityRefs(vec![entity("a", "1"), entity("b", "2")]))]),
            WrongType { key: "layout.slot.pr.entity".into(), expected: "exactly one entity ref" },
        ),
        (without(two_slots(), &["layout.slot.shell.kind"]), MissingFact { key: "layout.slot.shell.kind".into() }),
        (
            with(two_slots(), [("layout.slot.shell.kind", text("teleport"))]),
            InvalidValue { key: "layout.slot.shell.kind".into(), value: "teleport".into() },
        ),
        (without(two_slots(), &["layout.slot.shell.argv.0"]), ArgvGap { slot: "shell".into() }),
        (with(two_slots(), [("layout.slot.shell.argv.01", text("x"))]), ArgvGap { slot: "shell".into() }),
        (
            without(two_slots(), &["layout.slot.shell.argv.0", "layout.slot.shell.argv.1"]),
            MissingFact { key: "layout.slot.shell.argv.0".into() },
        ),
        (
            with(two_slots(), [("layout.slot.shell.rebind", text("sometimes"))]),
            InvalidValue { key: "layout.slot.shell.rebind".into(), value: "sometimes".into() },
        ),
        (
            with(two_slots(), [("layout.slot.shell.state", text("bogus"))]),
            InvalidValue { key: "layout.slot.shell.state".into(), value: "bogus".into() },
        ),
    ];
    for (facts, expected) in cases {
        assert_eq!(parse(&convoy(), &facts, PrimarySlot::IfPublished), Err(expected));
    }
}

fn arranged() -> FactMap {
    with(
        two_slots(),
        [
            ("layout.root", text("main")),
            ("layout.panel.main.axis", text("column")),
            ("layout.panel.main.children", list(&["top", "bottom"])),
            ("layout.panel.top.slots", list(&["shell"])),
            ("layout.panel.top.weight", MetadataValue::Integer(3)),
            ("layout.panel.bottom.slots", list(&["pr"])),
        ],
    )
}

#[test]
fn arrangement_parses_splits_tabs_weights_and_selection() {
    let arrangement = parse_ok(&arranged()).arrangement.unwrap();
    assert_eq!(arrangement.root, "main");
    assert_eq!(
        arrangement.panels["main"],
        Panel { weight: 1, node: PanelNode::Split { axis: Axis::Column, children: vec!["top".into(), "bottom".into()] } }
    );
    assert_eq!(
        arrangement.panels["top"],
        Panel { weight: 3, node: PanelNode::Tabs { slots: vec!["shell".into()], selected: "shell".into() } }
    );
    let tabs = with(
        arranged(),
        [
            ("layout.panel.main.children", list(&["top"])),
            ("layout.panel.top.slots", list(&["shell", "pr"])),
            ("layout.panel.top.selected", text("pr")),
        ],
    );
    let tabs = without(tabs, &["layout.panel.bottom.slots"]);
    assert_eq!(
        parse_ok(&tabs).arrangement.unwrap().panels["top"].node,
        PanelNode::Tabs { slots: vec!["shell".into(), "pr".into()], selected: "pr".into() }
    );
}

#[test]
fn arrangement_errors() {
    use LayoutError::*;
    let cases: Vec<(FactMap, LayoutError)> = vec![
        (with(arranged(), [("layout.panel.bottom.slots", list(&["x"]))]), UnknownSlot { panel: "bottom".into(), slot: "x".into() }),
        (with(arranged(), [("layout.panel.top.slots", list(&["shell", "pr"]))]), SlotInTwoPanels { slot: "pr".into() }),
        (without(arranged(), &["layout.root"]), MissingFact { key: "layout.root".into() }),
        (with(arranged(), [("layout.panel.main.children", list(&["top", "gone"]))]), UnknownPanel { panel: "gone".into() }),
        (with(arranged(), [("layout.panel.main.children", list(&["top", "top"]))]), PanelReused { panel: "top".into() }),
        (with(arranged(), [("layout.panel.main.children", list(&["top", "main"]))]), PanelReused { panel: "main".into() }),
        (with(arranged(), [("layout.panel.main.children", list(&["top"]))]), UnreachablePanel { panel: "bottom".into() }),
        (with(arranged(), [("layout.panel.main.slots", list(&["shell"]))]), PanelShape { panel: "main".into() }),
        (without(arranged(), &["layout.panel.main.axis"]), PanelShape { panel: "main".into() }),
        (with(arranged(), [("layout.panel.bottom.slots", list(&[]))]), PanelShape { panel: "bottom".into() }),
        (
            with(arranged(), [("layout.panel.main.axis", text("diagonal"))]),
            InvalidValue { key: "layout.panel.main.axis".into(), value: "diagonal".into() },
        ),
        (
            with(arranged(), [("layout.panel.top.weight", MetadataValue::Integer(0))]),
            InvalidValue { key: "layout.panel.top.weight".into(), value: "0".into() },
        ),
        (with(arranged(), [("layout.panel.top.selected", text("pr"))]), SelectedNotInPanel { panel: "top".into(), slot: "pr".into() }),
        (with(arranged(), [("layout.root", text("Main"))]), InvalidId { key: "layout.root".into(), id: "Main".into() }),
    ];
    for (facts, expected) in cases {
        assert_eq!(parse(&convoy(), &facts, PrimarySlot::IfPublished), Err(expected));
    }
}

// A slot the provider's arrangement does not place is still part of the
// layout; frontends place it by the default rule.
#[test]
fn unplaced_slots_are_allowed() {
    let facts = with(two_slots(), [("layout.root", text("only")), ("layout.panel.only.slots", list(&["shell"]))]);
    let layout = parse_ok(&facts);
    assert_eq!(layout.slots.len(), 2);
    assert_eq!(layout.arrangement.unwrap().panels.len(), 1);
}

// A workspace subject keeps its primary slot whatever the provider publishes:
// resolved from the opening recipe when there are no workspace.primary.*
// facts, and unavailable while there is no recipe either.
#[test]
fn a_subject_always_has_a_primary_slot() {
    let vessel = entity("vessel", "v1");
    let recipe = facts([("action.primary.recipe", text("flotilla attach v1")), ("git.root", text("/work"))]);
    let layout = parse(&vessel, &recipe, PrimarySlot::Always).unwrap().unwrap();
    assert_eq!(layout.version, BaselineVersion::PrimaryOnly);
    assert_eq!(
        layout.slots[0].resolution,
        SlotResolution::Ready {
            target: Some(vessel.action_target().to_string()),
            recipe: LocalRecipe::Command { line: CommandLine::Shell("flotilla attach v1".into()), cwd: Some("/work".into()) },
        }
    );
    let targeted = with(recipe.clone(), [("action.primary.target", text("vessel:elsewhere"))]);
    assert!(matches!(&parse(&vessel, &targeted, PrimarySlot::Always).unwrap().unwrap().slots[0].resolution,
        SlotResolution::Ready { target: Some(target), .. } if target == "vessel:elsewhere"));

    let bare = facts([("display.label", text("v1"))]);
    let layout = parse(&vessel, &bare, PrimarySlot::Always).unwrap().unwrap();
    assert_eq!(layout.slots.len(), 1);
    assert_eq!(layout.slots[0].key, "primary");
    assert_eq!(layout.slots[0].resolution, SlotResolution::Unavailable);
    // Not a subject: the same facts publish no layout.
    assert_eq!(parse(&vessel, &bare, PrimarySlot::IfPublished), Ok(None));
    // Published workspace.primary.* facts still win.
    let held = with(recipe, [("workspace.primary.state", text("held"))]);
    assert_eq!(parse(&vessel, &held, PrimarySlot::Always).unwrap().unwrap().slots[0].resolution, SlotResolution::Held);
}

fn cleat_slot() -> FactMap {
    facts([
        ("layout.version", text("1")),
        ("layout.slots", list(&["crew"])),
        ("layout.slot.crew.kind", text("cleat-session")),
        ("layout.slot.crew.transport", text("ssh")),
        ("layout.slot.crew.host", text("robert@feta")),
        ("layout.slot.crew.runtime_root", text("/run/cleat")),
        ("layout.slot.crew.daemon", text("d1")),
        ("layout.slot.crew.session", text("s1")),
    ])
}
fn cleat(transport: CleatTransport, host: &str) -> LocalRecipe {
    LocalRecipe::CleatSession { transport, host: host.into(), runtime_root: "/run/cleat".into(), daemon: "d1".into(), session: "s1".into() }
}

#[test]
fn a_cleat_session_slot_parses_and_needs_every_field_and_a_known_transport() {
    let crew = parse_ok(&cleat_slot());
    assert_eq!(crew.slot("crew").unwrap().spec.content, Content::Local(cleat(CleatTransport::Ssh, "robert@feta")));
    let local = with(cleat_slot(), [("layout.slot.crew.transport", text("local"))]);
    assert_eq!(parse_ok(&local).slot("crew").unwrap().spec.content, Content::Local(cleat(CleatTransport::Local, "robert@feta")));
    for field in ["transport", "host", "runtime_root", "daemon", "session"] {
        let key = format!("layout.slot.crew.{field}");
        assert_eq!(parse_err(&without(cleat_slot(), &[key.as_str()])), LayoutError::MissingFact { key: key.clone() }, "{field}");
        assert_eq!(parse_err(&with(cleat_slot(), [(key.as_str(), text(""))])), LayoutError::MissingFact { key: key.clone() }, "{field}");
    }
    let mosh = with(cleat_slot(), [("layout.slot.crew.transport", text("mosh"))]);
    assert_eq!(parse_err(&mosh), LayoutError::InvalidValue { key: "layout.slot.crew.transport".into(), value: "mosh".into() });
}

fn direct_facts() -> FactMap {
    facts([
        ("action.primary.direct.transport", text("ssh")),
        ("action.primary.direct.host", text("robert@feta")),
        ("action.primary.direct.runtime_root", text("/run/cleat")),
        ("action.primary.direct.daemon", text("d1")),
        ("action.primary.direct.session", text("s1")),
    ])
}

// wheelhouse#104: a published direct endpoint opens the primary as a direct
// Cleat packet view, at the same target; anything less keeps the command.
#[test]
fn the_primary_prefers_a_complete_direct_endpoint_to_its_command() {
    let vessel = entity("vessel", "v1");
    let recipe = facts([("action.primary.recipe", text("flotilla attach v1")), ("git.root", text("/work"))]);
    let resolution = |facts: &FactMap| parse(&vessel, facts, PrimarySlot::Always).unwrap().unwrap().slots[0].resolution.clone();
    let target = Some(vessel.action_target().to_string());
    let command = SlotResolution::Ready {
        target: target.clone(),
        recipe: LocalRecipe::Command { line: CommandLine::Shell("flotilla attach v1".into()), cwd: Some("/work".into()) },
    };
    let direct = SlotResolution::Ready { target: target.clone(), recipe: cleat(CleatTransport::Ssh, "robert@feta") };
    assert_eq!(resolution(&with(recipe.clone(), direct_facts())), direct);
    // The endpoint alone is enough.
    assert_eq!(resolution(&direct_facts()), direct);
    // Published workspace.primary.* facts keep their target.
    let ready = with(with(primary_facts(), direct_facts()), [("workspace.primary.target", text("vessel:v1#2"))]);
    assert_eq!(
        parse_ok(&ready).slots[0].resolution,
        SlotResolution::Ready { target: Some("vessel:v1#2".into()), recipe: cleat(CleatTransport::Ssh, "robert@feta") }
    );
    let reason = with(recipe.clone(), [("action.primary.direct.reason", text("no observed Cleat daemon endpoint"))]);
    assert_eq!(resolution(&reason), command);
    for key in ["transport", "host", "runtime_root", "daemon", "session"] {
        let key = format!("action.primary.direct.{key}");
        assert_eq!(resolution(&without(with(recipe.clone(), direct_facts()), &[key.as_str()])), command, "without {key}");
        assert_eq!(resolution(&with(with(recipe.clone(), direct_facts()), [(key.as_str(), text(""))])), command, "empty {key}");
    }
    let unknown = with(with(recipe, direct_facts()), [("action.primary.direct.transport", text("mosh"))]);
    assert_eq!(resolution(&unknown), command);
}

// Workspace records persist View Specs: a Cleat session round-trips.
#[test]
fn a_cleat_session_view_spec_round_trips_through_serde() {
    let spec =
        ViewSpec { content: Content::Local(cleat(CleatTransport::Local, "feta")), presentation: Some("terminal".into()), role: None };
    let json = serde_json::to_string(&spec).unwrap();
    assert!(json.contains(r#""transport":"local""#), "{json}");
    assert_eq!(serde_json::from_str::<ViewSpec>(&json).unwrap(), spec);
}
