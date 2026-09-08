//! Annotations, crossing C.
//!
//! The mutation half of the reading model's marks: turning the selection
//! into a highlight, a note or a bookmark; listing what a book carries;
//! jumping to one; recoloring and removing. The *geometry* half was
//! already here — `cb_session_range_rects` draws a stored highlight's
//! rects the same way it draws the live selection's — so this module is
//! what lets a shell that is not Rust close the loop a touch reader
//! runs: long-press, drag the handles, mark, and manage the marks later.
//!
//! Everything here persists through the library, so in a build without
//! one (`cb_capabilities()` lacks `CB_CAP_LIBRARY`) every entry point
//! declines with `CB_ERR_FORMAT_NOT_BUILT`: a mark nothing would
//! remember is not a mark, and pretending otherwise is how a reader
//! loses a highlight silently.
//!
//! Colors cross as the CSS hex strings the engine speaks —
//! `"#rrggbb"` or `"#rrggbbaa"` — and null means the theme's own
//! highlight color, which is also what a mark synced from a device that
//! never chose one shows.

use std::ffi::c_char;

use crate::error::{cb_status, fail, guard};
use crate::session::cb_session;

#[cfg(feature = "library")]
use crate::abi::{str_in, str_out};
#[cfg(feature = "library")]
use crate::error::clear_last_error;

/// What kind of mark a row is.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_annotation_kind {
    /// A point remembered, nothing painted.
    CB_ANNOTATION_BOOKMARK = 0,
    /// A range painted on the page.
    CB_ANNOTATION_HIGHLIGHT = 1,
    /// A range with words attached.
    CB_ANNOTATION_NOTE = 2,
}

/// One row of [`cb_session_annotation`] — plain data; the two strings
/// travel on their own calls.
#[repr(C)]
pub struct cb_annotation {
    /// The id every mutating call takes. Stable for the mark's life,
    /// including across sync.
    pub id: i64,
    pub kind: cb_annotation_kind,
    /// The spine unit the mark resolves against in this book.
    pub spine: usize,
    /// Whole-book progression of its start, 0.0..=1.0 — what orders a
    /// marks list and places its gutter dots.
    pub progression: f64,
    /// Whether [`cb_session_annotation_text`] has anything for this row.
    pub has_text: bool,
    /// Whether [`cb_session_annotation_color`] does.
    pub has_color: bool,
}

macro_rules! with_marks {
    (($($unused:ident),* $(,)?) $body:block) => {{
        #[cfg(feature = "library")]
        $body
        #[cfg(not(feature = "library"))]
        {
            $(let _ = $unused;)*
            fail(
                cb_status::CB_ERR_FORMAT_NOT_BUILT,
                "this build has no library, and a mark nothing would remember \
                 is not a mark",
            )
        }
    }};
}

/// Write an out-parameter, refusing a null destination.
#[cfg(feature = "library")]
macro_rules! out {
    ($ptr:expr, $value:expr, $what:literal) => {{
        if $ptr.is_null() {
            return fail(
                cb_status::CB_ERR_NULL_ARGUMENT,
                concat!($what, " out-pointer is null"),
            );
        }
        // SAFETY: checked non-null just above.
        unsafe { *$ptr = $value };
    }};
}

#[cfg(feature = "library")]
macro_rules! session_mut {
    ($session:expr) => {
        // SAFETY: a handle from an open call, not yet closed.
        match unsafe { ($session as *mut cb_session).as_mut() } {
            Some(session) => session,
            None => return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null"),
        }
    };
}

/// Turn the live selection into a stored highlight, in the theme's
/// color until one is chosen. The caller still owns the selection;
/// clearing it afterwards is the shell's move, so the paint order —
/// highlight replaces selection — is explicit rather than implied.
/// `CB_ERR_UNAVAILABLE` with nothing selected.
#[no_mangle]
pub unsafe extern "C" fn cb_session_add_highlight(
    session: *mut cb_session,
    id: *mut i64,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, id) {
            clear_last_error();
            let session = session_mut!(session);
            let Some(added) = session.inner.add_highlight() else {
                return fail(cb_status::CB_ERR_UNAVAILABLE, "nothing selected to highlight");
            };
            out!(id, added, "id");
            cb_status::CB_OK
        })
    })
}

/// Turn the live selection into a note carrying `body`.
/// `CB_ERR_UNAVAILABLE` with nothing selected.
#[no_mangle]
pub unsafe extern "C" fn cb_session_add_note(
    session: *mut cb_session,
    body: *const c_char,
    id: *mut i64,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, body, id) {
            clear_last_error();
            let session = session_mut!(session);
            // SAFETY: the header's contract.
            let Some(body) = (unsafe { str_in(body, "body") }) else {
                return cb_status::CB_ERR_NULL_ARGUMENT;
            };
            let Some(added) = session.inner.add_note(body) else {
                return fail(cb_status::CB_ERR_UNAVAILABLE, "nothing selected to annotate");
            };
            out!(id, added, "id");
            cb_status::CB_OK
        })
    })
}

/// Bookmark the current position — a point, nothing painted.
#[no_mangle]
pub unsafe extern "C" fn cb_session_add_bookmark(
    session: *mut cb_session,
    id: *mut i64,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, id) {
            clear_last_error();
            let session = session_mut!(session);
            let Some(added) = session.inner.add_bookmark() else {
                return fail(cb_status::CB_ERR_UNAVAILABLE, "nowhere to bookmark yet");
            };
            out!(id, added, "id");
            cb_status::CB_OK
        })
    })
}

/// The highlight under a point, `CB_ERR_UNAVAILABLE` on a miss — what a
/// tap on marked text asks before a shell opens its recolor-or-remove
/// menu. Checked after links and before tap zones, per `docs/SHELLS.md`.
#[no_mangle]
pub unsafe extern "C" fn cb_session_highlight_at(
    session: *mut cb_session,
    x: f32,
    y: f32,
    id: *mut i64,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, x, y, id) {
            clear_last_error();
            let session = session_mut!(session);
            let Some(found) = session.inner.highlight_at(x, y) else {
                return fail(cb_status::CB_ERR_UNAVAILABLE, "no highlight under the point");
            };
            out!(id, found, "id");
            cb_status::CB_OK
        })
    })
}

/// Recolor a highlight — `"#rrggbb"` or `"#rrggbbaa"`, or null to give
/// the theme's color back.
#[no_mangle]
pub unsafe extern "C" fn cb_session_set_highlight_color(
    session: *mut cb_session,
    id: i64,
    color: *const c_char,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, id, color) {
            clear_last_error();
            let session = session_mut!(session);
            let color = if color.is_null() {
                None
            } else {
                // SAFETY: the header's contract.
                match unsafe { str_in(color, "color") } {
                    Some(color) => Some(color),
                    None => return cb_status::CB_ERR_INVALID_UTF8,
                }
            };
            session.inner.set_highlight_color(id, color);
            cb_status::CB_OK
        })
    })
}

/// Remove a mark, whatever its kind. The removal reaches the book's
/// annotation container on the next sync; nothing here is silent.
#[no_mangle]
pub unsafe extern "C" fn cb_session_remove_annotation(
    session: *mut cb_session,
    id: i64,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, id) {
            clear_last_error();
            let session = session_mut!(session);
            session.inner.remove_annotation(id);
            cb_status::CB_OK
        })
    })
}

/// Jump to a mark. `*moved` reports whether the reader went anywhere; a
/// jump pushes the return position for the `Back` action, same as a
/// followed link.
#[no_mangle]
pub unsafe extern "C" fn cb_session_goto_annotation(
    session: *mut cb_session,
    id: i64,
    moved: *mut bool,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, id, moved) {
            clear_last_error();
            let session = session_mut!(session);
            let did = session.inner.goto_annotation(id);
            out!(moved, did, "moved");
            cb_status::CB_OK
        })
    })
}

/// How many marks this book carries — the count for the per-index calls
/// below. The list is re-read from the library per call and ordered by
/// progression, so indices are stable between mutations and not across
/// them; re-enumerate after any add or remove.
#[no_mangle]
pub unsafe extern "C" fn cb_session_annotation_count(
    session: *const cb_session,
    count: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, count) {
            clear_last_error();
            // SAFETY: a handle from an open call, not yet closed.
            let Some(session) = (unsafe { session.as_ref() }) else {
                return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
            };
            out!(count, session.inner.annotations().len(), "count");
            cb_status::CB_OK
        })
    })
}

#[cfg(feature = "library")]
fn annotation_row(
    session: &cb_session,
    index: usize,
) -> Result<chapbook_reader::AnnotationSummary, cb_status> {
    let mut rows = session.inner.annotations();
    if index >= rows.len() {
        return Err(fail(
            cb_status::CB_ERR_INVALID_ARGUMENT,
            format!("annotation index {index} out of {}", rows.len()),
        ));
    }
    Ok(rows.swap_remove(index))
}

/// One mark's plain data, by index. Its strings travel on
/// [`cb_session_annotation_text`] and [`cb_session_annotation_color`].
#[no_mangle]
pub unsafe extern "C" fn cb_session_annotation(
    session: *const cb_session,
    index: usize,
    out: *mut cb_annotation,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, index, out) {
            use chapbook_reader::chapbook_library::AnnotationKind;
            clear_last_error();
            // SAFETY: a handle from an open call, not yet closed.
            let Some(session) = (unsafe { session.as_ref() }) else {
                return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
            };
            if out.is_null() {
                return fail(cb_status::CB_ERR_NULL_ARGUMENT, "out is null");
            }
            let row = match annotation_row(session, index) {
                Ok(row) => row,
                Err(code) => return code,
            };
            let filled = cb_annotation {
                id: row.id,
                kind: match row.kind {
                    AnnotationKind::Bookmark => cb_annotation_kind::CB_ANNOTATION_BOOKMARK,
                    AnnotationKind::Highlight => cb_annotation_kind::CB_ANNOTATION_HIGHLIGHT,
                    AnnotationKind::Note => cb_annotation_kind::CB_ANNOTATION_NOTE,
                },
                spine: row.spine_index,
                progression: row.progression,
                has_text: row.text.is_some(),
                has_color: row.color.is_some(),
            };
            // SAFETY: checked non-null above.
            unsafe { *out = filled };
            cb_status::CB_OK
        })
    })
}

/// The quoted text of a highlight or the body of a note, by index.
/// `CB_ERR_UNAVAILABLE` for a row whose `has_text` was false.
#[no_mangle]
pub unsafe extern "C" fn cb_session_annotation_text(
    session: *const cb_session,
    index: usize,
    buf: *mut c_char,
    cap: usize,
    needed: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, index, buf, cap, needed) {
            clear_last_error();
            // SAFETY: a handle from an open call, not yet closed.
            let Some(session) = (unsafe { session.as_ref() }) else {
                return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
            };
            let row = match annotation_row(session, index) {
                Ok(row) => row,
                Err(code) => return code,
            };
            let Some(text) = row.text else {
                return fail(cb_status::CB_ERR_UNAVAILABLE, "this mark carries no text");
            };
            // SAFETY: the header's contract for the buffer triple.
            unsafe { str_out(&text, buf, cap, needed) }
        })
    })
}

/// A mark's chosen color, by index, as the hex string it was set with.
/// `CB_ERR_UNAVAILABLE` for a mark wearing the theme's color.
#[no_mangle]
pub unsafe extern "C" fn cb_session_annotation_color(
    session: *const cb_session,
    index: usize,
    buf: *mut c_char,
    cap: usize,
    needed: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        with_marks!((session, index, buf, cap, needed) {
            clear_last_error();
            // SAFETY: a handle from an open call, not yet closed.
            let Some(session) = (unsafe { session.as_ref() }) else {
                return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
            };
            let row = match annotation_row(session, index) {
                Ok(row) => row,
                Err(code) => return code,
            };
            let Some(color) = row.color else {
                return fail(cb_status::CB_ERR_UNAVAILABLE, "this mark wears the theme's color");
            };
            // SAFETY: the header's contract for the buffer triple.
            unsafe { str_out(&color, buf, cap, needed) }
        })
    })
}
