//! What a sidebar holds once built: counted by a global allocator (only in
//! this test binary), from a recorded snapshot and the deltas recorded
//! after it, with everything but the sidebar dropped.
//!
//! `ANDAMENTO_CATALOG_FIXTURE=snapshot.json ANDAMENTO_CATALOG_TEMPLATE=sidebar.kdl
//! cargo test -p andamento-core --test memory -- --ignored --nocapture`
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use andamento_core::{MetadataPatch, Sidebar};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> isize {
    LIVE.load(Ordering::Relaxed)
}

fn patches(json: &str) -> Vec<MetadataPatch> {
    serde_json::from_str(json).unwrap()
}

#[test]
#[ignore]
fn what_a_sidebar_holds() {
    let path = std::env::var("ANDAMENTO_CATALOG_FIXTURE").expect("ANDAMENTO_CATALOG_FIXTURE");
    let config = std::fs::read_to_string(std::env::var("ANDAMENTO_CATALOG_TEMPLATE").expect("ANDAMENTO_CATALOG_TEMPLATE")).unwrap();
    let snapshot = std::fs::read_to_string(&path).unwrap();
    let deltas = std::fs::read_to_string(std::path::Path::new(&path).with_file_name("deltas.jsonl")).unwrap_or_default();
    let base = live();
    let mut sidebar = Sidebar::new(&config).unwrap();
    let empty = live() - base;
    sidebar.apply(1, "subscription", patches(&snapshot)).unwrap();
    let _ = sidebar.snapshot_shared();
    let after_snapshot = live() - base;
    for (i, line) in deltas.lines().enumerate() {
        let request: serde_json::Value = serde_json::from_str(line).unwrap();
        let patches: Vec<MetadataPatch> = serde_json::from_value(request["patches"].clone()).unwrap();
        sidebar.apply(10 + i as u64, "subscription", patches).unwrap();
        let _ = sidebar.snapshot_shared();
    }
    let after_deltas = live() - base;
    let mb = |bytes: isize| bytes as f64 / 1e6;
    println!(
        "a sidebar holds {:.2} MB empty, {:.2} MB after the snapshot, {:.2} MB after {} deltas",
        mb(empty),
        mb(after_snapshot),
        mb(after_deltas),
        deltas.lines().count()
    );
    // MEMORY_FORGET=1 leaves it held, so heaptrack reports it as leaked,
    // by where it was allocated.
    if std::env::var_os("MEMORY_FORGET").is_some() {
        std::mem::forget(sidebar);
    }
}
