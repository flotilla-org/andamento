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
