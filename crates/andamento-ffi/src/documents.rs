//! Documents and records as plain C structs. include/andamento_documents.h is
//! generated from this file by cbindgen (scripts/generate-header.sh); edit
//! this file, not the header.
//!
//! A document Andamento returns is one allocation the host frees with its
//! `_free` call: every pointer in it, and all its text, stays valid until
//! then. A document the host passes in is only read, during the call.
use super::{run, slice, Andamento, Bytes, Text, WorkspaceIdView};
use andamento_core::{
    slots::{ArrangementDoc, ArrangementError, ArrangementView, Panel, PanelNode},
    suggested_layout::Axis,
};
use std::{ffi::c_char, ptr};

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
    /// ANDAMENTO_ARRANGEMENT_COMMITTED, or after a commit
    /// ANDAMENTO_ARRANGEMENT_STALE: the expected generation was not the
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
    error: *mut *mut c_char,
) -> *mut AndamentoWorkspaceArrangement {
    run(h, error, |h| {
        let view = h.sidebar.arrangement(workspace.into())?;
        Ok(arrangement_c(view, super::ARRANGEMENT_COMMITTED))
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
    error: *mut *mut c_char,
) -> *mut AndamentoWorkspaceArrangement {
    run(h, error, |h| {
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
            Ok(_) => super::ARRANGEMENT_COMMITTED,
            Err(ArrangementError::Stale { .. }) => super::ARRANGEMENT_STALE,
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

/// A record's generation, nonzero: it changes whenever the record does and
/// is never reused. 0 with an error for an unknown record.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_generation(
    h: *mut Andamento,
    name: Text,
    error: *mut *mut c_char,
) -> u64 {
    run(h, error, |h| h.sidebar.record_generation(&name.read()?)).unwrap_or(0)
}

/// A record to store as it is: its bytes are private to Andamento. Free
/// them with andamento_bytes_free.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_get(
    h: *mut Andamento,
    name: Text,
    out: *mut Bytes,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        let out = out.as_mut().ok_or("null output")?;
        *out = super::owned_bytes(h.sidebar.export_record(&name.read()?)?.into_bytes());
        Ok(())
    })
    .is_some() as u32
}

/// Restore a record andamento_record_get returned, before the first
/// observe. Rejected without change if it isn't one. Restoring
/// `workspace/<id>` registers the workspace.
#[no_mangle]
pub unsafe extern "C" fn andamento_record_put(
    h: *mut Andamento,
    name: Text,
    record: Text,
    error: *mut *mut c_char,
) -> u32 {
    run(h, error, |h| {
        h.sidebar.import_record(&name.read()?, &record.read()?)
    })
    .is_some() as u32
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
            assert_eq!((c.status, c.owned), (crate::ARRANGEMENT_COMMITTED, 1));
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
            assert_eq!((*stale).status, crate::ARRANGEMENT_STALE);
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
