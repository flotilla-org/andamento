//! Replays producer facts and reads the Suggested Layout each entity publishes.
use andamento_core::{
    replay::{read, Replay},
    suggested_layout::{
        parse, Axis, BaselineVersion, CommandLine, Content, LocalRecipe, PanelNode, RebindPolicy,
        SlotResolution, SuggestedLayout,
    },
    EntityRef, Sidebar,
};
use std::io::Cursor;

const CONFIG: &str = r#"
region "convoys" root-template="heading" form="compact" placement="convoys"
region "roles" root-template="heading" form="compact" placement="roles"
template "heading" { field "label" source="literal" value="Replay"; }
placement "convoys" {
  for "convoy" kind="convoy" { apply-template "line"; }
}
placement "roles" {
  for "role" kind="role" { apply-template "line"; }
}
template "line" { field "label" key="display.label"; }
"#;

fn replay(name: &str) -> Replay {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    Replay::new(read(Cursor::new(std::fs::read(path).unwrap())).unwrap()).unwrap()
}

/// The layout of the only entity of `kind` on the surface, if any.
fn layout(sidebar: &Sidebar, kind: &str) -> Option<SuggestedLayout> {
    let snapshot = sidebar.snapshot();
    let nodes: Vec<_> = snapshot
        .surface
        .sections
        .iter()
        .flat_map(|section| &section.nodes)
        .filter(|node| node.entity.kind == kind)
        .collect();
    match nodes.as_slice() {
        [] => None,
        [node] => parse(&node.entity, &node.facts).unwrap(),
        _ => panic!("expected one {kind}"),
    }
}

fn keys(layout: &SuggestedLayout) -> Vec<&str> {
    layout.slots.iter().map(|slot| slot.key.as_str()).collect()
}

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef {
        kind: kind.into(),
        id: id.into(),
    }
}

// fixtures/suggested-layout-review.jsonl: an in-crew review convoy. At 0 ms
// the coder (primary) and reviewer terminals, the PR and a notes artifact are
// arranged in two columns. At 1000 ms the reviewer is held with the same
// baseline. At 2000 ms baseline 2 adds a checks tab beside the PR and the
// reviewer resolves to a new generation.
#[test]
fn review_convoy_layout_replays_into_slots_and_an_arrangement_baseline() {
    let mut replay = replay("suggested-layout-review.jsonl");
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let root = "/srv/lab/andamento/review-147";
    let pr = entity("change_request", "github/flotilla-org/andamento!147");

    assert_eq!(replay.step(&mut sidebar).unwrap(), Some(0));
    let first = layout(&sidebar, "convoy").unwrap();
    assert_eq!(first.version, BaselineVersion::Published("1".into()));
    assert_eq!(keys(&first), ["primary", "reviewer", "pr", "notes"]);

    let primary = first.slot("primary").unwrap();
    assert_eq!(
        primary.spec.content,
        Content::ProviderFacet {
            entity: entity("convoy", "flotilla/review-147@fleet"),
            facet: "primary".into()
        }
    );
    assert_eq!(
        primary.resolution,
        SlotResolution::Ready {
            target: Some("vessel:flotilla/review-147/coder@lab".into()),
            recipe: LocalRecipe::Command {
                line: CommandLine::Shell(
                    "'flotilla' attach --host 'lab' 'review-147/coder'".into()
                ),
                cwd: Some(root.into()),
            },
        }
    );

    let reviewer = first.slot("reviewer").unwrap();
    assert_eq!(
        reviewer.spec.content,
        Content::ProviderFacet {
            entity: entity("vessel", "flotilla/review-147/reviewer@lab"),
            facet: "terminal".into()
        }
    );
    assert_eq!(reviewer.spec.presentation.as_deref(), Some("terminal"));
    assert_eq!(reviewer.rebind, RebindPolicy::KeepPrevious);
    assert_eq!(
        reviewer.resolution,
        SlotResolution::Ready {
            target: Some("vessel:flotilla/review-147/reviewer@lab#gen-1".into()),
            recipe: LocalRecipe::Command {
                line: CommandLine::Argv(
                    ["flotilla", "attach", "--host", "lab", "review-147/reviewer"]
                        .map(String::from)
                        .to_vec()
                ),
                cwd: Some(root.into()),
            },
        }
    );

    let pr_slot = first.slot("pr").unwrap();
    assert_eq!(
        pr_slot.spec.content,
        Content::ProviderFacet {
            entity: pr.clone(),
            facet: "review".into()
        }
    );
    assert_eq!(pr_slot.spec.presentation.as_deref(), Some("web"));
    assert!(matches!(
        &pr_slot.resolution,
        SlotResolution::Ready { recipe: LocalRecipe::Url { url }, .. }
            if url == "https://github.com/flotilla-org/andamento/pull/147"
    ));

    let notes = first.slot("notes").unwrap();
    let file = LocalRecipe::File {
        path: format!("{root}/.flotilla/review/notes.md"),
    };
    assert_eq!(notes.spec.content, Content::Local(file.clone()));
    assert_eq!(notes.spec.presentation.as_deref(), Some("markdown"));
    assert_eq!(
        notes.resolution,
        SlotResolution::Ready {
            target: None,
            recipe: file
        }
    );

    let arrangement = first.arrangement.as_ref().unwrap();
    assert_eq!(arrangement.root, "main");
    let panel = |id: &str| &arrangement.panels[id];
    assert_eq!(
        panel("main").node,
        PanelNode::Split {
            axis: Axis::Row,
            children: vec!["agents".into(), "review".into()]
        }
    );
    assert_eq!(panel("agents").weight, 3);
    assert_eq!(
        panel("agents").node,
        PanelNode::Tabs {
            slots: vec!["primary".into(), "reviewer".into()],
            selected: "reviewer".into()
        }
    );
    assert_eq!(panel("review").weight, 2);
    assert_eq!(
        panel("review").node,
        PanelNode::Split {
            axis: Axis::Column,
            children: vec!["pr-view".into(), "notes-view".into()]
        }
    );
    assert_eq!(panel("pr-view").weight, 2);
    assert_eq!(panel("notes-view").weight, 1);
    assert_eq!(
        panel("notes-view").node,
        PanelNode::Tabs {
            slots: vec!["notes".into()],
            selected: "notes".into()
        }
    );

    // A changed resolution is not a changed baseline.
    assert_eq!(replay.step(&mut sidebar).unwrap(), Some(1000));
    let held = layout(&sidebar, "convoy").unwrap();
    assert_eq!(held.version, first.version);
    assert_eq!(held.arrangement, first.arrangement);
    assert_eq!(keys(&held), keys(&first));
    for (a, b) in held.slots.iter().zip(&first.slots) {
        assert_eq!(a.spec, b.spec);
    }
    assert_eq!(
        held.slot("reviewer").unwrap().resolution,
        SlotResolution::Held
    );
    assert_eq!(held.slot("primary"), first.slot("primary"));

    // A new slot is a new baseline.
    assert_eq!(replay.step(&mut sidebar).unwrap(), Some(2000));
    let second = layout(&sidebar, "convoy").unwrap();
    assert_eq!(second.version, BaselineVersion::Published("2".into()));
    assert_eq!(
        keys(&second),
        ["primary", "reviewer", "pr", "checks", "notes"]
    );
    assert_eq!(
        second.slot("checks").unwrap().spec.content,
        Content::ProviderFacet {
            entity: pr,
            facet: "checks".into()
        }
    );
    assert_eq!(
        second.arrangement.as_ref().unwrap().panels["pr-view"].node,
        PanelNode::Tabs {
            slots: vec!["pr".into(), "checks".into()],
            selected: "pr".into()
        }
    );
    assert!(matches!(
        &second.slot("reviewer").unwrap().resolution,
        SlotResolution::Ready { target: Some(target), .. } if target.ends_with("#gen-2")
    ));

    // Silence: every fact shares one TTL, so the layout expires whole.
    assert_eq!(replay.step(&mut sidebar).unwrap(), None);
    replay.advance_to(&mut sidebar, 31_999).unwrap();
    assert_eq!(layout(&sidebar, "convoy"), Some(second));
    replay.advance_to(&mut sidebar, 32_001).unwrap();
    assert_eq!(layout(&sidebar, "convoy"), None);
}

// Today's managed primary-content facts are the one-slot special case.
#[test]
fn standing_role_primary_facts_replay_as_a_one_slot_layout() {
    let mut replay = replay("scripted-roll.jsonl");
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    for (at, generation) in [(0, Some("coder-1")), (1000, None), (2000, Some("coder-2"))] {
        assert_eq!(replay.step(&mut sidebar).unwrap(), Some(at));
        let layout = layout(&sidebar, "role").unwrap();
        assert_eq!(layout.version, BaselineVersion::PrimaryOnly);
        assert_eq!(layout.arrangement, None);
        assert_eq!(keys(&layout), ["primary"]);
        let slot = &layout.slots[0];
        assert!(matches!(
            &slot.spec.content,
            Content::ProviderFacet { entity, facet } if entity.kind == "role" && facet == "primary"
        ));
        match (generation, &slot.resolution) {
            (
                Some(generation),
                SlotResolution::Ready {
                    target: Some(target),
                    recipe:
                        LocalRecipe::Command {
                            line: CommandLine::Shell(command),
                            ..
                        },
                },
            ) => {
                assert!(target.contains(generation), "{target}");
                assert!(command.contains("attach"), "{command}");
            }
            (None, SlotResolution::Held) => {}
            (_, other) => panic!("at {at}: {other:?}"),
        }
    }
}
