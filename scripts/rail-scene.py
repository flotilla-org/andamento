#!/usr/bin/env python3
"""Emit a connector-format patch stream describing a realistic rail scene.

Written to give the native harness something with the shape of real work in
it. The two-convoy `fixtures/flotilla-connector-patches.jsonl` is enough to
prove the harness runs; it is not enough to see whether the rail is legible.

Faithful to `flotilla-manifest/src/projection.rs`: the same fact keys, the
same entity-id shapes, the same TTL and ordinal handling. Project and repo
names come from real Flotilla manifests, so the awkward cases are the ones
that actually occur — notably that `repo_label("flotilla-org/andamento")` is
`"andamento"`, identical to its project's name.

Usage: rail-scene.py > scene.jsonl
"""

import json
import sys

TTL = 30_000
CONNECTOR = "flotilla-connector"
FLOTILLA = "flotilla"
# projection.rs gives entities with no project and no repo this ordinal, which
# is what floats unattached work to the top rather than leaving it wherever
# insertion order put it.
ARCHIPELAGO_ORDINAL = -100

out = []


def text(value):
    return {"type": "text", "value": value}


def boolean(value):
    return {"type": "bool", "value": value}


def strlist(values):
    return {"type": "string-list", "value": list(values)}


def entity(kind, ident, facts, ordinal=None):
    """Mirror Catalog::assert_entity — identity facts, then the caller's."""
    base = [
        ("entity.kind", text(kind)),
        ("entity.id", text(ident)),
        ("source", text(FLOTILLA)),
    ]
    out.append(
        {
            "type": "metadata-patch",
            "target": {"kind": "entity", "value": {"kind": kind, "id": ident}},
            "source_id": CONNECTOR,
            "set": {
                key: {
                    "value": value,
                    "ttl_ms": TTL,
                    "precedence": None,
                    "ordinal": ordinal,
                }
                for key, value in base + facts
            },
            "unset": [],
        }
    )


def repo_label(slug):
    return slug.rsplit("/", 1)[-1]


def action(target_kind, target_id, recipe):
    # projection.rs also emits vehicle="pane" for independent sessions; this
    # scene has none, so the parameter would only ever take one value.
    return [
        ("action.primary.key", text("materialize")),
        ("action.primary.label", text("Open")),
        ("action.primary.vehicle", text("workspace")),
        ("action.primary.target", text(f"{target_kind}:{target_id}")),
        ("action.primary.recipe", text(recipe)),
    ]


# Convoys are mostly single-vessel, which is what actually happens, and their
# vessels are mostly named "worker" — which is why dropping the convoy level
# does not simplify the tree, it just makes three rows indistinguishable.
PROJECTS = [
    ("andamento", "flotilla-org/andamento"),
    ("cleat", "flotilla-org/cleat"),
    ("flotilla", "flotilla-org/flotilla"),
]

# project, convoy, phase, workflow, change request, message, vessels
# vessel: name, host, work phase, wants attention, message, crew
CONVOYS = [
    (
        "andamento",
        "interactive-phase-setup",
        "implementing",
        "implement-review",
        "1456",
        None,
        [("worker", "lab", "review", True, "review requested on #1456", ["coder", "reviewer"])],
    ),
    (
        "andamento",
        "grouping-live-session",
        "implementing",
        "implement-review",
        None,
        None,
        [("worker", "lab", "working", False, "rendering placement loops", ["coder"])],
    ),
    (
        "andamento",
        "scoping-regression",
        "complete",
        "single-agent-trusted",
        "63",
        None,
        [("worker", "lab", "complete", False, None, ["coder"])],
    ),
    (
        "cleat",
        "session-recording",
        "blocked",
        "implement-review",
        None,
        "waiting on credential grant",
        [("worker", "lab", "blocked", True, "needs lab forgejo token", ["coder"])],
    ),
    (
        "flotilla",
        "manifest-extraction",
        "implementing",
        "implement-review",
        "708",
        None,
        [
            ("coder", "lab", "working", False, "extracting wire mirror", ["coder"]),
            ("reviewer", "fleet", "review", True, "second opinion on ADR 0018", ["reviewer"]),
        ],
    ),
    (
        "flotilla",
        "daemon-socket-identity",
        "implementing",
        "single-agent-trusted",
        None,
        None,
        [("worker", "lab", "working", False, "socket root mismatch", ["coder"])],
    ),
]

# Work belonging to no project and no repo. The connector stamps these with
# ARCHIPELAGO_ORDINAL, so the scene exercises ordinal-driven ordering rather
# than only the every-entity-has-a-parent case.
ARCHIPELAGO = [
    ("adhoc-triage", "worker", "lab", "working", False, "chasing a flaky test"),
]

ISSUES = [
    ("https://github.com/flotilla-org/andamento#61", "Hover does nothing on UI elements", "andamento"),
    ("https://github.com/flotilla-org/andamento#62", "Clicking an attention item opens the config panel", "andamento"),
    ("https://github.com/flotilla-org/flotilla#1456", "Publish abbreviated display label tiers", "flotilla"),
]

project_ids = {}
repo_of = {}
for name, slug in PROJECTS:
    pid = f"flotilla/{name}@fleet"
    project_ids[name] = pid
    repo_of[name] = slug
    entity(
        "project",
        pid,
        [
            ("flotilla.project", text(pid)),
            ("flotilla.project.name", text(name)),
            ("display.label", text(name)),
        ],
    )
    # Repos carry the project's facts minus its label, per projection.rs.
    entity(
        "repo",
        slug,
        [
            ("flotilla.project", text(pid)),
            ("flotilla.project.name", text(name)),
            ("vcs.repo", text(slug)),
            ("vcs.repo.name", text(repo_label(slug))),
            ("display.label", text(repo_label(slug))),
        ],
    )

for project, cname, phase, workflow, change_request, message, vessels in CONVOYS:
    pid, slug = project_ids[project], repo_of[project]
    cid = f"flotilla/{cname}@fleet"
    done = sum(1 for vessel in vessels if vessel[2] == "complete")
    attention = any(vessel[3] for vessel in vessels)
    facts = [
        ("flotilla.project", text(pid)),
        ("flotilla.project.name", text(project)),
        ("vcs.repo", text(slug)),
        ("vcs.repo.name", text(repo_label(slug))),
        ("flotilla.convoy", text(cid)),
        ("flotilla.convoy.name", text(cname)),
        ("display.label", text(cname)),
        ("flotilla.convoy.phase", text(phase)),
        ("flotilla.convoy.workflow", text(workflow)),
        ("status.state", text("attention" if attention else phase)),
    ]
    if change_request:
        facts.append(("change_request.number", text(change_request)))
    if message:
        facts.append(("flotilla.convoy.message", text(message)))
    if attention:
        facts.append(("status.attention", boolean(True)))
    if vessels:
        facts.append(("summary.text", text(f"{done}/{len(vessels)} vessels done")))
    if len(vessels) == 1:
        facts += action(
            "vessel", f"flotilla/{cname}/{vessels[0][0]}@{vessels[0][1]}", f"flotilla attach {cname}"
        )
    entity("convoy", cid, facts)

    for vname, host, work_phase, wants_attention, vmessage, crew in vessels:
        vid = f"flotilla/{cname}/{vname}@{host}"
        vfacts = [
            ("flotilla.project", text(pid)),
            ("flotilla.project.name", text(project)),
            ("vcs.repo", text(slug)),
            ("vcs.repo.name", text(repo_label(slug))),
            ("flotilla.convoy", text(cid)),
            ("flotilla.convoy.name", text(cname)),
            ("flotilla.vessel", text(vid)),
            ("flotilla.vessel.name", text(vname)),
            ("display.label", text(vname)),
            ("flotilla.work.phase", text(work_phase)),
            ("flotilla.vessel.host", text(host)),
            ("status.state", text("attention" if wants_attention else work_phase)),
            ("flotilla.crew.roles", strlist(crew)),
        ]
        if wants_attention:
            vfacts.append(("status.attention", boolean(True)))
        if vmessage:
            vfacts.append(("summary.text", text(vmessage)))
        vfacts += action("vessel", vid, f"flotilla attach {cname}/{vname}")
        entity("vessel", vid, vfacts)

for cname, vname, host, work_phase, wants_attention, vmessage in ARCHIPELAGO:
    cid = f"flotilla/{cname}@fleet"
    vid = f"flotilla/{cname}/{vname}@{host}"
    entity(
        "convoy",
        cid,
        [
            ("flotilla.convoy", text(cid)),
            ("flotilla.convoy.name", text(cname)),
            ("display.label", text(cname)),
            ("flotilla.convoy.phase", text("implementing")),
            ("flotilla.convoy.workflow", text("single-agent-trusted")),
            ("status.state", text("implementing")),
            ("summary.text", text("0/1 vessels done")),
        ]
        + action("vessel", vid, f"flotilla attach {cname}"),
        ordinal=ARCHIPELAGO_ORDINAL,
    )
    entity(
        "vessel",
        vid,
        [
            ("flotilla.convoy", text(cid)),
            ("flotilla.convoy.name", text(cname)),
            ("flotilla.vessel", text(vid)),
            ("flotilla.vessel.name", text(vname)),
            ("display.label", text(vname)),
            ("flotilla.work.phase", text(work_phase)),
            ("flotilla.vessel.host", text(host)),
            ("status.state", text(work_phase)),
            ("summary.text", text(vmessage)),
        ]
        + action("vessel", vid, f"flotilla attach {cname}/{vname}"),
        ordinal=ARCHIPELAGO_ORDINAL,
    )

for iid, title, project in ISSUES:
    slug = repo_of[project]
    entity(
        "issue",
        iid,
        [
            ("flotilla.project", text(project_ids[project])),
            ("flotilla.project.name", text(project)),
            ("vcs.repo", text(slug)),
            ("vcs.repo.name", text(repo_label(slug))),
            ("flotilla.issue", text(iid)),
            ("display.label", text(title)),
            ("status.state", text("ready")),
        ],
    )

for patch in out:
    sys.stdout.write(json.dumps(patch) + "\n")
