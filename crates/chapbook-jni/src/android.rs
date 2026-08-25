//! The JNI surface. Names match `com.ophymx.chapbook.Native`.

use std::ffi::c_void;

use chapbook_reader::chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::Session;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring};
use jni::JNIEnv;

/// Android's own fonts. fontdb declines to look here — its system-font
/// discovery is `not(target_os = "android")` — so a shell that does not do
/// this by hand lays every page out with an empty font database and paints
/// nothing. See `docs/FFI.md`.
const SYSTEM_FONTS: &str = "/system/fonts";

/// What Android actually ships, mapped onto the five CSS generics. Without
/// this the generics resolve to fontdb's defaults, which are desktop family
/// names that no Android device has.
const GENERICS: [(&str, &str); 5] = [
    ("serif", "Noto Serif"),
    ("sans-serif", "Roboto"),
    ("monospace", "Droid Sans Mono"),
    ("cursive", "Noto Serif"),
    ("fantasy", "Roboto"),
];

// ---- Handles ----

/// A session, as a `jlong` Java holds onto. Null is the failure value, so
/// the Kotlin side never sees a Rust error type.
fn into_handle(session: Session) -> jlong {
    Box::into_raw(Box::new(session)) as jlong
}

/// # Safety
/// `handle` must have come from [`into_handle`] and not yet been closed.
unsafe fn session<'a>(handle: jlong) -> Option<&'a mut Session> {
    (handle as *mut Session).as_mut()
}

// ---- Lifecycle ----

/// Opens a book. `library_dir` is applied to the process environment before
/// the session reads it, which is the whole storage story for a spike:
/// `Session::open` reaches for `CHAPBOOK_LIBRARY_DIR` on its own, so no API
/// change is needed to point it at `context.filesDir`. That is a stopgap,
/// and it is why `docs/FFI.md` wants the directory to be a constructor
/// argument instead.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_open(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    library_dir: JString,
) -> jlong {
    let Ok(path): Result<String, _> = env.get_string(&path).map(Into::into) else {
        return 0;
    };
    if let Ok(dir) = env.get_string(&library_dir) {
        let dir: String = dir.into();
        // SAFETY: called once, from the JVM's main thread, before any
        // session exists and therefore before any thread reads it.
        unsafe { std::env::set_var("CHAPBOOK_LIBRARY_DIR", dir) };
    }

    let Ok(mut session) = Session::open(&path) else {
        return 0;
    };
    load_android_fonts(&mut session);
    into_handle(session)
}

/// Load `/system/fonts` and point the generics at families that exist.
///
/// Reaching the font database through `paint_resources` is a misuse worth
/// recording rather than hiding: that accessor exists so a shell can
/// rasterize a display list, and here it is being used to configure fonts
/// before anything is painted at all. It works only because the two share a
/// `FontSystem`. A font source belongs in the constructor.
fn load_android_fonts(session: &mut Session) {
    let (fonts, _) = session.paint_resources();
    let db = fonts.db_mut();
    db.load_fonts_dir(SYSTEM_FONTS);
    for (generic, family) in GENERICS {
        match generic {
            "serif" => db.set_serif_family(family),
            "sans-serif" => db.set_sans_serif_family(family),
            "monospace" => db.set_monospace_family(family),
            "cursive" => db.set_cursive_family(family),
            _ => db.set_fantasy_family(family),
        }
    }
}

/// How many faces the font database holds. Zero means every page will be
/// blank, and it is the first thing worth putting on screen.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_faceCount(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    match unsafe { session(handle) } {
        Some(s) => s.paint_resources().0.db_mut().len() as jint,
        None => -1,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_close(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // SAFETY: the handle came from `into_handle` and Kotlin promises
        // one close per open.
        drop(unsafe { Box::from_raw(handle as *mut Session) });
    }
}

// ---- Layout and navigation ----

#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_setMetrics(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    width: jfloat,
    height: jfloat,
    margin: jfloat,
    scale: jfloat,
) {
    if let Some(s) = unsafe { session(handle) } {
        s.set_metrics(PageMetrics {
            size: Size {
                w: width,
                h: height,
            },
            margins: EdgeSizes::uniform(margin),
            dpi_scale: scale,
            rotation: Rotation::None,
        });
    }
}

/// Returns whether the position moved — the answer `docs/SHELLS.md` insists
/// a shell use rather than comparing page numbers across the turn.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_nextPage(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    unsafe { session(handle) }.is_some_and(Session::next_page) as jboolean
}

#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_prevPage(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    unsafe { session(handle) }.is_some_and(Session::prev_page) as jboolean
}

/// `(spine, page)` packed into one `jlong`, because they are one value and
/// handing them over separately invites exactly the bug the conformance
/// harness exists to catch.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_position(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    match unsafe { session(handle) } {
        Some(s) => {
            let p = s.position();
            ((p.spine as jlong) << 32) | (p.page as jlong & 0xffff_ffff)
        }
        None => -1,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_title(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let title = match unsafe { session(handle) } {
        Some(s) => s.title().to_string(),
        None => String::new(),
    };
    string_out(env, &title)
}

fn string_out(env: JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(s) => s.into_raw(),
        Err(_) => JObject::null().into_raw(),
    }
}

// ---- Pixels ----

// libjnigraphics, which is on the NDK's stable list. This is the real
// `render_into` destination: memory Android owns, that we draw into.
#[repr(C)]
struct AndroidBitmapInfo {
    width: u32,
    height: u32,
    stride: u32,
    format: i32,
    flags: u32,
}

const ANDROID_BITMAP_FORMAT_RGBA_8888: i32 = 1;

// The `link` attribute is not decoration. Without it the crate builds
// clean, the `.so` is produced, and every AndroidBitmap symbol is left
// UND with no DT_NEEDED entry naming libjnigraphics — so the failure
// arrives at `System.loadLibrary`, as a crash in an app that compiled.
#[link(name = "jnigraphics")]
extern "C" {
    fn AndroidBitmap_getInfo(
        env: *mut jni::sys::JNIEnv,
        bitmap: jni::sys::jobject,
        info: *mut AndroidBitmapInfo,
    ) -> i32;
    fn AndroidBitmap_lockPixels(
        env: *mut jni::sys::JNIEnv,
        bitmap: jni::sys::jobject,
        pixels: *mut *mut c_void,
    ) -> i32;
    fn AndroidBitmap_unlockPixels(env: *mut jni::sys::JNIEnv, bitmap: jni::sys::jobject) -> i32;
}

/// Render the current page into an `ARGB_8888` bitmap.
///
/// The premultiplied-RGBA claim in `docs/FFI.md` is under test right here:
/// tiny-skia hands back premultiplied RGBA8888, an `ARGB_8888` bitmap is
/// premultiplied RGBA in memory, so this is a row-wise `memcpy` and nothing
/// else. If the colours come out with red and blue swapped, that assumption
/// was wrong and the doc needs correcting.
///
/// Returns 0 on success, or a negative code: -1 no session, -2 nothing to
/// render, -3 the bitmap is the wrong format or size, -4 the lock failed.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_renderInto(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    bitmap: JObject,
) -> jint {
    let Some(session) = (unsafe { session(handle) }) else {
        return -1;
    };
    let Some(pixmap) = session.render() else {
        return -2;
    };

    let raw_env = env.get_raw();
    let raw_bitmap = bitmap.as_raw();
    let mut info = AndroidBitmapInfo {
        width: 0,
        height: 0,
        stride: 0,
        format: 0,
        flags: 0,
    };
    // SAFETY: `env` and `bitmap` are live for the duration of the call, and
    // `info` is a valid out-pointer.
    if unsafe { AndroidBitmap_getInfo(raw_env, raw_bitmap, &mut info) } != 0 {
        return -3;
    }
    if info.format != ANDROID_BITMAP_FORMAT_RGBA_8888
        || info.width != pixmap.width()
        || info.height != pixmap.height()
    {
        return -3;
    }

    let mut pixels: *mut c_void = std::ptr::null_mut();
    // SAFETY: as above; the pointer is written only on success.
    if unsafe { AndroidBitmap_lockPixels(raw_env, raw_bitmap, &mut pixels) } != 0
        || pixels.is_null()
    {
        return -4;
    }

    let src = pixmap.data();
    let row_bytes = pixmap.width() as usize * 4;
    for y in 0..pixmap.height() as usize {
        // SAFETY: the destination row is within the locked mapping, whose
        // height and stride Android just reported, and the source row is
        // within a pixmap of the same dimensions. The two never overlap.
        unsafe {
            std::ptr::copy_nonoverlapping(
                src.as_ptr().add(y * row_bytes),
                (pixels as *mut u8).add(y * info.stride as usize),
                row_bytes,
            );
        }
    }

    // SAFETY: paired with the successful lock above.
    unsafe { AndroidBitmap_unlockPixels(raw_env, raw_bitmap) };
    0
}

// ---- Proving it ----

/// Run the shell conformance harness against this book and hand back its
/// report. This is the rung that tests the *binding* rather than the build:
/// the harness already watches the seam from outside, so driving it through
/// JNI asks whether the seam survived the trip.
///
/// It takes a path rather than an open handle because the harness opens its
/// own sessions — one per check, since "position survives a restart" cannot
/// be asked of a session that never stopped.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_conformance(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> jstring {
    let Ok(path): Result<String, _> = env.get_string(&path).map(Into::into) else {
        return string_out(env, "bad path");
    };
    let mut failed_to_open = false;
    let report = {
        let opener = || match Session::open(&path) {
            Ok(mut s) => {
                load_android_fonts(&mut s);
                Some(s)
            }
            Err(_) => None,
        };
        // The harness wants an infallible factory. A book that will not open
        // is not a conformance failure, it is a different question, so say so
        // rather than reporting eleven mysterious ones.
        match opener() {
            None => {
                failed_to_open = true;
                String::new()
            }
            Some(_) => chapbook_reader::conformance::Harness::new(|| {
                opener().expect("the book opened a moment ago")
            })
            .run()
            .to_string(),
        }
    };
    if failed_to_open {
        return string_out(env, "could not open the book");
    }
    string_out(env, &report)
}
