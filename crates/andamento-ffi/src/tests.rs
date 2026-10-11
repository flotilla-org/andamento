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
            let workspace = WorkspaceInput {
                id: tab_workspace_id(42).into(),
                position: 0,
                name: text("unplaced"),
                selected: 1,
            };
            assert_eq!(
                andamento_observe(h, &workspace, 1, ptr::null(), 0, ptr::null_mut()),
                1
            );
            let s = andamento_snapshot_acquire(h, ptr::null_mut());
            let section = node(s, 0);
            assert_eq!(section.default_host.read().unwrap(), "sidebar");
            assert_eq!(
                (section.has_order, section.order),
                (order.is_some() as u32, order.unwrap_or_default())
            );
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
        assert_eq!(
            andamento_apply_patch_json(h, 0, text(PROVIDER), text(patch), &mut error),
            1
        );
        for (declaration, expected) in [
            ("", "worker medium"),
            ("tier=\"short\"", "w"),
            ("tier=\"full\"", "a very long worker name"),
        ] {
            let config =
                config.replace("kind=\"vessel\"", &format!("kind=\"vessel\" {declaration}"));
            assert_eq!(andamento_configure(h, text(&config), &mut error), 1);
            let snapshot = andamento_snapshot_acquire(h, &mut error);
            let node = node(snapshot, 1);
            assert_eq!(node.label.read().unwrap(), "a very long worker name");
            assert_eq!(node.entity.provider.read().unwrap(), PROVIDER);
            let mut field = MaybeUninit::<FieldView>::uninit();
            assert_eq!(
                andamento_snapshot_field(snapshot, node.first_field, field.as_mut_ptr()),
                1
            );
            assert_eq!(field.assume_init_ref().text.read().unwrap(), expected);
            assert_eq!(
                andamento_snapshot_field(snapshot, node.first_detail, field.as_mut_ptr()),
                1
            );
            assert_eq!(
                field.assume_init_ref().text.read().unwrap(),
                "a very long worker name"
            );
            andamento_snapshot_release(snapshot);
        }
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
            assert_eq!(
                andamento_snapshot_control(snapshot, index, control.as_mut_ptr()),
                1
            );
            control.assume_init()
        };
        let controls: Vec<_> = (0..4).map(control).collect();
        let named: Vec<_> = controls
            .iter()
            .map(|c| {
                (
                    c.variable.read().unwrap(),
                    c.persist,
                    c.label.read().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                ("history".into(), 1, "Show finished".into()),
                ("temporary".into(), 0, "Temporary".into()),
                ("undeclared".into(), 0, "undeclared".into()),
                (String::new(), 0, String::new()),
            ]
        );
        assert_eq!(
            andamento_dispatch(h, snapshot, controls[0].action, &mut error),
            1
        );
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
            assert_eq!(
                andamento_apply_patch_json(h, 0, text(PROVIDER), text(&patch), &mut error),
                1
            );
        }
        let snapshot = andamento_snapshot_acquire(h, &mut error);
        let index = (*snapshot)
            .nodes
            .iter()
            .position(|n| n.entity.id == "github/org/b!281")
            .unwrap();
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
            assert_eq!(
                effect.recipe.read().unwrap(),
                "https://github.com/org/b/pull/281"
            );
            andamento_effects_release(effects);
        }
        let issue = EntityView {
            provider: text(PROVIDER),
            kind: text("issue"),
            id: text("github/org/a#115"),
        };
        assert_eq!(andamento_copy_subject_url(h, issue, &mut error), 1);
        let effects = andamento_effects_take(h, &mut error);
        assert_eq!(andamento_effects_count(effects), 1);
        andamento_effects_release(effects);
        // An entity names its provider.
        let unnamed = EntityView {
            provider: text(""),
            ..issue
        };
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
