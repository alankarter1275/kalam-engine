//! The session: opening a book, moving through it, and getting pixels.
//!
//! One opaque handle per open book, created and destroyed by explicit
//! calls, with no global state and no implicit singleton. `Session` is
//! `Send` and deliberately not `Sync`, and the handle inherits exactly
//! that: **a host may move it between threads and must never touch it from
//! two at once.** No lock is taken here to make that safe, because a lock
//! would be a silent tax on the single-threaded case that every shell
//! actually has.

use std::ffi::{c_char, c_void};

use chapbook_reader::chapbook_core::{
    EdgeSizes, Format, PageMetrics, Rotation, Size, Source, TapZones, Theme,
};
use chapbook_reader::Session;

use crate::abi::{str_in, str_out};
use crate::config::cb_config;
use crate::error::{cb_status, clear_last_error, fail, from_error, guard};

/// An open book. Opaque.
pub struct cb_session {
    pub(crate) inner: Session,
    /// The tap policy for this session — beside the session rather than a
    /// free-standing struct so the one field a host must *not* choose, the
    /// reading direction, is read off the book on every configuration and
    /// can never be handed in wrong. That shape was settled on a device:
    /// see `docs/FFI.md`, *What the touchscreen settled*.
    pub(crate) zones: TapZones,
}

/// Which reader opens the bytes. `CB_FORMAT_GUESS` decides from the bytes
/// themselves, and is the right answer even when a name is available — the
/// EPUB `mimetype` entry and the `%PDF` header do not lie and an extension
/// does.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_format {
    CB_FORMAT_GUESS = 0,
    CB_FORMAT_EPUB = 1,
    CB_FORMAT_CBZ = 2,
    CB_FORMAT_PDF = 3,
}

impl From<cb_format> for Format {
    fn from(format: cb_format) -> Format {
        match format {
            cb_format::CB_FORMAT_GUESS => Format::Guess,
            cb_format::CB_FORMAT_EPUB => Format::Epub,
            cb_format::CB_FORMAT_CBZ => Format::Cbz,
            cb_format::CB_FORMAT_PDF => Format::Pdf,
        }
    }
}

/// Quarter-turns clockwise between the page as laid out and the panel it is
/// painted into. A property of the output, not the layout.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_rotation {
    CB_ROTATION_NONE = 0,
    CB_ROTATION_QUARTER = 1,
    CB_ROTATION_HALF = 2,
    CB_ROTATION_THREE_QUARTER = 3,
}

impl From<cb_rotation> for Rotation {
    fn from(rotation: cb_rotation) -> Rotation {
        match rotation {
            cb_rotation::CB_ROTATION_NONE => Rotation::None,
            cb_rotation::CB_ROTATION_QUARTER => Rotation::Quarter,
            cb_rotation::CB_ROTATION_HALF => Rotation::Half,
            cb_rotation::CB_ROTATION_THREE_QUARTER => Rotation::ThreeQuarter,
        }
    }
}

/// Page ground and default text colours.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_theme {
    CB_THEME_LIGHT = 0,
    CB_THEME_SEPIA = 1,
    CB_THEME_DARK = 2,
}

/// What kind of book is open. Comics and PDFs page as images, which is why
/// a host may want to know before offering text-shaped affordances.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_book_kind {
    CB_BOOK_EPUB = 0,
    CB_BOOK_COMIC = 1,
    CB_BOOK_PDF = 2,
}

/// Where the reader is.
///
/// The pair, always. A page number alone is meaningless across a unit
/// boundary, and a host that stores one and compares it after a turn has a
/// bug the conformance harness exists to catch.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct cb_position {
    pub spine: u32,
    pub page: u32,
}

/// The page box, in logical units, plus the scale that turns it into
/// device pixels. Laying out at logical size and rasterizing at device
/// size is what keeps text a readable size on a dense panel.
///
/// **`width` and `height` are the page in *reading* orientation, not the
/// panel you paint into.** They are the same thing only while `rotation`
/// is `CB_ROTATION_NONE`. On a quarter or three-quarter turn the axes
/// swap, so a host with a 600x800 view that wants a turned page passes
/// 800x600 here and gets 600x800 back from `cb_session_render_size`.
///
/// Passing the view's own dimensions on a turn is not an error and will
/// not be reported as one: the page simply paginates to the wrong aspect,
/// and because you allocate from `cb_session_render_size` there is no
/// mismatch left for anything to catch. Rotation is a property of the
/// output; it must never change what the text reflows to.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct cb_metrics {
    pub width: f32,
    pub height: f32,
    pub margin_top: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub dpi_scale: f32,
    pub rotation: cb_rotation,
}

/// How a page is typeset. `base_font_px` and `line_height` are the two a
/// reader adjusts; the rest a shell usually sets once.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct cb_settings {
    pub base_font_px: f32,
    /// Unitless multiplier.
    pub line_height: f32,
    pub justify: bool,
    /// Honour the publisher's stylesheets. Off leaves UA and user sheets.
    pub publisher_styles: bool,
    pub theme: cb_theme,
}

/// Where a settings change sticks.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_settings_scope {
    /// Every book without an override of its own.
    CB_SCOPE_GLOBAL = 0,
    /// This book only.
    CB_SCOPE_THIS_BOOK = 1,
}

/// A dimension a caller may legally give us.
///
/// Spelled out rather than written `!(x > 0.0)` because the interesting
/// case is NaN, which arrives from C as easily as any other bit pattern and
/// compares false against everything. Infinity is refused for the same
/// reason: it is not a page size, and it propagates into layout as NaN.
fn usable(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

// ---- Opening and closing ----

fn open_with(source: Source, config: *mut cb_config) -> *mut cb_session {
    if config.is_null() {
        fail(cb_status::CB_ERR_NULL_ARGUMENT, "config is null");
        return std::ptr::null_mut();
    }
    // SAFETY: a handle from `config`, consumed exactly here. Consumed even
    // when the open fails, which the header states, because the alternative
    // is a host that cannot tell whether it still owns the thing.
    let config: Box<cb_config> = unsafe { Box::from_raw(config) };
    match Session::open_with(source, config.inner) {
        Ok(inner) => {
            clear_last_error();
            // The default bands, in the direction the book declares;
            // everything else waits for `cb_session_set_tap_zones`.
            let zones = TapZones::new(inner.reading_direction());
            Box::into_raw(Box::new(cb_session { inner, zones }))
        }
        Err(e) => {
            from_error(&e);
            std::ptr::null_mut()
        }
    }
}

/// Open a book from a filesystem path. **Consumes `config`** either way.
///
/// Returns null on failure; `cb_last_error_message` says why. The book is
/// imported into the library (when the config names one): copied, indexed,
/// and reopened at its stored reading position.
#[no_mangle]
pub unsafe extern "C" fn cb_session_open_path(
    path: *const c_char,
    config: *mut cb_config,
) -> *mut cb_session {
    guard(std::ptr::null_mut(), || {
        // SAFETY: the header's contract.
        let Some(path) = (unsafe { str_in(path, "path") }) else {
            cb_config_free_internal(config);
            return std::ptr::null_mut();
        };
        open_with(Source::Path(path.into()), config)
    })
}

/// Open a book from bytes the host already holds — a WASM `ArrayBuffer`, a
/// download it performed itself. The bytes are copied; the caller's buffer
/// is its own again on return.
///
/// **Consumes `config`.** Reaches the library by content: the bytes are
/// hashed and the book adopted — recorded, not copied — under the same
/// edition fingerprint a path import gets, so its position, annotations
/// and per-book settings persist. Keeping hold of the *file* for the next
/// launch stays the host's job.
#[no_mangle]
pub unsafe extern "C" fn cb_session_open_bytes(
    bytes: *const u8,
    len: usize,
    format: cb_format,
    config: *mut cb_config,
) -> *mut cb_session {
    guard(std::ptr::null_mut(), || {
        if bytes.is_null() {
            fail(cb_status::CB_ERR_NULL_ARGUMENT, "bytes is null");
            cb_config_free_internal(config);
            return std::ptr::null_mut();
        }
        // SAFETY: the header's contract — `len` readable bytes at `bytes`,
        // valid for the duration of this call.
        let copied = unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec();
        open_with(
            Source::Bytes {
                format: format.into(),
                bytes: copied,
            },
            config,
        )
    })
}

/// Open a book from an already-open file descriptor — an Android
/// `content://` URI resolved through `ParcelFileDescriptor`, an iOS
/// security-scoped file.
///
/// **Takes ownership of `fd`** and closes it when the session is closed, so
/// the caller must have detached it. **Consumes `config`.** Reaches the
/// library the same way bytes do: hashed on open, adopted by fingerprint,
/// position and annotations persist. The descriptor is not something the
/// library can reopen, so re-resolving the bookmark or URI grant on the
/// next launch stays the caller's job — resolve first, then open.
///
/// Unix only: a descriptor is what Android and iOS hand out, and Windows
/// has no analogue worth guessing at from here.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn cb_session_open_fd(
    fd: i32,
    format: cb_format,
    config: *mut cb_config,
) -> *mut cb_session {
    guard(std::ptr::null_mut(), || {
        if fd < 0 {
            fail(cb_status::CB_ERR_INVALID_ARGUMENT, "fd is negative");
            cb_config_free_internal(config);
            return std::ptr::null_mut();
        }
        // SAFETY: the header's contract — an owned descriptor the caller
        // has given up, which this `File` closes on drop.
        let file = unsafe {
            use std::os::fd::FromRawFd;
            std::fs::File::from_raw_fd(fd)
        };
        open_with(
            Source::Reader {
                format: format.into(),
                reader: Box::new(file),
            },
            config,
        )
    })
}

/// Free a config the caller handed us on a path that failed before
/// `open_with` could consume it. Keeps "consumes `config` either way" true.
fn cb_config_free_internal(config: *mut cb_config) {
    if !config.is_null() {
        // SAFETY: a handle from `config`, freed once.
        drop(unsafe { Box::from_raw(config) });
    }
}

/// Close a session and release everything it holds. Passing null is a
/// no-op. Every handle from a `cb_session_open_*` must reach this exactly
/// once.
#[no_mangle]
pub unsafe extern "C" fn cb_session_close(session: *mut cb_session) {
    guard((), || {
        if !session.is_null() {
            // SAFETY: a handle from an open call, closed once.
            drop(unsafe { Box::from_raw(session) });
        }
    })
}

// ---- Plumbing shared by the accessors ----

macro_rules! session_mut {
    ($session:expr) => {
        // SAFETY: a handle from an open call, not yet closed.
        match unsafe { $session.as_mut() } {
            Some(session) => session,
            None => return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null"),
        }
    };
}

macro_rules! session_ref {
    ($session:expr) => {
        // SAFETY: a handle from an open call, not yet closed.
        match unsafe { $session.as_ref() } {
            Some(session) => session,
            None => return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null"),
        }
    };
}

/// Write an out-parameter, refusing a null destination.
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

// ---- Diagnostics ----

/// The message behind the last failure **on this thread**, as a
/// NUL-terminated string.
///
/// Call with `cap` 0 and `buf` null to learn the size, then again with a
/// buffer. Empty when nothing has failed. **The text is not stable** — it
/// is for logs and bug reports; branch on the status code instead.
#[no_mangle]
pub unsafe extern "C" fn cb_last_error_message(
    buf: *mut c_char,
    cap: usize,
    needed: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let message = crate::error::last_error_message();
        // SAFETY: the header's contract for the buffer triple.
        unsafe { str_out(&message, buf, cap, needed) }
    })
}

// ---- Metrics and layout ----

/// Give the session its page box. Required before anything paginates: a
/// session with no metrics has no pages, and reports `CB_ERR_UNAVAILABLE`
/// when asked for a size or a render.
#[no_mangle]
pub unsafe extern "C" fn cb_session_set_metrics(
    session: *mut cb_session,
    metrics: cb_metrics,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        if !(usable(metrics.width) && usable(metrics.height) && usable(metrics.dpi_scale)) {
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                "width, height and dpi_scale must all be finite and positive",
            );
        }
        session.inner.set_metrics(PageMetrics {
            size: Size {
                w: metrics.width,
                h: metrics.height,
            },
            margins: EdgeSizes {
                top: metrics.margin_top,
                right: metrics.margin_right,
                bottom: metrics.margin_bottom,
                left: metrics.margin_left,
            },
            dpi_scale: metrics.dpi_scale,
            rotation: metrics.rotation.into(),
        });
        cb_status::CB_OK
    })
}

// ---- Navigation ----

/// Turn forward one page, crossing into the next unit at the end of this
/// one. `*moved` reports whether the position changed.
///
/// **Use `*moved`.** Do not compare positions across a turn: that is the
/// defect this out-parameter exists to prevent.
#[no_mangle]
pub unsafe extern "C" fn cb_session_next_page(
    session: *mut cb_session,
    moved: *mut bool,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        let did = session.inner.next_page();
        out!(moved, did, "moved");
        cb_status::CB_OK
    })
}

/// Turn back one page. `*moved` reports whether the position changed.
#[no_mangle]
pub unsafe extern "C" fn cb_session_prev_page(
    session: *mut cb_session,
    moved: *mut bool,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        let did = session.inner.prev_page();
        out!(moved, did, "moved");
        cb_status::CB_OK
    })
}

/// Skip to the start of the next unit. `*moved` reports whether it moved.
#[no_mangle]
pub unsafe extern "C" fn cb_session_next_unit(
    session: *mut cb_session,
    moved: *mut bool,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        let did = session.inner.next_unit();
        out!(moved, did, "moved");
        cb_status::CB_OK
    })
}

/// Skip to the start of the previous unit. `*moved` reports whether it moved.
#[no_mangle]
pub unsafe extern "C" fn cb_session_prev_unit(
    session: *mut cb_session,
    moved: *mut bool,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        let did = session.inner.prev_unit();
        out!(moved, did, "moved");
        cb_status::CB_OK
    })
}

/// Where the reader is, as the pair.
#[no_mangle]
pub unsafe extern "C" fn cb_session_position(
    session: *const cb_session,
    position: *mut cb_position,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        let at = session.inner.position();
        out!(
            position,
            cb_position {
                spine: at.spine as u32,
                page: at.page as u32,
            },
            "position"
        );
        cb_status::CB_OK
    })
}

/// How many units the spine holds. Never compacted: a dangling idref keeps
/// its slot, because indices are locator identity.
#[no_mangle]
pub unsafe extern "C" fn cb_session_spine_len(
    session: *const cb_session,
    len: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        out!(len, session.inner.spine_len(), "len");
        cb_status::CB_OK
    })
}

/// How many pages the current unit holds at the current metrics. Lays the
/// unit out if it has not been laid out yet, so it is not free.
#[no_mangle]
pub unsafe extern "C" fn cb_session_page_count(
    session: *mut cb_session,
    count: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        let n = session.inner.page_count();
        out!(count, n, "count");
        cb_status::CB_OK
    })
}

// ---- Metadata ----

/// The book's title. Caller-allocates; see [`cb_last_error_message`] for
/// the two-call idiom.
#[no_mangle]
pub unsafe extern "C" fn cb_session_title(
    session: *const cb_session,
    buf: *mut c_char,
    cap: usize,
    needed: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        // SAFETY: the header's contract for the buffer triple.
        unsafe { str_out(session.inner.title(), buf, cap, needed) }
    })
}

/// Whether the open book pages as text or as images.
#[no_mangle]
pub unsafe extern "C" fn cb_session_book_kind(
    session: *const cb_session,
    kind: *mut cb_book_kind,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        use chapbook_reader::chapbook_core::BookKind;
        let session = session_ref!(session);
        let value = match session.inner.kind() {
            BookKind::Epub => cb_book_kind::CB_BOOK_EPUB,
            BookKind::Comic => cb_book_kind::CB_BOOK_COMIC,
            BookKind::Pdf => cb_book_kind::CB_BOOK_PDF,
        };
        out!(kind, value, "kind");
        cb_status::CB_OK
    })
}

// ---- Settings ----

/// The settings in force for the open book.
#[no_mangle]
pub unsafe extern "C" fn cb_session_settings(
    session: *const cb_session,
    settings: *mut cb_settings,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        let current = session.inner.settings();
        out!(
            settings,
            cb_settings {
                base_font_px: current.base_font_px,
                line_height: current.line_height,
                justify: current.justify,
                publisher_styles: current.publisher_styles,
                theme: match current.theme {
                    Theme::Light => cb_theme::CB_THEME_LIGHT,
                    Theme::Sepia => cb_theme::CB_THEME_SEPIA,
                    Theme::Dark => cb_theme::CB_THEME_DARK,
                },
            },
            "settings"
        );
        cb_status::CB_OK
    })
}

/// Apply settings, keeping the reader's place across the reflow.
#[no_mangle]
pub unsafe extern "C" fn cb_session_set_settings(
    session: *mut cb_session,
    settings: cb_settings,
    scope: cb_settings_scope,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        use chapbook_reader::chapbook_core::ReadingSettings;
        use chapbook_reader::SettingsScope;
        let session = session_mut!(session);
        if !usable(settings.base_font_px) {
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                "base_font_px must be finite and positive",
            );
        }
        if !usable(settings.line_height) {
            // Real books ship `line-height: 0`, and the engine clamps it.
            // An explicit setting is a different thing: refuse it, rather
            // than silently disagree with the host about what it asked for.
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                "line_height must be finite and positive",
            );
        }
        session.inner.set_settings(
            ReadingSettings {
                base_font_px: settings.base_font_px,
                line_height: settings.line_height,
                justify: settings.justify,
                publisher_styles: settings.publisher_styles,
                theme: match settings.theme {
                    cb_theme::CB_THEME_LIGHT => Theme::Light,
                    cb_theme::CB_THEME_SEPIA => Theme::Sepia,
                    cb_theme::CB_THEME_DARK => Theme::Dark,
                },
            },
            match scope {
                cb_settings_scope::CB_SCOPE_GLOBAL => SettingsScope::Global,
                cb_settings_scope::CB_SCOPE_THIS_BOOK => SettingsScope::ThisBook,
            },
        );
        cb_status::CB_OK
    })
}

// ---- Pixels ----

/// The device-pixel size a surface must be for [`cb_session_render_into`],
/// rotation included.
///
/// A host cannot allocate a surface without this, and must not compute it
/// itself: the logical-to-device round trip does not always land on the
/// pixel it started from, and `cb_session_render_into` refuses a
/// mismatched buffer rather than misdrawing into it.
///
/// `CB_ERR_UNAVAILABLE` until metrics are set.
#[no_mangle]
pub unsafe extern "C" fn cb_session_render_size(
    session: *const cb_session,
    width: *mut u32,
    height: *mut u32,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        let Some((w, h)) = session.inner.render_size() else {
            return fail(
                cb_status::CB_ERR_UNAVAILABLE,
                "no render size yet: set metrics first",
            );
        };
        out!(width, w, "width");
        out!(height, h, "height");
        cb_status::CB_OK
    })
}

/// Rasterize the current page into a buffer the caller owns.
///
/// `width` and `height` must equal [`cb_session_render_size`]; `stride` is
/// the byte distance between rows and must be at least `width * 4`. Pixels
/// come back **premultiplied RGBA8888**, which is what Android's
/// `ARGB_8888` holds natively, so the common path converts nothing.
///
/// An unrotated page whose stride is exactly `width * 4` is rasterized
/// straight into `dst` with no intermediate and no copy. A rotated page or
/// a padded stride goes through a temporary and is copied row by row —
/// correct either way, free only in the first case, and every named
/// platform hits the first case.
///
/// A surface whose dimensions disagree with `cb_session_render_size`, a
/// stride narrower than a row, or a buffer too short for the two, are all
/// `CB_ERR_INVALID_ARGUMENT` and nothing is written. `CB_ERR_UNAVAILABLE`
/// means the size was right and there is simply nothing to draw yet.
#[no_mangle]
pub unsafe extern "C" fn cb_session_render_into(
    session: *mut cb_session,
    dst: *mut u8,
    len: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        if dst.is_null() {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "dst is null");
        }
        // Checked against the session's own answer first, so "your surface
        // is the wrong shape" is distinguishable from "there is nothing to
        // draw". Writing a C host against this is what showed they were
        // being conflated: a caller that got the width wrong was told the
        // page was unavailable, which sends them looking in the wrong
        // place entirely.
        match session.inner.render_size() {
            None => {
                return fail(
                    cb_status::CB_ERR_UNAVAILABLE,
                    "no render size yet: set metrics first",
                )
            }
            Some((want_w, want_h)) if (want_w, want_h) != (width, height) => {
                return fail(
                    cb_status::CB_ERR_INVALID_ARGUMENT,
                    format!(
                        "surface is {width}x{height}, this page renders at                          {want_w}x{want_h} — ask cb_session_render_size"
                    ),
                )
            }
            Some(_) => {}
        }
        let Some(row) = (width as usize).checked_mul(4) else {
            return fail(cb_status::CB_ERR_INVALID_ARGUMENT, "width overflows a row");
        };
        if stride < row {
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                format!("stride {stride} is narrower than a {row}-byte row"),
            );
        }
        if len < stride.saturating_mul(height as usize) {
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                format!(
                    "buffer holds {len} bytes, {} needed",
                    stride.saturating_mul(height as usize)
                ),
            );
        }
        // SAFETY: the header's contract — `len` writable bytes at `dst`,
        // not aliased by anything this crate holds, valid for the call. The
        // bounds above are checked before it is ever formed.
        let buffer = unsafe { std::slice::from_raw_parts_mut(dst, len) };
        if session.inner.render_into(buffer, width, height, stride) {
            cb_status::CB_OK
        } else {
            // The size already agreed, so this is genuinely "nothing to
            // draw" — an unloaded unit, most likely.
            fail(
                cb_status::CB_ERR_UNAVAILABLE,
                "nothing to render for the current page",
            )
        }
    })
}

// ---- Lifecycle ----

/// Save the reading position and let go of everything reconstructible.
///
/// Call it from the last callback the platform guarantees — Android's
/// `onStop`, iOS's `willResignActive`. The session stays usable; the
/// library reopens by itself if something needs it.
#[no_mangle]
pub unsafe extern "C" fn cb_session_suspend(session: *mut cb_session) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        session.inner.suspend();
        cb_status::CB_OK
    })
}

/// Drop cached layouts and decoded images. The current page is rebuilt on
/// the next render. For Android's `onTrimMemory` and iOS's memory warning.
#[no_mangle]
pub unsafe extern "C" fn cb_session_release_caches(session: *mut cb_session) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        session.inner.release_caches();
        cb_status::CB_OK
    })
}

/// Bytes the caches currently hold.
#[no_mangle]
pub unsafe extern "C" fn cb_session_cache_bytes(
    session: *const cb_session,
    bytes: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        out!(bytes, session.inner.cache_bytes(), "bytes");
        cb_status::CB_OK
    })
}

/// The ceiling those bytes are held under.
#[no_mangle]
pub unsafe extern "C" fn cb_session_cache_budget(
    session: *const cb_session,
    bytes: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        out!(bytes, session.inner.cache_budget(), "bytes");
        cb_status::CB_OK
    })
}

// ---- Fonts ----

/// How many faces the font source actually produced.
///
/// Worth showing once at startup. Zero never reaches a host — a source that
/// resolves to nothing fails the open with `CB_ERR_FONT` — but the number
/// still distinguishes a device that found its system fonts from one
/// running on a single embedded face.
#[no_mangle]
pub unsafe extern "C" fn cb_session_font_face_count(
    session: *const cb_session,
    count: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        out!(count, session.inner.font_report().faces, "count");
        cb_status::CB_OK
    })
}

/// Generic families that resolved to a name no loaded face carries, as
/// `generic=family`, one per line. Empty when everything resolved.
///
/// Not an error: a book that never asks for `cursive` never notices. It is
/// the difference between a device a developer can diagnose and one they
/// have to guess at.
#[no_mangle]
pub unsafe extern "C" fn cb_session_font_unresolved(
    session: *const cb_session,
    buf: *mut c_char,
    cap: usize,
    needed: *mut usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        let text = session
            .inner
            .font_report()
            .unresolved_generics
            .iter()
            .map(|(generic, family)| format!("{generic}={family}"))
            .collect::<Vec<_>>()
            .join("\n");
        // SAFETY: the header's contract for the buffer triple.
        unsafe { str_out(&text, buf, cap, needed) }
    })
}

// ---- Asynchronous loads ----

/// Called when a background load finishes and there is something new to
/// show. **Fires on the loader thread**, not the host's UI thread: it must
/// only hand a token to whatever the host's main loop watches — a pipe, a
/// run-loop source, `Handler.post`. Attaching a JVM thread or touching UI
/// from inside it is the shape this comment exists to prevent.
pub type cb_wake_fn = Option<extern "C" fn(user: *mut c_void)>;

/// Install the wake callback. `user` is handed back untouched.
///
/// The pointer must stay valid until the session is closed or the waker is
/// replaced, and the callback may run on any thread.
#[no_mangle]
pub unsafe extern "C" fn cb_session_set_waker(
    session: *mut cb_session,
    wake: cb_wake_fn,
    user: *mut c_void,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        let Some(wake) = wake else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "wake is null");
        };
        // The host promises `user` outlives the session; `usize` carries it
        // across the `Send + Sync` bound the waker requires, which a raw
        // pointer cannot satisfy on its own.
        let user = user as usize;
        session.inner.set_waker(move || wake(user as *mut c_void));
        cb_status::CB_OK
    })
}

/// Take delivery of anything the loader finished. `*changed` reports
/// whether the visible page is now different, and therefore whether a
/// repaint is worth doing.
#[no_mangle]
pub unsafe extern "C" fn cb_session_poll_loaded(
    session: *mut cb_session,
    changed: *mut bool,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_mut!(session);
        let did = session.inner.poll_loaded();
        out!(changed, did, "changed");
        cb_status::CB_OK
    })
}

/// Whether any unit is still being loaded. A host that wants to show a
/// spinner asks this; one that just repaints on wake does not need it.
#[no_mangle]
pub unsafe extern "C" fn cb_session_has_pending_loads(
    session: *const cb_session,
    pending: *mut bool,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let session = session_ref!(session);
        out!(pending, session.inner.has_pending_loads(), "pending");
        cb_status::CB_OK
    })
}
