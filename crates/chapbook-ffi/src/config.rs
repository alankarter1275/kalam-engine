//! Building a session's configuration from C.
//!
//! `SessionConfig` is a Rust builder with a required argument, and the
//! required argument is the reason this is two opaque types rather than a
//! struct the caller fills in. A font source is a tree — several face
//! sources, five generic families, a fallback list, a locale — and a host
//! that gets it wrong gets *blank pages*, not an error. So it is built by
//! calls that can each fail and say why, and `SessionConfig` will not
//! construct without one.

use std::ffi::c_char;

use chapbook_reader::chapbook_core::{Faces, FontSource, GenericFamilies, Generics};
use chapbook_reader::SessionConfig;

use crate::abi::str_in;
use crate::error::{cb_status, fail, guard};

/// A font source under construction. Opaque: its shape is Rust's.
pub struct cb_font_source {
    pub(crate) inner: FontSource,
}

/// A session configuration under construction. Opaque.
pub struct cb_config {
    pub(crate) inner: SessionConfig,
}

/// The host's installed fonts, its idea of the generics, its fallback list.
///
/// Right for a desktop shell and wrong everywhere else: it produces **no
/// faces at all** on Android, iOS and wasm, where `cb_session_open_*` then
/// fails with `CB_ERR_FONT` rather than paginating every book to one blank
/// page. That failure is the feature.
#[no_mangle]
pub extern "C" fn cb_font_source_host() -> *mut cb_font_source {
    guard(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(cb_font_source {
            inner: FontSource::host(),
        }))
    })
}

/// Exactly the fonts in `dir`, every generic pointed at `family`, no
/// fallback, a fixed locale. All three axes pinned, which is what makes
/// output comparable across machines — what a test corpus or a golden
/// wants, and what a shell shipping its own faces wants.
#[no_mangle]
pub unsafe extern "C" fn cb_font_source_embedded(
    dir: *const c_char,
    family: *const c_char,
) -> *mut cb_font_source {
    guard(std::ptr::null_mut(), || {
        // SAFETY: the header's contract for both pointers.
        let (Some(dir), Some(family)) = (unsafe { str_in(dir, "dir") }, unsafe {
            str_in(family, "family")
        }) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(cb_font_source {
            inner: FontSource::embedded(dir, family),
        }))
    })
}

/// Android's system fonts: `/system/fonts`, the families Android actually
/// ships, and Noto for fallback.
///
/// Present on every platform rather than behind a `cfg` so that the header
/// is the same file everywhere — a host that calls it off Android gets a
/// source that resolves to nothing and an honest `CB_ERR_FONT` at open.
#[no_mangle]
pub extern "C" fn cb_font_source_android_system() -> *mut cb_font_source {
    guard(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(cb_font_source {
            inner: FontSource::android_system(),
        }))
    })
}

/// Add a directory of faces, searched recursively. Later sources add to
/// earlier ones, so this composes with the presets above.
#[no_mangle]
pub unsafe extern "C" fn cb_font_source_add_dir(
    fonts: *mut cb_font_source,
    dir: *const c_char,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        // SAFETY: a handle from this module, not yet freed.
        let Some(fonts) = (unsafe { fonts.as_mut() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "font source is null");
        };
        // SAFETY: the header's contract.
        let Some(dir) = (unsafe { str_in(dir, "dir") }) else {
            return cb_status::CB_ERR_NULL_ARGUMENT;
        };
        fonts.inner.faces.push(Faces::Dir(dir.into()));
        cb_status::CB_OK
    })
}

/// Spell out all five CSS generic families.
///
/// There is no partial form on purpose. Every platform's built-in answer is
/// wrong somewhere and wrong silently — fontdb's defaults are Microsoft
/// family names no phone has — so this ABI makes a host that touches the
/// generics at all name every one of them.
#[no_mangle]
pub unsafe extern "C" fn cb_font_source_set_generics(
    fonts: *mut cb_font_source,
    serif: *const c_char,
    sans_serif: *const c_char,
    monospace: *const c_char,
    cursive: *const c_char,
    fantasy: *const c_char,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        // SAFETY: a handle from this module, not yet freed.
        let Some(fonts) = (unsafe { fonts.as_mut() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "font source is null");
        };
        // SAFETY: the header's contract for all five.
        let families = unsafe {
            let (Some(serif), Some(sans_serif), Some(monospace), Some(cursive), Some(fantasy)) = (
                str_in(serif, "serif"),
                str_in(sans_serif, "sans_serif"),
                str_in(monospace, "monospace"),
                str_in(cursive, "cursive"),
                str_in(fantasy, "fantasy"),
            ) else {
                return cb_status::CB_ERR_NULL_ARGUMENT;
            };
            GenericFamilies {
                serif: serif.into(),
                sans_serif: sans_serif.into(),
                monospace: monospace.into(),
                cursive: cursive.into(),
                fantasy: fantasy.into(),
            }
        };
        fonts.inner.generics = Generics::Explicit(families);
        cb_status::CB_OK
    })
}

/// Release a font source that was never handed to [`cb_config_new`].
///
/// Passing null is a no-op, so a host can free unconditionally on an error
/// path without testing first.
#[no_mangle]
pub unsafe extern "C" fn cb_font_source_free(fonts: *mut cb_font_source) {
    guard((), || {
        if !fonts.is_null() {
            // SAFETY: a handle from this module, freed once.
            drop(unsafe { Box::from_raw(fonts) });
        }
    })
}

/// Begin a configuration. **Consumes `fonts`**, whether or not it succeeds:
/// the handle is dead on return and must not be freed by the caller.
#[no_mangle]
pub unsafe extern "C" fn cb_config_new(fonts: *mut cb_font_source) -> *mut cb_config {
    guard(std::ptr::null_mut(), || {
        if fonts.is_null() {
            fail(cb_status::CB_ERR_NULL_ARGUMENT, "font source is null");
            return std::ptr::null_mut();
        }
        // SAFETY: a handle from this module, consumed exactly here.
        let fonts = unsafe { Box::from_raw(fonts) };
        Box::into_raw(Box::new(cb_config {
            inner: SessionConfig::new(fonts.inner),
        }))
    })
}

/// Where the library, its managed book copies and its covers live.
///
/// Unset, the session asks the platform's convention, which exists on
/// desktops and nowhere else. A sandboxed app knows its own answer and
/// nothing else can: `context.getFilesDir()`, `Library/Application
/// Support`. Without a library a session still reads — it just remembers
/// nothing, and says nothing about it.
#[no_mangle]
pub unsafe extern "C" fn cb_config_set_library_dir(
    config: *mut cb_config,
    dir: *const c_char,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        // SAFETY: a handle from this module, not yet consumed.
        let Some(config) = (unsafe { config.as_mut() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "config is null");
        };
        // SAFETY: the header's contract.
        let Some(dir) = (unsafe { str_in(dir, "dir") }) else {
            return cb_status::CB_ERR_NULL_ARGUMENT;
        };
        config.inner.library_dir = Some(dir.into());
        cb_status::CB_OK
    })
}

/// Cap what the session's caches may hold, in bytes.
///
/// A host that knows its own limits should say. Android kills a process for
/// exceeding them and gives `onTrimMemory` no argument about the number.
#[no_mangle]
pub unsafe extern "C" fn cb_config_set_cache_budget(
    config: *mut cb_config,
    bytes: usize,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        // SAFETY: a handle from this module, not yet consumed.
        let Some(config) = (unsafe { config.as_mut() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "config is null");
        };
        if bytes == 0 {
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                "a cache budget of zero would evict the page being read",
            );
        }
        config.inner.cache_budget = Some(bytes);
        cb_status::CB_OK
    })
}

/// Release a configuration that was never handed to a `cb_session_open_*`.
/// Passing null is a no-op.
#[no_mangle]
pub unsafe extern "C" fn cb_config_free(config: *mut cb_config) {
    guard((), || {
        if !config.is_null() {
            // SAFETY: a handle from this module, freed once.
            drop(unsafe { Box::from_raw(config) });
        }
    })
}
