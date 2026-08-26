//! The JNI surface. Names match `com.ophymx.chapbook.Native`.
//!
//! Rungs 1 through 4 of this binding were written against a session that
//! reached for its fonts, its library directory and its diagnostics behind
//! the caller's back, and the workarounds were the point: each one was a
//! defect report. They are gone now. Every capability this file needs
//! arrives through `SessionConfig` or `Source`, so what is left reads like
//! what the C ABI will wrap rather than like a list of complaints.

use std::ffi::c_void;

use chapbook_reader::chapbook_core::{EdgeSizes, FontSource, PageMetrics, Rotation, Size, Source};
use chapbook_reader::{Session, SessionConfig};
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring};
use jni::JNIEnv;

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

/// How this shell configures a session, in one place because the demo opens
/// sessions three ways — a path, a file descriptor, and one per conformance
/// check — and all three want the same answers.
///
/// [`FontSource::android_system`] is the whole font story: `/system/fonts`
/// for the faces, the families Android actually ships for the generics, and
/// Noto for fallback. Rung 3 had to do all three by hand through
/// `paint_resources`, which is an accessor for rasterizing a display list
/// and was never meant to configure anything.
fn config(library_dir: Option<String>) -> SessionConfig {
    let config = SessionConfig::new(FontSource::android_system());
    match library_dir {
        Some(dir) => config.with_library_dir(dir),
        None => config,
    }
}

/// A `String` from a `JString`, or `None` if the JVM would not give one up.
fn string_in(env: &mut JNIEnv, value: &JString) -> Option<String> {
    env.get_string(value).ok().map(Into::into)
}

fn string_out(env: &JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(s) => s.into_raw(),
        Err(_) => JObject::null().into_raw(),
    }
}

// ---- Diagnostics ----

/// Point the engine's `log` records at logcat.
///
/// Call before opening anything. Until a backend is installed the engine is
/// silent by design — and on Android that used to mean the failures only a
/// device hits were the ones nobody could see, which is what
/// `chapbook_core::diagnostics` was built for. `android_logger` is the
/// stock backend; nothing about it is chapbook's business beyond this call.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_initLogging(
    _env: JNIEnv,
    _class: JClass,
    verbose: jboolean,
) {
    let level = if verbose != 0 {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(level)
            .with_tag("chapbook"),
    );
    log::info!("logging to logcat at {level}");
}

// ---- Lifecycle ----

/// Opens a book from a filesystem path.
///
/// `library_dir` is `context.getFilesDir()`, handed over as an argument.
/// Rung 2 wrote it into the process environment with `setenv` because
/// `Session::open` read `CHAPBOOK_LIBRARY_DIR` on its own; that was a
/// stopgap on a sandboxed platform whose one true answer is not reachable
/// through an environment variable at all.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_open(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    library_dir: JString,
) -> jlong {
    let Some(path) = string_in(&mut env, &path) else {
        return 0;
    };
    let library_dir = string_in(&mut env, &library_dir);
    match Session::open_with(std::path::PathBuf::from(&path), config(library_dir)) {
        Ok(session) => into_handle(session),
        Err(e) => {
            log::error!("could not open {path}: {e}");
            0
        }
    }
}

/// Opens a book from a file descriptor — rung 5, and the only rung that
/// resembles what a real Android app does.
///
/// The storage access framework hands back a `content://` URI with no path
/// and, very often, no extension either: `ParcelFileDescriptor` is all
/// there is. `fd` must therefore be *detached* on the Kotlin side, because
/// the `File` built here owns it and closes it when the session drops.
///
/// `Format::Guess` is not a concession, it is the better answer — the EPUB
/// `mimetype` entry and the `%PDF` header are in the bytes, and a name that
/// was never going to arrive cannot be trusted anyway.
///
/// **A handle does not reach the library.** No path means no file to
/// fingerprint and no stable identity to key a position on, so a book
/// opened this way opens at the beginning every time. That is custody,
/// not source typing, and `docs/PLATFORM.md` owns it.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_openFd(
    mut env: JNIEnv,
    _class: JClass,
    fd: jint,
    library_dir: JString,
) -> jlong {
    if fd < 0 {
        log::error!("openFd got no descriptor");
        return 0;
    }
    let library_dir = string_in(&mut env, &library_dir);
    // SAFETY: Kotlin called `ParcelFileDescriptor.detachFd()`, which gives
    // up ownership; nothing else will read or close it.
    let file = unsafe {
        use std::os::fd::FromRawFd;
        std::fs::File::from_raw_fd(fd)
    };
    match Session::open_with(Source::reader(file), config(library_dir)) {
        Ok(session) => into_handle(session),
        Err(e) => {
            log::error!("could not open the descriptor: {e}");
            0
        }
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

/// What realizing the font source actually produced: faces loaded, and any
/// generic family pointed at a name no loaded face carries.
///
/// Zero faces means every page paginates blank, which takes navigation,
/// search and the table of contents with it — so this is still the first
/// thing worth putting on screen. It is now a fact the session reports
/// rather than a number read back out of the font database.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_fontReport(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let text = match unsafe { session(handle) } {
        None => "no session".to_string(),
        Some(s) => {
            let report = s.font_report();
            if report.is_clean() {
                format!("{} faces", report.faces)
            } else {
                let unresolved: Vec<String> = report
                    .unresolved_generics
                    .iter()
                    .map(|(generic, family)| format!("{generic}={family}"))
                    .collect();
                format!(
                    "{} faces, unresolved: {}",
                    report.faces,
                    unresolved.join(" ")
                )
            }
        }
    };
    string_out(&env, &text)
}

/// Let go of everything reconstructible, and save the position while there
/// is still a process to save it from.
///
/// This is `onStop`, which is the last callback Android guarantees. Rung 3
/// had nothing to call here.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_suspendSession(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if let Some(s) = unsafe { session(handle) } {
        s.suspend();
    }
}

/// `onTrimMemory`, which Android calls with a level and no argument about
/// what to do with it. Drops cached layouts and decoded images; the current
/// page is rebuilt on the next draw.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_releaseCaches(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if let Some(s) = unsafe { session(handle) } {
        s.release_caches();
    }
}

/// Bytes the session's caches are holding. The status line shows it beside
/// the budget, because a number nobody can see is a budget nobody trusts.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_cacheBytes(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    match unsafe { session(handle) } {
        Some(s) => s.cache_bytes() as jlong,
        None => -1,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_cacheBudget(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    match unsafe { session(handle) } {
        Some(s) => s.cache_budget() as jlong,
        None => -1,
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

/// Cycle the reading theme.
///
/// In the demo this is the middle tap zone, and it is there for a reason:
/// sepia is the only thing on screen whose red and blue channels differ, so
/// it is the one test that can tell premultiplied RGBA from BGRA. Black
/// text on white paper looks identical either way.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_cycleTheme(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if let Some(s) = unsafe { session(handle) } {
        s.cycle_theme();
    }
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
    string_out(&env, &title)
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

/// The device-pixel size the bitmap must be, packed `(width << 32) | height`.
///
/// The host allocates the surface, so the host has to be told how big it
/// is. `-1` until metrics are set.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_renderSize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jlong {
    match unsafe { session(handle) }.and_then(|s| s.render_size()) {
        Some((w, h)) => ((w as jlong) << 32) | (h as jlong & 0xffff_ffff),
        None => -1,
    }
}

/// Render the current page into an `ARGB_8888` bitmap.
///
/// Rung 3 rendered to a freshly allocated `Pixmap` and then memcpy'd it row
/// by row, once per page turn, because that was the only way in. It is now
/// what `render_into` was built for: `AndroidBitmap_lockPixels` hands back
/// the bitmap's own backing store, the engine rasterizes straight into it,
/// and for an unrotated page whose stride is exactly `width * 4` — which is
/// what Android gives — there is no intermediate and no copy at all.
///
/// The premultiplied-RGBA claim held up on a device: tiny-skia's output and
/// `ARGB_8888`'s memory layout are the same bytes, so nothing converts at
/// either end. Sepia is the check, since black on white cannot tell RGBA
/// from BGRA.
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
    let Some((want_w, want_h)) = session.render_size() else {
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
    // Checked here rather than left to `render_into`'s `false` so that a
    // caller who sized its bitmap from something other than `renderSize`
    // gets told which thing was wrong.
    if info.format != ANDROID_BITMAP_FORMAT_RGBA_8888
        || info.width != want_w
        || info.height != want_h
        || (info.stride as usize) < (want_w as usize) * 4
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

    let len = info.stride as usize * info.height as usize;
    // SAFETY: Android just reported this mapping's stride and height, and
    // the lock keeps it alive and unmoved until `unlockPixels` below.
    // Nothing else aliases it while it is locked.
    let dst = unsafe { std::slice::from_raw_parts_mut(pixels as *mut u8, len) };
    let drew = session.render_into(dst, info.width, info.height, info.stride as usize);

    // SAFETY: paired with the successful lock above.
    unsafe { AndroidBitmap_unlockPixels(raw_env, raw_bitmap) };
    if drew {
        0
    } else {
        -2
    }
}

// ---- Proving it ----

/// Run the shell conformance harness against this book and hand back its
/// report. This is the rung that tests the *binding* rather than the build:
/// the harness already watches the seam from outside, so driving it through
/// JNI asks whether the seam survived the trip.
///
/// It takes a path rather than an open handle because the harness opens its
/// own sessions — one per check, since "position survives a restart" cannot
/// be asked of a session that never stopped. That also means it needs the
/// library directory: a position has nowhere to survive to without one.
#[no_mangle]
pub extern "system" fn Java_com_ophymx_chapbook_Native_conformance(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    library_dir: JString,
) -> jstring {
    let Some(path) = string_in(&mut env, &path) else {
        return string_out(&env, "bad path");
    };
    let library_dir = string_in(&mut env, &library_dir);
    let open = || Session::open_with(std::path::PathBuf::from(&path), config(library_dir.clone()));
    // The harness wants an infallible factory. A book that will not open is
    // not a conformance failure, it is a different question, so say so
    // rather than reporting eleven mysterious ones.
    if let Err(e) = open() {
        log::error!("conformance could not open {path}: {e}");
        return string_out(&env, "could not open the book");
    }
    let report = chapbook_reader::conformance::Harness::new(|| {
        open().expect("the book opened a moment ago")
    })
    .run()
    .to_string();
    string_out(&env, &report)
}
