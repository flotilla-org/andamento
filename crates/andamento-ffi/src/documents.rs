//! Arrangement documents: a workspace's and the sidebar's.
//!
//! A document Andamento returns is one allocation the host frees with its
//! `_free` call: every pointer in it, and all its text, stays valid until
//! then. A document the host passes in is only read, during the call.
use super::*;
use andamento_core::{
    slots::{ArrangementDoc, ArrangementError, ArrangementView, Panel, PanelNode},
    suggested_layout::Axis,
};

/// A split panel: its children divide it along `axis`.
pub const ANDAMENTO_PANEL_SPLIT: u32 = 0;
/// A tab panel: its tabs are slot keys.
pub const ANDAMENTO_PANEL_TABS: u32 = 1;
/// Children left to right.
pub const ANDAMENTO_AXIS_ROW: u32 = 0;
/// Children top to bottom.
pub const ANDAMENTO_AXIS_COLUMN: u32 = 1;

/// A panel of an arrangement document and, for a split, its children.
#[repr(C)]
pub struct AndamentoDocPanel {
    /// Stable within its document; nonempty and unique.
    pub id: Text,
    /// Relative to its siblings: finite and positive. The root's is kept but
    /// means nothing.
    pub weight: f64,
    /// ANDAMENTO_PANEL_SPLIT or ANDAMENTO_PANEL_TABS.
    pub kind: u32,
    /// A split's ANDAMENTO_AXIS_*.
    pub axis: u32,
    /// A split's panels, in order; at least one.
    pub children: *const AndamentoDocPanel,
    pub child_count: usize,
    /// A tab panel's slot keys, in tab order; at least one.
    pub tabs: *const Text,
    pub tab_count: usize,
    /// The Selected View: one of `tabs`, or empty for none.
    pub selected: Text,
}

/// A whole arrangement document: a tree of panels.
#[repr(C)]
pub struct AndamentoArrangementDoc {
    /// NULL for a document with no panels.
    pub root: *const AndamentoDocPanel,
}

/// A note's key is a slot Andamento placed since the host last committed.
pub const ANDAMENTO_NOTE_PLACED: u32 = 0;
/// A note's key is a tab whose slot has gone; it stays until the host
/// removes it.
pub const ANDAMENTO_NOTE_GONE: u32 = 1;
/// A note's key is the panel ID of a soft override that no longer resolves.
pub const ANDAMENTO_NOTE_UNRESOLVED: u32 = 2;

#[repr(C)]
pub struct AndamentoNote {
    pub kind: u32,
    pub key: Text,
}

/// The provider changed its hint while the user owns the document.
pub const ANDAMENTO_ARRANGEMENT_FLAG_PROVIDER_CHANGED: u32 = 1;
/// Soft overrides apply.
pub const ANDAMENTO_ARRANGEMENT_FLAG_SOFT: u32 = 2;
/// A soft override no longer resolves (see ANDAMENTO_NOTE_UNRESOLVED).
pub const ANDAMENTO_ARRANGEMENT_FLAG_UNRESOLVED: u32 = 4;

/// A workspace's arrangement as stored, or after a commit.
#[repr(C)]
pub struct AndamentoWorkspaceArrangement {
    /// ANDAMENTO_ANDAMENTO_COMMITTED, or after a commit
    /// ANDAMENTO_ANDAMENTO_STALE: the expected generation was not the
    /// current one, nothing changed, and this is the current document.
    pub status: u32,
    /// 0 until a document is stored. Never reused, and never kept across
    /// sessions: commit at the generation last read.
    pub generation: u64,
    /// The user owns the document; until then it follows the provider's hint.
    pub owned: u32,
    /// ANDAMENTO_ARRANGEMENT_FLAG_* bits.
    pub flags: u32,
    pub doc: AndamentoArrangementDoc,
    pub notes: *const AndamentoNote,
    pub note_count: usize,
}

/// What the C struct points into; freed with it. Text points into the
/// view's strings, whose bytes don't move when the view does.
#[repr(C)]
struct OwnedArrangement {
    value: AndamentoWorkspaceArrangement,
    view: ArrangementView,
    keep: Keep,
}

#[derive(Default)]
struct Keep {
    panels: Vec<Vec<AndamentoDocPanel>>,
    tabs: Vec<Vec<Text>>,
    notes: Vec<AndamentoNote>,
    root: Option<Box<AndamentoDocPanel>>,
}

fn panel_c(panel: &Panel, keep: &mut Keep) -> AndamentoDocPanel {
    let mut c = AndamentoDocPanel {
        id: Text::borrowed(&panel.id),
        weight: panel.weight,
        kind: ANDAMENTO_PANEL_TABS,
        axis: ANDAMENTO_AXIS_ROW,
        children: ptr::null(),
        child_count: 0,
        tabs: ptr::null(),
        tab_count: 0,
        selected: Text::borrowed(""),
    };
    match &panel.node {
        PanelNode::Split { axis, children } => {
            let children: Vec<_> = children.iter().map(|child| panel_c(child, keep)).collect();
            c.kind = ANDAMENTO_PANEL_SPLIT;
            c.axis = match axis {
                Axis::Row => ANDAMENTO_AXIS_ROW,
                Axis::Column => ANDAMENTO_AXIS_COLUMN,
            };
            (c.children, c.child_count) = (children.as_ptr(), children.len());
            keep.panels.push(children);
        }
        PanelNode::Tabs { tabs, selected } => {
            let tabs: Vec<_> = tabs.iter().map(|tab| Text::borrowed(tab)).collect();
            (c.tabs, c.tab_count) = (tabs.as_ptr(), tabs.len());
            c.selected = Text::borrowed(selected.as_deref().unwrap_or(""));
            keep.tabs.push(tabs);
        }
    }
    c
}

fn arrangement_c(view: ArrangementView, status: u32) -> *mut AndamentoWorkspaceArrangement {
    let mut keep = Keep::default();
    keep.root = view
        .doc
        .root
        .as_ref()
        .map(|root| Box::new(panel_c(root, &mut keep)));
    let notes = [
        (ANDAMENTO_NOTE_PLACED, &view.placed),
        (ANDAMENTO_NOTE_GONE, &view.gone),
        (ANDAMENTO_NOTE_UNRESOLVED, &view.unresolved),
    ];
    for (kind, keys) in notes {
        keep.notes.extend(keys.iter().map(|key| AndamentoNote {
            kind,
            key: Text::borrowed(key),
        }));
    }
    let flags = [
        (
            view.provider_changed,
            ANDAMENTO_ARRANGEMENT_FLAG_PROVIDER_CHANGED,
        ),
        (!view.soft.is_empty(), ANDAMENTO_ARRANGEMENT_FLAG_SOFT),
        (
            !view.unresolved.is_empty(),
            ANDAMENTO_ARRANGEMENT_FLAG_UNRESOLVED,
        ),
    ];
    let value = AndamentoWorkspaceArrangement {
        status,
        generation: view.generation,
        owned: view.owned as u32,
        flags: flags.iter().filter(|(on, _)| *on).map(|(_, bit)| bit).sum(),
        doc: AndamentoArrangementDoc {
            root: keep
                .root
                .as_deref()
                .map_or(ptr::null(), |root| root as *const _),
        },
        notes: keep.notes.as_ptr(),
        note_count: keep.notes.len(),
    };
    Box::into_raw(Box::new(OwnedArrangement { value, view, keep })).cast()
}

/// Deeper than any arrangement a person makes; it stops a cycle of pointers.
const MAX_DEPTH: usize = 64;

unsafe fn panel_from_c(panel: &AndamentoDocPanel, depth: usize) -> Result<Panel, String> {
    if depth > MAX_DEPTH {
        return Err("panels nest too deeply".into());
    }
    let node = match panel.kind {
        ANDAMENTO_PANEL_SPLIT if panel.tab_count == 0 => PanelNode::Split {
            axis: match panel.axis {
                ANDAMENTO_AXIS_ROW => Axis::Row,
                ANDAMENTO_AXIS_COLUMN => Axis::Column,
                other => return Err(format!("unknown axis {other}")),
            },
            children: slice(panel.children, panel.child_count)?
                .iter()
                .map(|child| panel_from_c(child, depth + 1))
                .collect::<Result<_, _>>()?,
        },
        ANDAMENTO_PANEL_TABS if panel.child_count == 0 => PanelNode::Tabs {
            tabs: slice(panel.tabs, panel.tab_count)?
                .iter()
                .map(|tab| tab.read())
                .collect::<Result<_, _>>()?,
            selected: Some(panel.selected.read()?).filter(|key| !key.is_empty()),
        },
        ANDAMENTO_PANEL_SPLIT | ANDAMENTO_PANEL_TABS => {
            return Err("a split has panels, and a tab panel tabs".into())
        }
        other => return Err(format!("unknown panel kind {other}")),
    };
    Ok(Panel {
        id: panel.id.read()?,
        weight: panel.weight,
        node,
    })
}

/// A workspace's arrangement. Free it with andamento_arrangement_free.
#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_get(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    error_out: *mut *mut c_char,
) -> *mut AndamentoWorkspaceArrangement {
    run(h, error_out, |h| {
        let view = h.sidebar.arrangement(workspace.into())?;
        Ok(arrangement_c(view, ANDAMENTO_COMMITTED))
    })
    .unwrap_or(ptr::null_mut())
}

/// Commit a workspace's whole arrangement if `expected_generation` is the
/// current one, and return the arrangement after: COMMITTED, reconciled
/// (slots with no tab placed, gone tabs noted), or STALE and unchanged.
/// NULL with an error for an invalid document; nothing changes. Free the
/// result with andamento_arrangement_free.
#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_commit(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    doc: *const AndamentoArrangementDoc,
    expected_generation: u64,
    error_out: *mut *mut c_char,
) -> *mut AndamentoWorkspaceArrangement {
    run(h, error_out, |h| {
        let doc = doc.as_ref().ok_or("null document")?;
        let doc = ArrangementDoc {
            root: match doc.root.as_ref() {
                Some(root) => Some(panel_from_c(root, 0)?),
                None => None,
            },
        };
        let workspace = workspace.into();
        let status = match h
            .sidebar
            .set_arrangement(workspace, doc, expected_generation)
        {
            Ok(_) => ANDAMENTO_COMMITTED,
            Err(ArrangementError::Stale { .. }) => ANDAMENTO_STALE,
            Err(error) => return Err(error.to_string()),
        };
        Ok(arrangement_c(h.sidebar.arrangement(workspace)?, status))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_free(
    arrangement: *mut AndamentoWorkspaceArrangement,
) {
    if !arrangement.is_null() {
        drop(Box::from_raw(arrangement.cast::<OwnedArrangement>()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        andamento_create, andamento_destroy, andamento_slot_set, andamento_workspace_register,
    };

    fn text(s: &str) -> Text {
        Text::borrowed(s)
    }

    unsafe fn read(text: Text) -> String {
        text.read().unwrap()
    }

    /// A document as nested Rust values, for comparing.
    unsafe fn tree(panel: &AndamentoDocPanel) -> String {
        let children = slice(panel.children, panel.child_count).unwrap();
        let tabs = slice(panel.tabs, panel.tab_count).unwrap();
        format!(
            "{}:{}[{}|{}]<{}>",
            read(panel.id),
            panel.weight,
            children
                .iter()
                .map(|c| tree(c))
                .collect::<Vec<_>>()
                .join(","),
            tabs.iter().map(|t| read(*t)).collect::<Vec<_>>().join(","),
            read(panel.selected)
        )
    }

    #[test]
    fn documents_round_trip_as_nested_structs() {
        unsafe {
            let config = include_str!("../../andamento-core/tests/fixtures/abbreviation.kdl");
            let h = andamento_create(config.as_ptr(), config.len(), ptr::null_mut());
            let ws = WorkspaceIdView { bytes: [7; 16] };
            assert_eq!(andamento_workspace_register(h, ws, ptr::null_mut()), 1);
            for key in ["u:1", "u:2", "u:3"] {
                let mut spec: crate::ViewSpecView = std::mem::zeroed();
                spec.content = 2;
                spec.url = text("https://example.com");
                assert_eq!(
                    andamento_slot_set(h, ws, text(key), &spec, 0, ptr::null_mut()),
                    1
                );
            }
            let tabs = [text("u:1"), text("u:2")];
            let leaf = |id, tabs: &[Text], selected| AndamentoDocPanel {
                id: text(id),
                weight: 0.5,
                kind: ANDAMENTO_PANEL_TABS,
                axis: 0,
                children: ptr::null(),
                child_count: 0,
                tabs: tabs.as_ptr(),
                tab_count: tabs.len(),
                selected: text(selected),
            };
            let split = |id, children: &[AndamentoDocPanel]| AndamentoDocPanel {
                kind: ANDAMENTO_PANEL_SPLIT,
                axis: ANDAMENTO_AXIS_COLUMN,
                children: children.as_ptr(),
                child_count: children.len(),
                ..leaf(id, &[], "")
            };
            // u:3 is left out: committing places it.
            let inner = [leaf("2", &tabs, "u:2")];
            let outer = [split("1", &inner)];
            let root = split("root", &outer);
            let doc = AndamentoArrangementDoc { root: &root };
            let committed = andamento_arrangement_commit(h, ws, &doc, 0, ptr::null_mut());
            let c = &*committed;
            assert_eq!((c.status, c.owned), (crate::ANDAMENTO_COMMITTED, 1));
            assert_eq!(
                tree(&*c.doc.root),
                "root:0.5[1:0.5[2:0.5[|u:1,u:2,u:3]<u:2>|]<>|]<>"
            );
            let notes = slice(c.notes, c.note_count).unwrap();
            assert_eq!(notes.len(), 1);
            assert_eq!(
                (notes[0].kind, read(notes[0].key)),
                (ANDAMENTO_NOTE_PLACED, "u:3".into())
            );
            // What get returns is what the commit returned.
            let got = andamento_arrangement_get(h, ws, ptr::null_mut());
            assert_eq!((*got).generation, c.generation);
            assert_eq!(tree(&*(*got).doc.root), tree(&*c.doc.root));
            // A commit at the old generation is stale and returns the current one.
            let stale = andamento_arrangement_commit(h, ws, &doc, 0, ptr::null_mut());
            assert_eq!((*stale).status, crate::ANDAMENTO_STALE);
            assert_eq!((*stale).generation, c.generation);
            // An unknown kind is invalid: NULL with an error.
            let bad = AndamentoDocPanel {
                kind: 9,
                ..leaf("x", &tabs, "")
            };
            let mut error = ptr::null_mut();
            let invalid = andamento_arrangement_commit(
                h,
                ws,
                &AndamentoArrangementDoc { root: &bad },
                c.generation,
                &mut error,
            );
            assert!(invalid.is_null() && !error.is_null());
            crate::andamento_string_free(error);
            for result in [committed, got, stale] {
                andamento_arrangement_free(result);
            }
            andamento_destroy(h);
        }
    }
}

// The sidebar arrangement as flat panel and tab arrays.
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
            parent if panels[parent].kind != ANDAMENTO_PANEL_SPLIT => {
                return Err(format!("panel {index}'s parent is not a split"))
            }
            _ => {}
        }
        let node = match view.kind {
            ANDAMENTO_PANEL_SPLIT => {
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
            ANDAMENTO_PANEL_TABS => {
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

/// The sidebar's arrangement, owned until release.
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
                kind: ANDAMENTO_PANEL_SPLIT,
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
                kind: ANDAMENTO_PANEL_TABS,
                axis: 0,
                first_tab: first,
                tab_count: tabs.len(),
                selected,
            });
        }
    }
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

/// Keep the user's arrangement over the provider's change.
pub const ANDAMENTO_ARRANGEMENT_KEEP: u32 = 0;
/// Follow the provider's arrangement again, dropping ownership and soft
/// overrides.
pub const ANDAMENTO_ARRANGEMENT_FOLLOW: u32 = 1;

/// keep the user's arrangement over a provider change, or follow the
/// provider's again.
#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_resolve(
    h: *mut Andamento,
    workspace: WorkspaceIdView,
    choice: u32,
    expected_generation: u64,
    generation_out: *mut u64,
    error_out: *mut *mut c_char,
) -> u32 {
    use andamento_core::slots::{ArrangementError, Resolve};
    run(h, error_out, |h| {
        let choice = match choice {
            ANDAMENTO_ARRANGEMENT_KEEP => Resolve::Keep,
            ANDAMENTO_ARRANGEMENT_FOLLOW => Resolve::Follow,
            other => return Err(format!("unknown arrangement choice {other}")),
        };
        let workspace = workspace.into();
        let result = h
            .sidebar
            .resolve_arrangement(workspace, choice, expected_generation);
        let current = h.sidebar.arrangement(workspace).map(|v| v.generation);
        if let (Some(out), Ok(current)) = (generation_out.as_mut(), current) {
            *out = current;
        }
        match result {
            Ok(_) => Ok(ANDAMENTO_COMMITTED),
            Err(ArrangementError::Stale { .. }) => Ok(ANDAMENTO_STALE),
            Err(error) => Err(error.to_string()),
        }
    })
    .unwrap_or(ANDAMENTO_INVALID)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_arrangement_floating_first(
    arrangement: *const AndamentoArrangement,
) -> usize {
    arrangement.as_ref().map_or(0, |a| a.floating_first)
}

/// A declared section with no tab, closed on purpose.
pub const ANDAMENTO_SECTION_CLOSED: u32 = 0;
/// A section Andamento placed by its hints.
pub const ANDAMENTO_SECTION_PLACED: u32 = 1;
/// A closed section that has a tab again.
pub const ANDAMENTO_SECTION_RESTORED: u32 = 2;
/// A key whose later tabs were removed.
pub const ANDAMENTO_SECTION_DUPLICATE: u32 = 3;
/// A key that no longer resolves: a tabbed or closed section, or a saved
/// dashboard key ("display:", "collapse:", "order:" or "variable:" and
/// the key). Kept until the host drops it.
pub const ANDAMENTO_SECTION_UNRESOLVED: u32 = 4;

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
        Ok(_) => Ok(ANDAMENTO_COMMITTED),
        Err(andamento_core::slots::ArrangementError::Stale { .. }) => Ok(ANDAMENTO_STALE),
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
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let doc = read_sidebar_doc(
            slice(panels, panel_count)?,
            floating_first,
            slice(tabs, tab_count)?,
        )?;
        let result = h.sidebar.set_sidebar_arrangement(doc, expected_generation);
        sidebar_result(h, result, generation_out)
    })
    .unwrap_or(ANDAMENTO_INVALID)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_restore_section(
    h: *mut Andamento,
    key: Text,
    expected_generation: u64,
    generation_out: *mut u64,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let key = key.read()?;
        let result = h.sidebar.restore_sidebar_section(&key, expected_generation);
        sidebar_result(h, result, generation_out)
    })
    .unwrap_or(ANDAMENTO_INVALID)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_reset(
    h: *mut Andamento,
    expected_generation: u64,
    generation_out: *mut u64,
    error_out: *mut *mut c_char,
) -> u32 {
    run(h, error_out, |h| {
        let result = h.sidebar.reset_sidebar_arrangement(expected_generation);
        sidebar_result(h, result, generation_out)
    })
    .unwrap_or(ANDAMENTO_INVALID)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_arrangement_generation(
    h: *mut Andamento,
    error_out: *mut *mut c_char,
) -> u64 {
    run(h, error_out, |h| {
        Ok(h.sidebar.sidebar_arrangement_generation())
    })
    .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn andamento_sidebar_arrangement_acquire(
    h: *mut Andamento,
    error_out: *mut *mut c_char,
) -> *mut AndamentoArrangement {
    run(h, error_out, |h| {
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
            (
                ANDAMENTO_SECTION_CLOSED,
                view.closed.iter().collect::<Vec<_>>(),
            ),
            (ANDAMENTO_SECTION_PLACED, report.placed.iter().collect()),
            (ANDAMENTO_SECTION_RESTORED, report.restored.iter().collect()),
            (
                ANDAMENTO_SECTION_DUPLICATE,
                report.duplicates.iter().collect(),
            ),
            (
                ANDAMENTO_SECTION_UNRESOLVED,
                view.unresolved.iter().collect(),
            ),
        ];
        for (kind, keys) in notes {
            out.notes
                .extend(keys.into_iter().map(|key| (kind, key.clone())));
        }
        Ok(Box::into_raw(Box::new(out)))
    })
    .unwrap_or(ptr::null_mut())
}

/// the revision of workspace content (slots, resolutions and
/// arrangements), separate from the sidebar snapshot's.
#[no_mangle]
pub unsafe extern "C" fn andamento_workspace_content_revision(
    h: *mut Andamento,
    error_out: *mut *mut c_char,
) -> u64 {
    run(h, error_out, |h| Ok(h.sidebar.content_revision())).unwrap_or(0)
}
