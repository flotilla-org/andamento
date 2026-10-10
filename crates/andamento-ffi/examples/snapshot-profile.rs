//! Release C-ABI update-stream probe, using the same scripted stream for each run.
use andamento_ffi::*;
use std::{ptr, time::Instant};
fn text(s: &str) -> Text {
    Text {
        data: s.as_ptr(),
        len: s.len(),
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: snapshot-profile CONFIG.kdl PATCHES.jsonl");
        std::process::exit(2);
    }
    let config = std::fs::read_to_string(&args[1]).unwrap();
    let fixture = std::fs::read_to_string(&args[2]).unwrap();
    for n in [100, 300, 1000] {
        unsafe {
            let h = andamento_create(config.as_ptr(), config.len(), ptr::null_mut());
            assert!(!h.is_null());
            andamento_core::profile::start();
            for line in fixture.lines().filter(|l| !l.trim().is_empty()) {
                assert_eq!(
                    andamento_apply_patch_json(h, 100, text(line), ptr::null_mut()),
                    1
                );
            }
            for i in 0..n {
                let patch = format!(
                    r#"{{"target":{{"kind":"entity","value":{{"kind":"issue","id":"bench-{i}"}}}},"source_id":"bench","set":{{"flotilla.project":{{"value":{{"type":"text","value":"p"}}}},"display.label":{{"value":{{"type":"text","value":"Issue {i}"}}}},"flotilla.issue.state":{{"value":{{"type":"text","value":"open"}}}}}},"unset":[]}}"#
                );
                assert_eq!(
                    andamento_apply_patch_json(h, 100, text(&patch), ptr::null_mut()),
                    1
                );
            }
            println!(
                "load entities={n} phases={}",
                serde_json::to_string(&andamento_core::profile::take()).unwrap()
            );
            let warm = andamento_snapshot_acquire_details(h, ptr::null_mut());
            assert!(!warm.is_null());
            let nodes = andamento_snapshot_node_count(warm);
            andamento_snapshot_release(warm);
            andamento_core::profile::take();
            let mut plain = vec![];
            let mut detailed = vec![];
            for iteration in 0..10 {
                let patch = format!(
                    r#"{{"target":{{"kind":"entity","value":{{"kind":"issue","id":"bench-0"}}}},"source_id":"bench","set":{{"bench.tick":{{"value":{{"type":"text","value":"{iteration}"}}}}}},"unset":[]}}"#
                );
                assert_eq!(
                    andamento_apply_patch_json(h, 100 + iteration, text(&patch), ptr::null_mut()),
                    1
                );
                let start = Instant::now();
                let s = if iteration % 2 == 0 {
                    andamento_snapshot_acquire(h, ptr::null_mut())
                } else {
                    andamento_snapshot_acquire_details(h, ptr::null_mut())
                };
                assert!(!s.is_null());
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                if iteration % 2 == 0 {
                    plain.push(elapsed);
                } else {
                    detailed.push(elapsed);
                }
                andamento_snapshot_release(s);
            }
            plain.sort_by(f64::total_cmp);
            detailed.sort_by(f64::total_cmp);
            println!("updates entities={n} nodes={nodes} plain_median_ms={:.3} detailed_median_ms={:.3} phases={}", plain[2], detailed[2], serde_json::to_string(&andamento_core::profile::take()).unwrap());
            let mut demand = vec![];
            for iteration in 10..15 {
                let patch = format!(
                    r#"{{"target":{{"kind":"entity","value":{{"kind":"issue","id":"bench-0"}}}},"source_id":"bench","set":{{"bench.tick":{{"value":{{"type":"text","value":"{iteration}"}}}}}},"unset":[]}}"#
                );
                assert_eq!(
                    andamento_apply_patch_json(h, 100 + iteration, text(&patch), ptr::null_mut()),
                    1
                );
                let start = Instant::now();
                let s = andamento_snapshot_acquire(h, ptr::null_mut());
                for id in ["bench-0", "hidden-detail"] {
                    assert_ne!(
                        andamento_snapshot_detail_request(
                            h,
                            s,
                            text("issue"),
                            text(id),
                            ptr::null_mut()
                        ),
                        usize::MAX
                    );
                }
                demand.push(start.elapsed().as_secs_f64() * 1000.);
                // Same-snapshot requests use the materialized detail, with no evaluation.
                for _ in 0..10 {
                    andamento_snapshot_detail_request(
                        h,
                        s,
                        text("issue"),
                        text("bench-0"),
                        ptr::null_mut(),
                    );
                }
                andamento_snapshot_release(s);
            }
            demand.sort_by(f64::total_cmp);
            println!(
                "demand entities={n} two_cards_median_ms={:.3} phases={}",
                demand[2],
                serde_json::to_string(&andamento_core::profile::take()).unwrap()
            );
            let mut clicks = vec![];
            for _ in 0..5 {
                let s = andamento_snapshot_acquire(h, ptr::null_mut());
                let mut toggle = usize::MAX;
                for i in 0..andamento_snapshot_node_count(s) {
                    let mut node = std::mem::MaybeUninit::uninit();
                    assert_eq!(andamento_snapshot_node(s, i, node.as_mut_ptr()), 1);
                    let node = node.assume_init();
                    if node.toggle != usize::MAX {
                        toggle = node.toggle;
                        break;
                    }
                }
                assert_ne!(toggle, usize::MAX);
                andamento_core::profile::take();
                let start = Instant::now();
                assert_eq!(andamento_dispatch(h, s, toggle, ptr::null_mut()), 1);
                // Validation should consume the rendered revision without rebuilding.
                assert!(!andamento_core::profile::take().contains_key("catalog"));
                let next = andamento_snapshot_acquire(h, ptr::null_mut());
                clicks.push(start.elapsed().as_secs_f64() * 1000.);
                andamento_snapshot_release(next);
                andamento_snapshot_release(s);
            }
            clicks.sort_by(f64::total_cmp);
            println!(
                "clicks entities={n} median_ms={:.3} last_output_phases={}",
                clicks[2],
                serde_json::to_string(&andamento_core::profile::take()).unwrap()
            );
            // Arrangement commits (#144): one per docking gesture. They move
            // the content revision, not the sidebar's, so the rendered
            // snapshot stays current and no evaluation runs.
            let ws = WorkspaceIdView { bytes: [7; 16] };
            assert_eq!(andamento_workspace_register(h, ws, ptr::null_mut()), 1);
            let open = WorkspaceInput3 {
                id: ws,
                position: 0,
                name: text("Bench"),
                selected: 1,
            };
            assert_eq!(
                andamento_observe3(h, &open, 1, ptr::null(), 0, ptr::null_mut()),
                1
            );
            let keys: Vec<String> = (0..8).map(|i| format!("u:{i}")).collect();
            for key in &keys {
                let mut spec: ViewSpecView = std::mem::zeroed();
                spec.content = 2; // URL
                spec.url = text("https://example.com");
                assert_eq!(
                    andamento_slot_set(h, ws, text(key), &spec, 0, ptr::null_mut()),
                    1
                );
            }
            let rendered = andamento_snapshot_acquire(h, ptr::null_mut());
            andamento_core::profile::take();
            let tabs: Vec<TabView> = keys
                .iter()
                .map(|key| TabView {
                    slot: text(key),
                    placed: 0,
                    gone: 0,
                })
                .collect();
            let mut commits = vec![];
            let mut generation = 0;
            for iteration in 0..20 {
                let left = 0.3 + 0.02 * iteration as f64;
                let panel = |parent, id, weight, kind, first_tab, tab_count| PanelView {
                    parent,
                    id: text(id),
                    weight,
                    kind,
                    axis: 0,
                    first_tab,
                    tab_count,
                    selected: if tab_count > 0 { 0 } else { usize::MAX },
                };
                let panels = [
                    panel(usize::MAX, "root", 1.0, 0, 0, 0),
                    panel(0, "1", left, 1, 0, 4),
                    panel(0, "2", 1.0 - left, 1, 4, 4),
                ];
                let start = Instant::now();
                assert_eq!(
                    andamento_set_arrangement(
                        h,
                        ws,
                        panels.as_ptr(),
                        panels.len(),
                        tabs.as_ptr(),
                        tabs.len(),
                        generation,
                        &mut generation,
                        ptr::null_mut()
                    ),
                    1
                );
                assert_eq!(
                    andamento_snapshot_is_current(h, rendered, ptr::null_mut()),
                    1
                );
                commits.push(start.elapsed().as_secs_f64() * 1000.);
            }
            commits.sort_by(f64::total_cmp);
            let phases = andamento_core::profile::take();
            assert!(!phases.contains_key("revision-evaluation"));
            println!(
                "arrangements entities={n} commits=20 generation={generation} median_ms={:.4} max_ms={:.4} snapshot_current=20/20 phases={}",
                commits[10],
                commits[19],
                serde_json::to_string(&phases).unwrap()
            );
            // The sidebar arrangement: the document Andamento placed, with a
            // dock weight dragged at each commit.
            let sidebar = andamento_sidebar_arrangement_acquire(h, ptr::null_mut());
            assert!(!sidebar.is_null());
            let mut info: ArrangementInfoView = std::mem::zeroed();
            assert_eq!(andamento_arrangement_info(sidebar, &mut info), 1);
            let floating_first = andamento_arrangement_floating_first(sidebar);
            let mut panels: Vec<PanelView> = (0..info.panel_count)
                .map(|i| {
                    let mut panel: PanelView = std::mem::zeroed();
                    assert_eq!(andamento_arrangement_panel(sidebar, i, &mut panel), 1);
                    panel
                })
                .collect();
            let tabs: Vec<TabView> = (0..info.tab_count)
                .map(|i| {
                    let mut tab: TabView = std::mem::zeroed();
                    assert_eq!(andamento_arrangement_tab(sidebar, i, &mut tab), 1);
                    tab
                })
                .collect();
            let mut generation = info.generation;
            let mut commits = vec![];
            for iteration in 0..20 {
                if panels.len() > 1 {
                    panels[1].weight = 0.3 + 0.02 * iteration as f64;
                }
                let start = Instant::now();
                assert_eq!(
                    andamento_set_sidebar_arrangement(
                        h,
                        panels.as_ptr(),
                        panels.len(),
                        floating_first,
                        tabs.as_ptr(),
                        tabs.len(),
                        generation,
                        &mut generation,
                        ptr::null_mut()
                    ),
                    1
                );
                assert_eq!(
                    andamento_snapshot_is_current(h, rendered, ptr::null_mut()),
                    1
                );
                commits.push(start.elapsed().as_secs_f64() * 1000.);
            }
            andamento_arrangement_release(sidebar);
            andamento_snapshot_release(rendered);
            commits.sort_by(f64::total_cmp);
            println!(
                "sidebar-arrangement entities={n} panels={} commits=20 generation={generation} median_ms={:.4} max_ms={:.4} snapshot_current=20/20",
                panels.len(),
                commits[10],
                commits[19],
            );
            #[cfg(target_os = "linux")]
            println!(
                "memory entities={n} {}",
                std::fs::read_to_string("/proc/self/status")
                    .unwrap()
                    .lines()
                    .find(|l| l.starts_with("VmHWM:"))
                    .unwrap()
            );
            andamento_destroy(h);
        }
    }
}
