//! What a patch costs as the catalog grows (#161). A catalog shaped like a
//! working Flotilla fleet's (about 580 entities, mostly convoys and vessels
//! with ~25 facts each) arrives as one snapshot and then as small deltas;
//! neither may cost work in proportion to everything already stored.
use crate::metadata::ENTITY_RESOLUTIONS;
use crate::sidebar::Sidebar;
use crate::{EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate};
use std::collections::BTreeMap;

const PROVIDER: &str = "subscription";
const CONFIG: &str = include_str!("../tests/fixtures/sidebar.kdl");

fn text(value: impl Into<String>) -> MetadataValue {
    MetadataValue::Text(value.into())
}

fn patch(kind: &str, id: &str, facts: Vec<(String, MetadataValue)>) -> MetadataPatch {
    let mut set: BTreeMap<String, MetadataValueUpdate> =
        facts.into_iter().map(|(key, value)| (key, MetadataValueUpdate { value, ttl_ms: None, precedence: None, ordinal: None })).collect();
    for (key, value) in [("entity.kind", kind), ("entity.id", id), ("source", "flotilla")] {
        set.insert(key.into(), MetadataValueUpdate { value: text(value), ttl_ms: None, precedence: None, ordinal: None });
    }
    MetadataPatch {
        target: MetadataTarget::Entity(EntityRef::new(PROVIDER, kind, id)),
        source_id: "flotilla-connector".into(),
        set,
        unset: vec![],
    }
}

fn labels(label: &str) -> Vec<(String, MetadataValue)> {
    vec![
        ("display.label".into(), text(label)),
        ("display.label.medium".into(), text(label)),
        ("display.label.short".into(), text(&label[..1])),
    ]
}

/// A fleet of `convoys` convoys, each with a vessel, in a handful of
/// projects, with roles, change requests and repository memberships in the
/// proportions of the real catalog.
fn catalog(convoys: usize) -> Vec<MetadataPatch> {
    let projects = (convoys / 46).max(1);
    let project = |i: usize| format!("fleet/p{}@fleet", i % projects);
    let mut patches = vec![];
    for p in 0..projects {
        let id = project(p);
        let mut facts = labels(&format!("project {p}"));
        facts.extend([
            ("flotilla.project".into(), text(&id)),
            ("flotilla.project.name".into(), text(format!("p{p}"))),
            ("action.primary.recipe".into(), text("flotilla workspace open")),
            ("action.primary.target".into(), text(format!("view:project/{id}"))),
            ("count.convoys".into(), MetadataValue::Integer(convoys as i64)),
            ("status.state".into(), text("active")),
            ("summary.text".into(), text(format!("{convoys} entries"))),
        ]);
        patches.push(patch("project", &id, facts));
    }
    for c in 0..convoys {
        let id = format!("fleet/convoy-{c:08x}");
        let phase = if c % 5 == 0 { "active" } else { "landed" };
        let mut convoy = labels(&format!("convoy {c}"));
        convoy.extend([
            ("flotilla.convoy".into(), text(&id)),
            ("flotilla.convoy.name".into(), text(format!("convoy {c}"))),
            ("flotilla.convoy.phase".into(), text(phase)),
            ("flotilla.convoy.held".into(), MetadataValue::Bool(false)),
            ("flotilla.convoy.superseded".into(), MetadataValue::Bool(false)),
            ("flotilla.convoy.holds".into(), MetadataValue::StringList(vec![])),
            ("flotilla.convoy.pending_messages".into(), MetadataValue::StringList(vec![])),
            ("flotilla.convoy.stuck_messages".into(), MetadataValue::StringList(vec![])),
            ("flotilla.convoy.latest_ledger".into(), text(format!("artifact/{c:032x}"))),
            ("flotilla.project".into(), text(project(c))),
            ("flotilla.readiness.state".into(), text("ready")),
            ("flotilla.readiness.attach_available".into(), MetadataValue::Bool(false)),
            ("flotilla.readiness.blockers".into(), MetadataValue::StringList(vec![])),
            ("flotilla.surface.state".into(), text("available")),
            ("status.state".into(), text(if phase == "active" { "running" } else { "done" })),
            ("summary.text".into(), text(format!("convoy {c}"))),
            ("vcs.repo".into(), text("example/repo")),
            ("vcs.repo.name".into(), text("repo")),
        ]);
        patches.push(patch("convoy", &id, convoy));
        let mut vessel = labels("work");
        vessel.extend([
            ("flotilla.convoy".into(), text(&id)),
            ("flotilla.convoy.phase".into(), text(phase)),
            ("flotilla.vessel".into(), text(&id)),
            ("flotilla.vessel.name".into(), text("work")),
            ("flotilla.work.phase".into(), text("complete")),
            ("flotilla.project".into(), text(project(c))),
            ("flotilla.readiness.state".into(), text("ready")),
            ("flotilla.readiness.blockers".into(), MetadataValue::StringList(vec![])),
            ("flotilla.surface.state".into(), text("available")),
            ("status.state".into(), text("done")),
            ("vcs.repo".into(), text("example/repo")),
        ]);
        patches.push(patch("vessel", &id, vessel));
        if c % 14 == 0 {
            let cr = format!("github.com/example/repo!{c}");
            let mut facts = labels(&format!("repo!{c}"));
            facts.extend([
                ("flotilla.change_request.state".into(), text("open")),
                ("flotilla.change_request.title".into(), text(format!("Change {c}"))),
                ("flotilla.project".into(), text(project(c))),
                ("flotilla.subject.kind".into(), text("change_request")),
            ]);
            patches.push(patch("change_request", &cr, facts));
        }
        if c % 35 == 0 {
            let role = format!("{}/governor{c}", project(c));
            let mut facts = labels("governor");
            facts.extend([
                ("flotilla.role".into(), text(&role)),
                ("flotilla.role.state".into(), text("active")),
                ("flotilla.project".into(), text(project(c))),
                ("related.entities".into(), MetadataValue::EntityRefs(vec![EntityRef::new(PROVIDER, "convoy", &id)])),
            ]);
            patches.push(patch("role", &role, facts));
        }
    }
    patches
}

/// A delta that changes one convoy's phase, as the fleet does as it works.
fn delta(c: usize, phase: &str) -> MetadataPatch {
    let id = format!("fleet/convoy-{c:08x}");
    let mut patch = patch("convoy", &id, vec![("flotilla.convoy.phase".into(), text(phase))]);
    patch.set.retain(|key, _| key == "flotilla.convoy.phase");
    patch
}

fn resolutions(f: impl FnOnce()) -> usize {
    ENTITY_RESOLUTIONS.with(|count| count.set(0));
    f();
    ENTITY_RESOLUTIONS.with(|count| count.get())
}

/// Entity resolutions for the last patches of a catalog applied one at a
/// time, and for a delta afterwards.
fn per_patch(convoys: usize) -> (usize, usize) {
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let patches = catalog(convoys);
    let (head, tail) = patches.split_at(patches.len() - 10);
    for patch in head {
        sidebar.apply(1, PROVIDER, [patch.clone()]).unwrap();
    }
    let last = resolutions(|| {
        for patch in tail {
            sidebar.apply(1, PROVIDER, [patch.clone()]).unwrap();
        }
    }) / tail.len();
    let delta = resolutions(|| sidebar.apply(2, PROVIDER, [delta(1, "active")]).unwrap());
    (last, delta)
}

#[test]
fn a_patch_costs_the_same_whatever_the_catalog_holds() {
    let (small_patch, small_delta) = per_patch(20);
    let (large_patch, large_delta) = per_patch(280);
    assert_eq!(
        (large_patch, large_delta),
        (small_patch, small_delta),
        "entity resolutions per patch and per delta must not grow with the catalog"
    );
}

#[test]
fn a_snapshot_applied_at_once_resolves_each_entity_a_bounded_number_of_times() {
    let patches = catalog(280);
    let mut sidebar = Sidebar::new(CONFIG).unwrap();
    let count = resolutions(|| sidebar.apply(1, PROVIDER, patches.clone()).unwrap());
    assert!(count <= 2 * patches.len(), "{count} resolutions for {} patches", patches.len());
}

/// Timings against a real catalog, kept out of the repository:
/// `ANDAMENTO_CATALOG_FIXTURE=<snapshot.json> cargo test --release -p andamento-core
/// real_catalog -- --ignored --nocapture`. The file is the JSON array a
/// Flotilla connector posts to `/v1/metadata/snapshot`; a `deltas.jsonl`
/// beside it is replayed after it. ANDAMENTO_CATALOG_TEMPLATE names the
/// template to place it with (a host's, say), else the test fixture's.
#[test]
#[ignore]
fn real_catalog_timings() {
    let path = std::env::var("ANDAMENTO_CATALOG_FIXTURE").expect("ANDAMENTO_CATALOG_FIXTURE");
    let messages: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let patches: Vec<MetadataPatch> = messages.into_iter().map(|m| serde_json::from_value(m).unwrap()).collect();
    let time = |f: &mut dyn FnMut()| {
        let started = std::time::Instant::now();
        f();
        started.elapsed()
    };
    let template = std::env::var("ANDAMENTO_CATALOG_TEMPLATE").map(|path| std::fs::read_to_string(path).unwrap());
    let config = template.as_deref().unwrap_or(CONFIG);
    let mut sidebar = Sidebar::new(config).unwrap();
    let mut each = vec![];
    for patch in &patches {
        each.push(time(&mut || sidebar.apply(1, PROVIDER, [patch.clone()]).unwrap()));
    }
    let total: std::time::Duration = each.iter().sum();
    println!("one at a time: {} patches {:?} (first {:?}, last {:?})", patches.len(), total, each.first().unwrap(), each.last().unwrap());
    let mut batched = Sidebar::new(config).unwrap();
    println!("one batch: {:?}", time(&mut || batched.apply(1, PROVIDER, patches.clone()).unwrap()));
    println!("evaluation: {:?}", time(&mut || drop(batched.snapshot())));
    let first = patches.iter().find(|p| matches!(&p.target, MetadataTarget::Entity(e) if e.kind == "convoy")).unwrap().clone();
    let mut one = first.clone();
    one.set.retain(|key, _| key == "status.state");
    for (i, state) in ["running", "done", "running"].into_iter().enumerate() {
        one.set.get_mut("status.state").unwrap().value = text(state);
        let applied = time(&mut || batched.apply(2 + i as u64, PROVIDER, [one.clone()]).unwrap());
        println!("a delta: {applied:?}, then evaluation {:?}", time(&mut || drop(batched.snapshot())));
    }
    // The recorded deltas beside the snapshot (`deltas.jsonl`: one request's
    // patches per line), each followed by the evaluation a frame then pays.
    let deltas = std::path::Path::new(&std::env::var("ANDAMENTO_CATALOG_FIXTURE").unwrap()).with_file_name("deltas.jsonl");
    let Ok(lines) = std::fs::read_to_string(deltas) else {
        return;
    };
    crate::profile::start();
    let (mut applies, mut evaluations) = (vec![], vec![]);
    for (i, line) in lines.lines().enumerate() {
        let request: serde_json::Value = serde_json::from_str(line).unwrap();
        let patches: Vec<MetadataPatch> = serde_json::from_value(request["patches"].clone()).unwrap();
        applies.push(time(&mut || batched.apply(10 + i as u64, PROVIDER, patches.clone()).unwrap()));
        evaluations.push(time(&mut || drop(batched.snapshot())));
    }
    let median = |v: &mut Vec<std::time::Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    println!(
        "{} recorded deltas: apply median {:?}, evaluation median {:?}",
        applies.len(),
        median(&mut applies),
        median(&mut evaluations)
    );
    let mut phases: Vec<_> = crate::profile::take().into_iter().collect();
    phases.sort_by(|a, b| b.1.nanos.cmp(&a.1.nanos));
    for (name, phase) in phases.iter().take(12) {
        println!("  {name:28} {:?} over {} calls", std::time::Duration::from_nanos(phase.nanos as u64), phase.calls);
    }
}

/// What stays between evaluations (the store's resolutions, the placement
/// index) must place exactly what a fresh evaluation would: after deltas
/// that change, move, add and remove entities, evaluated one at a time, the
/// surface equals that of a sidebar given every patch at once.
#[test]
fn kept_state_presents_what_a_fresh_evaluation_does() {
    let vessel = |c: usize, facts: Vec<(&str, MetadataValue)>, unset: Vec<&str>| {
        let id = format!("fleet/convoy-{c:08x}");
        let mut patch = patch("vessel", &id, facts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        patch.set.retain(|key, _| !matches!(key.as_str(), "entity.kind" | "entity.id" | "source"));
        patch.unset = unset.into_iter().map(str::to_owned).collect();
        patch
    };
    let mut patches = catalog(60);
    // Entities come and go first (the catalog's list changes), then facts
    // change on entities already listed (only their positions change).
    patches.push(catalog(61).into_iter().rev().find(|p| matches!(&p.target, MetadataTarget::Entity(e) if e.kind == "vessel")).unwrap());
    patches.push(vessel(5, vec![], vec!["entity.id", "entity.kind"]));
    patches.extend([
        vessel(1, vec![("status.attention", MetadataValue::Bool(true))], vec![]),
        vessel(2, vec![("status.attention", MetadataValue::Bool(true)), ("display.label", text("renamed"))], vec![]),
        vessel(3, vec![("flotilla.project", text("fleet/p0@fleet"))], vec![]),
        vessel(1, vec![("status.attention", MetadataValue::Bool(false))], vec![]),
        vessel(4, vec![], vec!["flotilla.project"]),
        vessel(3, vec![("status.state", text("running"))], vec![]),
    ]);

    let config = include_str!("../tests/fixtures/sidebar.kdl");
    let mut kept = Sidebar::new(config).unwrap();
    for patch in &patches {
        kept.apply(1, PROVIDER, [patch.clone()]).unwrap();
        drop(kept.snapshot());
    }
    let mut fresh = Sidebar::new(config).unwrap();
    fresh.apply(1, PROVIDER, patches).unwrap();
    assert_eq!(kept.snapshot().surface, fresh.snapshot().surface);
}
