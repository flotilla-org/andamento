//! Typed, single-owner embedding interface. See include/andamento.h for the
//! ownership contract. JSON is an optional fact decoder, not a local protocol.
#![allow(clippy::missing_safety_doc)] // The C header specifies pointer safety for every operation.
use andamento_core::{
    host::PaneObservation,
    presentation::{Content, PlacementNode, PresentationState},
    sidebar::{Action, HostEffect, Workspace},
    template_config::{TemplateConfigFieldClass, TemplateControlKind},
    EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, PaneTarget,
    Sidebar, WorkspaceId,
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
/// A host-supplied 128-bit Workspace ID (ABI 3): 16 opaque bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WorkspaceIdView {
    pub bytes: [u8; 16],
}
impl From<WorkspaceIdView> for WorkspaceId {
    fn from(id: WorkspaceIdView) -> Self {
        WorkspaceId::from_bytes(id.bytes)
    }
}
impl From<WorkspaceId> for WorkspaceIdView {
    fn from(id: WorkspaceId) -> Self {
        Self {
            bytes: id.to_bytes(),
        }
    }
}
/// ABI 2's u64 field for an ID: the u64 it embeds, or 0 for a wider ID, which
/// only a host using ABI 3 can have supplied and reads through ABI 3 getters.
fn narrow(id: WorkspaceId) -> u64 {
    id.as_u64().unwrap_or(0)
}
unsafe fn write_workspace(out: *mut WorkspaceIdView, id: Option<WorkspaceId>) -> u32 {
    match (out.as_mut(), id) {
        (Some(out), Some(id)) => {
            *out = id.into();
            1
        }
        _ => 0,
    }
}

/// An entity named by kind and ID alone: the sidebar gives it its default
/// provider.
unsafe fn unnamed(kind: Text, id: Text) -> Result<EntityRef, String> {
    Ok(EntityRef::new("", kind.read()?, id.read()?))
}
/// A provider (subscription ID) a host passes: nonempty UTF-8.
unsafe fn provider(provider: Text) -> Result<String, String> {
    let provider = provider.read()?;
    if provider.is_empty() {
        return Err("a provider needs a name".into());
    }
    Ok(provider)
}

#[no_mangle]
pub extern "C" fn andamento_abi_version() -> u32 {
    3
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
/// ABI 3: as andamento_apply_patch_json, from `provider` (a subscription ID),
/// which is stamped over every entity the patch names.
#[cfg(feature = "json")]
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_patch_json_from(
    h: *mut Andamento,
    now_ms: u64,
    provider: Text,
    json: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let provider = self::provider(provider)?;
        let patch: MetadataPatch =
            serde_json::from_str(&json.read()?).map_err(|e| e.to_string())?;
        h.sidebar.apply_from(now_ms, &provider, [patch])
    })
    .is_some() as u32
}
/// ABI 3: the default provider, which ABI 2 calls and patches applied without
/// a provider are stamped with, and which entities named by kind and ID
/// alone belong to. It is "local" until set.
#[no_mangle]
pub unsafe extern "C" fn andamento_set_default_provider(
    h: *mut Andamento,
    provider: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.set_default_provider(&self::provider(provider)?)
    })
    .is_some() as u32
}
/// ABI 3: remove all of a provider's facts in one call.
#[no_mangle]
pub unsafe extern "C" fn andamento_provider_retract(
    h: *mut Andamento,
    provider: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.retract_provider(&self::provider(provider)?);
        Ok(())
    })
    .is_some() as u32
}
/// ABI 3: mark a provider stale (nonzero) or fresh again (zero).
#[no_mangle]
pub unsafe extern "C" fn andamento_provider_set_stale(
    h: *mut Andamento,
    provider: Text,
    stale: u32,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar
            .set_provider_stale(&self::provider(provider)?, stale != 0);
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
        let target = MetadataTarget::Entity(unnamed(kind, id)?);
        h.sidebar
            .apply(now_ms, [scalar_patch(target, source, facts, count)?]);
        Ok(())
    })
    .is_some() as u32
}
/// ABI 3: as andamento_apply_entity, from `provider` (a subscription ID).
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_entity_from(
    h: *mut Andamento,
    now_ms: u64,
    provider: Text,
    kind: Text,
    id: Text,
    source: Text,
    facts: *const Fact,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let provider = self::provider(provider)?;
        let target = MetadataTarget::Entity(unnamed(kind, id)?);
        let patch = scalar_patch(target, source, facts, count)?;
        h.sidebar.apply_from(now_ms, &provider, [patch])
    })
    .is_some() as u32
}
/// ABI 3: the same scalar facts, set on a workspace (a `tab` target), such as
/// the `.host.kind`/`.host.id` naming its host entity.
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_workspace(
    h: *mut Andamento,
    now_ms: u64,
    workspace: WorkspaceIdView,
    source: Text,
    facts: *const Fact,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let target = MetadataTarget::Tab(workspace.into());
        h.sidebar
            .apply(now_ms, [scalar_patch(target, source, facts, count)?]);
        Ok(())
    })
    .is_some() as u32
}
/// ABI 3: as andamento_apply_workspace, from `provider` (a subscription ID).
/// An entity named by `entity.kind`/`entity.id` (or the `.host.` keys) is the
/// provider's.
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_workspace_from(
    h: *mut Andamento,
    now_ms: u64,
    provider: Text,
    workspace: WorkspaceIdView,
    source: Text,
    facts: *const Fact,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let provider = self::provider(provider)?;
        let target = MetadataTarget::Tab(workspace.into());
        let patch = scalar_patch(target, source, facts, count)?;
        h.sidebar.apply_from(now_ms, &provider, [patch])
    })
    .is_some() as u32
}
unsafe fn scalar_patch(
    target: MetadataTarget,
    source: Text,
    facts: *const Fact,
    count: usize,
) -> Result<MetadataPatch, String> {
    let mut patch = MetadataPatch {
        target,
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
    Ok(patch)
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
/// ABI 3 topology: as WorkspaceInput/PaneInput, with 128-bit IDs.
#[repr(C)]
pub struct WorkspaceInput3 {
    pub id: WorkspaceIdView,
    pub position: usize,
    pub name: Text,
    pub selected: u32,
}
#[repr(C)]
pub struct PaneInput3 {
    pub workspace_id: WorkspaceIdView,
    pub pane_id: u32,
    pub kind: u32,
    pub selectable: u32,
    pub focused: u32,
    pub ordinal: i64,
}
unsafe fn workspace(
    id: WorkspaceId,
    position: usize,
    name: Text,
    selected: u32,
) -> Result<Workspace, String> {
    Ok(Workspace {
        id,
        position,
        name: name.read()?,
        selected: selected != 0,
    })
}
fn pane(
    workspace_id: WorkspaceId,
    pane_id: u32,
    kind: u32,
    selectable: u32,
    focused: u32,
    ordinal: i64,
) -> Result<PaneObservation, String> {
    Ok(PaneObservation {
        workspace_id,
        pane_id: match kind {
            0 => PaneTarget::Terminal(pane_id),
            1 => PaneTarget::Plugin(pane_id),
            _ => return Err("invalid pane kind".into()),
        },
        is_selectable: selectable != 0,
        is_focused: focused != 0,
        ordinal,
    })
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
            .map(|w| workspace(w.id.into(), w.position, w.name, w.selected))
            .collect::<Result<Vec<_>, String>>()?;
        let panes = slice(panes, pane_count)?
            .iter()
            .map(|p| {
                let id = p.workspace_id.into();
                pane(id, p.pane_id, p.kind, p.selectable, p.focused, p.ordinal)
            })
            .collect::<Result<Vec<_>, String>>()?;
        h.sidebar.observe(workspaces, panes);
        Ok(())
    })
    .is_some() as u32
}
#[no_mangle]
pub unsafe extern "C" fn andamento_observe3(
    h: *mut Andamento,
    workspaces: *const WorkspaceInput3,
    count: usize,
    panes: *const PaneInput3,
    pane_count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let workspaces = slice(workspaces, count)?
            .iter()
            .map(|w| workspace(w.id.into(), w.position, w.name, w.selected))
            .collect::<Result<Vec<_>, String>>()?;
        let panes = slice(panes, pane_count)?
            .iter()
            .map(|p| {
                let id = p.workspace_id.into();
                pane(id, p.pane_id, p.kind, p.selectable, p.focused, p.ordinal)
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
#[repr(C)]
pub struct WorkdirInput3 {
    pub workspace_id: WorkspaceIdView,
    pub cwd: Text,
}
/// ABI 3: as andamento_observe_workdirs, with 128-bit IDs.
#[no_mangle]
pub unsafe extern "C" fn andamento_observe_workdirs3(
    h: *mut Andamento,
    workdirs: *const WorkdirInput3,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let workdirs = slice(workdirs, count)?
            .iter()
            .map(|w| Ok((w.workspace_id.into(), w.cwd.read()?)))
            .collect::<Result<Vec<_>, String>>()?;
        h.sidebar.observe_workdirs(workdirs);
        Ok(())
    })
    .is_some() as u32
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
            .map(|w| Ok((w.workspace_id.into(), w.cwd.read()?)))
            .collect::<Result<Vec<_>, String>>()?;
        h.sidebar.observe_workdirs(workdirs);
        Ok(())
    })
    .is_some() as u32
}

/// Additive ABI 2: set the host-owned order of the sibling run identified by
/// `loop_key` (text from andamento_snapshot_node_loop_key). An empty list
/// returns the run to data order. Unknown entities are kept and ignored until
/// they match; entities the list omits keep data order relative to it.
#[no_mangle]
pub unsafe extern "C" fn andamento_set_sibling_order(
    h: *mut Andamento,
    loop_key: Text,
    entities: *const EntityView,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let key = loop_key.read()?;
        let key = andamento_core::PlacementLoopKey::decode(&key).ok_or("invalid loop key")?;
        let order = slice(entities, count)?
            .iter()
            .map(|e| unnamed(e.kind, e.id))
            .collect::<Result<Vec<_>, String>>()?;
        h.sidebar.set_sibling_order(key, order);
        Ok(())
    })
    .is_some() as u32
}
/// ABI 3: an entity with its provider.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EntityView3 {
    pub provider: Text,
    pub kind: Text,
    pub id: Text,
}
impl EntityView3 {
    unsafe fn read(self) -> Result<EntityRef, String> {
        Ok(EntityRef::new(
            provider(self.provider)?,
            self.kind.read()?,
            self.id.read()?,
        ))
    }
}
/// ABI 3: as andamento_set_sibling_order, naming each entity's provider.
#[no_mangle]
pub unsafe extern "C" fn andamento_set_sibling_order3(
    h: *mut Andamento,
    loop_key: Text,
    entities: *const EntityView3,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let key = loop_key.read()?;
        let key = andamento_core::PlacementLoopKey::decode(&key).ok_or("invalid loop key")?;
        let order = slice(entities, count)?
            .iter()
            .map(|e| e.read())
            .collect::<Result<Vec<_>, String>>()?;
        h.sidebar.set_sibling_order(key, order);
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
    complete(h, request_id, outcome, workspace_id.into(), message, error)
}
/// ABI 3: as andamento_complete; MATERIALIZE supplies the 128-bit ID.
#[no_mangle]
pub unsafe extern "C" fn andamento_complete3(
    h: *mut Andamento,
    request_id: u64,
    outcome: u32,
    workspace_id: WorkspaceIdView,
    message: Text,
    error: *mut *mut c_char,
) -> u32 {
    complete(h, request_id, outcome, workspace_id.into(), message, error)
}
unsafe fn complete(
    h: *mut Andamento,
    request_id: u64,
    outcome: u32,
    workspace_id: WorkspaceId,
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

/// ABI 3: declare a workspace the host created itself (one that never went
/// through MATERIALIZE). See Sidebar::register_workspace.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_register(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.register_workspace(workspace.into());
        Ok(())
    })
    .is_some() as u32
}
/// ABI 3: the host deleted the workspace rather than keeping it.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_forget(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.forget_workspace(workspace.into());
        Ok(())
    })
    .is_some() as u32
}
/// ABI 3: 1 if registered (or materialized) and not forgotten.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_registered(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        Ok(h.sidebar
            .registered_workspaces()
            .contains(&workspace.into()))
    })
    .unwrap_or(false) as u32
}

/// ABI 3: bytes the library allocated; the caller frees them with
/// andamento_bytes_free.
#[repr(C)]
pub struct Bytes {
    pub data: *mut u8,
    pub len: usize,
}
fn owned_bytes(bytes: Vec<u8>) -> Bytes {
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    Bytes {
        data: Box::into_raw(boxed) as *mut u8,
        len,
    }
}
#[no_mangle]
pub unsafe extern "C" fn andamento_bytes_free(bytes: Bytes) {
    if !bytes.data.is_null() {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
            bytes.data, bytes.len,
        )));
    }
}
unsafe fn write_bytes(out: *mut Bytes, bytes: Vec<u8>) -> Result<(), String> {
    let out = out.as_mut().ok_or("null output")?;
    *out = owned_bytes(bytes);
    Ok(())
}
/// ABI 3: the names of the records Andamento holds, one per line:
/// `dashboard`, then `workspace/<id>` for each registered workspace.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_names(
    h: *mut Andamento,
    out: *mut Bytes,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        write_bytes(out, h.sidebar.record_names().join("\n").into_bytes())
    })
    .is_some() as u32
}
/// ABI 3: a record's generation; 0 with an error for an unknown record.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_generation(
    h: *mut Andamento,
    name: Text,
    error: *mut *mut c_char,
) -> u64 {
    run(h, error, |h| h.sidebar.record_generation(&name.read()?)).unwrap_or(0)
}
/// ABI 3: a record as KDL text (UTF-8) in its versioned envelope.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_export(
    h: *mut Andamento,
    name: Text,
    out: *mut Bytes,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        if out.is_null() {
            return Err("null output".into());
        }
        let text = h.sidebar.export_record(&name.read()?)?;
        write_bytes(out, text.into_bytes())
    })
    .is_some() as u32
}
/// ABI 3: import a record; rejected without change if it doesn't parse, has
/// another version or names another record.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_import(
    h: *mut Andamento,
    name: Text,
    kdl: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.import_record(&name.read()?, &kdl.read()?)
    })
    .is_some() as u32
}
/// ABI 3: set a declared display variable: "true"/"false" for a boolean, a
/// declared value for an enum; empty returns it to its default.
#[no_mangle]
pub unsafe extern "C" fn andamento_set_display_variable(
    h: *mut Andamento,
    name: Text,
    value: Text,
    error: *mut *mut c_char,
) -> u32 {
    use andamento_core::DisplayVariableValue;
    run(h, error, |h| {
        let name = name.read()?;
        let value = value.read()?;
        if value.is_empty() {
            return h.sidebar.set_display_variable(&name, None);
        }
        let flag = match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        // An enum may declare "true" or "false" as a value.
        match flag {
            Some(flag) => h
                .sidebar
                .set_display_variable(&name, Some(DisplayVariableValue::Bool(flag)))
                .or_else(|_| {
                    h.sidebar
                        .set_display_variable(&name, Some(DisplayVariableValue::Enum(value)))
                }),
            None => h
                .sidebar
                .set_display_variable(&name, Some(DisplayVariableValue::Enum(value))),
        }
    })
    .is_some() as u32
}
/// ABI 3: a fact of a local entity. kind is text, bool or integer as in Fact,
/// or ANDAMENTO_FACT_ENTITY (4), a single entity reference.
#[repr(C)]
pub struct LocalFact {
    pub key: Text,
    pub kind: u32,
    pub text: Text,
    pub integer: i64,
    pub entity: EntityView,
}
/// ABI 3: set a local section, group or ref, replacing all its facts.
#[no_mangle]
pub unsafe extern "C" fn andamento_local_set(
    h: *mut Andamento,
    kind: Text,
    id: Text,
    facts: *const LocalFact,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let facts = slice(facts, count)?
            .iter()
            .map(|f| {
                let entity = || unnamed(f.entity.kind, f.entity.id);
                Ok((f.key, local_fact(f.kind, f.text, f.integer, entity)?))
            })
            .collect::<Result<Vec<_>, String>>()?;
        set_local(h, kind, id, facts)
    })
    .is_some() as u32
}
/// ABI 3: as AndamentoLocalFact, with the referenced entity's provider.
#[repr(C)]
pub struct LocalFact3 {
    pub key: Text,
    pub kind: u32,
    pub text: Text,
    pub integer: i64,
    pub entity: EntityView3,
}
/// ABI 3: as andamento_local_set, where entity facts name their provider.
#[no_mangle]
pub unsafe extern "C" fn andamento_local_set3(
    h: *mut Andamento,
    kind: Text,
    id: Text,
    facts: *const LocalFact3,
    count: usize,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let facts = slice(facts, count)?
            .iter()
            .map(|f| {
                Ok((
                    f.key,
                    local_fact(f.kind, f.text, f.integer, || f.entity.read())?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        set_local(h, kind, id, facts)
    })
    .is_some() as u32
}
unsafe fn local_fact(
    kind: u32,
    text: Text,
    integer: i64,
    entity: impl FnOnce() -> Result<EntityRef, String>,
) -> Result<MetadataValue, String> {
    Ok(match kind {
        1 => MetadataValue::Text(text.read()?),
        2 if integer == 0 || integer == 1 => MetadataValue::Bool(integer != 0),
        3 => MetadataValue::Integer(integer),
        4 => MetadataValue::EntityRefs(vec![entity()?]),
        _ => return Err("invalid local fact kind or boolean".into()),
    })
}
unsafe fn set_local(
    h: &mut Andamento,
    kind: Text,
    id: Text,
    facts: Vec<(Text, MetadataValue)>,
) -> Result<(), String> {
    let entity = EntityRef::local(kind.read()?, id.read()?);
    let mut values = std::collections::BTreeMap::new();
    for (key, value) in facts {
        if values.insert(key.read()?, value).is_some() {
            return Err("duplicate fact key".into());
        }
    }
    h.sidebar.set_local(entity, values)
}
/// ABI 3: remove a local entity; removing an unknown one is harmless.
#[no_mangle]
pub unsafe extern "C" fn andamento_local_remove(
    h: *mut Andamento,
    kind: Text,
    id: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar
            .remove_local(&EntityRef::local(kind.read()?, id.read()?));
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
    workspace: Option<WorkspaceId>,
    selected: bool,
    openable: bool,
    collapsed: bool,
    pinned: bool,
    stale: bool,
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
    /// The sidebar's default provider at acquisition, for ABI 2 lookups by
    /// kind and ID.
    default_provider: String,
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
            .flat_map(|s| {
                [
                    &s.loop_name,
                    &s.entity.provider,
                    &s.entity.kind,
                    &s.entity.id,
                ]
            })
            .map(|s| format!("{}:{}", s.len(), s))
            .collect();
        let (state, workspace, selected, openable) = match n.state {
            PresentationState::Catalog => (0, None, false, false),
            PresentationState::Latent { openable } => (1, None, false, openable),
            PresentationState::Opening => (2, None, false, false),
            PresentationState::Live {
                workspace_id,
                selected,
            } => (3, Some(workspace_id), selected, false),
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
            loop_key: n.loop_key.encode(),
            form: n.form.clone(),
            state,
            workspace,
            selected,
            openable: openable || has_url,
            collapsed: n.collapsed,
            pinned: false,
            stale: n.stale,
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
            default_provider: h.sidebar.default_provider().to_owned(),
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
                entity: EntityRef::new("", "", ""),
                stale: false,
                label: section.name,
                layout: String::new(),
                loop_key: String::new(),
                form: String::new(),
                state: 0,
                workspace: None,
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
    detail_request(h, s, || unnamed(kind, id), error)
}
/// ABI 3: as andamento_snapshot_detail_request, naming the entity's provider.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_request3(
    h: *mut Andamento,
    s: *mut AndamentoSnapshot,
    entity: EntityView3,
    error: *mut *mut c_char,
) -> usize {
    detail_request(h, s, || entity.read(), error)
}
unsafe fn detail_request(
    h: *mut Andamento,
    s: *mut AndamentoSnapshot,
    entity: impl FnOnce() -> Result<EntityRef, String>,
    error: *mut *mut c_char,
) -> usize {
    run(h, error, |h| {
        let s = s.as_mut().ok_or("null snapshot")?;
        let mut entity = entity()?;
        entity.fill_provider(h.sidebar.default_provider());
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
        .get(&EntityRef::new(s.default_provider.clone(), kind, id))
        .copied()
        .unwrap_or(NONE)
}
/// ABI 3: as andamento_snapshot_detail_find, naming the entity's provider.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_find3(
    s: *const AndamentoSnapshot,
    entity: EntityView3,
) -> usize {
    let (Some(s), Ok(entity)) = (s.as_ref(), entity.read()) else {
        return NONE;
    };
    s.detail_index.get(&entity).copied().unwrap_or(NONE)
}
/// ABI 3: the provider of a detail's entity.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_provider(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut Text,
) -> u32 {
    let (Some(d), Some(out)) = (s.as_ref().and_then(|s| s.details.get(index)), out.as_mut()) else {
        return 0;
    };
    *out = Text::borrowed(&d.card.entity.provider);
    1
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
        workspace_id: d.card.workspace_id.map_or(0, narrow),
    };
    1
}
/// ABI 3: the detail's 128-bit workspace ID. Returns 0 when it has none.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_workspace(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut WorkspaceIdView,
) -> u32 {
    write_workspace(
        out,
        s.as_ref()
            .and_then(|s| s.details.get(index)?.card.workspace_id),
    )
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
        workspace_id: n.workspace.map_or(0, narrow),
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
/// ABI 3: a live node's 128-bit workspace ID. Returns 0 for other nodes.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_node_workspace(
    s: *const AndamentoSnapshot,
    index: usize,
    out: *mut WorkspaceIdView,
) -> u32 {
    write_workspace(out, s.as_ref().and_then(|s| s.nodes.get(index)?.workspace))
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

/// ABI 3: a node's entity provider, and whether the provider of the entity it
/// presents (a ref's target) is stale. Returns 0 for section nodes.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_node_provider(
    s: *const AndamentoSnapshot,
    index: usize,
    provider: *mut Text,
    stale: *mut u32,
) -> u32 {
    let Some(node) = s
        .as_ref()
        .and_then(|s| s.nodes.get(index))
        .filter(|n| !n.section)
    else {
        return 0;
    };
    if provider.is_null() {
        return 0;
    }
    *provider = Text::borrowed(&node.entity.provider);
    if let Some(stale) = stale.as_mut() {
        *stale = node.stale as u32;
    }
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
            v.workspace_id = narrow(*workspace_id);
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
/// ABI 3: the provider of a MATERIALIZE or INSPECT effect's entity. Returns 0
/// for other effects.
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_provider(
    e: *const AndamentoEffects,
    index: usize,
    out: *mut Text,
) -> u32 {
    let entity = match e.as_ref().and_then(|e| e.effects.get(index)) {
        Some(HostEffect::Materialize { entity, .. } | HostEffect::Inspect { entity }) => entity,
        _ => return 0,
    };
    let Some(out) = out.as_mut() else {
        return 0;
    };
    *out = Text::borrowed(&entity.provider);
    1
}
/// ABI 3: a FOCUS effect's 128-bit workspace ID. Returns 0 for other effects.
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_workspace(
    e: *const AndamentoEffects,
    index: usize,
    out: *mut WorkspaceIdView,
) -> u32 {
    let id = match e.as_ref().and_then(|e| e.effects.get(index)) {
        Some(HostEffect::Focus { workspace_id, .. }) => Some(*workspace_id),
        _ => None,
    };
    write_workspace(out, id)
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
            entity: unnamed(kind, id)?,
        })?;
        h.effects.extend(effects);
        Ok(())
    })
    .is_some() as u32
}
/// ABI 3: as andamento_copy_subject_url, naming the entity's provider.
#[no_mangle]
pub unsafe extern "C" fn andamento_copy_subject_url3(
    h: *mut Andamento,
    entity: EntityView3,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let effects = h.sidebar.dispatch(Action::CopySubjectUrl {
            entity: entity.read()?,
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
    let workspace = workspace.into();
    let entity = || unnamed(kind, id);
    content_plan(h, workspace, entity, target, command, has_cwd, cwd, error)
}
/// ABI 3: the andamento_content_* calls with 128-bit IDs.
#[no_mangle]
pub unsafe extern "C" fn andamento_content_plan3(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    kind: Text,
    id: Text,
    target: Text,
    command: Text,
    has_cwd: u32,
    cwd: Text,
    error: *mut *mut c_char,
) -> *mut AndamentoContentPlan {
    let workspace = workspace.into();
    let entity = || unnamed(kind, id);
    content_plan(h, workspace, entity, target, command, has_cwd, cwd, error)
}
/// ABI 3: as andamento_content_plan3, naming the entity's provider.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn andamento_content_plan_entity(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    entity: EntityView3,
    target: Text,
    command: Text,
    has_cwd: u32,
    cwd: Text,
    error: *mut *mut c_char,
) -> *mut AndamentoContentPlan {
    let workspace = workspace.into();
    let entity = || entity.read();
    content_plan(h, workspace, entity, target, command, has_cwd, cwd, error)
}
#[allow(clippy::too_many_arguments)]
unsafe fn content_plan(
    h: *mut Andamento,
    workspace: WorkspaceId,
    entity: impl FnOnce() -> Result<EntityRef, String>,
    target: Text,
    command: Text,
    has_cwd: u32,
    cwd: Text,
    error: *mut *mut c_char,
) -> *mut AndamentoContentPlan {
    run(h, error, |h| {
        let mut entity = entity()?;
        entity.fill_provider(h.sidebar.default_provider());
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
    content_valid(h, workspace.into(), token, error)
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_valid3(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    token: u64,
    error: *mut *mut c_char,
) -> u32 {
    content_valid(h, workspace.into(), token, error)
}
unsafe fn content_valid(
    h: *mut Andamento,
    workspace: WorkspaceId,
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
    content_complete(h, workspace.into(), token, success, error)
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_complete3(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    token: u64,
    success: u32,
    error: *mut *mut c_char,
) -> u32 {
    content_complete(h, workspace.into(), token, success, error)
}
unsafe fn content_complete(
    h: *mut Andamento,
    workspace: WorkspaceId,
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
    content_retry(h, workspace.into(), error)
}
#[no_mangle]
pub unsafe extern "C" fn andamento_content_retry3(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error: *mut *mut c_char,
) -> u32 {
    content_retry(h, workspace.into(), error)
}
unsafe fn content_retry(h: *mut Andamento, workspace: WorkspaceId, error: *mut *mut c_char) -> u32 {
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

// ABI 3: Slots and arrangement documents. See include/andamento.h.

const SLOT_COMMAND: u32 = 0;
const SLOT_FILE: u32 = 1;
const SLOT_URL: u32 = 2;
const SLOT_JACKSTAY: u32 = 3;
const SLOT_FACET: u32 = 4;

/// A View Spec, or a resolution's recipe (whose presentation is unset).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ViewSpecView {
    pub content: u32,
    pub entity: EntityView3,
    pub facet: Text,
    pub command: Text,
    pub argv: *const Text,
    pub argc: usize,
    pub has_cwd: u32,
    pub cwd: Text,
    pub path: Text,
    pub url: Text,
    pub launcher: Text,
    pub endpoint: Text,
    pub has_presentation: u32,
    pub presentation: Text,
}

fn empty_entity() -> EntityView3 {
    EntityView3 {
        provider: Text::borrowed(""),
        kind: Text::borrowed(""),
        id: Text::borrowed(""),
    }
}

impl ViewSpecView {
    fn empty() -> Self {
        Self {
            content: SLOT_COMMAND,
            entity: empty_entity(),
            facet: Text::borrowed(""),
            command: Text::borrowed(""),
            argv: ptr::null(),
            argc: 0,
            has_cwd: 0,
            cwd: Text::borrowed(""),
            path: Text::borrowed(""),
            url: Text::borrowed(""),
            launcher: Text::borrowed(""),
            endpoint: Text::borrowed(""),
            has_presentation: 0,
            presentation: Text::borrowed(""),
        }
    }

    unsafe fn read(&self) -> Result<andamento_core::suggested_layout::ViewSpec, String> {
        use andamento_core::suggested_layout::{CommandLine, Content, LocalRecipe, ViewSpec};
        let content = match self.content {
            SLOT_FACET => {
                let entity = &self.entity;
                Content::ProviderFacet {
                    // An empty provider is the default provider.
                    entity: EntityRef::new(
                        entity.provider.read()?,
                        entity.kind.read()?,
                        entity.id.read()?,
                    ),
                    facet: self.facet.read()?,
                }
            }
            SLOT_COMMAND => {
                let argv = slice(self.argv, self.argc)?
                    .iter()
                    .map(|arg| arg.read())
                    .collect::<Result<Vec<_>, _>>()?;
                Content::Local(LocalRecipe::Command {
                    line: if argv.is_empty() {
                        CommandLine::Shell(self.command.read()?)
                    } else {
                        CommandLine::Argv(argv)
                    },
                    cwd: if self.has_cwd != 0 {
                        Some(self.cwd.read()?)
                    } else {
                        None
                    },
                })
            }
            SLOT_FILE => Content::Local(LocalRecipe::File {
                path: self.path.read()?,
            }),
            SLOT_URL => Content::Local(LocalRecipe::Url {
                url: self.url.read()?,
            }),
            SLOT_JACKSTAY => Content::Local(LocalRecipe::Jackstay {
                launcher: self.launcher.read()?,
                endpoint: self.endpoint.read()?,
            }),
            other => return Err(format!("unknown slot content kind {other}")),
        };
        Ok(ViewSpec {
            content,
            presentation: if self.has_presentation != 0 {
                Some(self.presentation.read()?)
            } else {
                None
            },
        })
    }
}

/// Output text a ViewSpecView borrows: argv as a Text array.
struct SpecStorage {
    argv: Vec<Text>,
}

fn recipe_view(
    recipe: &andamento_core::suggested_layout::LocalRecipe,
    out: &mut ViewSpecView,
) -> SpecStorage {
    use andamento_core::suggested_layout::{CommandLine, LocalRecipe};
    let mut storage = SpecStorage { argv: Vec::new() };
    match recipe {
        LocalRecipe::Command { line, cwd } => {
            out.content = SLOT_COMMAND;
            match line {
                CommandLine::Shell(shell) => out.command = Text::borrowed(shell),
                CommandLine::Argv(argv) => {
                    storage.argv = argv.iter().map(|arg| Text::borrowed(arg)).collect();
                }
            }
            out.has_cwd = cwd.is_some() as u32;
            out.cwd = Text::borrowed(cwd.as_deref().unwrap_or_default());
        }
        LocalRecipe::File { path } => {
            out.content = SLOT_FILE;
            out.path = Text::borrowed(path);
        }
        LocalRecipe::Url { url } => {
            out.content = SLOT_URL;
            out.url = Text::borrowed(url);
        }
        LocalRecipe::Jackstay { launcher, endpoint } => {
            out.content = SLOT_JACKSTAY;
            out.launcher = Text::borrowed(launcher);
            out.endpoint = Text::borrowed(endpoint);
        }
    }
    storage
}

fn spec_view(spec: &andamento_core::suggested_layout::ViewSpec) -> (ViewSpecView, SpecStorage) {
    use andamento_core::suggested_layout::Content;
    let mut out = ViewSpecView::empty();
    let storage = match &spec.content {
        Content::ProviderFacet { entity, facet } => {
            out.content = SLOT_FACET;
            out.entity = EntityView3 {
                provider: Text::borrowed(&entity.provider),
                kind: Text::borrowed(&entity.kind),
                id: Text::borrowed(&entity.id),
            };
            out.facet = Text::borrowed(facet);
            SpecStorage { argv: Vec::new() }
        }
        Content::Local(recipe) => recipe_view(recipe, &mut out),
    };
    out.has_presentation = spec.presentation.is_some() as u32;
    out.presentation = Text::borrowed(spec.presentation.as_deref().unwrap_or_default());
    (out, storage)
}

fn rebind_code(rebind: andamento_core::suggested_layout::RebindPolicy) -> u32 {
    use andamento_core::suggested_layout::RebindPolicy;
    match rebind {
        RebindPolicy::Replace => 0,
        RebindPolicy::KeepPrevious => 1,
        RebindPolicy::Ask => 2,
    }
}

fn rebind_policy(code: u32) -> Result<andamento_core::suggested_layout::RebindPolicy, String> {
    use andamento_core::suggested_layout::RebindPolicy;
    Ok(match code {
        0 => RebindPolicy::Replace,
        1 => RebindPolicy::KeepPrevious,
        2 => RebindPolicy::Ask,
        other => return Err(format!("unknown rebind policy {other}")),
    })
}

#[repr(C)]
pub struct SlotView {
    pub key: Text,
    pub spec: ViewSpecView,
    pub rebind: u32,
    pub in_baseline: u32,
    pub detached: u32,
}

/// A workspace's slots, owned until release.
pub struct AndamentoSlots {
    slots: Vec<andamento_core::slots::SlotInfo>,
    argv: Vec<Vec<Text>>,
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slots_acquire(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error: *mut *mut c_char,
) -> *mut AndamentoSlots {
    run(h, error, |h| {
        let mut slots = Box::new(AndamentoSlots {
            slots: h.sidebar.slots(workspace.into())?,
            argv: Vec::new(),
        });
        // The argv arrays borrow the boxed slots, which never move.
        let argv = slots
            .slots
            .iter()
            .map(|slot| spec_view(&slot.spec).1.argv)
            .collect();
        slots.argv = argv;
        Ok(Box::into_raw(slots))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slots_count(slots: *const AndamentoSlots) -> usize {
    slots.as_ref().map_or(0, |slots| slots.slots.len())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slots_get(
    slots: *const AndamentoSlots,
    index: usize,
    out: *mut SlotView,
) -> u32 {
    let (Some(slots), Some(out)) = (slots.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some(slot) = slots.slots.get(index) else {
        return 0;
    };
    let (mut spec, _) = spec_view(&slot.spec);
    let argv = &slots.argv[index];
    if !argv.is_empty() {
        spec.argv = argv.as_ptr();
        spec.argc = argv.len();
    }
    *out = SlotView {
        key: Text::borrowed(&slot.key),
        spec,
        rebind: rebind_code(slot.rebind),
        in_baseline: slot.in_baseline as u32,
        detached: slot.detached as u32,
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slots_release(slots: *mut AndamentoSlots) {
    if !slots.is_null() {
        drop(Box::from_raw(slots));
    }
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_set(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    spec: *const ViewSpecView,
    rebind: u32,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let spec = spec.as_ref().ok_or("a slot needs a view spec")?.read()?;
        h.sidebar
            .set_slot(workspace.into(), &key.read()?, spec, rebind_policy(rebind)?)?;
        Ok(())
    })
    .is_some() as u32
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_remove(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.remove_slot(workspace.into(), &key.read()?)?;
        Ok(())
    })
    .is_some() as u32
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_reattach(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.reattach_slot(workspace.into(), &key.read()?)?;
        Ok(())
    })
    .is_some() as u32
}

/// One slot's owned plan.
pub struct AndamentoSlotPlan {
    plan: andamento_core::managed::SlotPlan,
    argv: Vec<Text>,
}

#[repr(C)]
pub struct SlotContentView {
    pub state: u32,
    pub token: u64,
    pub resolution: Text,
    pub has_target: u32,
    pub target: Text,
    pub recipe: ViewSpecView,
    pub rebind: u32,
    pub has_previous: u32,
    pub previous: Text,
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_plan(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    applied: Text,
    error: *mut *mut c_char,
) -> *mut AndamentoSlotPlan {
    run(h, error, |h| {
        let plan = h
            .sidebar
            .plan_slot(workspace.into(), &key.read()?, &applied.read()?)?;
        let mut plan = Box::new(AndamentoSlotPlan {
            plan,
            argv: Vec::new(),
        });
        if let Some(update) = &plan.plan.update {
            let argv = recipe_view(&update.resolution.recipe, &mut ViewSpecView::empty()).argv;
            plan.argv = argv;
        }
        Ok(Box::into_raw(plan))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_plan_get(
    plan: *const AndamentoSlotPlan,
    out: *mut SlotContentView,
) -> u32 {
    let (Some(plan), Some(out)) = (plan.as_ref(), out.as_mut()) else {
        return 0;
    };
    use andamento_core::managed::ContentState;
    let content = &plan.plan;
    *out = SlotContentView {
        state: match content.state {
            ContentState::Unavailable => 0,
            ContentState::Held => 1,
            ContentState::Current => 2,
            ContentState::Updating => 3,
            ContentState::Failed => 4,
        },
        token: 0,
        resolution: Text::borrowed(""),
        has_target: 0,
        target: Text::borrowed(""),
        recipe: ViewSpecView::empty(),
        rebind: rebind_code(content.rebind),
        has_previous: content.previous.is_some() as u32,
        previous: Text::borrowed(content.previous.as_deref().unwrap_or_default()),
    };
    if let Some(update) = &content.update {
        out.token = update.token;
        out.resolution = Text::borrowed(&update.id);
        out.has_target = update.resolution.target.is_some() as u32;
        out.target = Text::borrowed(update.resolution.target.as_deref().unwrap_or_default());
        recipe_view(&update.resolution.recipe, &mut out.recipe);
        if !plan.argv.is_empty() {
            out.recipe.argv = plan.argv.as_ptr();
            out.recipe.argc = plan.argv.len();
        }
    }
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_plan_release(plan: *mut AndamentoSlotPlan) {
    if !plan.is_null() {
        drop(Box::from_raw(plan));
    }
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_valid(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    token: u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        Ok(h.sidebar
            .managed
            .slot_valid(workspace.into(), &key.read()?, token))
    })
    .unwrap_or(false) as u32
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_complete(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    token: u64,
    success: u32,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        Ok(h.sidebar
            .managed
            .complete_slot(workspace.into(), &key.read()?, token, success != 0))
    })
    .unwrap_or(false) as u32
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_retry(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.managed.retry_slot(workspace.into(), &key.read()?);
        Ok(())
    })
    .is_some() as u32
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_release_previous(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        Ok(h.sidebar
            .managed
            .release_previous(workspace.into(), &key.read()?))
    })
    .unwrap_or(false) as u32
}

const PANEL_SPLIT: u32 = 0;
const PANEL_TABS: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PanelView {
    pub parent: usize,
    pub id: Text,
    pub weight: f64,
    pub kind: u32,
    pub axis: u32,
    pub first_tab: usize,
    pub tab_count: usize,
    pub selected: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TabView {
    pub slot: Text,
    pub placed: u32,
    pub gone: u32,
}

/// A flat preorder panel array and tab array as a document.
unsafe fn read_arrangement(
    panels: &[PanelView],
    tabs: &[TabView],
) -> Result<andamento_core::slots::ArrangementDoc, String> {
    for (index, view) in panels.iter().enumerate() {
        match (index, view.parent) {
            (0, NONE) => {}
            (0, _) => return Err("the first panel is the root; its parent is NONE".into()),
            (_, NONE) => return Err(format!("panel {index} has no parent; only one root")),
            _ => {}
        }
    }
    Ok(andamento_core::slots::ArrangementDoc {
        root: read_trees(panels, tabs)?.into_iter().next(),
    })
}

/// A flat preorder panel array as trees: each panel whose parent is NONE
/// starts one, and every other names an earlier split as its parent.
unsafe fn read_trees(
    panels: &[PanelView],
    tabs: &[TabView],
) -> Result<Vec<andamento_core::slots::Panel>, String> {
    use andamento_core::slots::{Panel, PanelNode};
    use andamento_core::suggested_layout::Axis;
    let mut built: Vec<Option<Panel>> = Vec::with_capacity(panels.len());
    for (index, view) in panels.iter().enumerate() {
        match view.parent {
            NONE => {}
            parent if parent >= index => {
                return Err(format!("panel {index}'s parent is not an earlier panel"))
            }
            parent if panels[parent].kind != PANEL_SPLIT => {
                return Err(format!("panel {index}'s parent is not a split"))
            }
            _ => {}
        }
        let node = match view.kind {
            PANEL_SPLIT => {
                if view.tab_count != 0 {
                    return Err(format!("split {index} has tabs"));
                }
                PanelNode::Split {
                    axis: match view.axis {
                        0 => Axis::Row,
                        1 => Axis::Column,
                        other => return Err(format!("unknown axis {other}")),
                    },
                    children: Vec::new(),
                }
            }
            PANEL_TABS => {
                let end = view
                    .first_tab
                    .checked_add(view.tab_count)
                    .filter(|end| *end <= tabs.len())
                    .ok_or_else(|| format!("panel {index}'s tabs are out of range"))?;
                let keys = tabs[view.first_tab..end]
                    .iter()
                    .map(|tab| tab.slot.read())
                    .collect::<Result<Vec<_>, _>>()?;
                let selected = match view.selected {
                    NONE => None,
                    n => Some(
                        keys.get(n)
                            .ok_or_else(|| format!("panel {index} selects a tab it doesn't have"))?
                            .clone(),
                    ),
                };
                PanelNode::Tabs {
                    tabs: keys,
                    selected,
                }
            }
            other => return Err(format!("unknown panel kind {other}")),
        };
        built.push(Some(Panel {
            id: view.id.read()?,
            weight: view.weight,
            node,
        }));
    }
    // Attach children to parents, last first, so each parent is complete
    // when it is attached in turn.
    for index in (0..built.len()).rev() {
        let parent = panels[index].parent;
        if parent == NONE {
            continue;
        }
        let child = built[index].take().expect("attached once");
        if let Some(Panel {
            node: PanelNode::Split { children, .. },
            ..
        }) = &mut built[parent]
        {
            children.insert(0, child);
        }
    }
    Ok(built.into_iter().flatten().collect())
}

const ARRANGEMENT_INVALID: u32 = 0;
const ARRANGEMENT_COMMITTED: u32 = 1;
const ARRANGEMENT_STALE: u32 = 2;

#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn andamento_set_arrangement(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    panels: *const PanelView,
    panel_count: usize,
    tabs: *const TabView,
    tab_count: usize,
    expected_generation: u64,
    generation_out: *mut u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let doc = read_arrangement(slice(panels, panel_count)?, slice(tabs, tab_count)?)?;
        let workspace = workspace.into();
        let result = h
            .sidebar
            .set_arrangement(workspace, doc, expected_generation);
        let current = h.sidebar.arrangement(workspace).map(|v| v.generation);
        if let (Some(out), Ok(current)) = (generation_out.as_mut(), current) {
            *out = current;
        }
        match result {
            Ok(_) => Ok(ARRANGEMENT_COMMITTED),
            Err(andamento_core::slots::ArrangementError::Stale { .. }) => Ok(ARRANGEMENT_STALE),
            Err(error) => Err(error.to_string()),
        }
    })
    .unwrap_or(ARRANGEMENT_INVALID)
}

/// A workspace's or the sidebar's arrangement, owned until release.
pub struct AndamentoArrangement {
    generation: u64,
    owned: bool,
    panels: Vec<FlatPanel>,
    tabs: Vec<(String, bool, bool)>,
    /// The first floating panel's index: panel_count for a workspace.
    floating_first: usize,
    /// The sidebar's section notes: (kind, key).
    notes: Vec<(u32, String)>,
}

/// A panel of an acquired arrangement, owning its ID.
struct FlatPanel {
    parent: usize,
    id: String,
    weight: f64,
    kind: u32,
    axis: u32,
    first_tab: usize,
    tab_count: usize,
    selected: usize,
}

#[repr(C)]
pub struct ArrangementInfoView {
    pub generation: u64,
    pub owned: u32,
    pub panel_count: usize,
    pub tab_count: usize,
}

/// Flatten one panel tree into `out`, in preorder, under `parent`.
fn flatten_panel(
    panel: &andamento_core::slots::Panel,
    parent: usize,
    placed: &std::collections::BTreeSet<String>,
    gone: &std::collections::BTreeSet<String>,
    out: &mut AndamentoArrangement,
) {
    use andamento_core::slots::PanelNode;
    use andamento_core::suggested_layout::Axis;
    let index = out.panels.len();
    match &panel.node {
        PanelNode::Split { axis, children } => {
            let axis = match axis {
                Axis::Row => 0,
                Axis::Column => 1,
            };
            out.panels.push(FlatPanel {
                parent,
                id: panel.id.clone(),
                weight: panel.weight,
                kind: PANEL_SPLIT,
                axis,
                first_tab: 0,
                tab_count: 0,
                selected: NONE,
            });
            for child in children {
                flatten_panel(child, index, placed, gone, out);
            }
        }
        PanelNode::Tabs { tabs, selected } => {
            let first = out.tabs.len();
            for tab in tabs {
                out.tabs
                    .push((tab.clone(), placed.contains(tab), gone.contains(tab)));
            }
            let selected = selected
                .as_ref()
                .and_then(|s| tabs.iter().position(|t| t == s))
                .unwrap_or(NONE);
            out.panels.push(FlatPanel {
                parent,
                id: panel.id.clone(),
                weight: panel.weight,
                kind: PANEL_TABS,
                axis: 0,
                first_tab: first,
                tab_count: tabs.len(),
                selected,
            });
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_acquire(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error: *mut *mut c_char,
) -> *mut AndamentoArrangement {
    run(h, error, |h| {
        let view = h.sidebar.arrangement(workspace.into())?;
        let mut out = AndamentoArrangement {
            generation: view.generation,
            owned: view.owned,
            panels: Vec::new(),
            tabs: Vec::new(),
            floating_first: 0,
            notes: Vec::new(),
        };
        if let Some(root) = &view.doc.root {
            flatten_panel(root, NONE, &view.placed, &view.gone, &mut out);
        }
        out.floating_first = out.panels.len();
        Ok(Box::into_raw(Box::new(out)))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_info(
    arrangement: *const AndamentoArrangement,
    out: *mut ArrangementInfoView,
) -> u32 {
    let (Some(arrangement), Some(out)) = (arrangement.as_ref(), out.as_mut()) else {
        return 0;
    };
    *out = ArrangementInfoView {
        generation: arrangement.generation,
        owned: arrangement.owned as u32,
        panel_count: arrangement.panels.len(),
        tab_count: arrangement.tabs.len(),
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_panel(
    arrangement: *const AndamentoArrangement,
    index: usize,
    out: *mut PanelView,
) -> u32 {
    let (Some(arrangement), Some(out)) = (arrangement.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some(panel) = arrangement.panels.get(index) else {
        return 0;
    };
    *out = PanelView {
        parent: panel.parent,
        id: Text::borrowed(&panel.id),
        weight: panel.weight,
        kind: panel.kind,
        axis: panel.axis,
        first_tab: panel.first_tab,
        tab_count: panel.tab_count,
        selected: panel.selected,
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_tab(
    arrangement: *const AndamentoArrangement,
    index: usize,
    out: *mut TabView,
) -> u32 {
    let (Some(arrangement), Some(out)) = (arrangement.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some((slot, placed, gone)) = arrangement.tabs.get(index) else {
        return 0;
    };
    *out = TabView {
        slot: Text::borrowed(slot),
        placed: *placed as u32,
        gone: *gone as u32,
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_release(arrangement: *mut AndamentoArrangement) {
    if !arrangement.is_null() {
        drop(Box::from_raw(arrangement));
    }
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_floating_first(
    arrangement: *const AndamentoArrangement,
) -> usize {
    arrangement.as_ref().map_or(0, |a| a.floating_first)
}

const SECTION_CLOSED: u32 = 0;
const SECTION_PLACED: u32 = 1;
const SECTION_RESTORED: u32 = 2;
const SECTION_DUPLICATE: u32 = 3;
const SECTION_UNRESOLVED: u32 = 4;

#[repr(C)]
pub struct SectionNoteView {
    pub kind: u32,
    pub key: Text,
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_note_count(
    arrangement: *const AndamentoArrangement,
) -> usize {
    arrangement.as_ref().map_or(0, |a| a.notes.len())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_note(
    arrangement: *const AndamentoArrangement,
    index: usize,
    out: *mut SectionNoteView,
) -> u32 {
    let (Some(arrangement), Some(out)) = (arrangement.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some((kind, key)) = arrangement.notes.get(index) else {
        return 0;
    };
    *out = SectionNoteView {
        kind: *kind,
        key: Text::borrowed(key),
    };
    1
}

/// The sidebar's document from the host's arrays: panels before
/// `floating_first` are the dock (one tree, or none when it is 0); the rest
/// are floating panels, each a tree whose root's parent is NONE.
unsafe fn read_sidebar_doc(
    panels: &[PanelView],
    floating_first: usize,
    tabs: &[TabView],
) -> Result<andamento_core::sidebar_arrangement::SidebarDoc, String> {
    if floating_first > panels.len() {
        return Err("floating_first is past the last panel".into());
    }
    for (index, view) in panels.iter().enumerate() {
        let root = view.parent == NONE;
        let starts = index == 0 || index == floating_first;
        if index < floating_first && root != (index == 0) {
            return Err(format!(
                "panel {index}: the dock is one tree, rooted at panel 0"
            ));
        }
        if index >= floating_first && starts && !root {
            return Err(format!(
                "panel {index} starts the floating panels; its parent is NONE"
            ));
        }
        if index >= floating_first && !root && view.parent < floating_first {
            return Err(format!("panel {index} is floating; its parent is docked"));
        }
    }
    let mut trees = read_trees(panels, tabs)?.into_iter();
    let dock = if floating_first > 0 {
        trees.next()
    } else {
        None
    };
    Ok(andamento_core::sidebar_arrangement::SidebarDoc {
        dock: andamento_core::slots::ArrangementDoc { root: dock },
        floating: trees.collect(),
    })
}

/// The C result of a sidebar arrangement call, writing the generation after.
fn sidebar_result(
    h: &Andamento,
    result: Result<
        andamento_core::sidebar_arrangement::Report,
        andamento_core::slots::ArrangementError,
    >,
    generation_out: *mut u64,
) -> Result<u32, String> {
    if let Some(out) = unsafe { generation_out.as_mut() } {
        *out = h.sidebar.sidebar_arrangement_generation();
    }
    match result {
        Ok(_) => Ok(ARRANGEMENT_COMMITTED),
        Err(andamento_core::slots::ArrangementError::Stale { .. }) => Ok(ARRANGEMENT_STALE),
        Err(error) => Err(error.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn andamento_set_sidebar_arrangement(
    h: *mut Andamento,
    panels: *const PanelView,
    panel_count: usize,
    floating_first: usize,
    tabs: *const TabView,
    tab_count: usize,
    expected_generation: u64,
    generation_out: *mut u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let doc = read_sidebar_doc(
            slice(panels, panel_count)?,
            floating_first,
            slice(tabs, tab_count)?,
        )?;
        let result = h.sidebar.set_sidebar_arrangement(doc, expected_generation);
        sidebar_result(h, result, generation_out)
    })
    .unwrap_or(ARRANGEMENT_INVALID)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_restore_section(
    h: *mut Andamento,
    key: Text,
    expected_generation: u64,
    generation_out: *mut u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let key = key.read()?;
        let result = h.sidebar.restore_sidebar_section(&key, expected_generation);
        sidebar_result(h, result, generation_out)
    })
    .unwrap_or(ARRANGEMENT_INVALID)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_reset(
    h: *mut Andamento,
    expected_generation: u64,
    generation_out: *mut u64,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let result = h.sidebar.reset_sidebar_arrangement(expected_generation);
        sidebar_result(h, result, generation_out)
    })
    .unwrap_or(ARRANGEMENT_INVALID)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_arrangement_generation(
    h: *mut Andamento,
    error: *mut *mut c_char,
) -> u64 {
    run(h, error, |h| Ok(h.sidebar.sidebar_arrangement_generation())).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_arrangement_acquire(
    h: *mut Andamento,
    error: *mut *mut c_char,
) -> *mut AndamentoArrangement {
    run(h, error, |h| {
        let (view, report) = h.sidebar.sidebar_arrangement();
        let mut out = AndamentoArrangement {
            generation: view.generation,
            owned: view.owned,
            panels: Vec::new(),
            tabs: Vec::new(),
            floating_first: 0,
            notes: Vec::new(),
        };
        if let Some(root) = &view.doc.dock.root {
            flatten_panel(root, NONE, &view.placed, &view.unresolved, &mut out);
        }
        out.floating_first = out.panels.len();
        for panel in &view.doc.floating {
            flatten_panel(panel, NONE, &view.placed, &view.unresolved, &mut out);
        }
        let notes = [
            (SECTION_CLOSED, view.closed.iter().collect::<Vec<_>>()),
            (SECTION_PLACED, report.placed.iter().collect()),
            (SECTION_RESTORED, report.restored.iter().collect()),
            (SECTION_DUPLICATE, report.duplicates.iter().collect()),
            (SECTION_UNRESOLVED, view.unresolved.iter().collect()),
        ];
        for (kind, keys) in notes {
            out.notes
                .extend(keys.into_iter().map(|key| (kind, key.clone())));
        }
        Ok(Box::into_raw(Box::new(out)))
    })
    .unwrap_or(ptr::null_mut())
}

/// ABI 3: the revision of workspace content (slots, resolutions and
/// arrangements), separate from the sidebar snapshot's.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_content_revision(
    h: *mut Andamento,
    error: *mut *mut c_char,
) -> u64 {
    run(h, error, |h| Ok(h.sidebar.content_revision())).unwrap_or(0)
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
                } else if node.key.read().unwrap()
                    == andamento_core::presentation::UNPLACED_WORKSPACES_SECTION
                {
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
