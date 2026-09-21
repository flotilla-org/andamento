//! Typed, single-owner embedding interface. See include/andamento.h for the
//! ownership contract. JSON is an optional fact decoder, not a local protocol.
#![allow(clippy::missing_safety_doc)] // The C header specifies pointer safety for every operation.
use andamento_core::{
    host::PaneObservation,
    presentation::{Content, PlacementNode, PresentationState},
    sidebar::{Action, HostEffect, Workspace},
    template_config::{TemplateConfigFieldClass, TemplateControlKind},
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, PaneTarget,
    Sidebar,
};
use std::{
    ffi::{c_char, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);
const NONE: usize = usize::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Text {
    pub data: *const u8,
    pub len: usize,
}
impl Text {
    fn borrowed(s: &str) -> Self {
        Self {
            data: s.as_ptr(),
            len: s.len(),
        }
    }
    unsafe fn read(self) -> Result<String, String> {
        Ok(std::str::from_utf8(slice(self.data, self.len)?)
            .map_err(|e| e.to_string())?
            .to_owned())
    }
}
unsafe fn slice<'a, T>(data: *const T, len: usize) -> Result<&'a [T], String> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err("null input with nonzero length".into());
    }
    if len > isize::MAX as usize / std::mem::size_of::<T>().max(1) {
        return Err("input length overflow".into());
    }
    Ok(std::slice::from_raw_parts(data, len))
}
fn string(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "\\u0000")).unwrap().into_raw()
}

pub struct Andamento {
    sidebar: Sidebar,
    poisoned: bool,
    client: u64,
    effects: Vec<HostEffect>,
}
unsafe fn run<T>(
    handle: *mut Andamento,
    error: *mut *mut c_char,
    f: impl FnOnce(&mut Andamento) -> Result<T, String>,
) -> Option<T> {
    if !error.is_null() {
        *error = ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let h = handle.as_mut().ok_or("null sidebar handle")?;
        if h.poisoned {
            return Err("sidebar handle is poisoned; recreate it".into());
        }
        f(h)
    }));
    let result = result.unwrap_or_else(|_| {
        if let Some(h) = handle.as_mut() {
            h.poisoned = true;
        }
        Err("core panicked; recreate sidebar".into())
    });
    match result {
        Ok(value) => Some(value),
        Err(message) => {
            if !error.is_null() {
                *error = string(message);
            }
            None
        }
    }
}
#[no_mangle]
pub extern "C" fn andamento_abi_version() -> u32 {
    2
}
#[no_mangle]
pub unsafe extern "C" fn andamento_create(
    config: *const u8,
    len: usize,
    error: *mut *mut c_char,
) -> *mut Andamento {
    if !error.is_null() {
        *error = ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        Sidebar::new(&Text { data: config, len }.read()?)
    }))
    .unwrap_or_else(|_| Err("core panicked while creating sidebar".into()));
    match result {
        Ok(sidebar) => Box::into_raw(Box::new(Andamento {
            sidebar,
            poisoned: false,
            client: NEXT_CLIENT.fetch_add(1, Ordering::Relaxed),
            effects: vec![],
        })),
        Err(message) => {
            if !error.is_null() {
                *error = string(message);
            }
            ptr::null_mut()
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn andamento_configure(
    h: *mut Andamento,
    config: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.configure(&config.read()?)?;
        Ok(())
    })
    .is_some() as u32
}

/// Optional decoder for one producer MetadataPatch; other decoders can use the
/// Rust typed apply interface. No snapshot or effect is serialized here.
#[cfg(feature = "json")]
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_patch_json(
    h: *mut Andamento,
    now_ms: u64,
    json: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let patch: MetadataPatch =
            serde_json::from_str(&json.read()?).map_err(|e| e.to_string())?;
        h.sidebar.apply(now_ms, [patch]);
        Ok(())
    })
    .is_some() as u32
}

/// Typed entity facts for native hosts. Deliberately covers scalar facts used
/// by the fixture, rather than duplicating the entire producer schema in C.
#[repr(C)]
pub struct Fact {
    pub key: Text,
    pub kind: u32,
    pub text: Text,
    pub integer: i64,
    pub has_ttl: u32,
    pub ttl_ms: u64,
    pub has_precedence: u32,
    pub precedence: i64,
    pub has_ordinal: u32,
    pub ordinal: i64,
}
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_entity(
    h: *mut Andamento,
    now_ms: u64,
    kind: Text,
    id: Text,
    source: Text,
    facts: *const Fact,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let mut patch = MetadataPatch {
            target: MetadataTarget::Entity(EntityRef {
                kind: kind.read()?,
                id: id.read()?,
            }),
            source_id: source.read()?,
            set: Default::default(),
            unset: vec![],
        };
        let mut keys = std::collections::BTreeSet::new();
        for f in slice(facts, count)? {
            let key = f.key.read()?;
            if !keys.insert(key.clone()) {
                return Err("duplicate fact key".into());
            }
            let value = match f.kind {
                0 => {
                    patch.unset.push(key);
                    continue;
                }
                1 => MetadataValue::Text(f.text.read()?),
                2 if f.integer == 0 || f.integer == 1 => MetadataValue::Bool(f.integer != 0),
                3 => MetadataValue::Integer(f.integer),
                _ => return Err("invalid scalar fact kind or boolean".into()),
            };
            patch.set.insert(
                key,
                MetadataValueUpdate {
                    value,
                    ttl_ms: (f.has_ttl != 0).then_some(f.ttl_ms),
                    precedence: (f.has_precedence != 0).then_some(f.precedence),
                    ordinal: (f.has_ordinal != 0).then_some(f.ordinal),
                },
            );
        }
        h.sidebar.apply(now_ms, [patch]);
        Ok(())
    })
    .is_some() as u32
}
#[no_mangle]
pub unsafe extern "C" fn andamento_tick(
    h: *mut Andamento,
    now_ms: u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.apply(now_ms, []);
        Ok(())
    })
    .is_some() as u32
}
#[repr(C)]
pub struct WorkspaceInput {
    pub id: u64,
    pub position: usize,
    pub name: Text,
    pub selected: u32,
}
#[repr(C)]
pub struct PaneInput {
    pub workspace_id: u64,
    pub pane_id: u32,
    pub kind: u32,
    pub selectable: u32,
    pub focused: u32,
    pub ordinal: i64,
}
#[no_mangle]
pub unsafe extern "C" fn andamento_observe(
    h: *mut Andamento,
    workspaces: *const WorkspaceInput,
    count: usize,
    panes: *const PaneInput,
    pane_count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let workspaces = slice(workspaces, count)?
            .iter()
            .map(|w| {
                Ok(Workspace {
                    id: w.id,
                    position: w.position,
                    name: w.name.read()?,
                    selected: w.selected != 0,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let panes = slice(panes, pane_count)?
            .iter()
            .map(|p| {
                Ok(PaneObservation {
                    workspace_id: p.workspace_id,
                    pane_id: match p.kind {
                        0 => PaneTarget::Terminal(p.pane_id),
                        1 => PaneTarget::Plugin(p.pane_id),
                        _ => return Err("invalid pane kind".into()),
                    },
                    is_selectable: p.selectable != 0,
                    is_focused: p.focused != 0,
                    ordinal: p.ordinal,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        h.sidebar.observe(workspaces, panes);
        Ok(())
    })
    .is_some() as u32
}
#[no_mangle]
pub unsafe extern "C" fn andamento_complete(
    h: *mut Andamento,
    request_id: u64,
    outcome: u32,
    workspace_id: u64,
    message: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let result = match outcome {
            0 => Ok(None),
            1 => Ok(Some(workspace_id)),
            2 => Err(message.read()?),
            _ => return Err("invalid completion outcome".into()),
        };
        h.sidebar.complete(request_id, result);
        Ok(())
    })
    .is_some() as u32
}

struct Field {
    text: String,
    class: u32,
    priority: Option<i64>,
}
struct Control {
    kind: u32,
    label: String,
    glyph: String,
    action: usize,
    value: String,
    value_kind: u32,
    checked: bool,
}
struct Node {
    parent: usize,
    section: bool,
    key: String,
    entity: EntityRef,
    label: String,
    layout: String,
    loop_key: String,
    form: String,
    state: u32,
    workspace: u64,
    selected: bool,
    openable: bool,
    collapsed: bool,
    pinned: bool,
    fields: usize,
    field_count: usize,
    details: usize,
    detail_count: usize,
    controls: usize,
    control_count: usize,
    activate: usize,
    toggle: usize,
}
pub struct AndamentoSnapshot {
    client: u64,
    generation: u64,
    nodes: Vec<Node>,
    fields: Vec<Field>,
    controls: Vec<Control>,
    actions: Vec<Action>,
    diagnostics: Vec<String>,
}
impl AndamentoSnapshot {
    fn action(&mut self, a: Action) -> usize {
        let id = self.actions.len();
        self.actions.push(a);
        id
    }
    fn content(&mut self, content: &Content) -> (usize, usize, usize, usize) {
        let start = self.fields.len();
        for f in &content.fields {
            self.fields.push(Field {
                text: f.value.clone(),
                class: match f.class {
                    TemplateConfigFieldClass::Required => 0,
                    TemplateConfigFieldClass::Optional => 1,
                    TemplateConfigFieldClass::Priority => 2,
                },
                priority: f.priority,
            });
        }
        let controls = self.controls.len();
        for c in &content.controls {
            let kind = match c.kind {
                TemplateControlKind::OpenConfig => 0,
                TemplateControlKind::DisplayVariable => 1,
                TemplateControlKind::ScrollDown => 2,
                TemplateControlKind::ScrollUp => 3,
                TemplateControlKind::InspectRoot => 4,
            };
            let action = if kind == 1 {
                c.variable
                    .as_ref()
                    .map(|name| self.action(Action::ToggleDisplayVariable { name: name.clone() }))
                    .unwrap_or(NONE)
            } else {
                NONE
            };
            self.controls.push(Control {
                kind,
                label: c.variable.clone().unwrap_or_default(),
                glyph: c.glyph.clone().unwrap_or_default(),
                action,
                value: String::new(),
                value_kind: 0,
                checked: false,
            });
        }
        if let Some(error) = &content.error {
            self.diagnostics.push(error.clone());
        }
        (
            start,
            content.fields.len(),
            controls,
            content.controls.len(),
        )
    }
    fn node(&mut self, n: &PlacementNode, parent: usize) {
        let (fields, field_count, controls, control_count) = self.content(&n.content);
        let (details, detail_count, _, _) = self.content(&n.detail);
        let activate = self.action(Action::Activate {
            entity: n.entity.clone(),
        });
        let toggle = self.action(Action::TogglePlacement { key: n.key.clone() });
        // Length framing is collision-free even when IDs contain separators.
        // Hosts compare this opaque value; its encoding is not an interface.
        let key = n
            .key
            .0
            .iter()
            .flat_map(|s| [&s.loop_name, &s.entity.kind, &s.entity.id])
            .map(|s| format!("{}:{}", s.len(), s))
            .collect();
        let (state, workspace, selected, openable) = match n.state {
            PresentationState::Catalog => (0, 0, false, false),
            PresentationState::Latent { openable } => (1, 0, false, openable),
            PresentationState::Opening => (2, 0, false, false),
            PresentationState::Live {
                workspace_id,
                selected,
            } => (3, workspace_id, selected, false),
        };
        let index = self.nodes.len();
        self.nodes.push(Node {
            parent,
            section: false,
            key,
            entity: n.entity.clone(),
            label: n.label.clone(),
            layout: n.layout.clone().unwrap_or_default(),
            loop_key: std::iter::once(&n.loop_key.region)
                .chain(
                    n.loop_key
                        .parent
                        .0
                        .iter()
                        .flat_map(|s| [&s.loop_name, &s.entity.kind, &s.entity.id]),
                )
                .chain(std::iter::once(&n.loop_key.binding))
                .map(|s| format!("{}:{}", s.len(), s))
                .collect(),
            form: n.form.clone(),
            state,
            workspace,
            selected,
            openable,
            collapsed: n.collapsed,
            pinned: false,
            fields,
            field_count,
            details,
            detail_count,
            controls,
            control_count,
            activate,
            toggle,
        });
        for child in &n.children {
            self.node(child, index);
        }
    }
}
/// Cheap revision check; no snapshot construction or fact resolution.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_is_current(
    h: *mut Andamento,
    snapshot: *const AndamentoSnapshot,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        Ok(snapshot
            .as_ref()
            .is_some_and(|s| s.client == h.client && s.generation == h.sidebar.revision()))
    })
    .unwrap_or(false) as u32
}

#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_acquire(
    h: *mut Andamento,
    error: *mut *mut c_char,
) -> *mut AndamentoSnapshot {
    run(h, error, |h| {
        let snapshot = h.sidebar.snapshot();
        let mut out = AndamentoSnapshot {
            client: h.client,
            generation: h.sidebar.revision(),
            nodes: vec![],
            fields: vec![],
            controls: vec![],
            actions: vec![],
            diagnostics: snapshot.surface.diagnostics,
        };
        for e in snapshot.errors {
            out.diagnostics
                .push(format!("{}:{}: {}", e.entity.kind, e.entity.id, e.message));
        }
        for section in snapshot.surface.sections {
            let (fields, field_count, controls, control_count) = out.content(&section.content);
            let index = out.nodes.len();
            out.nodes.push(Node {
                parent: NONE,
                section: true,
                key: section.name.clone(),
                entity: EntityRef {
                    kind: String::new(),
                    id: String::new(),
                },
                label: section.name,
                layout: String::new(),
                loop_key: String::new(),
                form: String::new(),
                state: 0,
                workspace: 0,
                selected: false,
                openable: false,
                collapsed: false,
                pinned: section.pinned,
                fields,
                field_count,
                details: 0,
                detail_count: 0,
                controls,
                control_count,
                activate: NONE,
                toggle: NONE,
            });
            for n in section.nodes {
                out.node(&n, index);
            }
        }
        for control in &mut out.controls {
            if control.kind != 1 {
                continue;
            }
            if let Some(value) = snapshot.surface.display_values.get(&control.label) {
                match value {
                    andamento_core::DisplayVariableValue::Bool(value) => {
                        control.value_kind = 1;
                        control.checked = *value;
                    }
                    andamento_core::DisplayVariableValue::Enum(value) => {
                        control.value_kind = 2;
                        control.value = value.clone();
                    }
                }
            }
            if let Some(definition) = snapshot
                .surface
                .display_variables
                .iter()
                .find(|d| d.name == control.label)
            {
                if control.glyph.is_empty() {
                    control.glyph = definition.icon.clone();
                }
                control.label = if definition.label.is_empty() {
                    definition.name.clone()
                } else {
                    definition.label.clone()
                };
            }
        }
        Ok(Box::into_raw(Box::new(out)))
    })
    .unwrap_or(ptr::null_mut())
}
#[repr(C)]
pub struct NodeView {
    pub parent: usize,
    pub is_section: u32,
    pub key: Text,
    pub entity_kind: Text,
    pub entity_id: Text,
    pub label: Text,
    pub layout: Text,
    pub form: Text,
    pub state: u32,
    pub workspace_id: u64,
    pub selected: u32,
    pub openable: u32,
    pub collapsed: u32,
    pub pinned: u32,
    pub first_field: usize,
    pub field_count: usize,
    pub first_detail: usize,
    pub detail_count: usize,
    pub first_control: usize,
    pub control_count: usize,
    pub activate: usize,
    pub toggle: usize,
}
#[repr(C)]
pub struct FieldView {
    pub text: Text,
    pub class: u32,
    pub has_priority: u32,
    pub priority: i64,
}
#[repr(C)]
pub struct ControlView {
    pub kind: u32,
    pub label: Text,
    pub glyph: Text,
    pub action: usize,
    pub value_kind: u32,
    pub checked: u32,
    pub value: Text,
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_node_count(s: *const AndamentoSnapshot) -> usize {
    s.as_ref().map_or(0, |s| s.nodes.len())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_node(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut NodeView,
) -> u32 {
    let Some(n) = s.as_ref().and_then(|s| s.nodes.get(index)) else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    *out = NodeView {
        parent: n.parent,
        is_section: n.section as u32,
        key: Text::borrowed(&n.key),
        entity_kind: Text::borrowed(&n.entity.kind),
        entity_id: Text::borrowed(&n.entity.id),
        label: Text::borrowed(&n.label),
        layout: Text::borrowed(&n.layout),
        form: Text::borrowed(&n.form),
        state: n.state,
        workspace_id: n.workspace,
        selected: n.selected as u32,
        openable: n.openable as u32,
        collapsed: n.collapsed as u32,
        pinned: n.pinned as u32,
        first_field: n.fields,
        field_count: n.field_count,
        first_detail: n.details,
        detail_count: n.detail_count,
        first_control: n.controls,
        control_count: n.control_count,
        activate: n.activate,
        toggle: n.toggle,
    };
    1
}
/// Additive ABI 2 accessor: no change to the layout of AndamentoNode.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_node_loop_key(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut Text,
) -> u32 {
    let Some(node) = s.as_ref().and_then(|s| s.nodes.get(index)) else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    *out = Text::borrowed(&node.loop_key);
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_field(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut FieldView,
) -> u32 {
    let Some(f) = s.as_ref().and_then(|s| s.fields.get(index)) else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    *out = FieldView {
        text: Text::borrowed(&f.text),
        class: f.class,
        has_priority: f.priority.is_some() as u32,
        priority: f.priority.unwrap_or_default(),
    };
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_control(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut ControlView,
) -> u32 {
    let Some(c) = s.as_ref().and_then(|s| s.controls.get(index)) else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    *out = ControlView {
        kind: c.kind,
        label: Text::borrowed(&c.label),
        glyph: Text::borrowed(&c.glyph),
        action: c.action,
        value_kind: c.value_kind,
        checked: c.checked as u32,
        value: Text::borrowed(&c.value),
    };
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_diagnostic_count(s: *const AndamentoSnapshot) -> usize {
    s.as_ref().map_or(0, |s| s.diagnostics.len())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_diagnostic(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut Text,
) -> u32 {
    let Some(message) = s.as_ref().and_then(|s| s.diagnostics.get(index)) else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    *out = Text::borrowed(message);
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_dispatch(
    h: *mut Andamento,
    s: *const AndamentoSnapshot,
    action: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let s = s.as_ref().ok_or("null snapshot")?;
        if s.client != h.client {
            return Err("snapshot belongs to another sidebar".into());
        }
        if s.generation != h.sidebar.revision() {
            return Err("stale snapshot action; acquire a new snapshot".into());
        }
        let action = s
            .actions
            .get(action)
            .ok_or("invalid action reference")?
            .clone();
        h.effects.extend(h.sidebar.dispatch(action)?);
        Ok(())
    })
    .is_some() as u32
}

pub struct AndamentoEffects {
    effects: Vec<HostEffect>,
}
#[repr(C)]
pub struct EffectView {
    pub kind: u32,
    pub request_id: u64,
    pub workspace_id: u64,
    pub entity_kind: Text,
    pub entity_id: Text,
    pub name: Text,
    pub recipe: Text,
    pub has_cwd: u32,
    pub cwd: Text,
}
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_take(
    h: *mut Andamento,
    error: *mut *mut c_char,
) -> *mut AndamentoEffects {
    run(h, error, |h| {
        Ok(Box::into_raw(Box::new(AndamentoEffects {
            effects: std::mem::take(&mut h.effects),
        })))
    })
    .unwrap_or(ptr::null_mut())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_count(e: *const AndamentoEffects) -> usize {
    e.as_ref().map_or(0, |e| e.effects.len())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_get(
    e: *const AndamentoEffects,
    index: usize,
    out: *mut EffectView,
) -> u32 {
    let Some(e) = e.as_ref().and_then(|e| e.effects.get(index)) else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    let mut v = EffectView {
        kind: 0,
        request_id: 0,
        workspace_id: 0,
        entity_kind: Text::borrowed(""),
        entity_id: Text::borrowed(""),
        name: Text::borrowed(""),
        recipe: Text::borrowed(""),
        has_cwd: 0,
        cwd: Text::borrowed(""),
    };
    match e {
        HostEffect::Focus {
            request_id,
            workspace_id,
        } => {
            v.request_id = *request_id;
            v.workspace_id = *workspace_id;
        }
        HostEffect::Materialize {
            request_id,
            entity,
            name,
            recipe,
            cwd,
        } => {
            v.kind = 1;
            v.request_id = *request_id;
            v.entity_kind = Text::borrowed(&entity.kind);
            v.entity_id = Text::borrowed(&entity.id);
            v.name = Text::borrowed(name);
            v.recipe = Text::borrowed(recipe);
            v.has_cwd = cwd.is_some() as u32;
            v.cwd = Text::borrowed(cwd.as_deref().unwrap_or_default());
        }
        HostEffect::Inspect { entity } => {
            v.kind = 2;
            v.entity_kind = Text::borrowed(&entity.kind);
            v.entity_id = Text::borrowed(&entity.id);
        }
    }
    *out = v;
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_release(s: *mut AndamentoSnapshot) {
    if !s.is_null() {
        drop(Box::from_raw(s));
    }
}
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_release(e: *mut AndamentoEffects) {
    if !e.is_null() {
        drop(Box::from_raw(e));
    }
}
#[no_mangle]
pub unsafe extern "C" fn andamento_destroy(h: *mut Andamento) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}
#[no_mangle]
pub unsafe extern "C" fn andamento_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn declared_abbreviation_reaches_c_fields_but_keeps_full_details() {
        unsafe {
            let config = include_str!("../../andamento-core/tests/fixtures/abbreviation.kdl");
            let mut error = ptr::null_mut();
            let h = andamento_create(config.as_ptr(), config.len(), &mut error);
            assert!(!h.is_null());
            let patch = r#"{"type":"metadata-patch","target":{"kind":"entity","value":{"kind":"vessel","id":"worker"}},"source_id":"test","set":{"flotilla.vessel":{"value":{"type":"text","value":"worker"}},"display.label":{"value":{"type":"text","value":"a very long worker name"}},"display.label.medium":{"value":{"type":"text","value":"worker medium"}},"display.label.short":{"value":{"type":"text","value":"w"}}},"unset":[]}"#;
            assert_eq!(
                andamento_apply_patch_json(h, 0, Text::borrowed(patch), &mut error),
                1
            );
            for (declaration, expected) in [
                ("", "worker medium"),
                ("tier=\"short\"", "w"),
                ("tier=\"full\"", "a very long worker name"),
            ] {
                let config =
                    config.replace("kind=\"vessel\"", &format!("kind=\"vessel\" {declaration}"));
                assert_eq!(
                    andamento_configure(h, Text::borrowed(&config), &mut error),
                    1
                );
                let snapshot = andamento_snapshot_acquire(h, &mut error);
                assert!(!snapshot.is_null());
                let mut node = std::mem::MaybeUninit::<NodeView>::uninit();
                assert_eq!(andamento_snapshot_node(snapshot, 1, node.as_mut_ptr()), 1);
                let node = node.assume_init();
                assert_eq!(node.label.read().unwrap(), "a very long worker name");
                let mut field = std::mem::MaybeUninit::<FieldView>::uninit();
                assert_eq!(
                    andamento_snapshot_field(snapshot, node.first_field, field.as_mut_ptr()),
                    1
                );
                assert_eq!(field.assume_init_ref().text.read().unwrap(), expected);
                assert_eq!(
                    andamento_snapshot_field(snapshot, node.first_detail, field.as_mut_ptr()),
                    1
                );
                assert_eq!(
                    field.assume_init_ref().text.read().unwrap(),
                    "a very long worker name"
                );
                andamento_snapshot_release(snapshot);
            }
            andamento_destroy(h);
            assert!(error.is_null());
        }
    }

    #[test]
    fn panic_is_contained_and_poisoned_client_keeps_existing_snapshots_alive() {
        unsafe {
            let mut error = ptr::null_mut();
            let h = andamento_create(ptr::null(), 0, &mut error);
            assert!(!h.is_null());
            let s = andamento_snapshot_acquire(h, &mut error);
            assert!(!s.is_null());
            let count = andamento_snapshot_node_count(s);
            let result: Option<()> = run(h, &mut error, |_| panic!("injected failure"));
            assert!(result.is_none());
            assert!(CStr::from_ptr(error).to_str().unwrap().contains("panicked"));
            andamento_string_free(error);
            assert_eq!(andamento_tick(h, 0, &mut error), 0);
            assert!(CStr::from_ptr(error).to_str().unwrap().contains("poisoned"));
            andamento_string_free(error);
            andamento_destroy(h);
            assert_eq!(andamento_snapshot_node_count(s), count);
            andamento_snapshot_release(s);
        }
    }

    #[test]
    fn create_rejects_null_nonempty_and_invalid_utf8() {
        unsafe {
            for (data, len) in [(ptr::null(), 1), ([255].as_ptr(), 1)] {
                let mut error = ptr::null_mut();
                assert!(andamento_create(data, len, &mut error).is_null());
                assert!(!error.is_null());
                andamento_string_free(error);
            }
        }
    }
}
