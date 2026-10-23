//! A subject's tool strip (docs/sidebar-design/tool-strip.md): the related
//! workspaces and linked items its kind's `strip` lists. Each entry is an
//! entity; its label, state, hover card and actions are its detail
//! (andamento_snapshot_detail_request).
use super::*;
use andamento_core::tool_strip::{StripEntry, StripSection};

/// The workspaces next to the subject's: ancestors, then children.
pub const ANDAMENTO_STRIP_RELATED: u32 = 0;
/// Items the subject links to, such as its change requests and issues.
pub const ANDAMENTO_STRIP_LINKED: u32 = 1;

#[repr(C)]
pub struct AndamentoStripEntry {
    /// ANDAMENTO_STRIP_*.
    pub section: u32,
    pub entity: EntityView,
}

/// A subject's strip, in section order, then in the order its loops list
/// them.
#[repr(C)]
pub struct AndamentoStrip {
    pub entries: *const AndamentoStripEntry,
    pub count: usize,
}
type StripKeep = (Vec<StripEntry>, Vec<AndamentoStripEntry>);

/// The strip of subject's workspace, as the sidebar's current facts give it:
/// empty for a subject not in the catalog or a kind without a strip. It
/// follows the catalog: read it again when the snapshot's revision moves.
/// Free it with andamento_strip_free.
#[no_mangle]
pub unsafe extern "C" fn andamento_strip_get(h: *mut Andamento, subject: EntityView, error_out: *mut *mut c_char) -> *mut AndamentoStrip {
    run(h, error_out, |h| {
        let subject = subject.read()?;
        let listed = h.sidebar.strip(&subject);
        let entries: Vec<AndamentoStripEntry> = listed
            .iter()
            .map(|entry| AndamentoStripEntry {
                section: match entry.section {
                    StripSection::Related => ANDAMENTO_STRIP_RELATED,
                    StripSection::Linked => ANDAMENTO_STRIP_LINKED,
                },
                entity: EntityView::of(&entry.entity),
            })
            .collect();
        let value = AndamentoStrip { entries: entries.as_ptr(), count: entries.len() };
        Ok(give(value, (listed, entries)))
    })
    .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn andamento_strip_free(strip: *mut AndamentoStrip) {
    take::<_, StripKeep>(strip)
}
