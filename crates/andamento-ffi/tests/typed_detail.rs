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
        let s = andamento_snapshot_acquire(h, ptr::null_mut());
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
        let boundary = andamento_snapshot_acquire(h, ptr::null_mut());
        assert_eq!(field(boundary, card, "summary").has_value, 1);
        andamento_tick(h, 143, ptr::null_mut());
        let expired = andamento_snapshot_acquire(h, ptr::null_mut());
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
