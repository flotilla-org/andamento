use andamento_core::{
    replay::{read, Replay},
    sidebar::{Action, HostEffect},
    EntityRef, MetadataPatch, MetadataValue, Sidebar,
};
use std::io::Cursor;

fn reference(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
}
fn replay() -> Replay {
    Replay::new(
        read(Cursor::new(include_str!(
            "../../../fixtures/subject-entities.jsonl"
        )))
        .unwrap(),
    )
    .unwrap()
}

// The connector must preserve entity-ref identity and known facts while ignoring
// future value kinds; malformed known values remain errors.
#[test]
fn connector_decodes_refs_and_ignores_unknown_values() {
    for kind in ["convoy", "change_request", "issue", "forge", "role"] {
        let input = serde_json::json!({
            "target":{"kind":"entity","value":{"kind":kind,"id":"test"}}, "source_id":"test",
            "set":{
                "refs":{"value":{"type":"entity-refs","value":[{"kind":"convoy","id":"build"}]}},
                "future":{"value":{"type":"future-kind","value":{"arbitrary":true}}},
                "display.label":{"value":{"type":"text","value":"Known"}}
            }
        });
        let patch: MetadataPatch = serde_json::from_value(input).unwrap();
        assert_eq!(patch.set.len(), 2);
        assert_eq!(
            patch.set["refs"].value,
            // The wire names no provider; applying the patch stamps one.
            MetadataValue::EntityRefs(vec![EntityRef::new("", "convoy", "build")])
        );
        assert_eq!(
            serde_json::from_value::<MetadataPatch>(serde_json::to_value(&patch).unwrap()).unwrap(),
            patch
        );
    }
    let bad = serde_json::json!({"target":{"kind":"entity","value":{"kind":"forge","id":"test"}},"source_id":"test","set":{"bad":{"value":{"type":"entity-refs","value":["invalid"]}}}});
    assert!(serde_json::from_value::<MetadataPatch>(bad).is_err());
}

// Published subjects are joined under their convoy through subject_of; numeric
// text order crosses digit widths, duplicates do not duplicate rows, and live
// relationship patches rebuild the placement without restarting the sidebar.
#[test]
fn subjects_render_in_natural_order_and_rejoin_after_link_updates() {
    let mut sidebar =
        Sidebar::new(include_str!("../../../templates/flotilla-default.kdl")).unwrap();
    let mut replay = replay();
    replay.advance_to(&mut sidebar, 0).unwrap();
    let snapshot = sidebar.snapshot();
    let project = &snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "tree")
        .unwrap()
        .nodes[0];
    let convoy = project
        .children
        .iter()
        .find(|n| n.entity.kind == "convoy")
        .unwrap();
    assert_eq!(
        convoy
            .children
            .iter()
            .map(|n| n.entity.id.as_str())
            .collect::<Vec<_>>(),
        vec!["github/org/b!281", "github/org/a!1000", "github/org/a#115"]
    );
    let role = project
        .children
        .iter()
        .find(|n| n.entity.kind == "role")
        .unwrap();
    assert!(role
        .content
        .fields
        .iter()
        .any(|field| field.value == "working"));
    assert!(role
        .content
        .fields
        .iter()
        .any(|field| field.value == "Planning"));
    replay.advance_to(&mut sidebar, 1000).unwrap();
    let changed = sidebar.snapshot();
    assert!(changed.revision > snapshot.revision);
    let project = &changed
        .surface
        .sections
        .iter()
        .find(|s| s.name == "tree")
        .unwrap()
        .nodes[0];
    let convoy = project
        .children
        .iter()
        .find(|n| n.entity.kind == "convoy")
        .unwrap();
    assert_eq!(convoy.children.len(), 2);
    assert!(convoy
        .children
        .iter()
        .all(|n| n.entity.id != "github/org/a!1000"));
}

// Row activation and the copy action must both resolve the current forge's
// subject-specific template, including the repository scope and text number.
#[test]
fn subject_click_and_copy_resolve_forge_templates() {
    let mut sidebar =
        Sidebar::new(include_str!("../../../templates/flotilla-default.kdl")).unwrap();
    replay().advance_to(&mut sidebar, 0).unwrap();
    for (kind, id, url) in [
        (
            "change_request",
            "github/org/b!281",
            "https://github.com/org/b/pull/281",
        ),
        (
            "change_request",
            "github/org/a!1000",
            "https://github.com/org/a/pull/1000",
        ),
        (
            "issue",
            "github/org/a#115",
            "https://github.com/org/a/issues/115",
        ),
    ] {
        let entity = reference(kind, id);
        assert_eq!(
            sidebar
                .dispatch(Action::Activate {
                    entity: entity.clone()
                })
                .unwrap(),
            vec![HostEffect::OpenUrl { url: url.into() }]
        );
        assert_eq!(
            sidebar.dispatch(Action::CopySubjectUrl { entity }).unwrap(),
            vec![HostEffect::CopyUrl { url: url.into() }]
        );
    }
    let snapshot = sidebar.snapshot();
    let convoy = snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == "tree")
        .unwrap()
        .nodes[0]
        .children
        .iter()
        .find(|n| n.entity.kind == "convoy")
        .unwrap();
    let key = convoy.children[0].key.clone();
    assert_eq!(
        sidebar.dispatch(Action::ActivatePlacement { key }).unwrap(),
        vec![HostEffect::OpenUrl {
            url: "https://github.com/org/b/pull/281".into()
        }]
    );
    assert!(sidebar
        .dispatch(Action::CopySubjectUrl {
            entity: reference("issue", "unknown")
        })
        .is_err());
}

// URLs are resolved from current metadata on every action. Incomplete,
// multi-forge, unsafe-scheme and unsupported-placeholder records cannot copy
// a URL; expiry removes a once-available forge template.
#[test]
fn subject_urls_follow_updates_and_reject_incomplete_forges() {
    let mut sidebar =
        Sidebar::new(include_str!("../../../templates/flotilla-default.kdl")).unwrap();
    replay().advance_to(&mut sidebar, 0).unwrap();
    let entity = reference("change_request", "github/org/b!281");
    for (template, expected) in [
        (
            "{web_url}/{scope}/merge/{number}",
            Some("https://github.com/org/b/merge/281"),
        ),
        ("javascript:{number}", None),
        ("{web_url}/{missing}", None),
        ("{web_url}/{scope}/{number", None),
    ] {
        let patch = serde_json::json!({
            "target":{"kind":"entity","value":{"kind":"forge","id":"github"}},"source_id":"flotilla",
            "set":{"flotilla.forge.change_request_url_template":{"value":{"type":"text","value":template}}}
        });
        sidebar.apply(0, [serde_json::from_value::<MetadataPatch>(patch).unwrap()]);
        assert_eq!(sidebar.subject_url(&entity).as_deref(), expected);
        assert_eq!(
            sidebar
                .dispatch(Action::CopySubjectUrl {
                    entity: entity.clone()
                })
                .is_ok(),
            expected.is_some()
        );
    }
    for refs in [
        serde_json::json!([]),
        serde_json::json!([{"kind":"forge","id":"github"},{"kind":"forge","id":"other"}]),
        serde_json::json!([{"kind":"vessel","id":"github"}]),
    ] {
        let patch = serde_json::json!({"target":{"kind":"entity","value":entity},"source_id":"flotilla","set":{"flotilla.forge":{"value":{"type":"entity-refs","value":refs}}}});
        sidebar.apply(0, [serde_json::from_value::<MetadataPatch>(patch).unwrap()]);
        assert!(sidebar.subject_url(&entity).is_none());
    }
    let patch = serde_json::json!({"target":{"kind":"entity","value":entity},"source_id":"flotilla","set":{"flotilla.forge":{"value":{"type":"entity-refs","value":[{"kind":"forge","id":"github"}]}}}});
    sidebar.apply(0, [serde_json::from_value::<MetadataPatch>(patch).unwrap()]);
    let patch = serde_json::json!({"target":{"kind":"entity","value":{"kind":"forge","id":"github"}},"source_id":"flotilla","set":{"flotilla.forge.change_request_url_template":{"value":{"type":"text","value":"{web_url}/{scope}/pull/{number}"},"ttl_ms":10}}});
    sidebar.apply(0, [serde_json::from_value::<MetadataPatch>(patch).unwrap()]);
    assert!(sidebar.subject_url(&entity).is_some());
    sidebar.apply(10, []);
    assert!(sidebar.subject_url(&entity).is_some());
    sidebar.apply(11, []);
    assert!(sidebar.subject_url(&entity).is_none());
}
