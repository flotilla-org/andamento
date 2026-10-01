# Andamento

Andamento is an experimental Zellij workflow sidebar. It started as a vertical
tab list, but now prototypes a controller/rail/config split for metadata-driven
tab grouping, templated rendering, external enrichment, and workflow-aware tab
materialization.

The split prototype uses the `andamento-*` crate, plugin, artifact, and pipe
namespace throughout. The intended published home is `flotilla-org/andamento`;
this prototype does not maintain compatibility aliases for earlier scratch
names.

## Shared core and native embedding

The reusable implementation now lives in `andamento-core`, with terminal
rendering in `andamento-terminal`. Neither depends on Zellij. The plugins
retain their host adapters; `andamento-ffi` exposes a typed C embedding interface
for Wheelhouse, and `andamento-html` proves native geometry over the shared
placement snapshot. See [the embedding interface](docs/sidebar-design/core-interface.md)
for build commands, ownership rules, fixtures and the no-Zellij validation.

## Build

```sh
cargo build --manifest-path /Users/robert/dev/andamento/Cargo.toml --workspace --target wasm32-wasip1 --release
```

This produces the current controller/rail/config artifacts:

```text
target/wasm32-wasip1/release/andamento-controller.wasm
target/wasm32-wasip1/release/andamento-rail.wasm
target/wasm32-wasip1/release/andamento-config.wasm
```

## Use in Zellij

The local development layout wires the controller, rail, config plugin, and
example git watcher together:

```sh
cd /Users/robert/dev/zellij
ANDAMENTO_ROOT=/Users/robert/dev/andamento \
cargo run --profile dev-opt -- --layout /Users/robert/dev/andamento/layouts/andamento.kdl
```

Or use the scratch launcher, which defaults to the same dev binary:

```sh
/Users/robert/dev/andamento/run-andamento.sh
```

## Legacy Single-Plugin Prototype

The older `andamento` single-plugin experiment is still present in this
repository, but current Andamento work is happening in the controller/rail/config
prototype below. The legacy behavior was:

- active tab is highlighted
- click a row to switch tabs
- mouse wheel switches tabs
- the visible list stays centered on the active tab when there are more tabs than rows

## Controller/Rail Prototype

The current prototype is split across these plugins:

- `andamento-controller`: background state owner for pins, ordering, pane statuses,
  and the session-wide rail collapse/scroll snapshot
- `andamento-rail`: visible sidebar renderer, intended to appear in each tab
- `andamento-config`: in-rail settings, template diagnostics, and stats views

Rail UI synchronization deliberately uses an untargeted session broadcast. The
previous collapse path piggybacked on per-client view models addressed only to
registered renderers, so startup/registration ordering could omit an instance;
scroll position never left the originating rail at all. Rails now send actions
to the controller, consume one totally ordered collapse/scroll snapshot, and
request that current snapshot whenever a new instance starts.

## Placement templates

Placement is the only presentation pipeline. Producers publish entity facts;
section templates query them using named, nested loops. No grouping catalog,
presence classes, derived group paths, or pipeline switch remain.

The bundled `templates/flotilla-default.kdl` selects projects, places their
standing roles as pills on the project line, renders convoys and their vessels
beneath them, and collects issues in a number row. Single children stay nested.
A project's repository facts do not create extra levels. Catalog entries that
no placement loop selected appear as a count and an ID list in the existing
Inspect page. This interim diagnostic leaves #72's final destination and
intentional-hiding distinction open.

```kdl
plugin location="andamento-controller" {
    template_config_path "file:$ANDAMENTO_ROOT/templates/flotilla-default.kdl"
}
```

A surface is an ordered set of sections. Each names a root template and may
name a placement query; a section containing only fields or controls needs no
query. `pinned=true` reserves its rows outside the scrolling content.

```kdl
region "tree" root-template="tree/title" form="compact" placement="projects"
template "tree/title" {
    field "label" source="literal" value="Projects"
}
placement "projects" {
    for "project" kind="project" {
        order "display.label"
        apply-template "project/line"
    }
}
template "project/line" {
    toggle collapsed="▶" expanded="▼"
    field "label" key="display.label"
    for "convoy" kind="convoy" layout="lines" {
        match "flotilla.project" of="project"
        apply-template
    }
}
```

Queries use indexed equality predicates. `of=` refers to an enclosing loop's
binding; `apply-template` starts a fresh binding environment. Bare
`apply-template` selects `<entity.kind>/line`. Entity forms use
`<entity.kind>/compact` and `<entity.kind>/detail`, with a generic bundled
fallback for unknown kinds. The old `group-header`, `tab-title`, and
`tab-status` slots are rejected.

`layout="lines"` renders siblings vertically. `layout="inline"` puts a
complete loop instance on its parent's line when it fits; otherwise every item
moves to dedicated rows. `layout="row"` always uses dedicated rows, as the
bundled issue loop does. Widths and optional columns are resolved together for
siblings. Declared `tier="full|medium|short"` selects producer abbreviations
without trying tiers until content fits.

Templates still support `extends`, reusable `fragment`/`use`, field replacement,
`remove`, chrome, and effective-KDL inspection. User, repository, project,
fleet, and bundled layers retain their precedence. Node variables inherit down
the placement tree; Inspect renders their declared controls and setter
provenance. Collapse and variable overrides use placement keys, so two
appearances of one entity have independent state. Renaming a loop resets state
beneath that loop.

Controls are declared template content:

```kdl
template "controls" {
    control "open-config" glyph="⚙"
    control "scroll-down" glyph="▼"
    control "scroll-up" glyph="▲"
    control "inspect-root"
}
```

Display-variable controls remain available for declared variables. The old
presence-class `show-issues` gate is removed with presence classes; no new
suppression declaration replaces it. The detail card reserves four rows and
shows a hovered placement's full detail fields.

The harness replays connector JSONL through the controller and terminal
renderer. `--dump-template compact` or `detail` prints effective template KDL:

```sh
cargo run -p andamento-controller --target x86_64-unknown-linux-gnu -- \
  crates/andamento-core/tests/fixtures/default.jsonl --dump-template compact
scripts/rail-preview 24 46 64
```

To look at the rail rather than inspect one model, `scripts/rail-preview`
wraps that harness: it renders through the real controller and renderer at
whatever widths you ask for, and strips the model dump and colour so the
frames are readable side by side.

```sh
scripts/rail-preview                 # default template, 46 cols
scripts/rail-preview 30 46 80        # default template, three widths
scripts/rail-preview my-experiment.kdl 46
```

Width is worth varying deliberately: the strip layout degrades as it narrows,
and several rail defects only show up at one size. The scene is
`fixtures/rail-scene.jsonl`, generated from query-row scenario inputs through Flotilla's real Rust projection. See
[scenario regeneration](fixtures/scenarios/README.md). The old Python mirror
has been removed; its frozen output remains as a replay compatibility fixture.
Point the preview at your own capture with `RAIL_PREVIEW_SCENE=path.jsonl`, or
keep the model dump with `RAIL_PREVIEW_RAW=1`.

The independent Rust replay executable lives in `andamento-core`:

```sh
host=$(rustc -vV | sed -n 's/^host: //p')
cargo build -p andamento-core --bin andamento-replay --target "$host"
replay="target/$host/debug/andamento-replay"
# Timestamp bare connector patches arriving on stdin; flush each complete line.
"$replay" record < outgoing-patches.jsonl > capture.jsonl
# Emit bare patches at recorded wall-clock speed (suitable as TUI FACTS_COMMAND).
"$replay" play fixtures/scripted-roll.jsonl
# Emit immediately for consumers accepting ordinary connector JSONL.
"$replay" emit fixtures/rail-scene.jsonl > /tmp/rail-patches.jsonl
# Evaluate a native semantic snapshot at an exact capture offset, including TTL.
"$replay" snapshot fixtures/scripted-roll.jsonl sidebar.kdl 2000
```

The JSONL capture format is `{"offset_ms":123,"patch":{...}}`: one complete
connector metadata patch per line, elapsed **monotonic milliseconds** from
capture start. Equal offsets preserve file order; decreasing offsets and
malformed lines fail with line numbers. TTL, ordinal, precedence, source and
unsets remain untouched. Blank lines are ignored. Legacy bare patches are
accepted at offset zero. Captures have no implicit end-time: tests can advance
past the last patch to exercise expiry.

`andamento_core::replay::Replay` exposes `step` (one timestamp) and `advance_to`
against `Sidebar`; both use the real catalog clock. `andamento-replay snapshot`
provides native snapshot evaluation, and the controller render harness accepts
both capture and legacy JSONL. The standalone TUI's facts command can be
`andamento-replay play CAPTURE`. `emit` removes timing for static import; it does
not evaluate TTL, whereas `snapshot` does.

For Wheelhouse's `--sidebar_diagnostics` fixture directory, ship the capture
JSONL beside the KDL and use `andamento-replay emit CAPTURE` to produce its bare
connector input, or `play` for timed ingestion. Wheelhouse-side clock/fixture
wiring is a follow-up; this change requires no GUI checks. The independent-core
check builds the replay binary without Zellij or Flotilla dependencies.

The live connector tap belongs in Flotilla's shared `pm_connect::send_patches`
boundary (initial/diff/reassert traffic); [Flotilla #2281](https://github.com/flotilla-org/flotilla/issues/2281)
tracks that integration. `record` can timestamp a hand-captured outgoing stream
now. No real roll was available here: the checked-in scripted roll covers
hold/re-admission and attempt handover; [#105](https://github.com/flotilla-org/andamento/issues/105)
tracks adding a real recording.

Snapshots tell you a frame changed. They do not tell you whether the result
reads well, which is what most of the rail's open questions are about.

The before frame is recorded in
[`default-before.txt`](docs/sidebar-design/snapshots/default-before.txt).
The default tree and terminal frames at 24 and 64 columns are tested snapshots.
Open host workspaces keep their existing fallback navigation when no catalog
placement covers them; this does not insert unmatched catalog entities into
the rail.


## Rail Placement

The visible rail plugin reads its physical placement from its own layout config. This is separate from controller config because placement belongs to the pane instance:

```kdl
andamento-rail location="file:$ANDAMENTO_ROOT/target/wasm32-wasip1/release/andamento-rail.wasm" {
    rail_placement "left"
}
```

Supported values are `left`, `right`, `top`, and `bottom`; omitted placement defaults to `left`. The placement chooses which boundary is moved when rail widths are synchronized.

Set a pane status:

```sh
zellij pipe --name andamento-set-pane-status -- '{"type":"set-pane-status","pane_id":{"kind":"terminal","id":1},"priority":"waiting","title":"Claude waiting","detail":"Needs input","icon":null,"timestamp_ms":null}'
```

Clear it:

```sh
zellij pipe --name andamento-clear-pane-status -- '{"type":"clear-pane-status","pane_id":{"kind":"terminal","id":1}}'
```

Or use the helper:

```bash
./andamento-status.sh status --pane terminal:1 --priority waiting --title "Claude waiting" --detail "Needs input" --icon-png /Users/robert/dev/zellij/assets/logo.png
./andamento-status.sh clear --pane terminal:1
```

The helper uses `$ZELLIJ_BIN` when set, otherwise it prefers
`/Users/robert/dev/zellij/target/dev-opt/zellij` before falling back to `zellij`.

The pipe is intentionally broadcast by name so it reaches the already-running
controller instead of launching another controller instance.

## Host-independent git watcher

Install the native Rust producer (requires `git`; Wheelhouse transport also
requires `curl` with Unix-socket support):

```sh
cargo install --path crates/andamento-git-watcher --target x86_64-unknown-linux-gnu --locked
andamento-git-watcher --roots ~/dev --once
andamento-git-watcher --roots ~/dev --transport wheelhouse --socket /path/to/ingress.sock
andamento-git-watcher --transport zellij --factory-repo-manager \
  --factory-layout "$PWD/layouts/repo-manager-tab.kdl"
```

Use your native Rust target on other platforms. Repeat `--roots DIR` for multiple
containers or checkouts. Scanning descends real directories until it finds a
checkout, then includes all its linked worktrees (including those outside the
root). It does not follow directory symlinks. Observed host directories may be
combined with configured roots. No session is required for the default stdout
metadata-patch JSONL transport. Diagnostics go to stderr.

The producer publishes `repo` and `worktree` entities joined by `git.repo`, with
branch, upstream, dirty, ahead/behind, root, open state and a shell recipe. Local
repositories without an origin use their common Git directory as their parent
identity. Detached HEADs show their short commit. Missing upstream facts are
explicitly unset. Labels supply the full/medium/short abbreviation ladder;
`templates/andamento-git.kdl` and the daily-driver template place these facts in
the Git section.

Origin URL userinfo and query/fragment components are removed before publication.
Ambiguous URLs with a raw `@` in the path are omitted (using local repository
identity), since that can be part of an unencoded password; percent-encode
reserved characters in Git URLs.
Non-UTF-8 paths cannot be represented by the metadata protocol and are skipped.
Unreadable directories are skipped; a failed Git status drops that checkout until
the next refresh, letting its old facts expire instead of claiming a clean tree.

Refresh defaults to five seconds with a ten-second fact TTL (`--interval` and
`--ttl-ms`). HEAD/index/packed-refs changes trigger an earlier refresh; ordinary
working-file changes are detected by the regular refresh. Removed worktrees
expire by TTL. Failed host reads let existing facts expire instead of reporting
an empty inventory. Individual publish errors are reported while the remaining entities and adapter
reconciliation are still attempted. Cycles exceeding the TTL emit a diagnostic;
large/slow inventories should raise `--ttl-ms`. The loop retries transport
failures; `--once` exits nonzero.

Wheelhouse discovery uses `GET /v1/observed/workdirs` from wheelhouse#130,
preferring each terminal view's nonempty `live_cwd` over its saved `cwd`.
`POST /v1/metadata/patch` is unchanged. It accepts one patch per request, so
Wheelhouse transport currently starts one curl process per repo/worktree patch
per cycle; large-inventory batching is tracked in wheelhouse#136. `--socket` defaults to `WHEELHOUSE_SOCKET`.
`git.open` reports checkouts containing observed directories. Wheelhouse's paired
change binds matching open terminals to the entity and materialises latent
worktrees through `git.root` and `action.primary.recipe`.

The Zellij adapter uses `andamento-observed-identities` and
`andamento-apply-metadata-patch`; only this adapter emits pane identity patches.
`--zellij-bin` defaults to `ZELLIJ_BIN` or `zellij`; `--plugin-url` targets a
specific controller. `--factory-repo-manager` requires an existing
`--factory-layout PATH`; installed binaries do not retain a build-checkout path.
The opt-in factory preserves one `repo: owner/name` tab
per repository, dedupes against the live tab list on every refresh, and applies
durable repo identity to created or existing tabs. The daily-driver layouts
invoke the installed binary with the factory enabled. The supplied repeated tab
layout remains necessary because Zellij does not share tab chrome externally.

To view a configured-root stream through the existing standalone TUI:

```sh
andamento-tui my-tmux-session templates/andamento-git.kdl andamento-git-watcher --roots ~/dev
```

Record and replay with the existing connector harness (no separate wire format):

```sh
andamento-git-watcher --roots ~/dev --once | andamento-replay record > git.jsonl
andamento-replay snapshot git.jsonl templates/andamento-git.kdl 0
```

## Standalone TUI

`andamento-core` contains the sidebar's transport-neutral fact state and host
effects. It has no Zellij or Flotilla dependency. `andamento-rail` adapts
Zellij events and commands to that core. As a second adapter, `andamento-tui`
reads the same metadata-patch JSONL stream and controls tmux windows:

```sh
cargo run -p andamento-tui -- my-tmux-session templates/flotilla-default.kdl flotilla pm-connect ...
```

Use the arrow keys and Enter to activate a row; press `q` to quit. The TUI
observes live tmux windows, passes facts into `Sidebar`, renders the portable
surface through `andamento-terminal`, and executes the returned focus or
materialize effects through the tmux CLI.
