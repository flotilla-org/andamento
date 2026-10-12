//! Host-supplied 128-bit Workspace IDs, through the JSON requests a frontend
//! or replay tool sends.
mod common;
use andamento_core::presentation::{PresentationState, UNPLACED_WORKSPACES_SECTION};
use andamento_core::sidebar::{HostEffect, Request, Response};
use andamento_core::{EntityRef, MetadataPatch, MetadataTarget, Sidebar, WorkspaceId};
use common::CONFIG;
use serde_json::{json, Value};

/// A user-created workspace and a materialized one, as a host would generate them.
const NOTES: &str = "01920a6b-7c3d-7e4f-8a1b-2c3d4e5f6a7b";
const WORKER: &str = "01920a6b-9e10-7abc-9def-0123456789ab";
const OTHER: &str = "01920a6b-9e10-7abc-9def-0123456789ac";
const PROVIDER: &str = "sub-1";

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
    let set: serde_json::Map<_, _> = facts.iter().map(|(k, v)| (k.to_string(), fact(v))).collect();
    json!({"target": {"kind": "entity", "value": {"kind": kind, "id": entity}},
           "source_id": "fixture", "set": set})
}
fn subjects() -> Value {
    json!({"request": "apply", "now_ms": 100, "provider": PROVIDER, "patches": [
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
    response.snapshot.surface.sections[0].nodes[0].children[0].state.clone()
}

#[test]
fn wide_ids_flow_through_observe_snapshot_effects_and_completion() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    send(&mut sidebar, subjects());
    // The user made Notes themselves: the host declares it, then observes it
    // beside another.
    send(&mut sidebar, json!({"request": "register-workspace", "workspace_id": NOTES}));
    let response = send(
        &mut sidebar,
        json!({"request": "observe", "panes": [], "workspaces": [
            workspace(json!(NOTES), 0, "Notes"), workspace(json!(OTHER), 1, "Other")]}),
    );
    let live = |id, selected| PresentationState::Live { workspace_id: id, selected };
    assert_eq!(
        unplaced(&response),
        vec![(entity(".workspace", NOTES), live(id(NOTES), true)), (entity(".workspace", OTHER), live(id(OTHER), false)),]
    );

    // Materialize, and complete with the ID the host generated for it.
    let response = send(
        &mut sidebar,
        json!({"request": "dispatch", "action": {"action": "activate", "entity": {"provider": PROVIDER, "kind": "vessel", "id": "v"}}}),
    );
    let [HostEffect::Materialize { request_id, .. }] = response.effects[..] else {
        panic!("{:?}", response.effects);
    };
    send(&mut sidebar, json!({"request": "complete", "request_id": request_id, "workspace_id": WORKER, "error": null}));
    let response = send(
        &mut sidebar,
        json!({"request": "observe", "panes": [{"workspace_id": WORKER, "pane_id": {"kind": "terminal", "id": 3},
            "is_selectable": true, "is_focused": true, "ordinal": 0}], "workspaces": [
            workspace(json!(NOTES), 0, "Notes"), workspace(json!(WORKER), 1, "Worker"),
            workspace(json!(OTHER), 2, "Other")]}),
    );
    assert_eq!(vessel_state(&response), live(id(WORKER), false));
    assert!(sidebar.registered_workspaces().contains(&id(WORKER)));

    // Activating the live row focuses exactly that workspace; the effect
    // carries the wide ID as a UUID string in JSON.
    let key = response.snapshot.surface.sections[0].nodes[0].children[0].key.clone();
    let response = send(&mut sidebar, json!({"request": "dispatch", "action": {"action": "activate-placement", "key": key}}));
    let [HostEffect::Focus { request_id, workspace_id }] = response.effects[..] else {
        panic!("{:?}", response.effects);
    };
    assert_eq!(workspace_id, id(WORKER));
    assert_eq!(serde_json::to_value(&response.effects[0]).unwrap()["workspace_id"], json!(WORKER));
    send(&mut sidebar, json!({"request": "complete", "request_id": request_id, "workspace_id": null, "error": null}));

    // Deleting Notes forgets it; closing it alone would not.
    send(&mut sidebar, json!({"request": "forget-workspace", "workspace_id": NOTES}));
    assert_eq!(sidebar.registered_workspaces().iter().collect::<Vec<_>>(), [&id(WORKER)]);
}

#[test]
fn tab_targets_name_workspaces_by_uuid() {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    send(&mut sidebar, json!({"request": "observe", "panes": [], "workspaces": [workspace(json!(NOTES), 0, "Notes")]}));
    let host = |value: Value, host_id: &str| {
        json!({"target": {"kind": "tab", "value": value}, "source_id": "host",
               "set": {".host.kind": fact(".workspace"), ".host.id": fact(host_id)}})
    };
    // Hyphenated and bare hex, in either case.
    let compact = NOTES.replace('-', "").to_uppercase();
    for (value, expected) in [(NOTES, "notes"), (compact.as_str(), "again")] {
        let response =
            send(&mut sidebar, json!({"request": "apply", "now_ms": 100, "provider": PROVIDER, "patches": [host(json!(value), expected)]}));
        assert_eq!(unplaced(&response)[0].0, EntityRef::new(PROVIDER, ".workspace", expected));
    }
    let decoded: MetadataPatch = serde_json::from_value(host(json!(NOTES), "notes")).unwrap();
    assert_eq!(decoded.target, MetadataTarget::Tab(id(NOTES)));
    assert_eq!(serde_json::to_value(MetadataTarget::Tab(id(NOTES))).unwrap(), json!({"kind": "tab", "value": NOTES}));
    for bad in [json!(7), json!("7"), json!(NOTES.replace('7', "g"))] {
        assert!(serde_json::from_value::<MetadataPatch>(host(bad, "x")).is_err());
    }
}

fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef::local(kind, id)
}
