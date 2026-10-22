// PROTOTYPE - throwaway (wheelhouse#428). Answers: does SolidJS's
// fine-grained reactivity fit Andamento's core running as wasm, and what
// does the wasm surface need? Two bindings, switchable live:
//   delta    - the shim's keyed row deltas, applied per row to a store;
//   snapshot - the core's whole semantic snapshot, flattened in JS and
//              reconciled into the same store.
// The metrics panel shows each stage's cost and how much DOM changed.
import { render } from "solid-js/web";
import { createSignal, createMemo, createRoot, For, Show, batch, onMount } from "solid-js";
import { createStore, reconcile, produce } from "solid-js/store";
import init, { Core } from "./core/andamento_web_proto";
import "./style.css";

type Row = {
  id: string; section: string; depth: number; parent: string | null; kind: string;
  label: string; layout: string | null; state: { state: string; openable?: boolean };
  collapsed: boolean; stale: boolean; has_children: boolean; text: string[];
  chips: Record<string, string>; activate: unknown; toggle: unknown;
};
type Display = { sections: { name: string; title: string; controls: string[] }[]; values: Record<string, unknown> };
type Frame = { offset_ms: number; patch: any };

const [store, setStore] = createStore<{ rows: Record<string, Row>; order: string[]; display: Display }>({
  rows: {}, order: [], display: { sections: [], values: {} },
});
const [mode, setMode] = createSignal<"delta" | "snapshot">("delta");
const [metrics, setMetrics] = createSignal<Record<string, string | number>>({});
const [log, setLog] = createSignal<string[]>([]);
const [selected, setSelected] = createSignal<string | null>(null);

let core: Core;
(window as any).__updates = [];
let mutations = 0;

// Snapshot mode: the same projection the shim does, written again in JS.
// This duplication (chip prefixes included) is what the web inherits if the
// surface stays "whole snapshot".
const PREFIXES: [string, string][] = [
  ["chip-fields:", "fields"], ["chip-medium:", "medium"], ["chip-short:", "short"],
  ["chip-icon-override:", "icon_override"], ["chip-icon:", "icon"],
  ["chip-attention:", "attention"], ["chip-status:", "status"],
];
function flatten(snapshot: any) {
  const rows: Record<string, Row> = {};
  const order: string[] = [];
  const sections: Display["sections"] = [];
  const walk = (node: any, section: string, depth: number, parent: string | null) => {
    const id = JSON.stringify(node.key);
    const chips: Record<string, string> = {};
    const text: string[] = [];
    for (const f of node.content.fields) {
      const p = PREFIXES.find(([pre]) => f.value.startsWith(pre));
      if (p) chips[p[1]] = f.value.slice(p[0].length);
      else if (f.value) text.push(f.value);
    }
    rows[id] = {
      id, section, depth, parent, kind: node.entity.kind, label: node.label, layout: node.layout,
      state: node.state, collapsed: node.collapsed, stale: !!node.stale, has_children: node.children.length > 0,
      text, chips, activate: { action: "activate-placement", key: node.key }, toggle: { action: "toggle-placement", key: node.key },
    };
    order.push(id);
    for (const c of node.children) walk(c, section, depth + 1, id);
  };
  for (const s of snapshot.surface.sections) {
    if (!s.nodes.length && s.name === "workspaces") continue;
    sections.push({ name: s.name, title: s.content.fields.find((f: any) => f.value)?.value ?? s.name,
      controls: s.content.controls.filter((c: any) => c.variable).map((c: any) => c.variable) });
    for (const n of s.nodes) walk(n, s.name, 0, null);
  }
  return { rows, order, display: { sections, values: snapshot.surface.display_values } };
}

function refresh(applyMs: number) {
  const m: Record<string, string | number> = { mode: mode(), "core apply ms": applyMs.toFixed(2) };
  mutations = 0;
  if (mode() === "delta") {
    let t = performance.now();
    const text = core.delta_json();
    m["delta_json ms"] = (performance.now() - t).toFixed(2);
    m["bytes"] = text.length;
    t = performance.now();
    const d = JSON.parse(text);
    m["parse ms"] = (performance.now() - t).toFixed(2);
    m["upserts"] = d.upserts.length;
    m["removed"] = d.removed.length;
    m["order sent"] = d.order ? "yes" : "no";
    t = performance.now();
    batch(() => {
      setStore("rows", produce((rows) => {
        for (const id of d.removed) delete rows[id];
        for (const r of d.upserts) rows[r.id] = r;
      }));
      if (d.order) setStore("order", d.order);
      if (d.display) setStore("display", d.display);
    });
    m["store+DOM ms"] = (performance.now() - t).toFixed(2);
  } else {
    let t = performance.now();
    const text = core.snapshot_json();
    m["snapshot_json ms"] = (performance.now() - t).toFixed(2);
    m["bytes"] = text.length;
    t = performance.now();
    const next = flatten(JSON.parse(text));
    m["parse+flatten ms"] = (performance.now() - t).toFixed(2);
    t = performance.now();
    batch(() => {
      setStore("rows", reconcile(next.rows, { key: "id" }));
      setStore("order", reconcile(next.order));
      setStore("display", reconcile(next.display));
    });
    m["store+DOM ms"] = (performance.now() - t).toFixed(2);
    // Keep the delta baseline current so switching modes stays correct.
    core.delta_json();
  }
  m["rows"] = store.order.length;
  m["revision"] = core.revision();
  const changed = mode() === "snapshot" || Number(m["upserts"]) > 0 || Number(m["removed"]) > 0;
  queueMicrotask(() => {
    const full = { ...m, "DOM mutations": mutations };
    (window as any).__updates.push(full);
    if (changed) setMetrics(full);
  });
}

function timed(f: () => void) {
  const t = performance.now();
  f();
  refresh(performance.now() - t);
}

function dispatch(action: unknown) {
  let response = "";
  timed(() => { response = core.dispatch(JSON.stringify(action)); });
  setLog((l) => [`${JSON.stringify(action).slice(0, 90)}\n  → ${response.slice(0, 200)}`, ...l].slice(0, 8));
}

// ---- rendering --------------------------------------------------------------

const childrenOf = createRoot(() => createMemo(() => {
  const map = new Map<string | null, string[]>();
  for (const id of store.order) {
    const p = store.rows[id]?.parent ?? null;
    if (!map.has(p)) map.set(p, []);
    map.get(p)!.push(id);
  }
  return map;
}));

function statusClass(s?: string) {
  switch (s) {
    case "active": case "working": return "active";
    case "waiting": case "needs_you": return "stalled";
    case "failed": case "error": return "bad";
    default: return "idle";
  }
}

function Chip(props: { id: string }) {
  const r = () => store.rows[props.id];
  return (
    <span class="chip" classList={{ attn: r().chips.attention === "true", latent: r().state.state === "latent" }}
      title={`${r().kind}: ${r().label}`} onClick={(e) => { e.stopPropagation(); dispatch(r().activate); }}>
      <span class={`dot ${statusClass(r().chips.status)}`} />
      <b>{r().chips.medium || r().chips.short || r().label}</b>
    </span>
  );
}

function RowView(props: { id: string }) {
  const r = () => store.rows[props.id];
  const kids = () => childrenOf().get(props.id) ?? [];
  const inline = () => kids().filter((k) => store.rows[k]?.layout === "inline");
  const block = () => kids().filter((k) => store.rows[k]?.layout !== "inline");
  return (
    <Show when={r()}>
      <div class="row" classList={{ sel: selected() === props.id, latent: r().state.state === "latent", stale: r().stale, [r().kind]: true }}
        style={{ "padding-left": `${r().depth * 14 + 4}px` }}
        onClick={() => setSelected(props.id)} onDblClick={() => dispatch(r().activate)}>
        <span class="tw" onClick={(e) => { e.stopPropagation(); if (block().length) dispatch(r().toggle); }}>
          {block().length ? (r().collapsed ? "▸" : "▾") : ""}
        </span>
        <span class={`dot ${statusClass(r().chips.status)}`} />
        <span class="nm">{r().label}</span>
        <span class="sp" />
        <span class="chips"><For each={inline()}>{(k) => <Chip id={k} />}</For></span>
        <Show when={r().chips.attention === "true"}><span class="att">!</span></Show>
      </div>
      <Show when={!r().collapsed}>
        <For each={block()}>{(k) => <RowView id={k} />}</For>
      </Show>
    </Show>
  );
}

function Sidebar() {
  return (
    <div id="sidebar">
      <For each={store.display.sections}>{(s) => (
        <div class="sec">
          <div class="sec-h">
            <span>{s.title}</span>
            <span class="ctl"><For each={s.controls}>{(c) => (
              <button classList={{ on: !!store.display.values[c] }} title={c}
                onClick={() => dispatch({ action: "toggle-display-variable", name: c })}>{c.replace(/^show-/, "")}</button>
            )}</For></span>
          </div>
          <div class="sec-b">
            <For each={(childrenOf().get(null) ?? []).filter((id) => store.rows[id]?.section === s.name)}>
              {(id) => <RowView id={id} />}
            </For>
          </div>
        </div>
      )}</For>
    </div>
  );
}

// ---- replay and stress ------------------------------------------------------

function App(props: { frames: Frame[]; config: string }) {
  const [playing, setPlaying] = createSignal(false);
  const [speed, setSpeed] = createSignal(1);
  const [cursor, setCursor] = createSignal(0);
  const [stressN, setStressN] = createSignal(0);
  const [stressRate, setStressRate] = createSignal(10);
  const [running, setRunning] = createSignal(false);
  let clock = 0;
  const template = props.frames.find((f) => f.patch.target?.value?.kind === "vessel")!.patch;
  const convoys = [...new Set(props.frames.filter((f) => f.patch.target?.value?.kind === "convoy").map((f) => f.patch.target.value.id))];
  const text = (v: string) => ({ value: { type: "text", value: v } });

  const reset = () => {
    core?.free();
    core = new Core(props.config);
    clock = 0;
    setCursor(0);
    setStore({ rows: {}, order: [], display: { sections: [], values: {} } });
    timed(() => {});
  };

  const step = (until: number) => {
    const batchLines: string[] = [];
    let i = cursor();
    while (i < props.frames.length && props.frames[i].offset_ms <= until) batchLines.push(JSON.stringify(props.frames[i++].patch));
    setCursor(i);
    clock = until;
    if (batchLines.length) timed(() => core.apply(clock, batchLines.join("\n")));
  };

  const seed = () => {
    const lines: string[] = [];
    for (let i = 0; i < stressN(); i++) {
      const id = `flotilla/stress/v${i}@lab`;
      const p = structuredClone(template);
      p.target.value.id = id;
      Object.assign(p.set, {
        "entity.id": text(id), "display.label": text(`stress-${i}`), "display.label.medium": text(`s${i}`),
        "display.label.short": text(`${i}`), "flotilla.vessel": text(id), "flotilla.convoy": text(convoys[i % convoys.length]),
        "action.primary.target": text(`vessel:${id}`), "status.state": text("working"),
      });
      lines.push(JSON.stringify(p));
    }
    if (lines.length) timed(() => core.apply(clock, lines.join("\n")));
  };

  const states = ["working", "waiting", "idle", "active"];
  const flip = () => {
    if (!stressN()) return;
    const i = Math.floor(Math.random() * stressN());
    const id = `flotilla/stress/v${i}@lab`;
    const p = { type: "metadata-patch", source_id: "flotilla-connector", target: { kind: "entity", value: { kind: "vessel", id } },
      set: { "status.state": text(states[Math.floor(Math.random() * states.length)]),
             "status.attention": { value: { type: "bool", value: Math.random() < 0.2 } } }, unset: [] };
    timed(() => core.apply(clock, JSON.stringify(p)));
  };

  let last = performance.now();
  let flipAcc = 0;
  let tickAcc = 0;
  const loop = (now: number) => {
    const dt = now - last;
    last = now;
    if (playing()) step(clock + dt * speed());
    if (running()) {
      flipAcc += dt * stressRate() / 1000;
      while (flipAcc >= 1) { flip(); flipAcc -= 1; }
    }
    tickAcc += dt;
    if (tickAcc > 1000) { tickAcc = 0; clock += 1000; timed(() => core.tick(clock)); }
    requestAnimationFrame(loop);
  };

  onMount(() => {
    new MutationObserver((ms) => { for (const m of ms) mutations += m.addedNodes.length + m.removedNodes.length + (m.type === "characterData" || m.type === "attributes" ? 1 : 0); })
      .observe(document.getElementById("sidebar")!, { subtree: true, childList: true, characterData: true, attributes: true });
    reset();
    step(Number.MAX_SAFE_INTEGER); // land the whole scene, then explore
    requestAnimationFrame(loop);
  });

  return (
    <div id="app">
      <Sidebar />
      <div id="panel">
        <h3>Browser sidebar prototype</h3>
        <p class="dim">Andamento core as wasm, fed <code>rail-scene.jsonl</code> with Wheelhouse's daily-driver config. Click selects, double-click activates, ▸ toggles.</p>
        <fieldset><legend>Binding</legend>
          <label><input type="radio" checked={mode() === "delta"} onChange={() => setMode("delta")} /> keyed row deltas</label>
          <label><input type="radio" checked={mode() === "snapshot"} onChange={() => setMode("snapshot")} /> whole snapshot + reconcile</label>
        </fieldset>
        <fieldset><legend>Replay ({cursor()}/{props.frames.length} frames, t={Math.round(clock)}ms)</legend>
          <button onClick={() => { reset(); }}>Restart</button>
          <button onClick={() => setPlaying(!playing())}>{playing() ? "Pause" : "Play"}</button>
          <button onClick={() => step(Number.MAX_SAFE_INTEGER)}>To end</button>
          <label>speed <input type="number" min="0.1" step="0.5" value={speed()} onInput={(e) => setSpeed(+e.currentTarget.value)} /></label>
        </fieldset>
        <fieldset><legend>Stress</legend>
          <label>vessels <input type="number" min="0" step="50" value={stressN()} onInput={(e) => setStressN(+e.currentTarget.value)} /></label>
          <button onClick={seed}>Add</button>
          <label>flips/s <input type="number" min="1" value={stressRate()} onInput={(e) => setStressRate(+e.currentTarget.value)} /></label>
          <button onClick={() => setRunning(!running())}>{running() ? "Stop flips" : "Start flips"}</button>
        </fieldset>
        <fieldset><legend>Last update</legend>
          <table class="m"><tbody><For each={Object.entries(metrics())}>{([k, v]) => <tr><td>{k}</td><td>{String(v)}</td></tr>}</For></tbody></table>
        </fieldset>
        <fieldset><legend>Dispatched</legend><pre>{log().join("\n")}</pre></fieldset>
        <Show when={selected()}><fieldset><legend>Selected row</legend><pre>{JSON.stringify(store.rows[selected()!], null, 1)}</pre></fieldset></Show>
      </div>
    </div>
  );
}

(async () => {
  await init();
  const [config, capture] = await Promise.all([fetch("/daily-driver.kdl").then((r) => r.text()), fetch("/rail-scene.jsonl").then((r) => r.text())]);
  const frames: Frame[] = capture.split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l));
  render(() => <App frames={frames} config={config} />, document.getElementById("root")!);
})();
