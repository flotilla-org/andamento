//! Host-supplied 128-bit Workspace IDs, replayed as the JSON requests a
//! frontend or replay tool sends, beside ABI 2-style 64-bit IDs.
use andamento_core::presentation::{PresentationState, UNPLACED_WORKSPACES_SECTION};
use andamento_core::sidebar::{HostEffect, Request, Response};
use andamento_core::{EntityRef, MetadataPatch, MetadataTarget, Sidebar, WorkspaceId};
use serde_json::{json, Value};

const CONFIG: &str = include_str!("fixtures/sidebar.kdl");
/// A user-created workspace and a materialized one, as a host would generate them.
const NOTES: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b";
const WORKER: &str = "01920a6b-9e10-7abc-9def-0123456789ab";

fn id(text: &str) -> WorkspaceId {
    text.parse().unwrap()
}
fn send(sidebar: &mut Sidebar, request: Value) -> Response {
    let request: Request = serde_json::from_value(request).unwrap();
    sidebar.handle(request).unwrap()
}
fn fact(value: &str) -> Value {
    json!({"value": {"type": "text", "value": value}})
}
fn entity_patch(kind: &str, entity: &str, facts: &[(&str, &str)]) -> Value {
    let set: serde_json::Map<_, _> = facts
        .iter()
        .map(|(k, v)| (k.to_string(), fact(v)))
        .collect();
    json!({"target": {"kind": "entity", "value": {"kind": kind, "id": entity}},
           "source_id": "fixture", "set": set})
}
fn subjects() -> Value {
    json!({"request": "apply", "now_ms": 100, "patches": [
        entity_patch("project", "p", &[("flotilla.project", "p"), ("display.label", "Project P")]),
        entity_patch("vessel", "v", &[("flotilla.project", "p"), ("flotilla.vessel", "v"),
            ("display.label", "Worker"), ("action.primary.target", "vessel:v"),
            ("action.primary.recipe", "printf hello")]),
    ]})
}
fn workspace(id: Value, position: usize, name: &str) -> Value {
    json!({"id": id, "position": position, "name": name, "selected": position == 0})
}
fn unplaced(response: &Response) -> Vec<(EntityRef, PresentationState)> {
    response
        .snapshot
        .surface
        .sections
        .iter()
        .find(|s| s.name == UNPLACED_WORKSPACES_SECTION)
        .unwrap()
        .nodes
        .iter()
        .map(|n| (n.entity.clone(), n.state.clone()))
        .collect()
}
fn vessel_state(response: &Response) -> PresentationState {
    response.snapshot.surface.sections[0].nodes[0].children[0]
        .state
        .clone()
}

#[test]
fn wide_ids_flow_through_observe_snapshot_effects_and_completion() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    send(&mut sidebar, subjects());
    // The user made Notes themselves: the host declares it, then observes it
    // beside a workspace still using an ABI 2-style ID.
    send(
        &mut sidebar,
        json!({"request": "register-workspace", "workspace_id": NOTES}),
    );
    let response = send(
        &mut sidebar,
        json!({"request": "observe", "panes": [], "workspaces": [
            workspace(json!(NOTES), 0, "Notes"), workspace(json!(7), 1, "Legacy")]}),
    );
    let live = |id, selected| PresentationState::Live {
        workspace_id: id,
        selected,
    };
    assert_eq!(
        unplaced(&response),
        vec![
            (entity(".workspace", NOTES), live(id(NOTES), true)),
            (entity(".workspace", "7"), live(WorkspaceId::from(7), false)),
        ]
    );

    // Materialize, and complete with the ID the host generated for it.
    let response = send(
        &mut sidebar,
        json!({"request": "dispatch", "action": {"action": "activate", "entity": {"kind": "vessel", "id": "v"}}}),
    );
    let [HostEffect::Materialize { request_id, .. }] = response.effects[..] else {
        panic!("{:?}", response.effects);
    };
    send(
        &mut sidebar,
        json!({"request": "complete", "request_id": request_id, "workspace_id": WORKER, "error": null}),
    );
    let response = send(
        &mut sidebar,
        json!({"request": "observe", "panes": [{"workspace_id": WORKER, "pane_id": {"kind": "terminal", "id": 3},
            "is_selectable": true, "is_focused": true, "ordinal": 0}], "workspaces": [
            workspace(json!(NOTES), 0, "Notes"), workspace(json!(WORKER), 1, "Worker"),
            workspace(json!(7), 2, "Legacy")]}),
    );
    assert_eq!(vessel_state(&response), live(id(WORKER), false));
    assert!(sidebar.registered_workspaces().contains(&id(WORKER)));

    // Activating the live row focuses exactly that workspace; the effect
    // carries the wide ID as a UUID string in JSON.
    let key = response.snapshot.surface.sections[0].nodes[0].children[0]
        .key
        .clone();
    let response = send(
        &mut sidebar,
        json!({"request": "dispatch", "action": {"action": "activate-placement", "key": key}}),
    );
    let [HostEffect::Focus {
        request_id,
        workspace_id,
    }] = response.effects[..]
    else {
        panic!("{:?}", response.effects);
    };
    assert_eq!(workspace_id, id(WORKER));
    assert_eq!(
        serde_json::to_value(&response.effects[0]).unwrap()["workspace_id"],
        json!(WORKER)
    );
    send(
        &mut sidebar,
        json!({"request": "complete", "request_id": request_id, "workspace_id": null, "error": null}),
    );

    // Deleting Notes forgets it; closing it alone would not.
    send(
        &mut sidebar,
        json!({"request": "forget-workspace", "workspace_id": NOTES}),
    );
    assert_eq!(
        sidebar.registered_workspaces().iter().collect::<Vec<_>>(),
        [&id(WORKER)]
    );
}

#[test]
fn tab_targets_accept_wide_ids_in_every_patch_form() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    send(
        &mut sidebar,
        json!({"request": "observe", "panes": [], "workspaces": [
            workspace(json!(NOTES), 0, "Notes"), workspace(json!(7), 1, "Legacy")]}),
    );
    let host = |value: Value, host_id: &str| {
        json!({"target": {"kind": "tab", "value": value}, "source_id": "host",
               "set": {".host.kind": fact(".workspace"), ".host.id": fact(host_id)}})
    };
    // Hyphenated, bare hex and number forms; the typed Rust form is the same.
    let compact = NOTES.replace('-', "");
    for (patch, expected) in [
        (host(json!(NOTES), "notes"), "notes"),
        (
            host(json!(compact.to_uppercase()), "notes-again"),
            "notes-again",
        ),
    ] {
        let response = send(
            &mut sidebar,
            json!({"request": "apply", "now_ms": 100, "patches": [patch]}),
        );
        assert_eq!(unplaced(&response)[0].0, entity(".workspace", expected));
    }
    let response = send(
        &mut sidebar,
        json!({"request": "apply", "now_ms": 100, "patches": [host(json!(7), "legacy")]}),
    );
    assert_eq!(unplaced(&response)[1].0, entity(".workspace", "legacy"));

    let decoded: MetadataPatch = serde_json::from_value(host(json!(NOTES), "notes")).unwrap();
    assert_eq!(decoded.target, MetadataTarget::Tab(id(NOTES)));
    // Embedded IDs keep ABI 2's numeric JSON; wide ones print as UUIDs.
    let wire = |target| serde_json::to_value(target).unwrap();
    assert_eq!(
        wire(MetadataTarget::Tab(WorkspaceId::from(7))),
        json!({"kind": "tab", "value": 7})
    );
    assert_eq!(
        wire(MetadataTarget::Tab(id(NOTES))),
        json!({"kind": "tab", "value": NOTES})
    );
    for bad in [
        json!("not-a-uuid"),
        json!(-1),
        json!(NOTES.replace('7', "g")),
    ] {
        assert!(serde_json::from_value::<MetadataPatch>(host(bad, "x")).is_err());
    }
}

#[test]
fn abi_2_ids_are_the_embedded_wide_ids() {
    // A host moving to ABI 3 can name a workspace it observed as 7 by the
    // embedded 128-bit ID, and the snapshot and effects agree.
    let embedded = WorkspaceId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(embedded, WorkspaceId::from(7));
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    send(&mut sidebar, subjects());
    let response = send(
        &mut sidebar,
        json!({"request": "dispatch", "action": {"action": "activate", "entity": {"kind": "vessel", "id": "v"}}}),
    );
    let [HostEffect::Materialize { request_id, .. }] = response.effects[..] else {
        panic!("{:?}", response.effects);
    };
    assert!(sidebar.complete(request_id, Ok(Some(embedded))));
    let response = send(
        &mut sidebar,
        json!({"request": "observe", "panes": [], "workspaces": [workspace(json!(7), 0, "Worker")]}),
    );
    assert_eq!(
        vessel_state(&response),
        PresentationState::Live {
            workspace_id: WorkspaceId::from(7),
            selected: true
        }
    );
    assert_eq!(embedded.to_string(), "7");
}

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
}
