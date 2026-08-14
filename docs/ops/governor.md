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

- stable `cargo fmt --check` for the five workspace packages;
- `cargo clippy --workspace --target wasm32-wasip1 --locked`;
- library-only tests on `x86_64-unknown-linux-gnu` for
  `andamento-shared`, `andamento-controller`, `andamento-config`, and
  `andamento-rail`; and
- a locked, single-job release build of the workspace for `wasm32-wasip1`.

CI uses the pinned `rjwittams/zellij` revision from the workflow and exposes it
as the sibling `../zellij` path required by the workspace. When reproducing a
gate locally, use that revision rather than an arbitrary Zellij checkout.

## Current priorities

The tracker is the live source of priority. At the time this guidance was
written, the active direction is:

1. Carry the recent template brainstorm through the tracer-bullet placement
   pipeline: generic node-variable controls, one-section placement rendering,
   nested loops and bindings, placement-scoped identity, templated controls,
   declared abbreviation tiers, stable per-loop layout, and the reserved hover
   card. Cut over and remove the old grouping layer only after those slices
   land. See issues #63 through #71; #72 holds the unmatched-entity decision.
2. Keep the rail's core independent of its presentation surface. Zellij remains
   supported, but the direction in #51 is one surface-agnostic model and
   renderer with adapters for a Zellij plugin, a plain terminal, wheelhouse,
   and richer TUI surfaces.
3. Treat user-visible regressions (#60–#62), latent materialization correctness
   (#52), and empty-project visibility (#48) as part of daily-driver quality,
   not as reasons to bypass the declared template direction.

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
