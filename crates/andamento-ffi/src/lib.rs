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
#[repr(C)]
pub struct WorkdirInput {
    pub workspace_id: u64,
    pub cwd: Text,
}

/// Full replacement of ephemeral directory observations; does not bind a
/// managed terminal or persist a workspace opener identity.
#[no_mangle]
pub unsafe extern "C" fn andamento_observe_workdirs(
    h: *mut Andamento,
    workdirs: *const WorkdirInput,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let workdirs = slice(workdirs, count)?
            .iter()
            .map(|w| Ok((w.workspace_id, w.cwd.read()?)))
            .collect::<Result<Vec<_>, String>>()?;
        h.sidebar.observe_workdirs(workdirs);
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
    variable: String,
    persist: bool,
}
struct Node {
    parent: usize,
    section: bool,
    default_host: String,
    order: Option<i64>,
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
    copy_url: usize,
    toggle: usize,
}
/// Immutable catalog card and existing dispatch references, owned by a detail snapshot.
struct Detail {
    card: andamento_core::detail::DetailCard,
    activate: usize,
    copy_url: usize,
}
pub struct AndamentoSnapshot {
    now_ms: u64,
    details: Vec<Detail>,
    detail_index: std::collections::BTreeMap<EntityRef, usize>,
    client: u64,
    generation: u64,
    nodes: Vec<Node>,
    fields: Vec<Field>,
    controls: Vec<Control>,
    actions: Vec<Action>,
    diagnostics: Vec<String>,
}
impl AndamentoSnapshot {
    // Exported text borrows String/Vec heap buffers, never inline Detail/Action
    // storage. Appending may move structs but must not move those buffers or
    // renumber existing actions; small inline-string representations would break it.
    fn add_detail(&mut self, card: andamento_core::detail::DetailCard, sidebar: &Sidebar) -> usize {
        let activate = self.action(Action::Activate {
            entity: card.entity.clone(),
        });
        let copy_url = if sidebar.subject_url(&card.entity).is_some() {
            self.action(Action::CopySubjectUrl {
                entity: card.entity.clone(),
            })
        } else {
            NONE
        };
        let index = self.details.len();
        self.detail_index.insert(card.entity.clone(), index);
        self.details.push(Detail {
            card,
            activate,
            copy_url,
        });
        index
    }
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
                variable: c.variable.clone().unwrap_or_default(),
                persist: false,
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
    fn node(&mut self, n: &PlacementNode, parent: usize, sidebar: &Sidebar) {
        let (fields, field_count, controls, control_count) = self.content(&n.content);
        let (details, detail_count, _, _) = self.content(&n.detail);
        let activate = self.action(Action::ActivatePlacement { key: n.key.clone() });
        let has_url = sidebar.subject_url(&n.entity).is_some();
        let copy_url = if has_url {
            self.action(Action::CopySubjectUrl {
                entity: n.entity.clone(),
            })
        } else {
            NONE
        };
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
            default_host: String::new(),
            order: None,
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
            openable: openable || has_url,
            collapsed: n.collapsed,
            pinned: false,
            fields,
            field_count,
            details,
            detail_count,
            controls,
            control_count,
            activate,
            copy_url,
            toggle,
        });
        for child in &n.children {
            self.node(child, index, sidebar);
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
    acquire_snapshot(h, error, false)
}
/// Opt in to catalog detail resolution; legacy acquisition does no detail work.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_acquire_details(
    h: *mut Andamento,
    error: *mut *mut c_char,
) -> *mut AndamentoSnapshot {
    acquire_snapshot(h, error, true)
}
unsafe fn acquire_snapshot(
    h: *mut Andamento,
    error: *mut *mut c_char,
    include_details: bool,
) -> *mut AndamentoSnapshot {
    run(h, error, |h| {
        let _phase = andamento_core::profile::span("abi-acquire");
        let snapshot = h.sidebar.snapshot();
        let _flatten = andamento_core::profile::span("abi-flatten");
        let mut out = AndamentoSnapshot {
            now_ms: h.sidebar.now_ms(),
            details: vec![],
            detail_index: Default::default(),
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
                default_host: section.default_host.unwrap_or_default(),
                order: section.order,
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
                copy_url: NONE,
                toggle: NONE,
            });
            for n in section.nodes {
                out.node(&n, index, &h.sidebar);
            }
        }
        for control in &mut out.controls {
            if control.kind != 1 {
                continue;
            }
            if let Some(value) = snapshot.surface.display_values.get(&control.variable) {
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
                .find(|d| d.name == control.variable)
            {
                control.persist = definition.persist;
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
        if include_details {
            let (now_ms, cards) = h.sidebar.detail_cards();
            out.now_ms = now_ms;
            for card in cards {
                out.add_detail(card, &h.sidebar);
            }
        }
        Ok(Box::into_raw(Box::new(out)))
    })
    .unwrap_or(ptr::null_mut())
}
/// Optional structured-detail extension to ABI 2. All output text is snapshot borrowed.
#[repr(C)]
pub struct EntityView {
    pub kind: Text,
    pub id: Text,
}
#[repr(C)]
pub struct DetailView {
    pub entity: EntityView,
    pub label: Text,
    pub field_count: usize,
    pub activate: usize,
    pub copy_url: usize,
    pub now_ms: u64,
    pub error: Text,
    pub has_workspace: u32,
    pub workspace_id: u64,
}
#[repr(C)]
pub struct DetailActionView {
    pub intent: Text,
    pub label: Text,
    pub entity: EntityView,
    pub action: usize,
}
#[repr(C)]
pub struct DetailFieldView {
    pub name: Text,
    pub section: Text,
    pub role: u32,
    pub label: Text,
    pub has_value: u32,
    pub text: Text,
    pub source_key: Text,
    pub source_id: Text,
    pub has_observation: u32,
    pub observed_at_ms: u64,
    pub has_ttl: u32,
    pub ttl_ms: u64,
    pub stale: u32,
    pub relation_count: usize,
}
#[repr(C)]
pub struct DetailRelationView {
    pub entity: EntityView,
    pub display_text: Text,
    /// Index for detail lookup, or NONE if the referenced entity is unavailable.
    pub detail: usize,
}
fn entity_view(entity: &EntityRef) -> EntityView {
    EntityView {
        kind: Text::borrowed(&entity.kind),
        id: Text::borrowed(&entity.id),
    }
}
/// Append a requested detail to a current snapshot. Previously returned text and
/// action indices remain valid until snapshot release. Each catalog identity is
/// appended at most once; missing identities consume no snapshot storage.
/// Existing details in an old snapshot remain readable; no new stale evaluation
/// is permitted. NONE with no error means the exact identity is absent.
/// Misses are cached only in the bounded revision cache, not in this snapshot.
/// The caller must exclude concurrent reads, requests and release of this snapshot.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_request(
    h: *mut Andamento,
    s: *mut AndamentoSnapshot,
    kind: Text,
    id: Text,
    error: *mut *mut c_char,
) -> usize {
    run(h, error, |h| {
        let s = s.as_mut().ok_or("null snapshot")?;
        let entity = EntityRef {
            kind: kind.read()?,
            id: id.read()?,
        };
        if s.client != h.client {
            return Err("snapshot belongs to another client".into());
        }
        if let Some(index) = s.detail_index.get(&entity) {
            return Ok(*index);
        }
        if s.generation != h.sidebar.revision() {
            return Err("stale snapshot".into());
        }
        Ok(match h.sidebar.detail_card(&entity) {
            Some(card) => s.add_detail(card, &h.sidebar),
            None => NONE,
        })
    })
    .unwrap_or(NONE)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_count(s: *const AndamentoSnapshot) -> usize {
    s.as_ref().map_or(0, |s| s.details.len())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_find(
    s: *const AndamentoSnapshot,
    kind: Text,
    id: Text,
) -> usize {
    let (Some(s), Ok(kind), Ok(id)) = (s.as_ref(), kind.read(), id.read()) else {
        return NONE;
    };
    s.detail_index
        .get(&EntityRef { kind, id })
        .copied()
        .unwrap_or(NONE)
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut DetailView,
) -> u32 {
    let (Some(s), Some(out)) = (s.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some(d) = s.details.get(index) else {
        return 0;
    };
    *out = DetailView {
        entity: entity_view(&d.card.entity),
        label: Text::borrowed(&d.card.label),
        field_count: d.card.fields.len(),
        activate: d.activate,
        copy_url: d.copy_url,
        now_ms: s.now_ms,
        error: Text::borrowed(d.card.error.as_deref().unwrap_or("")),
        has_workspace: d.card.workspace_id.is_some() as u32,
        workspace_id: d.card.workspace_id.unwrap_or(0),
    };
    1
}
/// Index 0 is the primary workspace/subject control; index 1 is copy URL.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_action(
    s: *const AndamentoSnapshot,
    detail: usize,
    index: usize,
    out: *mut DetailActionView,
) -> u32 {
    let (Some(s), Some(out)) = (s.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some(d) = s.details.get(detail) else {
        return 0;
    };
    let (intent, label, action) = match index {
        0 => (
            d.card.primary_intent.as_str(),
            d.card.primary_label.as_str(),
            d.activate,
        ),
        1 if d.copy_url != NONE => ("copy-url", "Copy URL", d.copy_url),
        _ => return 0,
    };
    *out = DetailActionView {
        intent: Text::borrowed(intent),
        label: Text::borrowed(label),
        entity: entity_view(&d.card.entity),
        action,
    };
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_field(
    s: *const AndamentoSnapshot,
    detail: usize,
    field: usize,
    out: *mut DetailFieldView,
) -> u32 {
    let (Some(s), Some(out)) = (s.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some(f) = s.details.get(detail).and_then(|d| d.card.fields.get(field)) else {
        return 0;
    };
    let role = f.role as u32;
    let ttl = f.observation.as_ref().and_then(|o| o.ttl_ms);
    let observed = f.observation.as_ref().map_or(0, |o| o.updated_at);
    *out = DetailFieldView {
        name: Text::borrowed(&f.name),
        section: Text::borrowed(&f.section),
        role,
        label: Text::borrowed(&f.label),
        has_value: f.text.is_some() as u32,
        text: Text::borrowed(f.text.as_deref().unwrap_or("")),
        source_key: Text::borrowed(&f.source_key),
        source_id: Text::borrowed(f.source_id.as_deref().unwrap_or("")),
        has_observation: f.observation.is_some() as u32,
        observed_at_ms: observed,
        has_ttl: ttl.is_some() as u32,
        ttl_ms: ttl.unwrap_or(0),
        stale: ttl.is_some_and(|ttl| s.now_ms > observed.saturating_add(ttl)) as u32,
        relation_count: f.relations.len(),
    };
    1
}
/// Returns zero for relations already on the exact-identity navigation path.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_relation(
    s: *const AndamentoSnapshot,
    detail: usize,
    field: usize,
    relation: usize,
    path: *const EntityView,
    path_count: usize,
    out: *mut DetailRelationView,
) -> u32 {
    let (Some(s), Some(out), Ok(path)) = (s.as_ref(), out.as_mut(), slice(path, path_count)) else {
        return 0;
    };
    let Some(entity) = s
        .details
        .get(detail)
        .and_then(|d| d.card.fields.get(field))
        .and_then(|f| f.relations.get(relation))
    else {
        return 0;
    };
    for item in path {
        let (Ok(kind), Ok(id)) = (item.kind.read(), item.id.read()) else {
            return 0;
        };
        if entity.kind == kind && entity.id == id {
            return 0;
        }
    }
    let target = s.detail_index.get(entity).copied();
    *out = DetailRelationView {
        entity: entity_view(entity),
        display_text: Text::borrowed(target.map_or(entity.id.as_str(), |index| {
            s.details[index].card.label.as_str()
        })),
        detail: target.unwrap_or(NONE),
    };
    1
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
/// Placement defaults for a section; strings borrow the immutable snapshot.
/// Additive ABI 2 extension, leaving NodeView unchanged.
#[repr(C)]
pub struct RegionHints {
    pub default_host: Text,
    pub has_order: u32,
    pub order: i64,
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_region_hints(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut RegionHints,
) -> u32 {
    let Some(node) = s
        .as_ref()
        .and_then(|s| s.nodes.get(index))
        .filter(|n| n.section)
    else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    *out = RegionHints {
        default_host: Text::borrowed(&node.default_host),
        has_order: node.order.is_some() as u32,
        order: node.order.unwrap_or_default(),
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
/// Additive ABI 2 accessor: declarations own persistence policy, while hosts
/// own storage. The variable name is snapshot-owned, independent of its label.
/// Undeclared variables report persist=0. Invalid inputs and non-display controls
/// return 0 without changing either output.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_control_variable(
    s: *const AndamentoSnapshot,
    index: usize,
    name: *mut Text,
    persist: *mut u32,
) -> u32 {
    let Some(c) = s.as_ref().and_then(|s| s.controls.get(index)) else {
        return 0;
    };
    if c.kind != 1 || c.variable.is_empty() || name.is_null() || persist.is_null() {
        return 0;
    }
    *name = Text::borrowed(&c.variable);
    *persist = c.persist as u32;
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
            ..
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
        HostEffect::OpenUrl { url } | HostEffect::CopyUrl { url } => {
            v.kind = if matches!(e, HostEffect::OpenUrl { .. }) {
                3
            } else {
                4
            };
            v.recipe = Text::borrowed(url);
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
/// Copy action index for this snapshot's subject row, or NONE when unavailable.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_copy_url_action(
    snapshot: *const AndamentoSnapshot,
    index: usize,
) -> usize {
    snapshot
        .as_ref()
        .and_then(|s| s.nodes.get(index))
        .map_or(NONE, |n| n.copy_url)
}

/// Additive to ABI 2: dispatch copying a subject's current forge URL.
#[no_mangle]
pub unsafe extern "C" fn andamento_copy_subject_url(
    h: *mut Andamento,
    kind: Text,
    id: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let effects = h.sidebar.dispatch(Action::CopySubjectUrl {
            entity: EntityRef {
                kind: kind.read()?,
                id: id.read()?,
            },
        })?;
        h.effects.extend(effects);
        Ok(())
    })
    .is_some() as u32
}

/// Managed-content target resolved by a materialize effect. Additive to ABI 2:
/// returns 0 for other effects and for materializations without managed content.
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_primary_target(
    e: *const AndamentoEffects,
    index: usize,
    out: *mut Text,
) -> u32 {
    let Some(HostEffect::Materialize {
        primary_target: Some(target),
        ..
    }) = e.as_ref().and_then(|e| e.effects.get(index))
    else {
        return 0;
    };
    if out.is_null() {
        return 0;
    }
    *out = Text::borrowed(target);
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

/// Owned single-slot reconciliation plan. Text borrows remain valid until release.
pub struct AndamentoContentPlan(andamento_core::managed::ContentPlan);
#[repr(C)]
pub struct ContentView {
    pub state: u32,
    pub token: u64,
    pub target: Text,
    pub command: Text,
    pub has_cwd: u32,
    pub cwd: Text,
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_plan(
    h: *mut Andamento,
    workspace: u64,
    kind: Text,
    id: Text,
    target: Text,
    command: Text,
    has_cwd: u32,
    cwd: Text,
    error: *mut *mut c_char,
) -> *mut AndamentoContentPlan {
    run(h, error, |h| {
        let entity = EntityRef {
            kind: kind.read()?,
            id: id.read()?,
        };
        let applied = andamento_core::managed::TerminalContent {
            target: target.read()?,
            command: command.read()?,
            cwd: if has_cwd != 0 {
                Some(cwd.read()?)
            } else {
                None
            },
        };
        Ok(Box::into_raw(Box::new(AndamentoContentPlan(
            h.sidebar.managed.plan(workspace, entity, applied),
        ))))
    })
    .unwrap_or(ptr::null_mut())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_get(
    plan: *const AndamentoContentPlan,
    out: *mut ContentView,
) -> u32 {
    let (Some(plan), Some(out)) = (plan.as_ref(), out.as_mut()) else {
        return 0;
    };
    use andamento_core::managed::ContentState;
    *out = ContentView {
        state: match plan.0.state {
            ContentState::Unavailable => 0,
            ContentState::Held => 1,
            ContentState::Current => 2,
            ContentState::Updating => 3,
            ContentState::Failed => 4,
        },
        token: 0,
        target: Text::borrowed(""),
        command: Text::borrowed(""),
        has_cwd: 0,
        cwd: Text::borrowed(""),
    };
    if let Some(update) = &plan.0.update {
        out.token = update.token;
        out.target = Text::borrowed(&update.content.target);
        out.command = Text::borrowed(&update.content.command);
        out.has_cwd = update.content.cwd.is_some() as u32;
        out.cwd = Text::borrowed(update.content.cwd.as_deref().unwrap_or_default());
    }
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_valid(
    h: *mut Andamento,
    workspace: u64,
    token: u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| Ok(h.sidebar.managed.valid(workspace, token))).unwrap_or(false) as u32
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_complete(
    h: *mut Andamento,
    workspace: u64,
    token: u64,
    success: u32,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        Ok(h.sidebar.managed.complete(workspace, token, success != 0))
    })
    .unwrap_or(false) as u32
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_retry(
    h: *mut Andamento,
    workspace: u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.managed.retry(workspace);
        Ok(())
    })
    .is_some() as u32
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_release(plan: *mut AndamentoContentPlan) {
    if !plan.is_null() {
        drop(Box::from_raw(plan));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    // Real native snapshots expose defaults without changing the node ABI.
    // Generator spans missing hints and both signed integer boundaries.
    #[test]
    fn region_hints_native_contract() {
        unsafe {
            // kdl 4 rejects i64::MIN when parsing its unsigned magnitude;
            // core JSON/sort tests cover MIN, and this KDL boundary is MIN+1.
            for order in [None, Some(i64::MIN + 1), Some(0), Some(i64::MAX)] {
                let config = format!("region \"test\" root-template=\"flotilla/region/tree\" default-host=\"sidebar\"{}",
                    order.map(|o| format!(" order={o}")).unwrap_or_default());
                let h = andamento_create(config.as_ptr(), config.len(), ptr::null_mut());
                assert!(!h.is_null());
                let s = andamento_snapshot_acquire(h, ptr::null_mut());
                let mut hints = std::mem::MaybeUninit::uninit();
                assert_eq!(andamento_snapshot_region_hints(s, 0, hints.as_mut_ptr()), 1);
                let hints = hints.assume_init();
                assert_eq!(hints.default_host.read().unwrap(), "sidebar");
                assert_eq!(hints.has_order, order.is_some() as u32);
                assert_eq!(hints.order, order.unwrap_or_default());
                assert_eq!(
                    andamento_snapshot_region_hints(s, usize::MAX, ptr::null_mut()),
                    0
                );
                assert_eq!(andamento_snapshot_region_hints(s, 0, ptr::null_mut()), 0);
                andamento_snapshot_release(s);
                andamento_destroy(h);
            }
            let config = "region \"test\" root-template=\"flotilla/region/tree\"";
            let h = andamento_create(config.as_ptr(), config.len(), ptr::null_mut());
            let s = andamento_snapshot_acquire(h, ptr::null_mut());
            let mut hints = std::mem::MaybeUninit::uninit();
            assert_eq!(andamento_snapshot_region_hints(s, 0, hints.as_mut_ptr()), 1);
            let hints = hints.assume_init();
            assert_eq!(hints.default_host.len, 0);
            assert_eq!(hints.has_order, 0);
            andamento_snapshot_release(s);
            andamento_destroy(h);
        }
    }

    // Host-inventory coverage synthesizes an unhinted section. Entity nodes
    // never expose region hints, even when their parent is a synthetic section.
    #[test]
    fn synthetic_section_and_nonsection_hints() {
        unsafe {
            let config = "region \"test\" root-template=\"flotilla/region/tree\"";
            let h = andamento_create(config.as_ptr(), config.len(), ptr::null_mut());
            assert!(!h.is_null());
            let workspace = WorkspaceInput {
                id: 42,
                position: 0,
                name: Text::borrowed("unplaced"),
                selected: 1,
            };
            assert_eq!(
                andamento_observe(h, &workspace, 1, ptr::null(), 0, ptr::null_mut()),
                1
            );
            let snapshot = andamento_snapshot_acquire(h, ptr::null_mut());
            let mut found_section = false;
            let mut found_entity = false;
            for i in 0..andamento_snapshot_node_count(snapshot) {
                let mut node = std::mem::MaybeUninit::uninit();
                assert_eq!(andamento_snapshot_node(snapshot, i, node.as_mut_ptr()), 1);
                let node = node.assume_init();
                let mut hints = std::mem::MaybeUninit::uninit();
                if node.is_section == 0 {
                    assert_eq!(
                        andamento_snapshot_region_hints(snapshot, i, hints.as_mut_ptr()),
                        0
                    );
                    found_entity = true;
                } else if node.key.read().unwrap() == "andamento.unplaced-workspaces" {
                    assert_eq!(
                        andamento_snapshot_region_hints(snapshot, i, hints.as_mut_ptr()),
                        1
                    );
                    let hints = hints.assume_init();
                    assert_eq!(hints.default_host.len, 0);
                    assert_eq!(hints.has_order, 0);
                    found_section = true;
                }
            }
            assert!(found_section && found_entity);
            andamento_snapshot_release(snapshot);
            andamento_destroy(h);
        }
    }

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

    unsafe fn display_control_fixture() -> (*mut Andamento, *mut AndamentoSnapshot) {
        let config = r#"version 1
        display-variable "history" type="bool" default=false label="Show finished" icon="F" persist=true
        display-variable "temporary" type="bool" default=true label="Temporary" icon="T" persist=false
        region "tree" root-template="test" form="compact" placement="tree"
        placement "tree" { for "item" kind="test"; }
        template "test" slot="compact" node-kind="entity" {
            control "display-variable" variable="history"
            control "display-variable" variable="temporary"
            control "display-variable" variable="undeclared"
            control "open-config" label="Configure"
        }
    "#;
        let mut error = ptr::null_mut();
        let h = andamento_create(config.as_ptr(), config.len(), &mut error);
        assert!(
            !h.is_null(),
            "{}",
            if error.is_null() {
                "no error".into()
            } else {
                CStr::from_ptr(error).to_string_lossy()
            }
        );
        let snapshot = andamento_snapshot_acquire(h, &mut error);
        assert!(!snapshot.is_null());
        assert!(error.is_null());
        (h, snapshot)
    }

    #[test]
    fn display_control_identity_and_persistence_are_snapshot_owned() {
        unsafe {
            let (h, snapshot) = display_control_fixture();
            let mut error = ptr::null_mut();
            let index = (*snapshot)
                .controls
                .iter()
                .position(|c| c.variable == "history")
                .unwrap();
            let mut name = Text::borrowed("sentinel");
            let mut persist = 99;
            assert_eq!(
                andamento_snapshot_control_variable(snapshot, index, &mut name, &mut persist),
                1
            );
            assert_eq!(name.read().unwrap(), "history");
            assert_eq!((&(*snapshot).controls)[index].label, "Show finished");
            assert_eq!(persist, 1);
            assert_eq!(
                andamento_dispatch(
                    h,
                    snapshot,
                    (&(*snapshot).controls)[index].action,
                    &mut error
                ),
                1
            );
            let next = andamento_snapshot_acquire(h, &mut error);
            assert!((&(*next).controls)[index].checked);
            assert!(!(&(*snapshot).controls)[index].checked);
            let ephemeral = (*snapshot)
                .controls
                .iter()
                .position(|c| c.variable == "temporary")
                .unwrap();
            assert_eq!(
                andamento_snapshot_control_variable(snapshot, ephemeral, &mut name, &mut persist),
                1
            );
            assert_eq!(persist, 0);
            let undeclared = (*snapshot)
                .controls
                .iter()
                .position(|c| c.variable == "undeclared")
                .unwrap();
            assert_eq!(
                andamento_snapshot_control_variable(snapshot, undeclared, &mut name, &mut persist),
                1
            );
            assert_eq!(name.read().unwrap(), "undeclared");
            assert_eq!(persist, 0);
            andamento_destroy(h);
            // Declaration identity borrows from the snapshot, not the live core.
            assert_eq!(name.read().unwrap(), "undeclared");
            andamento_snapshot_release(snapshot);
            andamento_snapshot_release(next);
            assert!(error.is_null());
        }
    }

    #[test]
    fn display_control_invalid_inputs_preserve_outputs() {
        unsafe {
            let (h, snapshot) = display_control_fixture();
            let index = 0;
            let mut name = Text::borrowed("sentinel");
            let mut persist = 99;

            assert_eq!(
                andamento_snapshot_control_variable(snapshot, usize::MAX, &mut name, &mut persist),
                0
            );
            assert_eq!(name.read().unwrap(), "sentinel");
            assert_eq!(persist, 99);
            assert_eq!(
                andamento_snapshot_control_variable(snapshot, index, ptr::null_mut(), &mut persist),
                0
            );
            let host_control = (*snapshot)
                .controls
                .iter()
                .position(|c| c.kind == 0)
                .unwrap();
            persist = 99;
            assert_eq!(
                andamento_snapshot_control_variable(
                    snapshot,
                    host_control,
                    &mut name,
                    &mut persist
                ),
                0
            );
            assert_eq!(name.read().unwrap(), "sentinel");
            assert_eq!(persist, 99);
            assert_eq!(
                andamento_snapshot_control_variable(ptr::null(), index, &mut name, &mut persist),
                0
            );
            assert_eq!(
                andamento_snapshot_control_variable(snapshot, index, &mut name, ptr::null_mut()),
                0
            );
            assert_eq!(name.read().unwrap(), "sentinel");
            assert_eq!(persist, 99);
            andamento_destroy(h);
            andamento_snapshot_release(snapshot);
        }
    }

    #[test]
    fn display_control_empty_identity_preserves_outputs() {
        unsafe {
            let (h, snapshot) = display_control_fixture();
            let index = 0;
            let mut name = Text::borrowed("sentinel");
            let mut persist = 99;
            // KDL rejects missing variable attributes. Exercise the defensive
            // empty-name boundary with an intentionally invalid snapshot record.
            (&mut (*snapshot).controls)[index].variable.clear();
            assert_eq!(
                andamento_snapshot_control_variable(snapshot, index, &mut name, &mut persist),
                0
            );
            assert_eq!(name.read().unwrap(), "sentinel");
            assert_eq!(persist, 99);
            andamento_destroy(h);
            andamento_snapshot_release(snapshot);
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

    // The native host receives a usable activation and a snapshot-scoped copy
    // action for subjects, with resolved URLs owned by their effect batches.
    #[cfg(feature = "json")]
    #[test]
    fn subject_rows_expose_open_and_copy_url_actions() {
        unsafe {
            let mut error = ptr::null_mut();
            let config = include_str!("../../../templates/flotilla-default.kdl");
            let h = andamento_create(config.as_ptr(), config.len(), &mut error);
            assert!(!h.is_null());
            for line in include_str!("../../../fixtures/subject-entities.jsonl").lines() {
                let frame: serde_json::Value = serde_json::from_str(line).unwrap();
                if frame["offset_ms"] != 0 {
                    continue;
                }
                let patch = serde_json::to_string(&frame["patch"]).unwrap();
                assert_eq!(
                    andamento_apply_patch_json(h, 0, Text::borrowed(&patch), &mut error),
                    1
                );
            }
            let snapshot = andamento_snapshot_acquire(h, &mut error);
            let nodes = &(*snapshot).nodes;
            let index = nodes
                .iter()
                .position(|n| n.entity.id == "github/org/b!281")
                .unwrap();
            assert!(nodes[index].openable);
            let activate = nodes[index].activate;
            let copy = andamento_snapshot_copy_url_action(snapshot, index);
            assert_ne!(copy, NONE);
            assert_eq!(
                andamento_snapshot_copy_url_action(snapshot, usize::MAX),
                NONE
            );
            for (action, kind) in [(activate, 3), (copy, 4)] {
                assert_eq!(andamento_dispatch(h, snapshot, action, &mut error), 1);
                let effects = andamento_effects_take(h, &mut error);
                assert_eq!(andamento_effects_count(effects), 1);
                let mut effect = std::mem::MaybeUninit::<EffectView>::uninit();
                assert_eq!(andamento_effects_get(effects, 0, effect.as_mut_ptr()), 1);
                let effect = effect.assume_init();
                assert_eq!(effect.kind, kind);
                assert_eq!(
                    effect.recipe.read().unwrap(),
                    "https://github.com/org/b/pull/281"
                );
                andamento_effects_release(effects);
            }
            assert_eq!(
                andamento_copy_subject_url(
                    h,
                    Text::borrowed("issue"),
                    Text::borrowed("github/org/a#115"),
                    &mut error
                ),
                1
            );
            let effects = andamento_effects_take(h, &mut error);
            assert_eq!(andamento_effects_count(effects), 1);
            andamento_effects_release(effects);
            andamento_snapshot_release(snapshot);
            andamento_destroy(h);
            assert!(error.is_null());
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
