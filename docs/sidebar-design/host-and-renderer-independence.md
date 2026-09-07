# Host and renderer independence

Agreed direction, 2026-09-07. The destination is a native Wheelhouse sidebar.
A standalone TUI is useful evidence and a possible later product, but is not
on the critical path. Product decisions below were discussed with Robert on
2026-09-07. Detailed interface mechanics remain proposals to verify in code.

## Decisions from the grill

- Shared templates govern membership, ordering, nesting, controls, and logical
  layout intent such as inline and detail. Each frontend owns measurement,
  wrapping, typography, geometry, and the presentation of detail.
- Pinning belongs provisionally to the entity and survives workspace closure.
  Pinning has seen little use; revisit after experience, and do not make its
  redesign a prerequisite for extraction.
- UI state is independent per presentation client. Replicated Zellij rails
  must retain their existing synchronization. Cross-process cooperation,
  including Flotilla's prototype attention mechanisms, is separate future work.
- Activation focuses an existing presentation where possible and otherwise
  materializes the latent entity. Preserve existing behavior with the minimum
  work; do not add an MRU rule, chooser, or uniqueness restriction now.
- Canonical entity-to-presentation mapping needs experimentation. A future
  vessel activation could select that vessel inside a convoy workspace, with
  an option to pull it out into a top-level presentation. Keep semantic entity
  activation so this remains possible; do not implement it in this extraction.
- The first native slice needs labels, status, collapse and activation.
  Wheelhouse already has previews. Integrate those afterward, with intent
  along the lines of "put a preview here if available" and frontend-specific
  handling; exact template syntax and capability mechanics remain undesigned.
- Either Wheelhouse or an HTML proof is acceptable as the second frontend.
  Working choice: use Wheelhouse directly, with HTML as a fallback if its
  build blocks progress. A separate HTML implementation is not required.
- Start with `andamento-core` in this repository. A separate Manifest
  repository is not part of this delivery.

## Evidence and existing plans

Reviewed Andamento's README, sidebar design docs, current issues and PRs,
Wheelhouse's local glossary and sessions view, and project-map's presentation
model and extraction briefs. The Wheelhouse checkout is `~/dev/ui-scratch`.

Andamento's local main began at `b74932f` and was fast-forwarded, with the
user's authorization, to GitHub main `dddc508`. Placement identity (#66,
PR #89) and loop ordering (PR #88) have merged upstream. The abbreviation
and loop-layout PRs (#90, #91) remain open, as does placement cutover #71.
Descriptions of the current implementation below account for upstream main.

[Wheelhouse #17](https://github.com/flotilla-org/wheelhouse/issues/17) already
specifies three seams: facts in, local host control out, and rendering. Its
stage tickets are [Andamento #92](https://github.com/flotilla-org/andamento/issues/92),
[#93](https://github.com/flotilla-org/andamento/issues/93), and
[Wheelhouse #20](https://github.com/flotilla-org/wheelhouse/issues/20).
The original #92 required a standalone TUI before #93 started. The ticket
updates from this discussion remove that ordering and allow overlapping
core, rendering-interface and native-consumer work.

The older project-map Manifest extraction brief remains useful for the Rust
implementation / C ABI / producer protocol split. Its GroupPath identity,
group-targeted latent model, and proposed global wire renames should not be
adopted as the current contract. The later placement design and existing
connector payloads are the starting point. Start with the agreed in-repo
`andamento-core` crate; revisit independent packaging only if useful later.

## What is coupled today

The native harness already replays facts through the controller and real
terminal renderer without a running Zellij. It is neither an interactive TUI
nor a dependency-free core: it constructs Zellij `PipeMessage` values, and the
controller imports `TabInfo` and `PaneManifest` directly.

`andamento-shared` mixes patch and template types with plugin registration,
rail sizing, terminal segment rendering, and Zellij VFS access in the template
loader. Moving that crate wholesale would retain host dependencies.

`RenderedRail` contains ANSI lines, cell-coordinate hit regions, and terminal
viewport calculations. Replacing `PaletteColor`, `Styling`, and `SizeInPixels`
with owned types would free the terminal renderer from Zellij, but would not
make that output suitable for native layout.

Wheelhouse already has workspace selection, panels, views, and previews. Its
`sessions` view currently projects cleat tags into groups itself, defaulting
to `vessel`. Reuse its hosting machinery and replace the proof projection;
do not grow a second implementation of Andamento's metadata/template logic.

## Proposed module shape

The core accepts batches of metadata patches, host observations, semantic
user actions, and explicit time. It produces a revisioned semantic snapshot
and host effects. Clock access, file watching, message delivery, and command
execution belong to adapters. Start with whole snapshots and batch processing;
incremental rendering can follow measured need.

| Module | Owns |
| --- | --- |
| Core | Patch precedence and expiry, entity resolution, placement queries and identities, template inheritance and variables, inspect provenance, activation decisions |
| Facts adapter | Zellij pipe or HTTP/UDS ingress, framing, connection lifetime, retries and delivery handling |
| Host adapter | Workspace/pane observations, stable host-local handles, executing focus/materialize effects, reporting success or failure |
| Frontend | Measurement, geometry, drawing, hit testing, hover presentation, viewport and input interpretation |

Host independence requires both observations and effects. An `open/switch`
trait alone cannot tell the core that a workspace closed, which workspace is
selected, or whether materialization failed. Preserve the existing identity
rule: activation associates the new workspace with its entity immediately,
before the recipe starts supplying facts. Correlate results with requests so
delayed observations do not cause duplicate opens or move selection back.

Keep producer facts distinct from local host observations. Flotilla supplies
facts; host activation remains local. Existing producer payloads can remain
unchanged while adapters translate host types into owned types. A neutral
internal workspace vocabulary does not require renaming every wire key now.

The frontend consumes a resolved placement tree containing entity references,
placement keys, ordered typed content, controls, action targets, effective
variables, and presentation intent. Preserve loop identity and logical layout
intent where needed for consistent grouping and ordering. Terminal-specific
glyphs, truncation, line packing, and rectangles remain in the terminal
frontend. Wheelhouse chooses native typography, geometry and detail surfaces
while preserving the logical arrangement expressed by shared templates.

Frontends return actions such as activate-entity, toggle-placement and
set-variable, addressed by stable identities. Terminal mouse coordinates and
Wheelhouse widget events resolve to those actions locally. The core's inspect
data should be usable by either frontend without copying the config plugin.

Preview support should expose availability and an opaque host reference.
Wheelhouse owns preview resources, lifetime, refresh budgets and drawing.
Absence of previews must leave labels and activation usable. No GPU textures
or terminal image handles belong in the core snapshot.

## Delivery sequence

1. **Extract the core around existing behavior.** Introduce the in-repo Rust
   crate `andamento-core`; move patch application, resolution, templates and
   placement logic behind owned input/output types. Move Zellij VFS and event conversion out.
   Run the existing plugins and replay harness through it. Prove that the
   core builds and tests without the sibling Zellij checkout or Flotilla.
   Preserve batching, expiry, activation identity and existing rail behavior.

2. **Prove semantic rendering in a thin native Wheelhouse slice.** Expose the
   core through a small C ABI: opaque instance, owned input batches, retained/released
   snapshots, actions and effect results, explicit error and lifetime rules.
   Keep calls on the host UI thread initially; queue transport input onto it.
   Render labels, status and collapse controls for one project with a live
   and a latent entity in an existing panel;
   verify local activation, closing back to latent, and independent collapse
   for duplicate appearances. Begin with replay fixtures if the live feed
   is not ready. If Wheelhouse's build blocks this proof, use a small HTML
   consumer of the same typed snapshot, then resume native integration.
   Displaying escaped ANSI output does not prove the seam. This work can
   overlap extraction once the snapshot is usable. A Rust frontend trait is
   optional plumbing; the snapshot/action contract is what Wheelhouse needs.
   Add preview integration after this path works, using Wheelhouse's existing
   preview implementation.

3. **Connect live facts and finish the migration.** Implement the
   Wheelhouse-owned HTTP/1-over-UDS contract, then test the same publisher
   stream through both hosts, including duplicate delivery and reconnect.
   Remove Wheelhouse's primitive proof-sidebar as #20 requires. Keep the
   Zellij integration supported. Build the standalone terminal event loop
   later if wanted; it should reuse the core and terminal frontend.

Placement identity is available upstream and should be the new shared
identity. Core extraction can start before all visual tickets finish.
However, the temporary placement-to-legacy renderer adapter must remain
private, and #71's removal of the legacy pipeline must happen before the
long-lived C snapshot contract is treated as stable. Do not export GroupPath
rows as that contract merely because they exist today.

## Issue changes applied

- **#92:** make core extraction and Zellij regression the required delivery;
  move standalone TUI and alternate CLI host demonstrations to optional work.
- **#93:** make the acceptance criteria about semantic snapshot/actions and
  two consumers with different geometry. Remove the dependency on a finished
  standalone TUI. Preview capability is optional and host-owned.
- **#51:** retain as the broader modes direction, with #92/#93 as execution
  tickets, rather than a competing extraction plan.
- **#71:** coordinate its deletion work with the snapshot contract. #68/#69
  already have open PRs; avoid parallel replacements for their work.
- **Wheelhouse #20:** separate an early native fixture slice from live
  transport completion so neither side has to wait for the whole other side.
- **Wheelhouse #22:** settle the HTTP resource and delivery contract early.
  It corrects #20's ambiguous “HTTP-like” wording: real HTTP/1 over a Unix
  socket is required. Coordinate with Flotilla #1857 rather than treating
  its client as the contract authority.
- **Wheelhouse #17:** record the agreed scope and overlapping delivery order;
  this supersedes its historical strict-stage ticket-index comment.

GitHub completion relationships now record Wheelhouse #20's dependencies on
Wheelhouse #19, #22 and Flotilla #1857. Its early fixture slice can proceed
before live transport completes. Andamento #71's six existing prerequisites
(#63, #66, #67, #68, #69, #70) also have GitHub relationships; the first three
are already closed. #92/#93 and the early native slice coordinate through
usable interfaces, without a whole-ticket completion cycle.

#25/#70 (inspect and hover design) can inform native presentation without
blocking core extraction. #72 (unmatched entities) and #76 (a child rendered
in its parent's line) remain design questions, not permission to invent new
default structure during the extraction.

## Deferred questions and implementation checks

Entity pins are provisional. Current code pins Zellij tab IDs; retain that
behavior during mechanical extraction if changing it would enlarge the work.
Canonical presentation mapping, nested selection and pulling a presentation
out to top level need use-driven experiments, not speculative policy now.

Cross-process presentation cooperation is outside this work. A standalone
Andamento controlling Zellij or tmux, possibly on another host, can own its
UI state while its host adapter carries commands and observations. It does
not imply shared UI state. Preserve the existing Zellij rail synchronization
and verify client scoping during extraction; do not silently broaden it.

The C ABI lifetime, threading and error details above are implementation
proposals. Confirm them against Wheelhouse while building the first consumer.
Preview declaration syntax and capability transport are follow-up design.

Updated and read back seven issue bodies: Andamento #51, #71, #92, #93;
Wheelhouse #17, #20, #22. Cross-project local docs remain unchanged; the
linked live tickets carry the updated execution scope. The core extraction and renderer-separation implementation now lives in
`andamento-core`, `andamento-terminal`, `andamento-html` and `andamento-ffi`.
See [the embedding interface](core-interface.md) for the implemented surface
and its remaining placement-cutover constraint. Wheelhouse integration is
still the next consumer task.
