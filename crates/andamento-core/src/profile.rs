//! Opt-in, per-thread replay measurements. Phases can nest; do not add their times.
use std::{cell::RefCell, collections::BTreeMap, time::Instant};

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Phase {
    pub calls: u64,
    pub nanos: u128,
}
thread_local! {
    static STATS: RefCell<Option<BTreeMap<&'static str, Phase>>> = const { RefCell::new(None) };
}
/// Enable counters on this thread and discard earlier measurements.
pub fn start() {
    STATS.with(|s| *s.borrow_mut() = Some(BTreeMap::new()));
}
/// Read and reset counters without disabling them.
pub fn take() -> BTreeMap<&'static str, Phase> {
    STATS.with(|s| {
        s.borrow_mut()
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    })
}
pub struct Span {
    phase: &'static str,
    start: Option<Instant>,
}
pub fn span(phase: &'static str) -> Span {
    Span {
        phase,
        start: STATS.with(|s| s.borrow().is_some().then(Instant::now)),
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            STATS.with(|s| {
                if let Some(stats) = s.borrow_mut().as_mut() {
                    let p = stats.entry(self.phase).or_default();
                    p.calls += 1;
                    p.nanos += start.elapsed().as_nanos();
                }
            });
        }
    }
}
