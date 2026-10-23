//! Marshalling the core can't see: what C reads from snapshots and effects.
use super::*;
use andamento_core::tab_workspace_id;
use std::ffi::CStr;
use std::mem::MaybeUninit;

const PROVIDER: &str = "sub-1";

fn text(s: &str) -> Text {
    Text::borrowed(s)
}

unsafe fn create(config: &str) -> *mut Andamento {
    let mut error = ptr::null_mut();
    let h = andamento_create(config.as_ptr(), config.len(), &mut error);
    assert!(!h.is_null(), "{}", CStr::from_ptr(error).to_string_lossy());
    h
}

unsafe fn node(s: *const AndamentoSnapshot, index: usize) -> NodeView {
    let mut node = MaybeUninit::uninit();
    assert_eq!(andamento_snapshot_node(s, index, node.as_mut_ptr()), 1);
    node.assume_init()
}

// A section node carries its region's hints; entity nodes and the
// synthesised workspace section have none. Orders span both signed
// boundaries (kdl 4 rejects i64::MIN, so the low one is MIN+1).
#[test]
fn section_nodes_carry_their_region_hints() {
    unsafe {
        for order in [None, Some(i64::MIN + 1), Some(0), Some(i64::MAX)] {
            let config = format!(
                "region \"test\" root-template=\"flotilla/region/tree\" default-host=\"sidebar\"{}",
                order.map(|o| format!(" order={o}")).unwrap_or_default()
            );
            let h = create(&config);
            let workspace = WorkspaceInput { id: tab_workspace_id(42).into(), position: 0, name: text("unplaced"), selected: 1 };
            assert_eq!(andamento_observe(h, &workspace, 1, ptr::null(), 0, ptr::null_mut()), 1);
            let s = andamento_snapshot_acquire(h, ptr::null_mut());
            let section = node(s, 0);
            assert_eq!(section.default_host.read().unwrap(), "sidebar");
            assert_eq!((section.has_order, section.order), (order.is_some() as u32, order.unwrap_or_default()));
            for i in 1..andamento_snapshot_node_count(s) {
                let other = node(s, i);
                assert_eq!((other.default_host.len, other.has_order), (0, 0));
                if other.is_section == 0 {
                    assert_eq!(other.workspace_id.bytes, tab_workspace_id(42).to_bytes());
                }
            }
            andamento_snapshot_release(s);
            andamento_destroy(h);
        }
    }
}

#[test]
fn declared_abbreviation_reaches_c_fields_but_keeps_full_details() {
    unsafe {
        let config = include_str!("../../andamento-core/tests/fixtures/abbreviation.kdl");
        let mut error = ptr::null_mut();
        let h = create(config);
        let patch = r#"{"type":"metadata-patch","target":{"kind":"entity","value":{"kind":"vessel","id":"worker"}},"source_id":"test","set":{"flotilla.vessel":{"value":{"type":"text","value":"worker"}},"display.label":{"value":{"type":"text","value":"a very long worker name"}},"display.label.medium":{"value":{"type":"text","value":"worker medium"}},"display.label.short":{"value":{"type":"text","value":"w"}}},"unset":[]}"#;
        assert_eq!(andamento_apply_patch_json(h, 0, text(PROVIDER), text(patch), &mut error), 1);
        for (declaration, expected) in [("", "worker medium"), ("tier=\"short\"", "w"), ("tier=\"full\"", "a very long worker name")] {
            let config = config.replace("kind=\"vessel\"", &format!("kind=\"vessel\" {declaration}"));
            assert_eq!(andamento_configure(h, text(&config), &mut error), 1);
            let snapshot = andamento_snapshot_acquire(h, &mut error);
            let node = node(snapshot, 1);
            assert_eq!(node.label.read().unwrap(), "a very long worker name");
            assert_eq!(node.entity.provider.read().unwrap(), PROVIDER);
            let mut field = MaybeUninit::<FieldView>::uninit();
            assert_eq!(andamento_snapshot_field(snapshot, node.first_field, field.as_mut_ptr()), 1);
            assert_eq!(field.assume_init_ref().text.read().unwrap(), expected);
            assert_eq!(andamento_snapshot_field(snapshot, node.first_detail, field.as_mut_ptr()), 1);
            assert_eq!(field.assume_init_ref().text.read().unwrap(), "a very long worker name");
            andamento_snapshot_release(snapshot);
        }
        andamento_destroy(h);
        assert!(error.is_null());
    }
}

// A workspace subject without an opening recipe is openable, and its
// materialization carries an empty recipe and no managed target.
#[test]
fn a_workspace_subject_materializes_with_an_empty_recipe() {
    unsafe {
        let config = format!("{}\nworkspace kind=\"vessel\"\n", include_str!("../../andamento-core/tests/fixtures/abbreviation.kdl"));
        let mut error = ptr::null_mut();
        let h = create(&config);
        let patch = r#"{"type":"metadata-patch","target":{"kind":"entity","value":{"kind":"vessel","id":"worker"}},"source_id":"test","set":{"flotilla.vessel":{"value":{"type":"text","value":"worker"}},"display.label":{"value":{"type":"text","value":"worker"}}},"unset":[]}"#;
        assert_eq!(andamento_apply_patch_json(h, 0, text(PROVIDER), text(patch), &mut error), 1);
        let snapshot = andamento_snapshot_acquire(h, &mut error);
        let row = node(snapshot, 1);
        assert_eq!((row.state, row.openable), (ANDAMENTO_LATENT, 1));
        assert_eq!(andamento_dispatch(h, snapshot, row.activate, &mut error), 1);
        let effects = andamento_effects_take(h, &mut error);
        assert_eq!(andamento_effects_count(effects), 1);
        let mut effect = MaybeUninit::<EffectView>::uninit();
        assert_eq!(andamento_effects_get(effects, 0, effect.as_mut_ptr()), 1);
        let effect = effect.assume_init();
        assert_eq!((effect.kind, effect.recipe.read().unwrap().as_str(), effect.has_primary_target), (1, "", 0));
        andamento_effects_release(effects);
        andamento_snapshot_release(snapshot);
        andamento_destroy(h);
        assert!(error.is_null());
    }
}

// wheelhouse#104 through C: a subject with a direct Cleat endpoint
// materializes with an empty recipe and no primary target, and the first
// plan of its primary carries the Cleat session. A host's own Cleat session
// slot round-trips through set and get.
#[test]
fn a_direct_cleat_primary_reaches_c_and_cleat_slots_round_trip() {
    unsafe {
        let config = format!("{}\nworkspace kind=\"vessel\"\n", include_str!("../../andamento-core/tests/fixtures/abbreviation.kdl"));
        let mut error = ptr::null_mut();
        let h = create(&config);
        let fact = |key: &str, value: &str| format!(r#""{key}":{{"value":{{"type":"text","value":"{value}"}}}}"#);
        let set = [
            fact("flotilla.vessel", "worker"),
            fact("display.label", "worker"),
            fact("action.primary.recipe", "flotilla attach worker"),
            fact("action.primary.direct.transport", "ssh"),
            fact("action.primary.direct.host", "robert@feta"),
            fact("action.primary.direct.runtime_root", "/run/cleat"),
            fact("action.primary.direct.daemon", "d1"),
            fact("action.primary.direct.session", "s1"),
        ]
        .join(",");
        let patch = format!(
            r#"{{"type":"metadata-patch","target":{{"kind":"entity","value":{{"kind":"vessel","id":"worker"}}}},"source_id":"test","set":{{{set}}},"unset":[]}}"#
        );
        assert_eq!(andamento_apply_patch_json(h, 0, text(PROVIDER), text(&patch), &mut error), 1);
        let snapshot = andamento_snapshot_acquire(h, &mut error);
        assert_eq!(andamento_dispatch(h, snapshot, node(snapshot, 1).activate, &mut error), 1);
        andamento_snapshot_release(snapshot);
        let effects = andamento_effects_take(h, &mut error);
        let mut effect = MaybeUninit::<EffectView>::uninit();
        assert_eq!(andamento_effects_get(effects, 0, effect.as_mut_ptr()), 1);
        let effect = effect.assume_init();
        assert_eq!((effect.kind, effect.recipe.read().unwrap().as_str(), effect.has_primary_target), (ANDAMENTO_EFFECT_MATERIALIZE, "", 0));
        let request = effect.request_id;
        andamento_effects_release(effects);
        let id = WorkspaceId::from_bytes([9; 16]);
        assert_eq!(andamento_complete(h, request, ANDAMENTO_COMPLETE_MATERIALIZE, id.into(), text(""), &mut error), 1);
        let workspace = WorkspaceInput { id: id.into(), position: 0, name: text("worker"), selected: 1 };
        assert_eq!(andamento_observe(h, &workspace, 1, ptr::null(), 0, &mut error), 1);

        let cleat_fields = |spec: &ViewSpecView| {
            [&spec.cleat_transport, &spec.cleat_host, &spec.cleat_runtime_root, &spec.cleat_daemon, &spec.cleat_session]
                .map(|field| field.read().unwrap())
        };
        let plan = andamento_slot_plan(h, id.into(), text("primary"), text(""), &mut error);
        assert_eq!(((*plan).state, (*plan).recipe.content), (ANDAMENTO_CONTENT_UPDATING, ANDAMENTO_SLOT_CLEAT_SESSION));
        assert_eq!(cleat_fields(&(*plan).recipe), ["ssh", "robert@feta", "/run/cleat", "d1", "s1"]);
        assert_eq!((*plan).recipe.command.read().unwrap(), "");
        andamento_slot_plan_free(plan);

        let mut spec: ViewSpecView = std::mem::zeroed();
        spec.content = ANDAMENTO_SLOT_CLEAT_SESSION;
        spec.cleat_transport = text("local");
        spec.cleat_host = text("feta");
        spec.cleat_runtime_root = text("/run/cleat");
        spec.cleat_daemon = text("d2");
        spec.cleat_session = text("s2");
        assert_eq!(andamento_slot_set(h, id.into(), text("u:crew"), &spec, ANDAMENTO_REBIND_REPLACE, &mut error), 1);
        let slots = andamento_slots_get(h, id.into(), &mut error);
        let crew = slice((*slots).slots, (*slots).slot_count).unwrap().iter().find(|slot| slot.key.read().unwrap() == "u:crew").unwrap();
        assert_eq!(crew.spec.content, ANDAMENTO_SLOT_CLEAT_SESSION);
        assert_eq!(cleat_fields(&crew.spec), ["local", "feta", "/run/cleat", "d2", "s2"]);
        andamento_slots_free(slots);
        assert!(error.is_null());
        // An unknown transport, an empty field and an option are refused.
        for (transport, session) in [("mosh", "s2"), ("ssh", ""), ("ssh", "-oProxyCommand=x")] {
            spec.cleat_transport = text(transport);
            spec.cleat_session = text(session);
            assert_eq!(andamento_slot_set(h, id.into(), text("u:bad"), &spec, ANDAMENTO_REBIND_REPLACE, &mut error), 0);
            assert!(!error.is_null());
            andamento_string_free(error);
            error = ptr::null_mut();
        }
        andamento_destroy(h);
    }
}

// A workspace's layouts reach C: their list, the one an arrangement is of,
// the slots it leaves unplaced, choosing another and the slots' roles.
#[test]
fn workspace_layouts_reach_c() {
    unsafe {
        let template = r#"
workspace kind="vessel" {
  slot "shell" role="shell" kind="command" command="zsh"
  layout "agent" { tabs id="main" { slots role="agent"; }; }
  layout "all" { tabs id="main" { slots rest=true; }; }
}
"#;
        let config = format!("{}\n{template}", include_str!("../../andamento-core/tests/fixtures/abbreviation.kdl"));
        let mut error = ptr::null_mut();
        let h = create(&config);
        let patch = r#"{"type":"metadata-patch","target":{"kind":"entity","value":{"kind":"vessel","id":"worker"}},"source_id":"test","set":{"flotilla.vessel":{"value":{"type":"text","value":"worker"}},"display.label":{"value":{"type":"text","value":"worker"}}},"unset":[]}"#;
        assert_eq!(andamento_apply_patch_json(h, 0, text(PROVIDER), text(patch), &mut error), 1);
        let snapshot = andamento_snapshot_acquire(h, &mut error);
        assert_eq!(andamento_dispatch(h, snapshot, node(snapshot, 1).activate, &mut error), 1);
        andamento_snapshot_release(snapshot);
        let effects = andamento_effects_take(h, &mut error);
        let mut effect = MaybeUninit::<EffectView>::uninit();
        assert_eq!(andamento_effects_get(effects, 0, effect.as_mut_ptr()), 1);
        let request = effect.assume_init().request_id;
        andamento_effects_release(effects);
        let id = WorkspaceId::from_bytes([7; 16]);
        assert_eq!(andamento_complete(h, request, ANDAMENTO_COMPLETE_MATERIALIZE, id.into(), text(""), &mut error), 1);
        let workspace = WorkspaceInput { id: id.into(), position: 0, name: text("worker"), selected: 1 };
        assert_eq!(andamento_observe(h, &workspace, 1, ptr::null(), 0, &mut error), 1);

        let list = andamento_layouts_get(h, id.into(), &mut error);
        let layouts: Vec<(String, u32)> =
            slice((*list).layouts, (*list).count).unwrap().iter().map(|layout| (layout.name.read().unwrap(), layout.flags)).collect();
        assert_eq!(layouts, [("agent".into(), ANDAMENTO_LAYOUT_DEFAULT | ANDAMENTO_LAYOUT_CURRENT), ("all".into(), 0)]);
        andamento_layouts_free(list);

        let arrangement = |error: &mut *mut c_char| {
            let arrangement = andamento_arrangement_get(h, id.into(), error);
            let layout = (*arrangement).layout.read().unwrap();
            let unplaced: Vec<String> = slice((*arrangement).notes, (*arrangement).note_count)
                .unwrap()
                .iter()
                .filter(|note| note.kind == ANDAMENTO_NOTE_UNPLACED)
                .map(|note| note.key.read().unwrap())
                .collect();
            let has = (*arrangement).has_layout;
            andamento_arrangement_free(arrangement);
            (has, layout, unplaced)
        };
        assert_eq!(arrangement(&mut error), (1, "agent".into(), vec!["t:shell".to_owned()]));
        assert_eq!(andamento_layout_choose(h, id.into(), 1, text("all"), &mut error), 1);
        assert_eq!(arrangement(&mut error), (1, "all".into(), vec![]));
        assert_eq!(andamento_layout_choose(h, id.into(), 1, text("nope"), &mut error), 0);
        assert!(!error.is_null());
        andamento_string_free(error);
        error = ptr::null_mut();
        // A name that isn't UTF-8, and a null handle, fail with an error and
        // change nothing.
        let bad = [0xffu8, 0xfe];
        assert_eq!(andamento_layout_choose(h, id.into(), 1, Text { data: bad.as_ptr(), len: bad.len() }, &mut error), 0);
        assert!(!error.is_null());
        andamento_string_free(error);
        error = ptr::null_mut();
        assert_eq!(arrangement(&mut error), (1, "all".into(), vec![]));
        assert_eq!(andamento_layout_choose(ptr::null_mut(), id.into(), 0, text(""), &mut error), 0);
        assert!(!error.is_null());
        andamento_string_free(error);
        error = ptr::null_mut();
        assert!(andamento_layouts_get(ptr::null_mut(), id.into(), &mut error).is_null());
        assert!(!error.is_null());
        andamento_string_free(error);
        error = ptr::null_mut();

        let slots = andamento_slots_get(h, id.into(), &mut error);
        let roles: Vec<(String, u32, String)> = slice((*slots).slots, (*slots).slot_count)
            .unwrap()
            .iter()
            .map(|slot| (slot.key.read().unwrap(), slot.spec.has_role, slot.spec.role.read().unwrap()))
            .collect();
        assert_eq!(roles, [("primary".into(), 1, "agent".into()), ("t:shell".into(), 1, "shell".into())]);
        andamento_slots_free(slots);
        andamento_destroy(h);
        assert!(error.is_null());
    }
}

// A display-variable control names its variable and whether it persists;
// both borrow from the snapshot, which outlives the sidebar.
#[test]
fn display_controls_name_their_variable_and_persistence() {
    unsafe {
        let h = create(
            r#"version 1
            display-variable "history" type="bool" default=false label="Show finished" icon="F" persist=true
            display-variable "temporary" type="bool" default=true label="Temporary" icon="T" persist=false
            region "tree" root-template="test" form="compact" placement="tree"
            placement "tree" { for "item" kind="test"; }
            template "test" slot="compact" node-kind="entity" {
                control "display-variable" variable="history"
                control "display-variable" variable="temporary"
                control "display-variable" variable="undeclared"
                control "open-config" label="Configure"
            }"#,
        );
        let mut error = ptr::null_mut();
        let snapshot = andamento_snapshot_acquire(h, &mut error);
        let control = |index| {
            let mut control = MaybeUninit::<ControlView>::uninit();
            assert_eq!(andamento_snapshot_control(snapshot, index, control.as_mut_ptr()), 1);
            control.assume_init()
        };
        let controls: Vec<_> = (0..4).map(control).collect();
        let named: Vec<_> = controls.iter().map(|c| (c.variable.read().unwrap(), c.persist, c.label.read().unwrap())).collect();
        assert_eq!(
            named,
            [
                ("history".into(), 1, "Show finished".into()),
                ("temporary".into(), 0, "Temporary".into()),
                ("undeclared".into(), 0, "undeclared".into()),
                (String::new(), 0, String::new()),
            ]
        );
        assert_eq!(andamento_dispatch(h, snapshot, controls[0].action, &mut error), 1);
        let next = andamento_snapshot_acquire(h, &mut error);
        assert!((&(*next).controls)[0].checked && !(&(*snapshot).controls)[0].checked);
        andamento_destroy(h);
        assert_eq!(controls[2].variable.read().unwrap(), "undeclared");
        andamento_snapshot_release(snapshot);
        andamento_snapshot_release(next);
        assert!(error.is_null());
    }
}

#[test]
fn panic_is_contained_and_poisoned_client_keeps_existing_snapshots_alive() {
    unsafe {
        let mut error = ptr::null_mut();
        let h = andamento_create(ptr::null(), 0, &mut error);
        let s = andamento_snapshot_acquire(h, &mut error);
        let count = andamento_snapshot_node_count(s);
        let result: Option<()> = run(h, &mut error, |_| panic!("injected failure"));
        assert!(result.is_none());
        assert!(CStr::from_ptr(error).to_str().unwrap().contains("panicked"));
        andamento_string_free(error);
        assert_eq!(andamento_tick(h, 0, &mut error), 0);
        assert!(CStr::from_ptr(error).to_str().unwrap().contains("poisoned"));
        andamento_string_free(error);
        andamento_destroy(h);
        assert_eq!(andamento_snapshot_node_count(s), count);
        andamento_snapshot_release(s);
    }
}

// The native host receives a usable activation and a snapshot-scoped copy
// action for subjects, with resolved URLs owned by their effect batches.
#[cfg(feature = "json")]
#[test]
fn subject_rows_expose_open_and_copy_url_actions() {
    unsafe {
        let mut error = ptr::null_mut();
        let h = create(include_str!("../../../templates/flotilla-default.kdl"));
        for line in include_str!("../../../fixtures/subject-entities.jsonl").lines() {
            let frame: serde_json::Value = serde_json::from_str(line).unwrap();
            if frame["offset_ms"] != 0 {
                continue;
            }
            let patch = serde_json::to_string(&frame["patch"]).unwrap();
            assert_eq!(andamento_apply_patch_json(h, 0, text(PROVIDER), text(&patch), &mut error), 1);
        }
        let snapshot = andamento_snapshot_acquire(h, &mut error);
        let index = (*snapshot).nodes.iter().position(|n| n.entity.id == "github/org/b!281").unwrap();
        let row = node(snapshot, index);
        assert_eq!(row.openable, 1);
        assert_ne!(row.copy_url, NONE);
        for (action, kind) in [(row.activate, 3), (row.copy_url, 4)] {
            assert_eq!(andamento_dispatch(h, snapshot, action, &mut error), 1);
            let effects = andamento_effects_take(h, &mut error);
            assert_eq!(andamento_effects_count(effects), 1);
            let mut effect = MaybeUninit::<EffectView>::uninit();
            assert_eq!(andamento_effects_get(effects, 0, effect.as_mut_ptr()), 1);
            let effect = effect.assume_init();
            assert_eq!(effect.kind, kind);
            assert_eq!(effect.recipe.read().unwrap(), "https://github.com/org/b/pull/281");
            andamento_effects_release(effects);
        }
        let issue = EntityView { provider: text(PROVIDER), kind: text("issue"), id: text("github/org/a#115") };
        assert_eq!(andamento_copy_subject_url(h, issue, &mut error), 1);
        let effects = andamento_effects_take(h, &mut error);
        assert_eq!(andamento_effects_count(effects), 1);
        andamento_effects_release(effects);
        // An entity names its provider.
        let unnamed = EntityView { provider: text(""), ..issue };
        assert_eq!(andamento_copy_subject_url(h, unnamed, &mut error), 0);
        assert!(CStr::from_ptr(error).to_str().unwrap().contains("provider"));
        andamento_string_free(error);
        andamento_snapshot_release(snapshot);
        andamento_destroy(h);
    }
}

#[test]
fn create_rejects_null_nonempty_and_invalid_utf8() {
    unsafe {
        for (data, len) in [(ptr::null(), 1), ([255].as_ptr(), 1)] {
            let mut error = ptr::null_mut();
            assert!(andamento_create(data, len, &mut error).is_null());
            assert!(!error.is_null());
            andamento_string_free(error);
        }
    }
}
