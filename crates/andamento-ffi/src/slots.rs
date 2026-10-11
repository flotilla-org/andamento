//! A workspace's slots, their plans, portable Target Resolutions and its
//! Workspace Overlay. Every call names a registered workspace.
use super::*;

/// A View Spec's content: a local command (argv when argc is nonzero, else
/// the shell line in command; optional cwd).
pub const ANDAMENTO_SLOT_COMMAND: u32 = 0;
/// A local file (path).
pub const ANDAMENTO_SLOT_FILE: u32 = 1;
/// A URL (url).
pub const ANDAMENTO_SLOT_URL: u32 = 2;
/// A Jackstay view (launcher and endpoint).
pub const ANDAMENTO_SLOT_JACKSTAY: u32 = 3;
/// A provider facet: the facet of entity's Suggested Layout.
pub const ANDAMENTO_SLOT_FACET: u32 = 4;

/// A rebind closes the instance it replaces.
pub const ANDAMENTO_REBIND_REPLACE: u32 = 0;
/// A rebind keeps the instance it replaces reachable (a plan's previous)
/// until andamento_slot_release_previous.
pub const ANDAMENTO_REBIND_KEEP_PREVIOUS: u32 = 1;
/// A rebind asks the user first; declining is completing the update
/// unsuccessfully.
pub const ANDAMENTO_REBIND_ASK: u32 = 2;

/// The user overrode the slot's content: it no longer follows the provider.
pub const ANDAMENTO_SLOT_FLAG_DETACHED: u32 = 1;
/// The provider's content for the key is not what the user's override or
/// tombstone was made against: an override still wins, and a tombstone no
/// longer hides the slot. Setting, removing or reattaching settles it.
pub const ANDAMENTO_SLOT_FLAG_CHANGED: u32 = 2;
/// The provider removed this overridden slot; it is kept as the user's own.
pub const ANDAMENTO_SLOT_FLAG_REMOVED: u32 = 4;
/// The user changed the slot's rebind policy.
pub const ANDAMENTO_SLOT_FLAG_REBIND: u32 = 8;

const _: () = {
    use andamento_core::slots::flag;
    assert!(ANDAMENTO_SLOT_FLAG_DETACHED == flag::DETACHED);
    assert!(ANDAMENTO_SLOT_FLAG_CHANGED == flag::CHANGED);
    assert!(ANDAMENTO_SLOT_FLAG_REMOVED == flag::REMOVED);
    assert!(ANDAMENTO_SLOT_FLAG_REBIND == flag::REBIND);
};

/// A View Spec, or a resolution's recipe (whose presentation is unset).
/// Fields its content kind doesn't use are ignored on input and empty on
/// output. presentation is an open vocabulary ("terminal", "web", ...).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ViewSpecView {
    pub content: u32,
    pub entity: EntityView,
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

impl ViewSpecView {
    fn empty() -> Self {
        Self {
            content: ANDAMENTO_SLOT_COMMAND,
            entity: EntityView::empty(),
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
            ANDAMENTO_SLOT_FACET => Content::ProviderFacet {
                entity: self.entity.read()?,
                facet: self.facet.read()?,
            },
            ANDAMENTO_SLOT_COMMAND => {
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
            ANDAMENTO_SLOT_FILE => Content::Local(LocalRecipe::File {
                path: self.path.read()?,
            }),
            ANDAMENTO_SLOT_URL => Content::Local(LocalRecipe::Url {
                url: self.url.read()?,
            }),
            ANDAMENTO_SLOT_JACKSTAY => Content::Local(LocalRecipe::Jackstay {
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
            out.content = ANDAMENTO_SLOT_COMMAND;
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
            out.content = ANDAMENTO_SLOT_FILE;
            out.path = Text::borrowed(path);
        }
        LocalRecipe::Url { url } => {
            out.content = ANDAMENTO_SLOT_URL;
            out.url = Text::borrowed(url);
        }
        LocalRecipe::Jackstay { launcher, endpoint } => {
            out.content = ANDAMENTO_SLOT_JACKSTAY;
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
            out.content = ANDAMENTO_SLOT_FACET;
            out.entity = EntityView::of(entity);
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
        RebindPolicy::Replace => ANDAMENTO_REBIND_REPLACE,
        RebindPolicy::KeepPrevious => ANDAMENTO_REBIND_KEEP_PREVIOUS,
        RebindPolicy::Ask => ANDAMENTO_REBIND_ASK,
    }
}

fn rebind_policy(code: u32) -> Result<andamento_core::suggested_layout::RebindPolicy, String> {
    use andamento_core::suggested_layout::RebindPolicy;
    Ok(match code {
        ANDAMENTO_REBIND_REPLACE => RebindPolicy::Replace,
        ANDAMENTO_REBIND_KEEP_PREVIOUS => RebindPolicy::KeepPrevious,
        ANDAMENTO_REBIND_ASK => RebindPolicy::Ask,
        other => return Err(format!("unknown rebind policy {other}")),
    })
}

/// One of a workspace's slots.
#[repr(C)]
pub struct SlotView {
    pub key: Text,
    pub spec: ViewSpecView,
    /// ANDAMENTO_REBIND_*.
    pub rebind: u32,
    /// The current baseline has this key.
    pub in_baseline: u32,
    /// The user overrode it, so it no longer follows the provider.
    pub detached: u32,
    /// ANDAMENTO_SLOT_FLAG_* bits.
    pub flags: u32,
}

/// A workspace's slots, owned until release: its subject's Suggested Layout
/// slots (a cached baseline), then detached slots the baseline has since
/// dropped, then the slots the host added ("u:<id>"), which is also their
/// default placement order.
pub struct AndamentoSlots {
    slots: Vec<andamento_core::slots::SlotInfo>,
    argv: Vec<Vec<Text>>,
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slots_acquire(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error_out: *mut *mut c_char,
) -> *mut AndamentoSlots {
    run(h, error_out, |h| {
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
        flags: slot.flags,
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slots_release(slots: *mut AndamentoSlots) {
    if !slots.is_null() {
        drop(Box::from_raw(slots));
    }
}

/// Add the user's own slot ("u:<id>", <id> 1 to 64 of [a-z0-9_-]; provider
/// keys never contain ':'), or edit a baseline slot: a View Spec other than
/// the baseline's overrides its content, detaching it; a rebind policy other
/// than the baseline's is a rebind edit, which doesn't. Setting the
/// baseline's own spec and policy drops both.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_set(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    spec: *const ViewSpecView,
    rebind: u32,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let spec = spec.as_ref().ok_or("a slot needs a view spec")?.read()?;
        h.sidebar
            .set_slot(workspace.into(), &key.read()?, spec, rebind_policy(rebind)?)?;
        Ok(())
    })
    .is_some() as u32
}

/// Remove the user's own slot or a detached slot the baseline dropped, or
/// give a baseline slot a tombstone, hiding it. An unknown key is harmless.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_remove(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.remove_slot(workspace.into(), &key.read()?)?;
        Ok(())
    })
    .is_some() as u32
}

/// Drop a baseline slot's override or tombstone, so it follows the provider
/// again.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_reattach(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.reattach_slot(workspace.into(), &key.read()?)?;
        Ok(())
    })
    .is_some() as u32
}

/// Content is unavailable: missing or expired facts suspend updates; they
/// never delete the current content.
pub const ANDAMENTO_CONTENT_UNAVAILABLE: u32 = 0;
/// The provider holds the content: don't update it.
pub const ANDAMENTO_CONTENT_HELD: u32 = 1;
/// What the host applied is current.
pub const ANDAMENTO_CONTENT_CURRENT: u32 = 2;
/// The host should update to the plan's resolution.
pub const ANDAMENTO_CONTENT_UPDATING: u32 = 3;
/// The last update failed; it is retried when the resolution changes or on
/// andamento_slot_retry.
pub const ANDAMENTO_CONTENT_FAILED: u32 = 4;

/// One slot's plan, owned until release. Plan with the resolution identity
/// the host applied (empty for none), prepare the runtime instance without
/// touching the current one, validate the token immediately before
/// committing, then complete. Repeated plans return the same token until
/// the desired resolution or binding changes.
pub struct AndamentoSlotPlan {
    plan: andamento_core::managed::SlotPlan,
    argv: Vec<Text>,
    /// Views of `plan.saved`'s fields.
    fields: Vec<ResolutionFieldView>,
}

/// A slot's plan. While UPDATING, token, resolution (an opaque identity:
/// record it and pass it back as applied), target (the backing instance,
/// when the provider names one) and recipe (what to run) describe the
/// update.
#[repr(C)]
pub struct SlotContentView {
    /// ANDAMENTO_CONTENT_*.
    pub state: u32,
    pub token: u64,
    pub resolution: Text,
    pub has_target: u32,
    pub target: Text,
    pub recipe: ViewSpecView,
    /// ANDAMENTO_REBIND_*.
    pub rebind: u32,
    /// The resolution a KEEP_PREVIOUS rebind replaced, until
    /// andamento_slot_release_previous.
    pub has_previous: u32,
    pub previous: Text,
    /// The portable Target Resolution saved for the slot, in every state.
    /// While UPDATING to the resolution it was saved against, try
    /// connecting through it before resolving the target afresh.
    pub has_saved: u32,
    pub saved: ResolutionView,
}

/// Plan a slot. A provider facet resolves from the subject's layout while
/// the slot follows it, else from the facet entity's own layout ("primary":
/// its workspace.primary.* facts); a local recipe resolves to itself.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_plan(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    applied: Text,
    error_out: *mut *mut c_char,
) -> *mut AndamentoSlotPlan {
    run(h, error_out, |h| {
        let plan = h
            .sidebar
            .plan_slot(workspace.into(), &key.read()?, &applied.read()?)?;
        let mut plan = Box::new(AndamentoSlotPlan {
            plan,
            argv: Vec::new(),
            fields: Vec::new(),
        });
        if let Some(saved) = &plan.plan.saved {
            plan.fields = field_views(saved);
        }
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
            ContentState::Unavailable => ANDAMENTO_CONTENT_UNAVAILABLE,
            ContentState::Held => ANDAMENTO_CONTENT_HELD,
            ContentState::Current => ANDAMENTO_CONTENT_CURRENT,
            ContentState::Updating => ANDAMENTO_CONTENT_UPDATING,
            ContentState::Failed => ANDAMENTO_CONTENT_FAILED,
        },
        token: 0,
        resolution: Text::borrowed(""),
        has_target: 0,
        target: Text::borrowed(""),
        recipe: ViewSpecView::empty(),
        rebind: rebind_code(content.rebind),
        has_previous: content.previous.is_some() as u32,
        previous: Text::borrowed(content.previous.as_deref().unwrap_or_default()),
        has_saved: content.saved.is_some() as u32,
        saved: resolution_view(content.saved.as_ref(), &plan.fields),
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

/// 1 if the plan's token is still valid: check immediately before
/// committing the update.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_valid(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    token: u64,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        Ok(h.sidebar
            .managed
            .slot_valid(workspace.into(), &key.read()?, token))
    })
    .unwrap_or(false) as u32
}

/// The host committed (success nonzero) or abandoned the update. Returns 1
/// if the token was current.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_complete(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    token: u64,
    success: u32,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        Ok(h.sidebar
            .managed
            .complete_slot(workspace.into(), &key.read()?, token, success != 0))
    })
    .unwrap_or(false) as u32
}

/// Plan a FAILED slot's update again.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_retry(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        h.sidebar.managed.retry_slot(workspace.into(), &key.read()?);
        Ok(())
    })
    .is_some() as u32
}

/// The host has dealt with the instance a rebind replaced: a KEEP_PREVIOUS
/// slot's previous instance, or the instance of a slot the provider removed
/// (a DEPARTED overlay note). Returns 1 if there was one.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_release_previous(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        h.sidebar
            .release_slot_previous(workspace.into(), &key.read()?)
    })
    .unwrap_or(false) as u32
}

/// One named field of a portable Target Resolution.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResolutionFieldView {
    pub name: Text,
    pub value: Text,
}

/// A portable Target Resolution saved for a slot: a way of connecting that
/// works from any device, as an opaque typed value (a kind and named text
/// fields, in name order; kind and names are 1 to 64 of [a-z0-9_-]). It was
/// made for the slot resolution whose identity is against, and is cleared
/// when the slot resolves to something else, its content is edited, or it
/// goes.
#[repr(C)]
pub struct ResolutionView {
    pub generation: u64,
    pub kind: Text,
    pub fields: *const ResolutionFieldView,
    pub field_count: usize,
    pub against: Text,
}

/// Views of a saved resolution's fields, borrowing its text: the owner keeps
/// both, and never changes the resolution while the views live.
fn field_views(
    saved: &andamento_core::target_resolution::SavedResolution,
) -> Vec<ResolutionFieldView> {
    saved
        .resolution
        .fields
        .iter()
        .map(|(name, value)| ResolutionFieldView {
            name: Text::borrowed(name),
            value: Text::borrowed(value),
        })
        .collect()
}

/// A saved resolution as C reads it; all empty for none.
fn resolution_view(
    saved: Option<&andamento_core::target_resolution::SavedResolution>,
    fields: &[ResolutionFieldView],
) -> ResolutionView {
    ResolutionView {
        generation: saved.map_or(0, |saved| saved.generation),
        kind: Text::borrowed(saved.map_or("", |saved| &saved.resolution.kind)),
        fields: if fields.is_empty() {
            ptr::null()
        } else {
            fields.as_ptr()
        },
        field_count: fields.len(),
        against: Text::borrowed(saved.map_or("", |saved| &saved.against)),
    }
}

/// Save a slot's portable Target Resolution in the workspace record, made
/// for the slot resolution the host applied (applied: that identity, as
/// passed to andamento_slot_plan). If the slot is known to resolve to
/// something else, it is INVALID: plan again. A compare-and-set on the
/// slot's saved generation (0 for none); saving what is saved keeps it.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_resolution_set(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    kind: Text,
    fields: *const ResolutionFieldView,
    field_count: usize,
    applied: Text,
    expected_generation: u64,
    generation_out: *mut u64,
    error_out: *mut *mut c_char,
) -> u32 {
    use andamento_core::target_resolution::{PortableResolution, ResolutionError};
    run(h, error_out, |h| {
        let key = key.read()?;
        let mut resolution = PortableResolution {
            kind: kind.read()?,
            fields: Default::default(),
        };
        for field in slice(fields, field_count)? {
            let name = field.name.read()?;
            if resolution
                .fields
                .insert(name.clone(), field.value.read()?)
                .is_some()
            {
                return Err(format!("resolution field {name:?} is given twice"));
            }
        }
        let workspace = workspace.into();
        let result = h.sidebar.set_slot_resolution(
            workspace,
            &key,
            resolution,
            &applied.read()?,
            expected_generation,
        );
        let current = match &result {
            Ok(generation) => Some(*generation),
            Err(ResolutionError::Stale { current }) => Some(*current),
            Err(ResolutionError::Invalid(_)) => None,
        };
        if let (Some(out), Some(current)) = (generation_out.as_mut(), current) {
            *out = current;
        }
        match result {
            Ok(_) => Ok(ANDAMENTO_COMMITTED),
            Err(ResolutionError::Stale { .. }) => Ok(ANDAMENTO_STALE),
            Err(error) => Err(error.to_string()),
        }
    })
    .unwrap_or(ANDAMENTO_INVALID)
}

/// Clear a slot's saved portable Target Resolution: a compare-and-set as
/// for andamento_slot_resolution_set (generation_out receives 0).
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_resolution_clear(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    expected_generation: u64,
    generation_out: *mut u64,
    error_out: *mut *mut c_char,
) -> u32 {
    use andamento_core::target_resolution::ResolutionError;
    run(h, error_out, |h| {
        let result =
            h.sidebar
                .clear_slot_resolution(workspace.into(), &key.read()?, expected_generation);
        let current = match &result {
            Ok(_) => Some(0),
            Err(ResolutionError::Stale { current }) => Some(*current),
            Err(ResolutionError::Invalid(_)) => None,
        };
        if let (Some(out), Some(current)) = (generation_out.as_mut(), current) {
            *out = current;
        }
        match result {
            Ok(_) => Ok(ANDAMENTO_COMMITTED),
            Err(ResolutionError::Stale { .. }) => Ok(ANDAMENTO_STALE),
            Err(error) => Err(error.to_string()),
        }
    })
    .unwrap_or(ANDAMENTO_INVALID)
}

/// A slot's saved portable Target Resolution, owned until release.
/// Acquiring fails for an unknown slot.
pub struct AndamentoSlotResolution {
    saved: Option<andamento_core::target_resolution::SavedResolution>,
    fields: Vec<ResolutionFieldView>,
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_resolution_acquire(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    key: Text,
    error_out: *mut *mut c_char,
) -> *mut AndamentoSlotResolution {
    run(h, error_out, |h| {
        let saved = h.sidebar.slot_resolution(workspace.into(), &key.read()?)?;
        let fields = saved.as_ref().map(field_views).unwrap_or_default();
        Ok(Box::into_raw(Box::new(AndamentoSlotResolution {
            saved,
            fields,
        })))
    })
    .unwrap_or(ptr::null_mut())
}

/// Fill out with the saved resolution; returns 0 when there is none.
#[no_mangle]
pub unsafe extern "C" fn andamento_slot_resolution_get(
    resolution: *const AndamentoSlotResolution,
    out: *mut ResolutionView,
) -> u32 {
    let (Some(resolution), Some(out)) = (resolution.as_ref(), out.as_mut()) else {
        return 0;
    };
    if resolution.saved.is_none() {
        return 0;
    }
    *out = resolution_view(resolution.saved.as_ref(), &resolution.fields);
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_slot_resolution_release(
    resolution: *mut AndamentoSlotResolution,
) {
    if !resolution.is_null() {
        drop(Box::from_raw(resolution));
    }
}

/// An optional text argument: `has` says whether it is set.
unsafe fn optional_text(has: u32, text: Text) -> Result<Option<String>, String> {
    Ok(if has != 0 { Some(text.read()?) } else { None })
}

/// Set the user's name for a workspace over the provider's (has_name 0
/// drops it).
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_set_name(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    has_name: u32,
    name: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let name = optional_text(has_name, name)?;
        h.sidebar.set_workspace_name(workspace.into(), name)?;
        Ok(())
    })
    .is_some() as u32
}

/// Set the user's mood for a workspace over the provider's (has_mood 0
/// drops it).
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_set_mood(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    has_mood: u32,
    mood: Text,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let mood = optional_text(has_mood, mood)?;
        h.sidebar.set_workspace_mood(workspace.into(), mood)?;
        Ok(())
    })
    .is_some() as u32
}

/// A workspace's overlay, owned until release.
pub struct AndamentoOverlay {
    baseline: Option<andamento_core::suggested_layout::BaselineVersion>,
    name: Option<String>,
    mood: Option<String>,
    owned: bool,
    notes: Vec<(u32, String, u32)>,
}

/// The Workspace Overlay's baseline version (has_baseline 0: no Suggested
/// Layout; primary_only: only the primary facts), the user's name and mood,
/// whether it owns the arrangement, and its note count.
#[repr(C)]
pub struct OverlayInfoView {
    pub has_baseline: u32,
    pub primary_only: u32,
    pub baseline_version: Text,
    pub has_name: u32,
    pub name: Text,
    pub has_mood: u32,
    pub mood: Text,
    pub owned: u32,
    pub note_count: usize,
}

/// An overlay note: ANDAMENTO_OVERLAY_TOMBSTONED or _DEPARTED.
#[repr(C)]
pub struct OverlayNoteView {
    pub kind: u32,
    pub key: Text,
    pub rebind: u32,
}

/// A baseline slot the user removed, hidden while the provider's content is
/// what it was removed against; reattach it to show it again.
pub const ANDAMENTO_OVERLAY_TOMBSTONED: u32 = 0;
/// An untouched slot the provider removed: rebind is the policy its live
/// instance follows, until andamento_slot_release_previous.
pub const ANDAMENTO_OVERLAY_DEPARTED: u32 = 1;

#[no_mangle]
pub unsafe extern "C" fn andamento_overlay_acquire(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error_out: *mut *mut c_char,
) -> *mut AndamentoOverlay {
    run(h, error_out, |h| {
        let view = h.sidebar.overlay(workspace.into())?;
        let mut notes: Vec<(u32, String, u32)> = view
            .tombstoned
            .iter()
            .map(|key| (ANDAMENTO_OVERLAY_TOMBSTONED, key.clone(), 0))
            .collect();
        notes.extend(view.departed.iter().map(|(key, rebind)| {
            (
                ANDAMENTO_OVERLAY_DEPARTED,
                key.clone(),
                rebind_code(*rebind),
            )
        }));
        Ok(Box::into_raw(Box::new(AndamentoOverlay {
            baseline: view.baseline,
            name: view.name,
            mood: view.mood,
            owned: view.owned,
            notes,
        })))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_overlay_info(
    overlay: *const AndamentoOverlay,
    out: *mut OverlayInfoView,
) -> u32 {
    use andamento_core::suggested_layout::BaselineVersion;
    let (Some(overlay), Some(out)) = (overlay.as_ref(), out.as_mut()) else {
        return 0;
    };
    let version = match &overlay.baseline {
        Some(BaselineVersion::Published(version)) => version.as_str(),
        _ => "",
    };
    *out = OverlayInfoView {
        has_baseline: overlay.baseline.is_some() as u32,
        primary_only: matches!(overlay.baseline, Some(BaselineVersion::PrimaryOnly)) as u32,
        baseline_version: Text::borrowed(version),
        has_name: overlay.name.is_some() as u32,
        name: Text::borrowed(overlay.name.as_deref().unwrap_or_default()),
        has_mood: overlay.mood.is_some() as u32,
        mood: Text::borrowed(overlay.mood.as_deref().unwrap_or_default()),
        owned: overlay.owned as u32,
        note_count: overlay.notes.len(),
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_overlay_note(
    overlay: *const AndamentoOverlay,
    index: usize,
    out: *mut OverlayNoteView,
) -> u32 {
    let (Some(overlay), Some(out)) = (overlay.as_ref(), out.as_mut()) else {
        return 0;
    };
    let Some((kind, key, rebind)) = overlay.notes.get(index) else {
        return 0;
    };
    *out = OverlayNoteView {
        kind: *kind,
        key: Text::borrowed(key),
        rebind: *rebind,
    };
    1
}

#[no_mangle]
pub unsafe extern "C" fn andamento_overlay_release(overlay: *mut AndamentoOverlay) {
    if !overlay.is_null() {
        drop(Box::from_raw(overlay));
    }
}
