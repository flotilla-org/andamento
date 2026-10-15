//! Andamento's C ABI: a typed, single-owner embedding interface.
//! include/andamento.h is generated from this crate by cbindgen
//! (scripts/generate-header.sh); every doc comment here is the header's.
//! docs/sidebar-design/host-interface.md describes the interface as a whole.
#![allow(clippy::missing_safety_doc)] // The header specifies pointer safety for every call.
use andamento_core::{
    detail::DetailCard,
    host::PaneObservation,
    presentation::{Content, PlacementNode, PresentationState},
    profile,
    sidebar::{Action, HostEffect, Workspace},
    template_config::{TemplateConfigFieldClass, TemplateControlKind},
    DisplayVariableValue, EntityRef, MetadataPatch, MetadataTarget, MetadataValue, MetadataValueUpdate, PaneTarget, Sidebar, WorkspaceId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{c_char, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

pub mod dashboard;
pub mod documents;
pub mod slots;
pub use dashboard::*;
pub use documents::*;
pub use slots::*;

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

/// ANDAMENTO_NONE (SIZE_MAX): an index that names nothing.
const NONE: usize = usize::MAX;

/// UTF-8 text, length-delimited (it may contain NUL). Input text is
/// borrowed for the call only; output text is borrowed from the object that
/// returned it. Empty output text has a non-NULL pointer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Text {
    pub data: *const u8,
    pub len: usize,
}
impl Text {
    fn borrowed(s: &str) -> Self {
        Self { data: s.as_ptr(), len: s.len() }
    }
    unsafe fn read(self) -> Result<String, String> {
        Ok(std::str::from_utf8(slice(self.data, self.len)?).map_err(|e| e.to_string())?.to_owned())
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

/// Bytes the library allocated; free them with andamento_bytes_free (a
/// zeroed AndamentoBytes is harmless). Out-parameters are written only on
/// success.
#[repr(C)]
pub struct Bytes {
    pub data: *mut u8,
    pub len: usize,
}
fn owned_bytes(bytes: Vec<u8>) -> Bytes {
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    Bytes { data: Box::into_raw(boxed) as *mut u8, len }
}
unsafe fn write_bytes(out: *mut Bytes, bytes: Vec<u8>) -> Result<(), String> {
    let out = out.as_mut().ok_or("null output")?;
    *out = owned_bytes(bytes);
    Ok(())
}

/// A struct returned to the host and what it points into, in one
/// allocation the host frees with the struct's `_free` call.
#[repr(C)]
struct Owned<T, K> {
    value: T,
    keep: K,
}
fn give<T, K>(value: T, keep: K) -> *mut T {
    Box::into_raw(Box::new(Owned { value, keep })).cast()
}
unsafe fn take<T, K>(value: *mut T) {
    if !value.is_null() {
        drop(Box::from_raw(value.cast::<Owned<T, K>>()));
    }
}

/// A sidebar: one presentation client's core. Calls on one are serialised
/// by the host.
pub struct Andamento {
    sidebar: Sidebar,
    poisoned: bool,
    client: u64,
    effects: Vec<HostEffect>,
}
unsafe fn run<T>(handle: *mut Andamento, error: *mut *mut c_char, f: impl FnOnce(&mut Andamento) -> Result<T, String>) -> Option<T> {
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

/// A compare-and-set's result: rejected with an error, and nothing changed.
pub const ANDAMENTO_INVALID: u32 = 0;
/// A compare-and-set's result: stored (or already what was stored).
pub const ANDAMENTO_COMMITTED: u32 = 1;
/// A compare-and-set's result: the expected generation was not the current
/// one, and nothing changed (no error). Read again and retry.
pub const ANDAMENTO_STALE: u32 = 2;

/// A Workspace ID: 16 bytes the host supplies (UUIDv7 in practice).
/// Andamento never generates one, and only compares and prints them.
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
        Self { bytes: id.to_bytes() }
    }
}
/// A Workspace ID for output: zeroes for none.
fn workspace_view(id: Option<WorkspaceId>) -> WorkspaceIdView {
    id.map_or(WorkspaceIdView { bytes: [0; 16] }, WorkspaceIdView::from)
}

/// An entity: `{provider, kind, id}`. The provider is the Dashboard
/// subscription its facts come from ("local" for Andamento's own sections,
/// groups and refs); the same kind and ID under two providers are two
/// entities. An entity a host passes names its provider.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EntityView {
    pub provider: Text,
    pub kind: Text,
    pub id: Text,
}
impl EntityView {
    fn empty() -> Self {
        Self { provider: Text::borrowed(""), kind: Text::borrowed(""), id: Text::borrowed("") }
    }
    fn of(entity: &EntityRef) -> Self {
        Self { provider: Text::borrowed(&entity.provider), kind: Text::borrowed(&entity.kind), id: Text::borrowed(&entity.id) }
    }
    unsafe fn read(self) -> Result<EntityRef, String> {
        Ok(EntityRef::new(provider(self.provider)?, self.kind.read()?, self.id.read()?))
    }
}
/// A provider (subscription ID) a host passes: nonempty UTF-8.
unsafe fn provider(provider: Text) -> Result<String, String> {
    let provider = provider.read()?;
    if provider.is_empty() {
        return Err("a provider needs a name".into());
    }
    Ok(provider)
}

/// The ABI this library implements. A host requires exactly the version it
/// was built against.
#[no_mangle]
pub extern "C" fn andamento_abi_version() -> u32 {
    7
}
/// A sidebar configured with a template catalog (KDL), or NULL with an
/// error.
#[no_mangle]
pub unsafe extern "C" fn andamento_create(config_kdl: *const u8, len: usize, error_out: *mut *mut c_char) -> *mut Andamento {
    if !error_out.is_null() {
        *error_out = ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| Sidebar::new(&Text { data: config_kdl, len }.read()?)))
        .unwrap_or_else(|_| Err("core panicked while creating sidebar".into()));
    match result {
        Ok(sidebar) => Box::into_raw(Box::new(Andamento {
            sidebar,
            poisoned: false,
            client: NEXT_CLIENT.fetch_add(1, Ordering::Relaxed),
            effects: vec![],
        })),
        Err(message) => {
            if !error_out.is_null() {
                *error_out = string(message);
            }
            ptr::null_mut()
        }
    }
}
/// Replace the template catalog; an invalid one changes nothing.
#[no_mangle]
pub unsafe extern "C" fn andamento_configure(h: *mut Andamento, kdl: Text, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| h.sidebar.configure(&kdl.read()?)).is_some() as u32
}
#[no_mangle]
pub unsafe extern "C" fn andamento_destroy(h: *mut Andamento) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}
/// Free an error string.
#[no_mangle]
pub unsafe extern "C" fn andamento_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
#[no_mangle]
pub unsafe extern "C" fn andamento_bytes_free(bytes: Bytes) {
    if !bytes.data.is_null() {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(bytes.data, bytes.len)));
    }
}

/// Apply one producer MetadataPatch, as JSON, from `provider` (a
/// subscription ID other than "local"), which is stamped over every entity
/// the patch names: its target, entity references among its values, and the
/// entity a workspace's entity.kind/entity.id or .host.kind/.host.id facts
/// name. A patch never names its own provider. Requires the default "json"
/// feature.
#[cfg(feature = "json")]
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_patch_json(
    h: *mut Andamento,
    now_ms: u64,
    provider: Text,
    json: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let patch: MetadataPatch = serde_json::from_str(&json.read()?).map_err(|e| e.to_string())?;
        h.sidebar.apply(now_ms, &provider.read()?, [patch])
    })
    .is_some() as u32
}
/// Remove all of a provider's facts, as when its subscription is removed.
/// What open workspaces and refs showed of its entities is retained, as when
/// facts expire.
#[no_mangle]
pub unsafe extern "C" fn andamento_provider_retract(h: *mut Andamento, provider: Text, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.retract_provider(&self::provider(provider)?);
        Ok(())
    })
    .is_some() as u32
}
/// Mark a provider stale (nonzero), when its connection drops, or fresh
/// again (zero). A stale provider's facts are kept, and nodes and detail
/// fields presenting its entities report stale. Retracting a provider
/// removes its facts and clears its staleness.
#[no_mangle]
pub unsafe extern "C" fn andamento_provider_set_stale(h: *mut Andamento, provider: Text, stale: u32, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.set_provider_stale(&self::provider(provider)?, stale != 0);
        Ok(())
    })
    .is_some() as u32
}

/// An AndamentoFact that unsets its key.
pub const ANDAMENTO_FACT_UNSET: u32 = 0;
pub const ANDAMENTO_FACT_TEXT: u32 = 1;
/// A boolean: integer 0 or 1.
pub const ANDAMENTO_FACT_BOOL: u32 = 2;
pub const ANDAMENTO_FACT_INTEGER: u32 = 3;
/// One entity reference (local facts only).
pub const ANDAMENTO_FACT_ENTITY: u32 = 4;

/// A scalar fact (ANDAMENTO_FACT_UNSET, _TEXT, _BOOL or _INTEGER), with
/// optional TTL, precedence and ordinal: a workspace's facts may carry
/// them; an entity's ignore them (andamento_apply_entity). Keys
/// are unique within a call.
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
/// Apply scalar facts to an entity, from `provider` as for
/// andamento_apply_patch_json. The batch is validated before it applies.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_entity(
    h: *mut Andamento,
    now_ms: u64,
    provider: Text,
    kind: Text,
    id: Text,
    source: Text,
    facts: *const Fact,
    count: usize,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let target = MetadataTarget::Entity(EntityRef::new("", kind.read()?, id.read()?));
        let patch = scalar_patch(target, source, facts, count)?;
        h.sidebar.apply(now_ms, &provider.read()?, [patch])
    })
    .is_some() as u32
}
/// Apply scalar facts to a workspace (a tab target), such as the
/// .host.kind/.host.id naming its host entity, which is the provider's.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn andamento_apply_workspace(
    h: *mut Andamento,
    now_ms: u64,
    provider: Text,
    workspace: WorkspaceIdView,
    source: Text,
    facts: *const Fact,
    count: usize,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let target = MetadataTarget::Tab(workspace.into());
        let patch = scalar_patch(target, source, facts, count)?;
        h.sidebar.apply(now_ms, &provider.read()?, [patch])
    })
    .is_some() as u32
}
unsafe fn scalar_patch(target: MetadataTarget, source: Text, facts: *const Fact, count: usize) -> Result<MetadataPatch, String> {
    let mut patch = MetadataPatch { target, source_id: source.read()?, set: Default::default(), unset: vec![] };
    let mut keys = BTreeSet::new();
    for f in slice(facts, count)? {
        let key = f.key.read()?;
        if !keys.insert(key.clone()) {
            return Err("duplicate fact key".into());
        }
        let value = match f.kind {
            ANDAMENTO_FACT_UNSET => {
                patch.unset.push(key);
                continue;
            }
            kind => scalar(kind, f.text, f.integer)?,
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
unsafe fn scalar(kind: u32, text: Text, integer: i64) -> Result<MetadataValue, String> {
    Ok(match kind {
        ANDAMENTO_FACT_TEXT => MetadataValue::Text(text.read()?.into()),
        ANDAMENTO_FACT_BOOL if integer == 0 || integer == 1 => MetadataValue::Bool(integer != 0),
        ANDAMENTO_FACT_INTEGER => MetadataValue::Integer(integer),
        _ => return Err("invalid fact kind or boolean".into()),
    })
}
/// Advance time without facts. Time is monotonic milliseconds scoped to
/// this sidebar. Drain facts, topology and completions before acquiring a
/// snapshot.
#[no_mangle]
pub unsafe extern "C" fn andamento_tick(h: *mut Andamento, now_ms: u64, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.tick(now_ms);
        Ok(())
    })
    .is_some() as u32
}

pub const ANDAMENTO_PANE_TERMINAL: u32 = 0;
pub const ANDAMENTO_PANE_PLUGIN: u32 = 1;

/// One open workspace in the host's topology.
#[repr(C)]
pub struct WorkspaceInput {
    pub id: WorkspaceIdView,
    pub position: usize,
    pub name: Text,
    pub selected: u32,
}
/// A pane in a workspace (Zellij's terminal/plugin u32 identity). Hosts
/// with other kinds of views pass no panes.
#[repr(C)]
pub struct PaneInput {
    pub workspace_id: WorkspaceIdView,
    pub pane_id: u32,
    /// ANDAMENTO_PANE_TERMINAL or ANDAMENTO_PANE_PLUGIN.
    pub kind: u32,
    pub selectable: u32,
    pub focused: u32,
    pub ordinal: i64,
}
/// Replace the whole topology: the open workspaces and their panes.
#[no_mangle]
pub unsafe extern "C" fn andamento_observe(
    h: *mut Andamento,
    workspaces: *const WorkspaceInput,
    count: usize,
    panes: *const PaneInput,
    pane_count: usize,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let workspaces = slice(workspaces, count)?
            .iter()
            .map(|w| Ok(Workspace { id: w.id.into(), position: w.position, name: w.name.read()?, selected: w.selected != 0 }))
            .collect::<Result<Vec<_>, String>>()?;
        let panes = slice(panes, pane_count)?
            .iter()
            .map(|p| {
                Ok(PaneObservation {
                    workspace_id: p.workspace_id.into(),
                    pane_id: match p.kind {
                        ANDAMENTO_PANE_TERMINAL => PaneTarget::Terminal(p.pane_id),
                        ANDAMENTO_PANE_PLUGIN => PaneTarget::Plugin(p.pane_id),
                        _ => return Err("invalid pane kind".to_owned()),
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
/// A workspace's working directory, normalised by the host.
#[repr(C)]
pub struct WorkdirInput {
    pub workspace_id: WorkspaceIdView,
    pub cwd: Text,
}
/// Replace the directory associations (supply topology first). Matching is
/// exact; empty paths and unknown workspaces are ignored, and a workspace's
/// explicit identity wins. Nothing is persisted.
#[no_mangle]
pub unsafe extern "C" fn andamento_observe_workdirs(
    h: *mut Andamento,
    workdirs: *const WorkdirInput,
    count: usize,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let workdirs =
            slice(workdirs, count)?.iter().map(|w| Ok((w.workspace_id.into(), w.cwd.read()?))).collect::<Result<Vec<_>, String>>()?;
        h.sidebar.observe_workdirs(workdirs);
        Ok(())
    })
    .is_some() as u32
}

/// The host focused the workspace.
pub const ANDAMENTO_COMPLETE_FOCUS: u32 = 0;
/// The host materialized a workspace: workspace_id is its new ID.
pub const ANDAMENTO_COMPLETE_MATERIALIZE: u32 = 1;
/// The request failed, timed out or was cancelled: message says why.
pub const ANDAMENTO_COMPLETE_ERROR: u32 = 2;

/// Complete a FOCUS or MATERIALIZE effect's request (every one, including
/// cancellation and timeout). Unknown or repeated request IDs are ignored.
#[no_mangle]
pub unsafe extern "C" fn andamento_complete(
    h: *mut Andamento,
    request_id: u64,
    outcome: u32,
    workspace_id: WorkspaceIdView,
    message: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let result = match outcome {
            ANDAMENTO_COMPLETE_FOCUS => Ok(None),
            ANDAMENTO_COMPLETE_MATERIALIZE => Ok(Some(workspace_id.into())),
            ANDAMENTO_COMPLETE_ERROR => Err(message.read()?),
            _ => return Err("invalid completion outcome".into()),
        };
        h.sidebar.complete(request_id, result);
        Ok(())
    })
    .is_some() as u32
}

/// Declare a workspace the host created itself (one that never went through
/// MATERIALIZE, whose completion registers it too). Registration gives it a
/// record and is independent of topology; registering twice is harmless.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_register(h: *mut Andamento, workspace: WorkspaceIdView, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.register_workspace(workspace.into());
        Ok(())
    })
    .is_some() as u32
}
/// The host deleted the workspace rather than closing it: its record goes.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_forget(h: *mut Andamento, workspace: WorkspaceIdView, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.forget_workspace(workspace.into());
        Ok(())
    })
    .is_some() as u32
}
/// 1 if the workspace is registered (or materialized) and not forgotten.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_registered(h: *mut Andamento, workspace: WorkspaceIdView, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| Ok(h.sidebar.registered_workspaces().contains(&workspace.into()))).unwrap_or(false) as u32
}

// Snapshots, details, dispatch and effects. A snapshot owns all the text it
// returns and survives sidebar mutation and destruction; release it after
// rendering.

pub const ANDAMENTO_CATALOG: u32 = 0;
pub const ANDAMENTO_LATENT: u32 = 1;
pub const ANDAMENTO_OPENING: u32 = 2;
pub const ANDAMENTO_LIVE: u32 = 3;

pub const ANDAMENTO_FIELD_REQUIRED: u32 = 0;
pub const ANDAMENTO_FIELD_OPTIONAL: u32 = 1;
pub const ANDAMENTO_FIELD_PRIORITY: u32 = 2;

pub const ANDAMENTO_CONTROL_OPEN_CONFIG: u32 = 0;
pub const ANDAMENTO_CONTROL_DISPLAY_VARIABLE: u32 = 1;
pub const ANDAMENTO_CONTROL_SCROLL_DOWN: u32 = 2;
pub const ANDAMENTO_CONTROL_SCROLL_UP: u32 = 3;
pub const ANDAMENTO_CONTROL_INSPECT_ROOT: u32 = 4;

/// A control's value: none, a boolean (checked) or an enum value.
pub const ANDAMENTO_VALUE_NONE: u32 = 0;
pub const ANDAMENTO_VALUE_BOOL: u32 = 1;
pub const ANDAMENTO_VALUE_ENUM: u32 = 2;

pub const ANDAMENTO_DETAIL_IDENTITY: u32 = 0;
pub const ANDAMENTO_DETAIL_TITLE: u32 = 1;
pub const ANDAMENTO_DETAIL_STATE: u32 = 2;
pub const ANDAMENTO_DETAIL_FACT: u32 = 3;
pub const ANDAMENTO_DETAIL_RELATION: u32 = 4;

/// Focus workspace_id, then complete request_id.
pub const ANDAMENTO_EFFECT_FOCUS: u32 = 0;
/// Make a workspace for entity with recipe (named name, in cwd), then
/// complete request_id with its new ID.
pub const ANDAMENTO_EFFECT_MATERIALIZE: u32 = 1;
/// Show entity's inspector; needs no completion.
pub const ANDAMENTO_EFFECT_INSPECT: u32 = 2;
/// Open the URL in recipe; needs no completion.
pub const ANDAMENTO_EFFECT_OPEN_URL: u32 = 3;
/// Copy the URL in recipe; needs no completion.
pub const ANDAMENTO_EFFECT_COPY_URL: u32 = 4;

pub(crate) struct Field {
    text: String,
    class_: u32,
    priority: Option<i64>,
}
pub(crate) struct Control {
    pub(crate) kind: u32,
    pub(crate) label: String,
    pub(crate) glyph: String,
    pub(crate) action: usize,
    pub(crate) value: String,
    pub(crate) value_kind: u32,
    pub(crate) checked: bool,
    pub(crate) variable: String,
    pub(crate) persist: bool,
}
pub(crate) struct Node {
    pub(crate) parent: usize,
    pub(crate) section: bool,
    pub(crate) default_host: String,
    pub(crate) order: Option<i64>,
    pub(crate) key: String,
    pub(crate) entity: EntityRef,
    pub(crate) label: String,
    pub(crate) layout: String,
    pub(crate) loop_key: String,
    pub(crate) form: String,
    pub(crate) state: u32,
    pub(crate) workspace: Option<WorkspaceId>,
    pub(crate) selected: bool,
    pub(crate) openable: bool,
    pub(crate) collapsed: bool,
    pub(crate) pinned: bool,
    pub(crate) stale: bool,
    pub(crate) fields: usize,
    pub(crate) field_count: usize,
    pub(crate) details: usize,
    pub(crate) detail_count: usize,
    pub(crate) controls: usize,
    pub(crate) control_count: usize,
    pub(crate) activate: usize,
    pub(crate) copy_url: usize,
    pub(crate) toggle: usize,
}
/// Immutable catalog card and existing dispatch references, owned by a detail snapshot.
pub(crate) struct Detail {
    card: DetailCard,
    activate: usize,
    copy_url: usize,
    /// Its entity's provider was stale when the snapshot was taken.
    stale: bool,
}
pub struct AndamentoSnapshot {
    pub(crate) now_ms: u64,
    pub(crate) details: Vec<Detail>,
    pub(crate) detail_index: BTreeMap<EntityRef, usize>,
    pub(crate) client: u64,
    pub(crate) generation: u64,
    pub(crate) nodes: Vec<Node>,
    pub(crate) fields: Vec<Field>,
    pub(crate) controls: Vec<Control>,
    pub(crate) actions: Vec<Action>,
    pub(crate) diagnostics: Vec<String>,
}
impl AndamentoSnapshot {
    // Exported text borrows String/Vec heap buffers, never inline Detail/Action
    // storage. Appending may move structs but must not move those buffers or
    // renumber existing actions; small inline-string representations would break it.
    fn add_detail(&mut self, card: DetailCard, sidebar: &Sidebar) -> usize {
        let activate = self.action(Action::Activate { entity: card.entity.clone() });
        let copy_url = if sidebar.subject_url(&card.entity).is_some() {
            self.action(Action::CopySubjectUrl { entity: card.entity.clone() })
        } else {
            NONE
        };
        let index = self.details.len();
        self.detail_index.insert(card.entity.clone(), index);
        let stale = sidebar.provider_is_stale(&card.entity.provider);
        self.details.push(Detail { card, activate, copy_url, stale });
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
                class_: match f.class {
                    TemplateConfigFieldClass::Required => ANDAMENTO_FIELD_REQUIRED,
                    TemplateConfigFieldClass::Optional => ANDAMENTO_FIELD_OPTIONAL,
                    TemplateConfigFieldClass::Priority => ANDAMENTO_FIELD_PRIORITY,
                },
                priority: f.priority,
            });
        }
        let controls = self.controls.len();
        for c in &content.controls {
            let kind = match c.kind {
                TemplateControlKind::OpenConfig => ANDAMENTO_CONTROL_OPEN_CONFIG,
                TemplateControlKind::DisplayVariable => ANDAMENTO_CONTROL_DISPLAY_VARIABLE,
                TemplateControlKind::ScrollDown => ANDAMENTO_CONTROL_SCROLL_DOWN,
                TemplateControlKind::ScrollUp => ANDAMENTO_CONTROL_SCROLL_UP,
                TemplateControlKind::InspectRoot => ANDAMENTO_CONTROL_INSPECT_ROOT,
            };
            let action = if kind == ANDAMENTO_CONTROL_DISPLAY_VARIABLE {
                c.variable.as_ref().map(|name| self.action(Action::ToggleDisplayVariable { name: name.clone() })).unwrap_or(NONE)
            } else {
                NONE
            };
            self.controls.push(Control {
                kind,
                label: c.variable.clone().unwrap_or_default(),
                glyph: c.glyph.clone().unwrap_or_default(),
                action,
                value: String::new(),
                value_kind: ANDAMENTO_VALUE_NONE,
                checked: false,
                variable: c.variable.clone().unwrap_or_default(),
                persist: false,
            });
        }
        if let Some(error) = &content.error {
            self.diagnostics.push(error.clone());
        }
        (start, content.fields.len(), controls, content.controls.len())
    }
    fn node(&mut self, n: &PlacementNode, parent: usize, sidebar: &Sidebar) {
        let (fields, field_count, controls, control_count) = self.content(&n.content);
        let (details, detail_count, _, _) = self.content(&n.detail);
        let activate = self.action(Action::ActivatePlacement { key: n.key.clone() });
        let has_url = sidebar.subject_url(&n.entity).is_some();
        let copy_url = if has_url { self.action(Action::CopySubjectUrl { entity: n.entity.clone() }) } else { NONE };
        let toggle = self.action(Action::TogglePlacement { key: n.key.clone() });
        // Length framing is collision-free even when IDs contain separators.
        // Hosts compare this opaque value; its encoding is not an interface.
        let key = n
            .key
            .0
            .iter()
            .flat_map(|s| [&s.loop_name, &s.entity.provider, &s.entity.kind, &s.entity.id])
            .map(|s| format!("{}:{}", s.len(), s))
            .collect();
        let (state, workspace, selected, openable) = match n.state {
            PresentationState::Catalog => (ANDAMENTO_CATALOG, None, false, false),
            PresentationState::Latent { openable } => (ANDAMENTO_LATENT, None, false, openable),
            PresentationState::Opening => (ANDAMENTO_OPENING, None, false, false),
            PresentationState::Live { workspace_id, selected } => (ANDAMENTO_LIVE, Some(workspace_id), selected, false),
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
/// 1 if the snapshot still matches this sidebar; 0 for a stale, NULL or
/// other sidebar's snapshot (no error), or an invalid or poisoned sidebar
/// (error). Unchanged heartbeats and ticks keep it current; recipe changes
/// don't, even when nothing visible changed. It builds nothing: check after
/// draining a batch, and acquire only when stale.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_is_current(
    h: *mut Andamento,
    snapshot: *const AndamentoSnapshot,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| Ok(snapshot.as_ref().is_some_and(|s| s.client == h.client && s.generation == h.sidebar.revision())))
        .unwrap_or(false) as u32
}

/// The sidebar as nodes in preorder: sections have parent ANDAMENTO_NONE,
/// and every other node's parent is an earlier node. Collapsed children
/// are included.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_acquire(h: *mut Andamento, error_out: *mut *mut c_char) -> *mut AndamentoSnapshot {
    acquire_snapshot(h, error_out, false)
}
/// As andamento_snapshot_acquire, with every catalog card's detail.
/// andamento_snapshot_acquire resolves none (detail_count is 0 until
/// requested).
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_acquire_details(h: *mut Andamento, error_out: *mut *mut c_char) -> *mut AndamentoSnapshot {
    acquire_snapshot(h, error_out, true)
}
unsafe fn acquire_snapshot(h: *mut Andamento, error_out: *mut *mut c_char, include_details: bool) -> *mut AndamentoSnapshot {
    run(h, error_out, |h| {
        let _phase = profile::span("abi-acquire");
        let snapshot = h.sidebar.snapshot();
        let _flatten = profile::span("abi-flatten");
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
            out.diagnostics.push(format!("{}:{}: {}", e.entity.kind, e.entity.id, e.message));
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
            if control.kind != ANDAMENTO_CONTROL_DISPLAY_VARIABLE {
                continue;
            }
            if let Some(value) = snapshot.surface.display_values.get(&control.variable) {
                match value {
                    DisplayVariableValue::Bool(value) => {
                        control.value_kind = ANDAMENTO_VALUE_BOOL;
                        control.checked = *value;
                    }
                    DisplayVariableValue::Enum(value) => {
                        control.value_kind = ANDAMENTO_VALUE_ENUM;
                        control.value = value.clone();
                    }
                }
            }
            if let Some(definition) = snapshot.surface.display_variables.iter().find(|d| d.name == control.variable) {
                control.persist = definition.persist;
                if control.glyph.is_empty() {
                    control.glyph = definition.icon.clone();
                }
                control.label = if definition.label.is_empty() { definition.name.clone() } else { definition.label.clone() };
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
/// A catalog card: an entity's exact detail, independent of where it is
/// placed. Observations use host monotonic milliseconds; a field's
/// observed_at_ms is the winning fact's receipt time, and TTL and source_id
/// give its freshness and producer.
#[repr(C)]
pub struct DetailView {
    pub entity: EntityView,
    pub label: Text,
    pub field_count: usize,
    pub activate: usize,
    pub copy_url: usize,
    pub now_ms: u64,
    pub error: Text,
    /// The workspace live for the entity, if any.
    pub has_workspace: u32,
    pub workspace_id: WorkspaceIdView,
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
    /// Observed from a provider that is stale (its connection dropped).
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
/// Append an entity's detail to a current snapshot and return its index:
/// ANDAMENTO_NONE without an error when the catalog has no such entity.
/// Each entity is appended once; earlier text and action indices stay valid
/// until release. A stale snapshot's existing details stay readable, but it
/// takes no new requests. Not safe with concurrent use of the snapshot.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_request(
    h: *mut Andamento,
    s: *mut AndamentoSnapshot,
    entity: EntityView,
    error_out: *mut *mut c_char,
) -> usize {
    run(h, error_out, |h| {
        let s = s.as_mut().ok_or("null snapshot")?;
        let entity = entity.read()?;
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
/// The index of an entity's detail in the snapshot, or ANDAMENTO_NONE.
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail_find(s: *const AndamentoSnapshot, entity: EntityView) -> usize {
    let (Some(s), Ok(entity)) = (s.as_ref(), entity.read()) else {
        return NONE;
    };
    s.detail_index.get(&entity).copied().unwrap_or(NONE)
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_detail(s: *const AndamentoSnapshot, index: usize, out: *mut DetailView) -> u32 {
    let (Some(s), Some(out)) = (s.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some(d) = s.details.get(index) else {
        return 0;
    };
    *out = DetailView {
        entity: EntityView::of(&d.card.entity),
        label: Text::borrowed(&d.card.label),
        field_count: d.card.fields.len(),
        activate: d.activate,
        copy_url: d.copy_url,
        now_ms: s.now_ms,
        error: Text::borrowed(d.card.error.as_deref().unwrap_or("")),
        has_workspace: d.card.workspace_id.is_some() as u32,
        workspace_id: workspace_view(d.card.workspace_id),
    };
    1
}
/// A detail's action: index 0 is its primary action (intent open-url,
/// focus-workspace, materialize-workspace or inspect), index 1 copies its URL
/// (0 when it has none). Both dispatch like snapshot actions.
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
        0 => (d.card.primary_intent.as_str(), d.card.primary_label.as_str(), d.activate),
        1 if d.copy_url != NONE => ("copy-url", "Copy URL", d.copy_url),
        _ => return 0,
    };
    *out =
        DetailActionView { intent: Text::borrowed(intent), label: Text::borrowed(label), entity: EntityView::of(&d.card.entity), action };
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
    let Some(d) = s.details.get(detail) else {
        return 0;
    };
    let Some(f) = d.card.fields.get(field) else {
        return 0;
    };
    let role = f.role as u32;
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
        stale: (f.observation.is_some() && d.stale) as u32,
        relation_count: f.relations.len(),
    };
    1
}
/// A relation of a detail's field: an entity, its detail (or
/// ANDAMENTO_NONE) and the text to show. Pass the navigation path of
/// entities: a relation already on it returns 0.
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
    let Some(entity) = s.details.get(detail).and_then(|d| d.card.fields.get(field)).and_then(|f| f.relations.get(relation)) else {
        return 0;
    };
    for item in path {
        match item.read() {
            Ok(item) if item == *entity => return 0,
            Ok(_) => {}
            Err(_) => return 0,
        }
    }
    let target = s.detail_index.get(entity).copied();
    *out = DetailRelationView {
        entity: EntityView::of(entity),
        display_text: Text::borrowed(target.map_or(entity.id.as_str(), |index| s.details[index].card.label.as_str())),
        detail: target.unwrap_or(NONE),
    };
    1
}

/// A snapshot node. Keys are opaque and stable for a placement across
/// snapshots; qualify a section's with is_section.
#[repr(C)]
pub struct NodeView {
    pub parent: usize,
    pub is_section: u32,
    pub key: Text,
    /// Empty for a section.
    pub entity: EntityView,
    /// The provider of the entity the node presents (a ref's target) is stale.
    pub stale: u32,
    pub label: Text,
    pub layout: Text,
    pub form: Text,
    /// Opaque loop invocation key: compare for equality, don't parse. Empty
    /// for a section.
    pub loop_key: Text,
    /// ANDAMENTO_CATALOG, _LATENT, _OPENING or _LIVE.
    pub state: u32,
    /// A LIVE node's workspace; zeroes otherwise.
    pub workspace_id: WorkspaceIdView,
    pub selected: u32,
    pub openable: u32,
    pub collapsed: u32,
    pub pinned: u32,
    /// A section's placement defaults: its region's default host (empty when
    /// absent) and order (when has_order).
    pub default_host: Text,
    pub has_order: u32,
    pub order: i64,
    pub first_field: usize,
    pub field_count: usize,
    pub first_detail: usize,
    pub detail_count: usize,
    pub first_control: usize,
    pub control_count: usize,
    pub activate: usize,
    pub toggle: usize,
    /// Copy the subject's URL; NONE when it has none.
    pub copy_url: usize,
}
/// A rendered field: ANDAMENTO_FIELD_REQUIRED, _OPTIONAL or _PRIORITY.
#[repr(C)]
pub struct FieldView {
    pub text: Text,
    pub class_: u32,
    pub has_priority: u32,
    pub priority: i64,
}
/// A control (ANDAMENTO_CONTROL_*). One without a core action has action
/// ANDAMENTO_NONE: its intent (such as scrolling) is the host's.
#[repr(C)]
pub struct ControlView {
    pub kind: u32,
    pub label: Text,
    pub glyph: Text,
    pub action: usize,
    /// ANDAMENTO_VALUE_NONE, _BOOL (checked) or _ENUM (value).
    pub value_kind: u32,
    pub checked: u32,
    pub value: Text,
    /// A display-variable control's variable name (empty otherwise), and
    /// whether its declaration persists it.
    pub variable: Text,
    pub persist: u32,
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_node_count(s: *const AndamentoSnapshot) -> usize {
    s.as_ref().map_or(0, |s| s.nodes.len())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_node(s: *const AndamentoSnapshot, index: usize, out: *mut NodeView) -> u32 {
    let (Some(n), Some(out)) = (s.as_ref().and_then(|s| s.nodes.get(index)), out.as_mut()) else {
        return 0;
    };
    *out = NodeView {
        parent: n.parent,
        is_section: n.section as u32,
        key: Text::borrowed(&n.key),
        entity: EntityView::of(&n.entity),
        stale: n.stale as u32,
        label: Text::borrowed(&n.label),
        layout: Text::borrowed(&n.layout),
        form: Text::borrowed(&n.form),
        loop_key: Text::borrowed(&n.loop_key),
        state: n.state,
        workspace_id: workspace_view(n.workspace),
        selected: n.selected as u32,
        openable: n.openable as u32,
        collapsed: n.collapsed as u32,
        pinned: n.pinned as u32,
        default_host: Text::borrowed(&n.default_host),
        has_order: n.order.is_some() as u32,
        order: n.order.unwrap_or_default(),
        first_field: n.fields,
        field_count: n.field_count,
        first_detail: n.details,
        detail_count: n.detail_count,
        first_control: n.controls,
        control_count: n.control_count,
        activate: n.activate,
        toggle: n.toggle,
        copy_url: n.copy_url,
    };
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_field(s: *const AndamentoSnapshot, index: usize, out: *mut FieldView) -> u32 {
    let (Some(f), Some(out)) = (s.as_ref().and_then(|s| s.fields.get(index)), out.as_mut()) else {
        return 0;
    };
    *out = FieldView {
        text: Text::borrowed(&f.text),
        class_: f.class_,
        has_priority: f.priority.is_some() as u32,
        priority: f.priority.unwrap_or_default(),
    };
    1
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_control(s: *const AndamentoSnapshot, index: usize, out: *mut ControlView) -> u32 {
    let (Some(c), Some(out)) = (s.as_ref().and_then(|s| s.controls.get(index)), out.as_mut()) else {
        return 0;
    };
    *out = ControlView {
        kind: c.kind,
        label: Text::borrowed(&c.label),
        glyph: Text::borrowed(&c.glyph),
        action: c.action,
        value_kind: c.value_kind,
        checked: c.checked as u32,
        value: Text::borrowed(&c.value),
        variable: Text::borrowed(if c.kind == ANDAMENTO_CONTROL_DISPLAY_VARIABLE { &c.variable } else { "" }),
        persist: (c.kind == ANDAMENTO_CONTROL_DISPLAY_VARIABLE && c.persist) as u32,
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_diagnostic_count(s: *const AndamentoSnapshot) -> usize {
    s.as_ref().map_or(0, |s| s.diagnostics.len())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_snapshot_diagnostic(s: *const AndamentoSnapshot, index: usize, out: *mut Text) -> u32 {
    let (Some(message), Some(out)) = (s.as_ref().and_then(|s| s.diagnostics.get(index)), out.as_mut()) else {
        return 0;
    };
    *out = Text::borrowed(message);
    1
}
/// Dispatch a snapshot's action. Actions belong to one snapshot: another
/// sidebar's, or one from before a revision-changing mutation, is rejected.
/// Dispatch a click against the snapshot it was drawn from before draining
/// queued facts; if an update intervened, discard it and redraw. Dispatch
/// queues effects; it runs nothing.
#[no_mangle]
pub unsafe extern "C" fn andamento_dispatch(
    h: *mut Andamento,
    s: *const AndamentoSnapshot,
    action: usize,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let s = s.as_ref().ok_or("null snapshot")?;
        if s.client != h.client {
            return Err("snapshot belongs to another sidebar".into());
        }
        if s.generation != h.sidebar.revision() {
            return Err("stale snapshot action; acquire a new snapshot".into());
        }
        let action = s.actions.get(action).ok_or("invalid action reference")?.clone();
        h.effects.extend(h.sidebar.dispatch(action)?);
        Ok(())
    })
    .is_some() as u32
}

/// A batch of effects, taken once. It owns its text and survives the
/// sidebar; releasing it does not complete its requests.
pub struct AndamentoEffects {
    effects: Vec<HostEffect>,
}
#[repr(C)]
pub struct EffectView {
    /// ANDAMENTO_EFFECT_*.
    pub kind: u32,
    pub request_id: u64,
    /// A FOCUS effect's workspace; zeroes otherwise.
    pub workspace_id: WorkspaceIdView,
    /// A MATERIALIZE or INSPECT effect's entity; empty otherwise.
    pub entity: EntityView,
    pub name: Text,
    /// A MATERIALIZE effect's recipe, or an OPEN_URL or COPY_URL effect's URL.
    pub recipe: Text,
    pub has_cwd: u32,
    pub cwd: Text,
    /// The managed-content target a MATERIALIZE effect's recipe resolves.
    /// Record it as the applied target, so the first plan of the primary
    /// slot sees the new content as current.
    pub has_primary_target: u32,
    pub primary_target: Text,
}
/// Drain the effect queue; an empty queue returns an empty batch.
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_take(h: *mut Andamento, error_out: *mut *mut c_char) -> *mut AndamentoEffects {
    run(h, error_out, |h| Ok(Box::into_raw(Box::new(AndamentoEffects { effects: std::mem::take(&mut h.effects) }))))
        .unwrap_or(ptr::null_mut())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_count(e: *const AndamentoEffects) -> usize {
    e.as_ref().map_or(0, |e| e.effects.len())
}
#[no_mangle]
pub unsafe extern "C" fn andamento_effects_get(e: *const AndamentoEffects, index: usize, out: *mut EffectView) -> u32 {
    let (Some(e), Some(out)) = (e.as_ref().and_then(|e| e.effects.get(index)), out.as_mut()) else {
        return 0;
    };
    let mut v = EffectView {
        kind: ANDAMENTO_EFFECT_FOCUS,
        request_id: 0,
        workspace_id: workspace_view(None),
        entity: EntityView::empty(),
        name: Text::borrowed(""),
        recipe: Text::borrowed(""),
        has_cwd: 0,
        cwd: Text::borrowed(""),
        has_primary_target: 0,
        primary_target: Text::borrowed(""),
    };
    match e {
        HostEffect::Focus { request_id, workspace_id } => {
            v.request_id = *request_id;
            v.workspace_id = (*workspace_id).into();
        }
        HostEffect::Materialize { request_id, entity, name, recipe, cwd, primary_target } => {
            v.kind = ANDAMENTO_EFFECT_MATERIALIZE;
            v.request_id = *request_id;
            v.entity = EntityView::of(entity);
            v.name = Text::borrowed(name);
            v.recipe = Text::borrowed(recipe);
            v.has_cwd = cwd.is_some() as u32;
            v.cwd = Text::borrowed(cwd.as_deref().unwrap_or_default());
            v.has_primary_target = primary_target.is_some() as u32;
            v.primary_target = Text::borrowed(primary_target.as_deref().unwrap_or_default());
        }
        HostEffect::OpenUrl { url } | HostEffect::CopyUrl { url } => {
            v.kind = if matches!(e, HostEffect::OpenUrl { .. }) { ANDAMENTO_EFFECT_OPEN_URL } else { ANDAMENTO_EFFECT_COPY_URL };
            v.recipe = Text::borrowed(url);
        }
        HostEffect::Inspect { entity } => {
            v.kind = ANDAMENTO_EFFECT_INSPECT;
            v.entity = EntityView::of(entity);
        }
    }
    *out = v;
    1
}
/// Queue a COPY_URL effect for a subject's current forge URL.
#[no_mangle]
pub unsafe extern "C" fn andamento_copy_subject_url(h: *mut Andamento, entity: EntityView, error_out: *mut *mut c_char) -> u32 {
    run(h, error_out, |h| {
        let effects = h.sidebar.dispatch(Action::CopySubjectUrl { entity: entity.read()? })?;
        h.effects.extend(effects);
        Ok(())
    })
    .is_some() as u32
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

#[cfg(test)]
mod tests;
