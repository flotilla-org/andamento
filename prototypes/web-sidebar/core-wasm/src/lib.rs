//! PROTOTYPE - throwaway. The surface the browser sidebar binds to:
//! patches in, intents in, and either the whole semantic snapshot or
//! keyed row deltas out. The delta projection here stands in for the
//! core's own generation-tagged deltas (wheelhouse#441).
use andamento_core::{
    presentation::PlacementNode,
    replay,
    sidebar::{Action, Request},
    MetadataPatch, Sidebar,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

const CHIP_PREFIXES: &[(&str, &str)] = &[
    ("chip-fields:", "fields"),
    ("chip-medium:", "medium"),
    ("chip-short:", "short"),
    ("chip-icon-override:", "icon_override"),
    ("chip-icon:", "icon"),
    ("chip-attention:", "attention"),
    ("chip-status:", "status"),
];

#[wasm_bindgen]
pub struct Core {
    sidebar: Sidebar,
    rows: HashMap<String, String>,
    order: Vec<String>,
    sections: String,
}

fn err(e: impl ToString) -> JsError {
    JsError::new(&e.to_string())
}

/// One sidebar row: the node without its children, facts or debugging text,
/// with the chip conventions native parses from field prefixes made typed.
fn row(node: &PlacementNode, section: &str, depth: usize, parent: Option<&str>, id: &str) -> Value {
    let mut chips = Map::new();
    let mut text = Vec::new();
    for field in &node.content.fields {
        match CHIP_PREFIXES.iter().find(|(p, _)| field.value.starts_with(p)) {
            Some((prefix, name)) => {
                chips.insert(name.to_string(), Value::String(field.value[prefix.len()..].to_string()));
            }
            None if !field.value.is_empty() => text.push(Value::String(field.value.clone())),
            None => {}
        }
    }
    json!({
        "id": id,
        "section": section,
        "depth": depth,
        "parent": parent,
        "kind": node.entity.kind,
        "label": node.label,
        "layout": node.layout,
        "state": node.state,
        "collapsed": node.collapsed,
        "stale": node.stale,
        "has_children": !node.children.is_empty(),
        "text": text,
        "chips": chips,
        "activate": Action::ActivatePlacement { key: node.key.clone() },
        "toggle": Action::TogglePlacement { key: node.key.clone() },
    })
}

fn walk(node: &PlacementNode, section: &str, depth: usize, parent: Option<&str>, out: &mut Vec<(String, Value)>) {
    let id = serde_json::to_string(&node.key).expect("keys serialize");
    out.push((id.clone(), row(node, section, depth, parent, &id)));
    // Collapsed nodes keep their children; the frontend hides them.
    for child in &node.children {
        walk(child, section, depth + 1, Some(&id), out);
    }
}

#[wasm_bindgen]
impl Core {
    #[wasm_bindgen(constructor)]
    pub fn new(config_kdl: &str) -> Result<Core, JsError> {
        Ok(Core { sidebar: Sidebar::new(config_kdl).map_err(err)?, rows: HashMap::new(), order: Vec::new(), sections: String::new() })
    }

    /// Apply newline-separated metadata patches (replay capture wire form).
    pub fn apply(&mut self, now_ms: f64, patches_jsonl: &str) -> Result<(), JsError> {
        let patches = patches_jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str::<MetadataPatch>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        self.sidebar.apply(now_ms as u64, replay::PROVIDER, patches).map_err(err)
    }

    pub fn tick(&mut self, now_ms: f64) -> Result<(), JsError> {
        self.sidebar.handle(Request::Tick { now_ms: now_ms as u64 }).map(|_| ()).map_err(err)
    }

    /// Dispatch an Action (JSON) and return the core's Response (JSON).
    pub fn dispatch(&mut self, action_json: &str) -> Result<String, JsError> {
        let action: Action = serde_json::from_str(action_json).map_err(err)?;
        let response = self.sidebar.handle(Request::Dispatch { action }).map_err(err)?;
        serde_json::to_string(&response).map_err(err)
    }

    pub fn set_display_variable(&mut self, request_json: &str) -> Result<(), JsError> {
        let request: Request = serde_json::from_str(request_json).map_err(err)?;
        self.sidebar.handle(request).map(|_| ()).map_err(err)
    }

    pub fn revision(&self) -> f64 {
        self.sidebar.snapshot_shared().revision as f64
    }

    /// The whole semantic snapshot, as the core serializes it today.
    pub fn snapshot_json(&self) -> Result<String, JsError> {
        serde_json::to_string(self.sidebar.snapshot_shared()).map_err(err)
    }

    /// Keyed row deltas since the last call: changed or new rows, removed
    /// ids, the full order only when it changed, and section headers when
    /// they changed.
    pub fn delta_json(&mut self) -> Result<String, JsError> {
        let snapshot = self.sidebar.snapshot_shared();
        let mut rows = Vec::new();
        let mut sections = Vec::new();
        for section in snapshot.surface.visible_sections() {
            let title = section.content.fields.iter().map(|f| f.value.as_str()).find(|v| !v.is_empty()).unwrap_or(&section.name);
            let controls: Vec<_> = section.content.controls.iter().filter_map(|c| c.variable.clone()).collect();
            sections.push(json!({ "name": section.name, "title": title, "controls": controls }));
            for node in &section.nodes {
                walk(node, &section.name, 0, None, &mut rows);
            }
        }
        let display = json!({ "sections": sections, "values": snapshot.surface.display_values });
        let display = display.to_string();
        let mut upserts = Vec::new();
        let mut next = HashMap::with_capacity(rows.len());
        let order: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
        for (id, value) in rows {
            let text = value.to_string();
            if self.rows.get(&id) != Some(&text) {
                upserts.push(value);
            }
            next.insert(id, text);
        }
        let removed: Vec<_> = self.rows.keys().filter(|id| !next.contains_key(*id)).cloned().collect();
        let order_changed = order != self.order;
        let sections_changed = display != self.sections;
        self.rows = next;
        let out = json!({
            "revision": snapshot.revision,
            "upserts": upserts,
            "removed": removed,
            "order": if order_changed { Some(&order) } else { None },
            "display": if sections_changed { serde_json::from_str::<Value>(&display).ok() } else { None },
        });
        self.order = order;
        self.sections = display;
        Ok(out.to_string())
    }
}
