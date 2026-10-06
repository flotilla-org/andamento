#![cfg(feature = "json")]
use andamento_ffi::*;
use std::{mem::MaybeUninit, ptr};
fn text(s: &str) -> Text {
    Text {
        data: s.as_ptr(),
        len: s.len(),
    }
}
unsafe fn read(t: Text) -> String {
    String::from_utf8(std::slice::from_raw_parts(t.data, t.len).to_vec()).unwrap()
}
unsafe fn field(s: *const AndamentoSnapshot, card: usize, name: &str) -> DetailFieldView {
    let mut d = MaybeUninit::uninit();
    assert_eq!(andamento_snapshot_detail(s, card, d.as_mut_ptr()), 1);
    for i in 0..d.assume_init().field_count {
        let mut f = MaybeUninit::uninit();
        assert_eq!(
            andamento_snapshot_detail_field(s, card, i, f.as_mut_ptr()),
            1
        );
        let f = f.assume_init();
        if read(f.name) == name {
            return f;
        }
    }
    panic!("missing field {name}")
}
unsafe fn acquire_worktree(h: *mut Andamento) -> *mut AndamentoSnapshot {
    let s = andamento_snapshot_acquire(h, ptr::null_mut());
    assert_ne!(
        andamento_snapshot_detail_request(h, s, text("worktree"), text("w"), ptr::null_mut()),
        usize::MAX
    );
    s
}
// Acceptance scenario: all six catalog kinds preserve typed roles, known-empty
// facts, observation provenance and relations even when there is no tree node.
#[test]
fn six_kind_native_detail_contract() {
    unsafe {
        let h = andamento_create(ptr::null(), 0, ptr::null_mut());
        assert!(!h.is_null());
        for line in include_str!("../../../fixtures/typed-detail.jsonl").lines() {
            assert_eq!(
                andamento_apply_patch_json(h, 42, text(line), ptr::null_mut()),
                1
            );
        }
        let legacy = andamento_snapshot_acquire(h, ptr::null_mut());
        assert_eq!(andamento_snapshot_detail_count(legacy), 0);
        assert_eq!(
            andamento_snapshot_detail_find(legacy, text("worktree"), text("worktree:identity / #")),
            usize::MAX
        );
        andamento_snapshot_release(legacy);
        let s = andamento_snapshot_acquire_details(h, ptr::null_mut());
        assert_eq!(andamento_abi_version(), 2);
        assert_eq!(andamento_snapshot_detail_count(s), 6);
        for kind in [
            "change_request",
            "issue",
            "convoy",
            "role",
            "project",
            "worktree",
        ] {
            let card = andamento_snapshot_detail_find(
                s,
                text(kind),
                text(&format!("{kind}:identity / #")),
            );
            assert_ne!(card, usize::MAX);
            let f = field(s, card, "summary");
            assert_eq!(f.role, 3);
            assert_eq!(read(f.section), "facts");
            assert_eq!(read(f.label), "Summary");
            assert_eq!(f.has_value, 1);
            assert_eq!(read(f.text), "");
            assert!(!f.text.data.is_null());
            assert_eq!(f.has_observation, 1);
            assert_eq!(f.observed_at_ms, 42);
            assert_eq!(f.has_ttl, 1);
            assert_eq!(f.ttl_ms, 100);
            assert_eq!(f.stale, 0);
            assert_eq!(read(f.source_key), "summary.text");
            assert_eq!(read(f.source_id), "fixture-producer");
            assert_eq!(field(s, card, "identity").role, 0);
            assert_eq!(field(s, card, "label").role, 1);
            assert_eq!(field(s, card, "state").role, 2);
            let related = field(s, card, "related");
            assert_eq!(related.role, 4);
            assert!(related.relation_count >= 1);
        }
        // Missing facts remain declared, without fabricating an empty observation.
        let card =
            andamento_snapshot_detail_find(s, text("worktree"), text("worktree:identity / #"));
        let missing = field(s, card, "branch");
        assert_eq!(missing.has_value, 0);
        assert_eq!(missing.has_observation, 0);
        assert!(!missing.text.data.is_null());
        assert!(!missing.source_id.data.is_null());
        let mut d = MaybeUninit::uninit();
        assert_eq!(andamento_snapshot_detail(s, card, d.as_mut_ptr()), 1);
        let d = d.assume_init();
        let mut related_index = None;
        for i in 0..d.field_count {
            let mut f = MaybeUninit::uninit();
            andamento_snapshot_detail_field(s, card, i, f.as_mut_ptr());
            if read(f.assume_init().name) == "related" {
                related_index = Some(i);
            }
        }
        let related_index = related_index.unwrap();
        let mut r = MaybeUninit::uninit();
        assert_eq!(
            andamento_snapshot_detail_relation(
                s,
                card,
                related_index,
                0,
                ptr::null(),
                0,
                r.as_mut_ptr()
            ),
            1
        );
        let r = r.assume_init();
        assert_eq!(read(r.entity.kind), "issue");
        assert_eq!(read(r.entity.id), "issue:identity / #");
        assert_eq!(read(r.display_text), "Display issue");
        assert_ne!(r.detail, usize::MAX);
        let path = [r.entity];
        assert_eq!(
            andamento_snapshot_detail_relation(
                s,
                card,
                related_index,
                0,
                path.as_ptr(),
                1,
                MaybeUninit::uninit().as_mut_ptr()
            ),
            0
        );
        let mut unavailable = MaybeUninit::uninit();
        assert_eq!(
            andamento_snapshot_detail_relation(
                s,
                card,
                related_index,
                1,
                ptr::null(),
                0,
                unavailable.as_mut_ptr()
            ),
            1
        );
        assert_eq!(unavailable.assume_init().detail, usize::MAX);
        let mut action = MaybeUninit::uninit();
        assert_eq!(
            andamento_snapshot_detail_action(s, card, 0, action.as_mut_ptr()),
            1
        );
        let action = action.assume_init();
        assert_eq!(read(action.intent), "inspect");
        assert_eq!(read(action.entity.id), "worktree:identity / #");
        assert_eq!(action.action, d.activate);
        assert_eq!(d.has_workspace, 0);
        // Existing dispatch takes the card's stable entity action and emits Inspect.
        assert_eq!(andamento_dispatch(h, s, d.activate, ptr::null_mut()), 1);
        // Snapshot ownership permits reading old facts after mutations; a new
        // snapshot observes expiry at TTL+1, not at the inclusive TTL boundary.
        andamento_tick(h, 142, ptr::null_mut());
        let boundary = andamento_snapshot_acquire_details(h, ptr::null_mut());
        assert_eq!(field(boundary, card, "summary").has_value, 1);
        andamento_tick(h, 143, ptr::null_mut());
        let expired = andamento_snapshot_acquire_details(h, ptr::null_mut());
        assert_eq!(field(expired, card, "summary").has_value, 0);
        assert_eq!(field(s, card, "summary").has_value, 1);
        assert_eq!(
            andamento_snapshot_detail_find(s, text("unknown"), text("")),
            usize::MAX
        );
        assert_eq!(
            andamento_snapshot_detail_field(s, usize::MAX, 0, ptr::null_mut()),
            0
        );
        andamento_snapshot_release(expired);
        andamento_snapshot_release(boundary);
        andamento_snapshot_release(s);
        andamento_destroy(h);
    }
}

// Lifecycle scenario: materialization uses the primary control label, identical
// lease renewals count as fresh observations, and a retained expired fact exposes
// its original winner and stale receipt time through the actual native ABI.
#[test]
fn native_controls_renewals_and_retained_expiry() {
    unsafe {
        let config = r#"
        region "tree" root-template="title" placement="tree"
        template "title" { field "label" literal="Tree"; }
        placement "tree" { for "worktree" kind="worktree" { field "label" key="display.label"; }; }
    "#;
        let h = andamento_create(config.as_ptr(), config.len(), ptr::null_mut());
        assert!(!h.is_null());
        let patch = r#"{"target":{"kind":"entity","value":{"kind":"worktree","id":"w"}},"source_id":"producer","set":{"display.label":{"value":{"type":"text","value":"Worker"},"ttl_ms":10},"summary.text":{"value":{"type":"text","value":"observed"},"ttl_ms":10},"action.primary.target":{"value":{"type":"text","value":"worktree:w"},"ttl_ms":10},"action.primary.recipe":{"value":{"type":"text","value":"exec sh"},"ttl_ms":10},"action.primary.label":{"value":{"type":"text","value":"Attach"},"ttl_ms":10}}}"#;
        assert_eq!(
            andamento_apply_patch_json(h, 100, text(patch), ptr::null_mut()),
            1
        );
        let s = acquire_worktree(h);
        let card = andamento_snapshot_detail_find(s, text("worktree"), text("w"));
        let mut action = MaybeUninit::uninit();
        assert_eq!(
            andamento_snapshot_detail_action(s, card, 0, action.as_mut_ptr()),
            1
        );
        let action = action.assume_init();
        assert_eq!(read(action.intent), "materialize-workspace");
        assert_eq!(read(action.label), "Attach");
        assert_eq!(andamento_dispatch(h, s, action.action, ptr::null_mut()), 1);
        let effects = andamento_effects_take(h, ptr::null_mut());
        let mut effect = MaybeUninit::uninit();
        assert_eq!(andamento_effects_get(effects, 0, effect.as_mut_ptr()), 1);
        let effect = effect.assume_init();
        assert_eq!(effect.kind, 1);
        assert_eq!(
            andamento_complete(h, effect.request_id, 1, 7, text(""), ptr::null_mut()),
            1
        );
        andamento_effects_release(effects);
        let workspace = WorkspaceInput {
            id: 7,
            position: 0,
            name: text("Worker"),
            selected: 1,
        };
        assert_eq!(
            andamento_observe(h, &workspace, 1, ptr::null(), 0, ptr::null_mut()),
            1
        );
        assert_eq!(
            andamento_apply_patch_json(h, 105, text(patch), ptr::null_mut()),
            1
        );
        let fresh = acquire_worktree(h);
        assert_eq!(field(fresh, card, "summary").observed_at_ms, 105);
        assert_eq!(field(s, card, "summary").observed_at_ms, 100);
        assert_eq!(andamento_tick(h, 115, ptr::null_mut()), 1);
        let boundary = acquire_worktree(h);
        assert_eq!(field(boundary, card, "summary").stale, 0);
        assert_eq!(andamento_tick(h, 116, ptr::null_mut()), 1);
        let expired = acquire_worktree(h);
        let f = field(expired, card, "summary");
        assert_eq!(f.has_value, 1);
        assert_eq!(f.stale, 1);
        assert_eq!(f.observed_at_ms, 105);
        assert_eq!(read(f.source_id), "producer");
        let mut d = MaybeUninit::uninit();
        assert_eq!(andamento_snapshot_detail(expired, card, d.as_mut_ptr()), 1);
        let d = d.assume_init();
        assert!(!d.error.data.is_null());
        assert_eq!(d.has_workspace, 1);
        assert_eq!(d.workspace_id, 7);
        for s in [expired, boundary, fresh, s] {
            andamento_snapshot_release(s);
        }
        andamento_destroy(h);
    }
}

// Identical entries from competing producers still expose the same deterministic
// winner as fact arbitration, and losing-source removal cannot change provenance.
#[test]
fn identical_producer_provenance() {
    unsafe {
        let h = andamento_create(ptr::null(), 0, ptr::null_mut());
        for source in ["z-source", "a-source"] {
            let patch = format!(
                r#"{{"target":{{"kind":"entity","value":{{"kind":"worktree","id":"w"}}}},"source_id":"{source}","set":{{"summary.text":{{"value":{{"type":"text","value":"same"}}}}}}}}"#
            );
            assert_eq!(
                andamento_apply_patch_json(h, 0, text(&patch), ptr::null_mut()),
                1
            );
        }
        let s = acquire_worktree(h);
        let card = andamento_snapshot_detail_find(s, text("worktree"), text("w"));
        assert_eq!(read(field(s, card, "summary").source_id), "a-source");
        let unset = r#"{"target":{"kind":"entity","value":{"kind":"worktree","id":"w"}},"source_id":"a-source","unset":["summary.text"]}"#;
        assert_eq!(
            andamento_apply_patch_json(h, 1, text(unset), ptr::null_mut()),
            1
        );
        let next = acquire_worktree(h);
        assert_eq!(read(field(next, card, "summary").source_id), "z-source");
        andamento_snapshot_release(next);
        andamento_snapshot_release(s);
        andamento_destroy(h);
    }
}

// Demand queries preserve the eager contract for all catalog identities, including
// hidden relations. Appending never moves borrowed text; old actions are rejected.
#[test]
fn demand_details_equal_eager_and_keep_snapshot_owned_lifetimes() {
    unsafe {
        let h = andamento_create(ptr::null(), 0, ptr::null_mut());
        for line in include_str!("../../../fixtures/typed-detail.jsonl").lines() {
            assert_eq!(
                andamento_apply_patch_json(h, 42, text(line), ptr::null_mut()),
                1
            );
        }
        let plain = andamento_snapshot_acquire(h, ptr::null_mut());
        let eager = andamento_snapshot_acquire_details(h, ptr::null_mut());
        assert_eq!(andamento_snapshot_detail_count(plain), 0);
        assert_eq!(
            andamento_snapshot_node_count(plain),
            andamento_snapshot_node_count(eager)
        );
        let mut held = None;
        for kind in [
            "worktree",
            "issue",
            "change_request",
            "convoy",
            "role",
            "project",
        ] {
            let id = format!("{kind}:identity / #");
            let index =
                andamento_snapshot_detail_request(h, plain, text(kind), text(&id), ptr::null_mut());
            let original = andamento_snapshot_detail_find(eager, text(kind), text(&id));
            assert_ne!(index, usize::MAX);
            assert_eq!(
                andamento_snapshot_detail_request(h, plain, text(kind), text(&id), ptr::null_mut()),
                index
            );
            let mut a = MaybeUninit::uninit();
            let mut b = MaybeUninit::uninit();
            assert_eq!(andamento_snapshot_detail(plain, index, a.as_mut_ptr()), 1);
            assert_eq!(
                andamento_snapshot_detail(eager, original, b.as_mut_ptr()),
                1
            );
            let a = a.assume_init();
            let b = b.assume_init();
            assert_eq!(
                (read(a.label), a.field_count, a.has_workspace),
                (read(b.label), b.field_count, b.has_workspace)
            );
            for f in 0..a.field_count {
                let mut x = MaybeUninit::uninit();
                let mut y = MaybeUninit::uninit();
                assert_eq!(
                    andamento_snapshot_detail_field(plain, index, f, x.as_mut_ptr()),
                    1
                );
                assert_eq!(
                    andamento_snapshot_detail_field(eager, original, f, y.as_mut_ptr()),
                    1
                );
                let x = x.assume_init();
                let y = y.assume_init();
                assert_eq!(
                    (
                        read(x.name),
                        read(x.text),
                        x.has_value,
                        x.observed_at_ms,
                        x.stale
                    ),
                    (
                        read(y.name),
                        read(y.text),
                        y.has_value,
                        y.observed_at_ms,
                        y.stale
                    )
                );
            }
            if held.is_none() {
                held = Some((a.label, read(a.label), a.activate));
            }
        }
        let (borrowed, label, action) = held.unwrap();
        assert_eq!(read(borrowed), label);
        assert_eq!(andamento_snapshot_detail_count(plain), 6);
        assert_eq!(
            andamento_snapshot_detail_request(h, plain, text("missing"), text(""), ptr::null_mut()),
            usize::MAX
        );
        let patch = r#"{"target":{"kind":"entity","value":{"kind":"issue","id":"new"}},"source_id":"test","set":{"display.label":{"value":{"type":"text","value":"New"}}},"unset":[]}"#;
        assert_eq!(
            andamento_apply_patch_json(h, 43, text(patch), ptr::null_mut()),
            1
        );
        assert_eq!(read(borrowed), label);
        let mut error = ptr::null_mut();
        assert_eq!(
            andamento_snapshot_detail_request(h, plain, text("issue"), text("new"), &mut error),
            usize::MAX
        );
        assert!(!error.is_null());
        andamento_string_free(error);
        assert_eq!(andamento_dispatch(h, plain, action, ptr::null_mut()), 0);
        let fresh = andamento_snapshot_acquire(h, ptr::null_mut());
        assert_ne!(
            andamento_snapshot_detail_request(
                h,
                fresh,
                text("issue"),
                text("new"),
                ptr::null_mut()
            ),
            usize::MAX
        );
        let unset = r#"{"target":{"kind":"entity","value":{"kind":"issue","id":"new"}},"source_id":"test","unset":["display.label"]}"#;
        assert_eq!(
            andamento_apply_patch_json(h, 44, text(unset), ptr::null_mut()),
            1
        );
        let removed = andamento_snapshot_acquire(h, ptr::null_mut());
        assert_eq!(
            andamento_snapshot_detail_request(
                h,
                removed,
                text("issue"),
                text("new"),
                ptr::null_mut()
            ),
            usize::MAX
        );
        assert_ne!(
            andamento_snapshot_detail_find(fresh, text("issue"), text("new")),
            usize::MAX
        );
        andamento_snapshot_release(removed);
        // Cached detail on the old snapshot remains readable after mutation.
        assert_ne!(
            andamento_snapshot_detail_request(
                h,
                plain,
                text("worktree"),
                text("worktree:identity / #"),
                ptr::null_mut()
            ),
            usize::MAX
        );
        for snapshot in [fresh, eager, plain] {
            andamento_snapshot_release(snapshot);
        }
        andamento_destroy(h);
    }
}

// A non-placement region still resolves its template and reports errors through
// plain and detailed native snapshots; the narrow path cannot silently skip it.
#[test]
fn nonplacement_region_reports_template_diagnostic() {
    unsafe {
        let config = "region \"legacy\" root-template=\"missing-template\"";
        let h = andamento_create(config.as_ptr(), config.len(), ptr::null_mut());
        assert!(!h.is_null());
        for snapshot in [
            andamento_snapshot_acquire(h, ptr::null_mut()),
            andamento_snapshot_acquire_details(h, ptr::null_mut()),
        ] {
            assert!(andamento_snapshot_diagnostic_count(snapshot) > 0);
            andamento_snapshot_release(snapshot);
        }
        andamento_destroy(h);
    }
}
