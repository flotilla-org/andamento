# Governing Andamento

This is the Andamento-specific guidance for the standing Governor. It layers
project facts over the generic [platform Governor's Charter][charter]; the
charter remains authoritative for the role, its seven verbs, settlement
discipline, and authority boundaries. [ADR 0030][adr-0030] defines the standing
convoy's orient-then-idle behavior and its restart rule: the durable record is
the memory.

## Orientation and gates

On station, inspect fleet state, open pull requests, and the ready issue queue.
Report what is active, blocked, ready, or awaiting settlement, then wait for an
operator turn before making any change.

Andamento's required CI gates are defined in `.github/workflows/ci.yml`:

- stable `cargo fmt --check` across the workspace packages;
- `cargo clippy --workspace --target wasm32-wasip1 --locked`;
- locked, library-only workspace tests on `x86_64-unknown-linux-gnu`;
- a locked, single-job release build of the workspace for `wasm32-wasip1`; and
- `scripts/check-independent-core.py`, which builds and tests the reusable
  core and frontend crates in an isolated workspace with no Zellij checkout
  and runs a real C client against the exported ABI.

The Zellij-dependent jobs use the pinned `rjwittams/zellij` revision from the
workflow and expose it as the sibling `../zellij` path required by the
workspace. When reproducing one of those gates locally, use that revision
rather than an arbitrary Zellij checkout. The independence gate deliberately
runs without one.

## Current priorities

The tracker is the live source of priority and of specific tickets; this
section records only standing direction. Ready work carries the `ready` label
and review follow-ups carry `from-review`.

1. The main development thrust is Wheelhouse's use of Andamento: the shared
   sidebar core embedded through its C interface, with native geometry owned
   by the host. Judge slices by whether they move that embedding forward.
2. Keep the core independent of its presentation surface. Zellij remains a
   supported host through the plugin adapters, but behaviour belongs in the
   surface-agnostic model and renderer, with thin adapters per surface.
3. Finish the placement pipeline and cut over. The legacy grouping layer and
   its throwaway renderer adapter stay byte-identical until the declared
   cut-over slice removes them; open template-semantics decisions are settled
   on their tickets before the slices that depend on them are dispatched.
4. Daily-driver quality (visible regressions, latent materialization
   correctness, empty-project visibility) is part of the work, not a reason to
   bypass the declared template direction.

Do not infer ordering among equally ready tickets from their issue numbers.
Ask the operator when the durable record does not establish the next slice.

## House conventions

- Issue bodies are the dispatch contract. Rewrite the body before dispatch when
  a decision, grill, or scope change makes comments authoritative.
- Prefer complete tracer-bullet slices through parser, model, renderer, and
  native snapshot harness. Keep the legacy renderer byte-identical until the
  declared cut-over slice removes it.
- Put behavior in generic templates, variables, loops, placements, and surface
  adapters. Do not add entity-kind special cases or parallel identity schemes
  to make one screenshot work.
- Preserve the controller/rail/config ownership split described in `README.md`.
  The controller owns shared state, the rail renders, and config/inspect
  surfaces explain and edit declarations.
- Preserve the core/host boundary described in
  `docs/sidebar-design/core-interface.md`. The reusable crates must not depend
  on Zellij; host adapters stay in the plugins, and the independence CI gate
  enforces this.
- Keep metadata entity-scoped and UI state placement-scoped. Producers publish
  flat facts; they do not publish rendered paths.
- This experimental repository uses hard cut-overs rather than compatibility
  aliases. Delete replaced paths in the ticket that makes the new path the
  default, after its prerequisites are green.
- Judge claims against checkout conditions and pull-request evidence. Never
  merge governor-authored work without green checks and clean review.

## Escalation

Escalate product ordering, visual tradeoffs, ambiguous template semantics,
authority expansion, destructive recovery, and merge exceptions to the human
operator (`@rjwittams`). Record project work on
[`flotilla-org/andamento`][andamento-issues]. Record platform, daemon,
placement, credential, or charter defects on
[`flotilla-org/flotilla`][flotilla-issues] and link the affected Andamento
work. Follow the charter's escalation rules whenever these instructions are
silent or conflict.

[charter]: https://github.com/flotilla-org/flotilla/blob/main/docs/charters/governor.md
[adr-0030]: https://github.com/flotilla-org/flotilla/blob/main/docs/adr/0030-the-first-standing-agent-is-a-governor-entry-point.md
[andamento-issues]: https://github.com/flotilla-org/andamento/issues
[flotilla-issues]: https://github.com/flotilla-org/flotilla/issues
