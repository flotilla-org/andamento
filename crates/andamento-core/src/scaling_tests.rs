//! What a patch costs as the catalog grows (#161). A catalog shaped like a
//! working Flotilla fleet's (about 580 entities, mostly convoys and vessels
//! with ~25 facts each) arrives as one snapshot and then as small deltas;
//! neither may cost work in proportion to everything already stored.
use crate::census::Census;
use crate::metadata::ENTITY_RESOLUTIONS;
use crate::presentation::{PlacementNode, SurfaceSnapshot};
use crate::sidebar::{Action, Sidebar, Workspace};
use crate::state::ControllerState;
use crate::template_config::{parse_template_config_kdl, TemplateConfigCatalog, TemplateConfigLayer};
use crate::{tab_workspace_id, EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, WorkspaceId};
use std::collections::BTreeMap;
use std::rc::Rc;

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

/// The shape of a generated fleet.
#[derive(Debug, Clone, Copy)]
struct Shape {
    projects: usize,
    convoys_per_project: usize,
    /// A change request for one convoy in this many (0: none).
    change_request_every: usize,
    /// A role for one convoy in this many (0: none).
    role_every: usize,
    /// One convoy in this many is active, the rest landed.
    active_every: usize,
    /// One vessel in this many needs attention (0: none).
    attention_every: usize,
}

impl Shape {
    /// The real catalog's proportions, at `convoys` convoys.
    fn like_the_fleet(convoys: usize) -> Self {
        let projects = (convoys / 46).max(1);
        Self {
            projects,
            convoys_per_project: convoys.div_ceil(projects),
            change_request_every: 14,
            role_every: 35,
            active_every: 5,
            attention_every: 0,
        }
    }
}

/// A fleet of `convoys` convoys, each with a vessel, in a handful of
/// projects, with roles and change requests in the proportions of the real
/// catalog.
fn catalog(convoys: usize) -> Vec<MetadataPatch> {
    fleet(Shape::like_the_fleet(convoys), convoys)
}

/// `limit` convoys (at most the shape's) of a fleet shaped like `shape`.
/// Convoy `c` belongs to project `c / convoys_per_project`.
fn fleet(shape: Shape, limit: usize) -> Vec<MetadataPatch> {
    let convoys = limit.min(shape.projects * shape.convoys_per_project);
    let projects = convoys.div_ceil(shape.convoys_per_project).max(1);
    let project = |c: usize| format!("fleet/p{}@fleet", c / shape.convoys_per_project);
    let every = |n: usize, c: usize| n != 0 && c % n == 0;
    let mut patches = vec![];
    for p in 0..projects {
        let id = format!("fleet/p{p}@fleet");
        let mut facts = labels(&format!("project {p}"));
        facts.extend([
            ("flotilla.project".into(), text(&id)),
            ("flotilla.project.name".into(), text(format!("p{p}"))),
            ("action.primary.recipe".into(), text("flotilla workspace open")),
            ("action.primary.target".into(), text(format!("view:project/{id}"))),
            ("count.convoys".into(), MetadataValue::Integer(shape.convoys_per_project as i64)),
            ("status.state".into(), text("active")),
            ("summary.text".into(), text(format!("{} entries", shape.convoys_per_project))),
        ]);
        patches.push(patch("project", &id, facts));
    }
    for c in 0..convoys {
        let id = format!("fleet/convoy-{c:08x}");
        let phase = if every(shape.active_every, c) { "active" } else { "landed" };
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
        if every(shape.attention_every, c) {
            vessel.push(("status.attention".into(), MetadataValue::Bool(true)));
        }
        patches.push(patch("vessel", &id, vessel));
        if every(shape.change_request_every, c) {
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
        if every(shape.role_every, c) {
            let role = format!("{}/governor{c}", project(c));
            let mut facts = labels("governor");
            facts.extend([
                ("flotilla.role".into(), text(&role)),
                ("flotilla.role.state".into(), text("active")),
                ("flotilla.project".into(), text(project(c))),
                ("related.entities".into(), MetadataValue::EntityRefs(vec![EntityRef::new(PROVIDER, "convoy", &id)])),
                ("flotilla.role.attempts".into(), MetadataValue::EntityRefs(vec![EntityRef::new(PROVIDER, "convoy", &id)])),
                ("flotilla.role.current_attempt".into(), MetadataValue::EntityRefs(vec![EntityRef::new(PROVIDER, "convoy", &id)])),
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

/// Everything evaluation keeps between revisions must present exactly what
/// a first evaluation of the same state does. After each step of a session
/// that changes facts, workspaces, focus, collapse, variables, display
/// variables, sibling order, staleness and time, the kept surface equals a
/// cold one, for the fixture template and for Wheelhouse's daily driver.
#[test]
fn kept_evaluation_matches_a_cold_one_at_every_step() {
    for config in [CONFIG, include_str!("../tests/fixtures/daily-driver.kdl")] {
        kept_matches_cold_through_a_session(config);
    }
}

fn kept_matches_cold_through_a_session(config: &str) {
    let convoy = |c: usize| EntityRef::new(PROVIDER, "convoy", format!("fleet/convoy-{c:08x}"));
    let change = move |kind: &str, c: usize, facts: Vec<(&str, MetadataValue)>, unset: Vec<&str>| {
        let id = format!("fleet/convoy-{c:08x}");
        let mut patch = patch(kind, &id, facts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        patch.set.retain(|key, _| !matches!(key.as_str(), "entity.kind" | "entity.id" | "source"));
        patch.unset = unset.into_iter().map(str::to_owned).collect();
        patch
    };
    let bind = |workspace: WorkspaceId, entity: &EntityRef| {
        let mut patch = patch(&entity.kind, &entity.id, vec![]);
        patch.target = MetadataTarget::Tab(workspace);
        patch.source_id = "host".into();
        patch
    };
    let (w1, w2) = (tab_workspace_id(1), tab_workspace_id(2));
    let workspaces = move |selected: WorkspaceId| {
        [w1, w2]
            .into_iter()
            .enumerate()
            .map(|(position, id)| Workspace { id, position, name: format!("w{position}"), selected: id == selected })
            .collect::<Vec<_>>()
    };
    // The first placed node with children, and one of its children.
    let keys = |sidebar: &Sidebar| {
        let surface = &sidebar.snapshot_shared().surface;
        let parent = surface.sections.iter().flat_map(|s| &s.nodes).find(|n| !n.children.is_empty()).expect("a nested node");
        (parent.key.clone(), parent.children[0].key.clone(), parent.children[0].loop_key.clone())
    };

    type Step = Box<dyn Fn(&mut Sidebar)>;
    let steps: Vec<(&str, Step)> = vec![
        ("snapshot", Box::new(|s: &mut Sidebar| s.apply(1, PROVIDER, catalog(60)).unwrap())),
        ("open workspaces", Box::new(move |s: &mut Sidebar| s.observe(workspaces(w1), vec![]))),
        ("bind workspaces", Box::new(move |s: &mut Sidebar| s.apply(2, PROVIDER, [bind(w1, &convoy(3)), bind(w2, &convoy(10))]).unwrap())),
        (
            "attention",
            Box::new(move |s: &mut Sidebar| {
                s.apply(3, PROVIDER, [change("vessel", 7, vec![("status.attention", MetadataValue::Bool(true))], vec![])]).unwrap()
            }),
        ),
        (
            "rename",
            Box::new(move |s: &mut Sidebar| {
                s.apply(4, PROVIDER, [change("convoy", 3, vec![("display.label", text("renamed"))], vec![])]).unwrap()
            }),
        ),
        (
            "land",
            Box::new(move |s: &mut Sidebar| {
                s.apply(5, PROVIDER, [change("convoy", 10, vec![("flotilla.convoy.phase", text("landed"))], vec![])]).unwrap()
            }),
        ),
        (
            "reactivate",
            Box::new(move |s: &mut Sidebar| {
                s.apply(6, PROVIDER, [change("convoy", 1, vec![("flotilla.convoy.phase", text("active"))], vec![])]).unwrap()
            }),
        ),
        (
            "move project",
            Box::new(move |s: &mut Sidebar| {
                s.apply(7, PROVIDER, [change("convoy", 5, vec![("flotilla.project", text("fleet/p0@fleet"))], vec![])]).unwrap()
            }),
        ),
        (
            "unset project",
            Box::new(move |s: &mut Sidebar| s.apply(8, PROVIDER, [change("vessel", 6, vec![], vec!["flotilla.project"])]).unwrap()),
        ),
        (
            "remove",
            Box::new(move |s: &mut Sidebar| {
                s.apply(9, PROVIDER, [change("vessel", 12, vec![], vec!["entity.id", "entity.kind"])]).unwrap()
            }),
        ),
        ("add", Box::new(|s: &mut Sidebar| s.apply(10, PROVIDER, catalog(64)[catalog(60).len()..].to_vec()).unwrap())),
        (
            "local refs",
            Box::new(move |s: &mut Sidebar| {
                let (section, group) = (EntityRef::local(".section", "s"), EntityRef::local(".group", "g"));
                let refs = |entity: EntityRef| MetadataValue::EntityRefs(vec![entity]);
                s.set_local(section.clone(), BTreeMap::from([("display.label".into(), text("Pinned"))])).unwrap();
                s.set_local(group.clone(), BTreeMap::from([(".section".into(), refs(section))])).unwrap();
                s.set_local(
                    EntityRef::local(".ref", "r"),
                    BTreeMap::from([(".group".into(), refs(group)), (".target".into(), refs(convoy(4)))]),
                )
                .unwrap();
            }),
        ),
        (
            "rename a ref's target",
            Box::new(move |s: &mut Sidebar| {
                s.apply(10, PROVIDER, [change("convoy", 4, vec![("display.label", text("referenced"))], vec![])]).unwrap()
            }),
        ),
        (
            "retarget a ref",
            Box::new(move |s: &mut Sidebar| {
                let refs = |entity: EntityRef| MetadataValue::EntityRefs(vec![entity]);
                s.set_local(
                    EntityRef::local(".ref", "r"),
                    BTreeMap::from([(".group".into(), refs(EntityRef::local(".group", "g"))), (".target".into(), refs(convoy(8)))]),
                )
                .unwrap();
            }),
        ),
        (
            "replace entities, keeping the count",
            Box::new(move |s: &mut Sidebar| {
                let mut patches = vec![
                    change("convoy", 13, vec![], vec!["entity.id", "entity.kind"]),
                    change("vessel", 13, vec![], vec!["entity.id", "entity.kind"]),
                ];
                patches.extend(
                    catalog(65).into_iter().filter(|p| matches!(&p.target, MetadataTarget::Entity(e) if e.id.ends_with("00000040"))),
                );
                assert_eq!(patches.len(), 4, "two removed, two added");
                s.apply(10, PROVIDER, patches).unwrap();
            }),
        ),
        (
            "rename a role's attempt",
            // Convoy 35's role shows it in its detail.
            Box::new(move |s: &mut Sidebar| {
                s.apply(10, PROVIDER, [change("convoy", 35, vec![("display.label", text("attempt renamed"))], vec![])]).unwrap()
            }),
        ),
        (
            "rekey a project",
            Box::new(|s: &mut Sidebar| {
                // Its nested loops now look up convoys of a project there are none of.
                let mut patch = patch("project", "fleet/p0@fleet", vec![("flotilla.project".into(), text("fleet/elsewhere"))]);
                patch.set.retain(|key, _| key == "flotilla.project");
                s.apply(10, PROVIDER, [patch]).unwrap();
            }),
        ),
        (
            "restore its key",
            Box::new(|s: &mut Sidebar| {
                // And back.
                let mut patch = patch("project", "fleet/p0@fleet", vec![("flotilla.project".into(), text("fleet/p0@fleet"))]);
                patch.set.retain(|key, _| key == "flotilla.project");
                s.apply(10, PROVIDER, [patch]).unwrap();
            }),
        ),
        (
            "supersede bound",
            Box::new(move |s: &mut Sidebar| {
                s.apply(11, PROVIDER, [change("convoy", 3, vec![("flotilla.convoy.superseded", MetadataValue::Bool(true))], vec![])])
                    .unwrap()
            }),
        ),
        ("focus", Box::new(move |s: &mut Sidebar| s.observe(workspaces(w2), vec![]))),
        (
            "collapse",
            Box::new(move |s: &mut Sidebar| {
                let (parent, _, _) = keys(s);
                s.dispatch(Action::TogglePlacement { key: parent }).unwrap();
            }),
        ),
        (
            "node variable",
            Box::new(move |s: &mut Sidebar| {
                let (_, child, _) = keys(s);
                for name in ["density", "convoy.tier"] {
                    let _ = s.dispatch(Action::SetVariable { key: child.clone(), name: name.into(), value: Some("short".into()) });
                    let _ = s.dispatch(Action::SetVariable { key: child.clone(), name: name.into(), value: Some("compact".into()) });
                }
            }),
        ),
        (
            "sibling order",
            Box::new(move |s: &mut Sidebar| {
                let (_, _, loop_key) = keys(s);
                s.set_sibling_order(loop_key, vec![convoy(2), convoy(1)]).unwrap();
            }),
        ),
        (
            "show finished",
            Box::new(|s: &mut Sidebar| {
                let _ = s.dispatch(Action::ToggleDisplayVariable { name: "show-finished".into() });
            }),
        ),
        (
            "show issues",
            Box::new(|s: &mut Sidebar| {
                let _ = s.dispatch(Action::ToggleDisplayVariable { name: "show-issues".into() });
            }),
        ),
        (
            "expiring fact",
            Box::new(move |s: &mut Sidebar| {
                let mut patch = change("vessel", 9, vec![("status.attention", MetadataValue::Bool(true))], vec![]);
                patch.set.values_mut().for_each(|update| update.ttl_ms = Some(50));
                s.apply(12, PROVIDER, [patch]).unwrap();
            }),
        ),
        ("expire", Box::new(|s: &mut Sidebar| s.tick(100))),
        (
            "stale",
            Box::new(|s: &mut Sidebar| {
                s.set_provider_stale(PROVIDER, true);
            }),
        ),
        (
            "delta while stale",
            Box::new(move |s: &mut Sidebar| {
                s.apply(101, PROVIDER, [change("convoy", 2, vec![("display.label", text("while stale"))], vec![])]).unwrap()
            }),
        ),
        (
            "fresh",
            Box::new(|s: &mut Sidebar| {
                s.set_provider_stale(PROVIDER, false);
            }),
        ),
        (
            "close a workspace",
            Box::new(move |s: &mut Sidebar| s.observe(workspaces(w2).into_iter().filter(|w| w.id == w2).collect(), vec![])),
        ),
        (
            "retract",
            Box::new(|s: &mut Sidebar| {
                s.retract_provider(PROVIDER);
            }),
        ),
        ("snapshot again", Box::new(|s: &mut Sidebar| s.apply(200, PROVIDER, catalog(60)).unwrap())),
    ];
    let mut sidebar = Sidebar::new(config).unwrap();
    for (name, step) in steps {
        step(&mut sidebar);
        let kept = sidebar.snapshot_shared().surface.clone();
        assert!(kept == sidebar.cold_surface(), "after {name:?}, the kept surface differs from a cold evaluation");
    }
}

/// A node inherits its parent's variables. When a parent's change (here, its
/// project joining a layer that sets a variable) changes them, its children
/// inherit the new values, though their own placements are kept.
#[test]
fn kept_variables_follow_a_parents_change() {
    let user = parse_template_config_kdl(
        r#"
        variable "density" default="roomy" {
          value "roomy"
          value "compact"
        }
        region "tree" root-template="title" form="compact" placement="tree"
        template "title" slot="compact" node-kind="entity" { field "label" source="literal" value="Tree"; }
        placement "tree" {
          for "project" kind="project" {
            for "vessel" kind="vessel" {
              match "group" of="project"
              apply-template "row"
            }
          }
        }
        template "row" { field "density" key="var.density"; }
        "#,
    )
    .unwrap();
    let special = parse_template_config_kdl(r#"set "density" "compact""#).unwrap();
    let mut state = ControllerState::default();
    state.set_template_catalog(Some(TemplateConfigCatalog::from_layers(vec![
        TemplateConfigLayer::user("user", user),
        TemplateConfigLayer::project("special", "special", special),
    ])));
    let project =
        |membership: &str| patch("project", "p", vec![("group".into(), text("g")), ("flotilla.project".into(), text(membership))]);
    state.apply_provider_patch(PROVIDER, project("ordinary"));
    state.apply_provider_patch(PROVIDER, patch("vessel", "v", vec![("group".into(), text("g"))]));
    let surface = |state: &ControllerState| state.presentation_in(&state.evaluate_revision());
    let density = |surface: &SurfaceSnapshot| surface.sections[0].nodes[0].children[0].content.fields[0].value.clone();
    assert_eq!(density(&surface(&state)), "roomy");
    state.apply_provider_patch(PROVIDER, project("special"));
    let kept = surface(&state);
    assert_eq!(kept, state.cold(surface));
    assert_eq!(density(&kept), "compact");
}

/// Where scaling breaks: generated fleets from the real catalog's size up
/// to hundreds of projects of hundreds of convoys, and lopsided shapes.
/// `cargo test --release -p andamento-core stress_shapes -- --ignored
/// --nocapture`; ANDAMENTO_CATALOG_TEMPLATE as for `real_catalog_timings`.
#[test]
#[ignore]
fn stress_shapes() {
    let template = std::env::var("ANDAMENTO_CATALOG_TEMPLATE").map(|path| std::fs::read_to_string(path).unwrap());
    let config = template.as_deref().unwrap_or(CONFIG);
    let base =
        Shape { projects: 0, convoys_per_project: 0, change_request_every: 14, role_every: 35, active_every: 5, attention_every: 50 };
    let shapes = [
        ("real-size 12x46", Shape { projects: 12, convoys_per_project: 46, ..base }),
        ("30x100", Shape { projects: 30, convoys_per_project: 100, ..base }),
        ("100x100", Shape { projects: 100, convoys_per_project: 100, ..base }),
        ("100x300", Shape { projects: 100, convoys_per_project: 300, ..base }),
        ("300x300", Shape { projects: 300, convoys_per_project: 300, ..base }),
        ("deep 3x10000", Shape { projects: 3, convoys_per_project: 10000, ..base }),
        ("wide 3000x10", Shape { projects: 3000, convoys_per_project: 10, ..base }),
        ("busy 100x100 all active", Shape { projects: 100, convoys_per_project: 100, active_every: 1, attention_every: 3, ..base }),
    ];
    let only = std::env::var("STRESS_ONLY").ok();
    let deltas = std::env::var("STRESS_DELTAS").ok().and_then(|n| n.parse().ok()).unwrap_or(40);
    let phases_shown = std::env::var("STRESS_PHASES").ok().and_then(|n| n.parse().ok()).unwrap_or(5);
    let time = |f: &mut dyn FnMut()| {
        let started = std::time::Instant::now();
        f();
        started.elapsed()
    };
    for (name, shape) in shapes {
        if only.as_deref().is_some_and(|only| !name.contains(only)) {
            continue;
        }
        let patches = fleet(shape, usize::MAX);
        let mut sidebar = Sidebar::new(config).unwrap();
        let snapshot = time(&mut || sidebar.apply(1, PROVIDER, patches.clone()).unwrap());
        let first = time(&mut || {
            sidebar.snapshot_shared();
        });
        let nodes = sidebar.snapshot_shared().surface.sections.iter().map(|s| count_nodes(&s.nodes)).sum::<usize>();
        crate::profile::start();
        let (mut applies, mut evaluations) = (vec![], vec![]);
        let convoys = shape.projects * shape.convoys_per_project;
        for i in 0..deltas {
            let c = (i * 7919) % convoys;
            let change = if i % 2 == 0 { delta(c, "active") } else { delta(c, "landed") };
            applies.push(time(&mut || sidebar.apply(2 + i as u64, PROVIDER, [change.clone()]).unwrap()));
            evaluations.push(time(&mut || {
                sidebar.snapshot_shared();
            }));
        }
        applies.sort();
        evaluations.sort();
        let mut phases: Vec<_> = crate::profile::take().into_iter().collect();
        phases.sort_by(|a, b| b.1.nanos.cmp(&a.1.nanos));
        let top: Vec<String> = phases
            .iter()
            .take(phases_shown)
            .map(|(phase, stats)| format!("{phase} {:.2}ms", stats.nanos as f64 / 1e6 / applies.len() as f64))
            .collect();
        println!(
            "{name:24} {:>7} patches {:>6} nodes | snapshot {:>9.1?} first eval {:>9.1?} | delta apply {:>8.1?} eval {:>8.1?} | per delta: {}",
            patches.len(),
            nodes,
            snapshot,
            first,
            applies[applies.len() / 2],
            evaluations[evaluations.len() / 2],
            top.join(", ")
        );
    }
}

fn count_nodes(nodes: &[Rc<PlacementNode>]) -> usize {
    nodes.iter().map(|node| 1 + count_nodes(&node.children)).sum()
}

/// What the sidebar holds, by kind: strings and their copies, and
/// string-keyed maps by shape. On the real catalog and its recorded deltas
/// (ANDAMENTO_CATALOG_FIXTURE, ANDAMENTO_CATALOG_TEMPLATE as for
/// `real_catalog_timings`), else a generated fleet (CENSUS_CONVOYS, 580).
/// `cargo test -p andamento-core census -- --ignored --nocapture`
#[test]
#[ignore]
fn census() {
    let template = std::env::var("ANDAMENTO_CATALOG_TEMPLATE").map(|path| std::fs::read_to_string(path).unwrap());
    let mut sidebar = Sidebar::new(template.as_deref().unwrap_or(CONFIG)).unwrap();
    match std::env::var("ANDAMENTO_CATALOG_FIXTURE") {
        Ok(path) => {
            let patches: Vec<MetadataPatch> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            sidebar.apply(1, PROVIDER, patches).unwrap();
            let _ = sidebar.snapshot_shared();
            let deltas = std::path::Path::new(&path).with_file_name("deltas.jsonl");
            for (i, line) in std::fs::read_to_string(deltas).unwrap_or_default().lines().enumerate() {
                let request: serde_json::Value = serde_json::from_str(line).unwrap();
                let patches: Vec<MetadataPatch> = serde_json::from_value(request["patches"].clone()).unwrap();
                sidebar.apply(10 + i as u64, PROVIDER, patches).unwrap();
                let _ = sidebar.snapshot_shared();
            }
        }
        Err(_) => {
            let convoys = std::env::var("CENSUS_CONVOYS").ok().and_then(|n| n.parse().ok()).unwrap_or(580);
            sidebar.apply(1, PROVIDER, catalog(convoys)).unwrap();
            let _ = sidebar.snapshot_shared();
            for i in 0..20 {
                sidebar.apply(2 + i as u64, PROVIDER, [delta(i * 7 % convoys, "active")]).unwrap();
                let _ = sidebar.snapshot_shared();
            }
        }
    }
    let mut census = Census::default();
    sidebar.census(&mut census);
    println!("{}", census.report());
}
