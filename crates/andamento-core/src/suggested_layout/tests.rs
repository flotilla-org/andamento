use super::*;

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef {
        kind: kind.into(),
        id: id.into(),
    }
}
fn text(value: &str) -> MetadataValue {
    MetadataValue::Text(value.into())
}
fn list(values: &[&str]) -> MetadataValue {
    MetadataValue::StringList(values.iter().map(|v| v.to_string()).collect())
}
fn facts(pairs: impl IntoIterator<Item = (&'static str, MetadataValue)>) -> FactMap {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}
type FactMap = BTreeMap<String, MetadataValue>;
fn convoy() -> EntityRef {
    entity("convoy", "c")
}
fn parse_ok(facts: &FactMap) -> SuggestedLayout {
    parse(&convoy(), facts).unwrap().unwrap()
}
fn parse_err(facts: &FactMap) -> LayoutError {
    parse(&convoy(), facts).unwrap_err()
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
        (
            "layout.slot.pr.entity",
            MetadataValue::EntityRefs(vec![entity("change_request", "gh/o/r!1")]),
        ),
        ("layout.slot.pr.facet", text("review")),
        ("layout.slot.pr.state", text("ready")),
        ("layout.slot.pr.target", text("cr:1")),
        ("layout.slot.pr.kind", text("url")),
        ("layout.slot.pr.url", text("https://example.test/1")),
    ])
}
fn with<K: Into<String>>(
    mut base: FactMap,
    pairs: impl IntoIterator<Item = (K, MetadataValue)>,
) -> FactMap {
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
    assert_eq!(parse(&convoy(), &facts), Ok(None));
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
                content: Content::ProviderFacet {
                    entity: convoy(),
                    facet: "primary".into(),
                },
                presentation: None,
            },
            rebind: RebindPolicy::Replace,
            resolution: SlotResolution::Ready {
                target: Some("vessel:v1".into()),
                recipe: LocalRecipe::Command {
                    line: CommandLine::Shell("flotilla attach v1".into()),
                    cwd: Some("/work".into()),
                },
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
        with(
            primary_facts(),
            [("workspace.primary.state", text("bogus"))],
        ),
        with(primary_facts(), [("workspace.primary.target", text(""))]),
        without(primary_facts(), &["action.primary.recipe"]),
        with(
            primary_facts(),
            [("action.primary.recipe", MetadataValue::Bool(true))],
        ),
    ] {
        let layout = parse_ok(&broken);
        assert_eq!(layout.slots.len(), 1);
        assert_eq!(layout.slots[0].resolution, SlotResolution::Unavailable);
    }
    let no_cwd = without(primary_facts(), &["git.root"]);
    assert!(matches!(
        &parse_ok(&no_cwd).slots[0].resolution,
        SlotResolution::Ready {
            recipe: LocalRecipe::Command { cwd: None, .. },
            ..
        }
    ));
    // Without a state there is no primary slot, as there is no desired content.
    let stateless = without(primary_facts(), &["workspace.primary.state"]);
    assert_eq!(parse(&convoy(), &stateless), Ok(None));
}

#[test]
fn local_and_facet_slots_parse_without_an_arrangement() {
    let layout = parse_ok(&two_slots());
    assert_eq!(layout.version, BaselineVersion::Published("7".into()));
    assert_eq!(layout.arrangement, None);
    let shell = layout.slot("shell").unwrap();
    let command = LocalRecipe::Command {
        line: CommandLine::Argv(vec!["git".into(), "status".into()]),
        cwd: Some("/work".into()),
    };
    assert_eq!(shell.spec.content, Content::Local(command.clone()));
    assert_eq!(
        shell.resolution,
        SlotResolution::Ready {
            target: None,
            recipe: command
        }
    );
    let pr = layout.slot("pr").unwrap();
    assert_eq!(
        pr.spec.content,
        Content::ProviderFacet {
            entity: entity("change_request", "gh/o/r!1"),
            facet: "review".into()
        }
    );
    assert_eq!(
        pr.resolution,
        SlotResolution::Ready {
            target: Some("cr:1".into()),
            recipe: LocalRecipe::Url {
                url: "https://example.test/1".into()
            }
        }
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
    let recipes: Vec<_> = layout
        .slots
        .iter()
        .map(|s| s.spec.content.clone())
        .collect();
    assert_eq!(
        recipes,
        vec![
            Content::Local(LocalRecipe::Command {
                line: CommandLine::Shell("make test".into()),
                cwd: None
            }),
            Content::Local(LocalRecipe::File {
                path: "/r/notes.md".into()
            }),
            Content::Local(LocalRecipe::Url {
                url: "https://luchs.test/page".into()
            }),
            Content::Local(LocalRecipe::Jackstay {
                launcher: "retroarch".into(),
                endpoint: "lab:4711".into()
            }),
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
        Content::Local(LocalRecipe::Command {
            line: CommandLine::Argv(_),
            ..
        })
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
        with(
            two_slots(),
            [("layout.slot.pr.state", MetadataValue::Integer(1))],
        ),
    ] {
        let layout = parse_ok(&broken);
        assert_eq!(
            layout.slot("pr").unwrap().resolution,
            SlotResolution::Unavailable
        );
        assert!(matches!(
            layout.slot("shell").unwrap().resolution,
            SlotResolution::Ready { .. }
        ));
    }
    let held = without(
        with(two_slots(), [("layout.slot.pr.state", text("held"))]),
        &[
            "layout.slot.pr.kind",
            "layout.slot.pr.url",
            "layout.slot.pr.target",
        ],
    );
    assert_eq!(
        parse_ok(&held).slot("pr").unwrap().resolution,
        SlotResolution::Held
    );
}

#[test]
fn local_slots_may_be_held() {
    let held = with(two_slots(), [("layout.slot.shell.state", text("held"))]);
    assert_eq!(
        parse_ok(&held).slot("shell").unwrap().resolution,
        SlotResolution::Held
    );
}

#[test]
fn primary_joins_a_published_layout() {
    // Unlisted, it leads the slot order.
    let joined = parse_ok(&with(two_slots(), primary_facts()));
    let keys: Vec<_> = joined.slots.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["primary", "shell", "pr"]);
    assert_eq!(joined.version, BaselineVersion::Published("7".into()));
    // Listed, it takes its listed position.
    let listed = with(
        with(two_slots(), primary_facts()),
        [("layout.slots", list(&["shell", "primary", "pr"]))],
    );
    let keys: Vec<_> = parse_ok(&listed)
        .slots
        .iter()
        .map(|s| s.key.clone())
        .collect();
    assert_eq!(keys, ["shell", "primary", "pr"]);
    // Listed without its facts is a torn publication.
    let torn = with(
        two_slots(),
        [("layout.slots", list(&["shell", "primary", "pr"]))],
    );
    assert_eq!(parse_err(&torn), LayoutError::MissingPrimary);
    let reserved = with(
        with(two_slots(), primary_facts()),
        [("layout.slot.primary.kind", text("url"))],
    );
    assert!(matches!(
        parse_err(&reserved),
        LayoutError::ReservedPrimary { .. }
    ));
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
    assert_eq!(
        layout.slot("pr").unwrap().spec.presentation.as_deref(),
        Some("hologram")
    );
}

#[test]
fn malformed_specs_are_errors() {
    use LayoutError::*;
    let cases: Vec<(FactMap, LayoutError)> = vec![
        (
            without(two_slots(), &["layout.version"]),
            MissingFact {
                key: "layout.version".into(),
            },
        ),
        (
            without(two_slots(), &["layout.slots"]),
            MissingFact {
                key: "layout.slots".into(),
            },
        ),
        (
            with(two_slots(), [("layout.version", MetadataValue::Integer(7))]),
            WrongType {
                key: "layout.version".into(),
                expected: "text",
            },
        ),
        (
            with(
                two_slots(),
                [("layout.slots", list(&["shell", "pr", "shell"]))],
            ),
            DuplicateSlot {
                slot: "shell".into(),
            },
        ),
        (
            with(
                two_slots(),
                [("layout.slots", list(&["shell", "pr", "Bad.Key"]))],
            ),
            InvalidId {
                key: "layout.slots".into(),
                id: "Bad.Key".into(),
            },
        ),
        (
            with(two_slots(), [("layout.slots", list(&["shell"]))]),
            UnlistedSlot {
                key: "layout.slot.pr.entity".into(),
            },
        ),
        (
            without(two_slots(), &["layout.slot.pr.facet"]),
            IncompleteFacet { slot: "pr".into() },
        ),
        (
            with(
                two_slots(),
                [(
                    "layout.slot.pr.entity",
                    MetadataValue::EntityRefs(vec![entity("a", "1"), entity("b", "2")]),
                )],
            ),
            WrongType {
                key: "layout.slot.pr.entity".into(),
                expected: "exactly one entity ref",
            },
        ),
        (
            without(two_slots(), &["layout.slot.shell.kind"]),
            MissingFact {
                key: "layout.slot.shell.kind".into(),
            },
        ),
        (
            with(two_slots(), [("layout.slot.shell.kind", text("teleport"))]),
            InvalidValue {
                key: "layout.slot.shell.kind".into(),
                value: "teleport".into(),
            },
        ),
        (
            without(two_slots(), &["layout.slot.shell.argv.0"]),
            ArgvGap {
                slot: "shell".into(),
            },
        ),
        (
            with(two_slots(), [("layout.slot.shell.argv.01", text("x"))]),
            ArgvGap {
                slot: "shell".into(),
            },
        ),
        (
            without(
                two_slots(),
                &["layout.slot.shell.argv.0", "layout.slot.shell.argv.1"],
            ),
            MissingFact {
                key: "layout.slot.shell.argv.0".into(),
            },
        ),
        (
            with(
                two_slots(),
                [("layout.slot.shell.rebind", text("sometimes"))],
            ),
            InvalidValue {
                key: "layout.slot.shell.rebind".into(),
                value: "sometimes".into(),
            },
        ),
        (
            with(two_slots(), [("layout.slot.shell.state", text("bogus"))]),
            InvalidValue {
                key: "layout.slot.shell.state".into(),
                value: "bogus".into(),
            },
        ),
    ];
    for (facts, expected) in cases {
        assert_eq!(parse(&convoy(), &facts), Err(expected));
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
        Panel {
            weight: 1,
            node: PanelNode::Split {
                axis: Axis::Column,
                children: vec!["top".into(), "bottom".into()]
            }
        }
    );
    assert_eq!(
        arrangement.panels["top"],
        Panel {
            weight: 3,
            node: PanelNode::Tabs {
                slots: vec!["shell".into()],
                selected: "shell".into()
            }
        }
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
        PanelNode::Tabs {
            slots: vec!["shell".into(), "pr".into()],
            selected: "pr".into()
        }
    );
}

#[test]
fn arrangement_errors() {
    use LayoutError::*;
    let cases: Vec<(FactMap, LayoutError)> = vec![
        (
            with(arranged(), [("layout.panel.bottom.slots", list(&["x"]))]),
            UnknownSlot {
                panel: "bottom".into(),
                slot: "x".into(),
            },
        ),
        (
            with(
                arranged(),
                [("layout.panel.top.slots", list(&["shell", "pr"]))],
            ),
            SlotInTwoPanels { slot: "pr".into() },
        ),
        (
            without(arranged(), &["layout.root"]),
            MissingFact {
                key: "layout.root".into(),
            },
        ),
        (
            with(
                arranged(),
                [("layout.panel.main.children", list(&["top", "gone"]))],
            ),
            UnknownPanel {
                panel: "gone".into(),
            },
        ),
        (
            with(
                arranged(),
                [("layout.panel.main.children", list(&["top", "top"]))],
            ),
            PanelReused {
                panel: "top".into(),
            },
        ),
        (
            with(
                arranged(),
                [("layout.panel.main.children", list(&["top", "main"]))],
            ),
            PanelReused {
                panel: "main".into(),
            },
        ),
        (
            with(arranged(), [("layout.panel.main.children", list(&["top"]))]),
            UnreachablePanel {
                panel: "bottom".into(),
            },
        ),
        (
            with(arranged(), [("layout.panel.main.slots", list(&["shell"]))]),
            PanelShape {
                panel: "main".into(),
            },
        ),
        (
            without(arranged(), &["layout.panel.main.axis"]),
            PanelShape {
                panel: "main".into(),
            },
        ),
        (
            with(arranged(), [("layout.panel.bottom.slots", list(&[]))]),
            PanelShape {
                panel: "bottom".into(),
            },
        ),
        (
            with(arranged(), [("layout.panel.main.axis", text("diagonal"))]),
            InvalidValue {
                key: "layout.panel.main.axis".into(),
                value: "diagonal".into(),
            },
        ),
        (
            with(
                arranged(),
                [("layout.panel.top.weight", MetadataValue::Integer(0))],
            ),
            InvalidValue {
                key: "layout.panel.top.weight".into(),
                value: "0".into(),
            },
        ),
        (
            with(arranged(), [("layout.panel.top.selected", text("pr"))]),
            SelectedNotInPanel {
                panel: "top".into(),
                slot: "pr".into(),
            },
        ),
        (
            with(arranged(), [("layout.root", text("Main"))]),
            InvalidId {
                key: "layout.root".into(),
                id: "Main".into(),
            },
        ),
    ];
    for (facts, expected) in cases {
        assert_eq!(parse(&convoy(), &facts), Err(expected));
    }
}

// A slot the provider's arrangement does not place is still part of the
// layout; frontends place it by the default rule.
#[test]
fn unplaced_slots_are_allowed() {
    let facts = with(
        two_slots(),
        [
            ("layout.root", text("only")),
            ("layout.panel.only.slots", list(&["shell"])),
        ],
    );
    let layout = parse_ok(&facts);
    assert_eq!(layout.slots.len(), 2);
    assert_eq!(layout.arrangement.unwrap().panels.len(), 1);
}
