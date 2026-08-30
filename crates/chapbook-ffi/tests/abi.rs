//! Drive the C ABI the way a host does — through its own `extern "C"`
//! entry points, with raw pointers, out-parameters and status codes.
//!
//! It exists because of a hole the Android spike left: `chapbook-jni`'s
//! binding is `#[cfg(target_os = "android")]`, so **nothing in `cargo test`
//! ever compiled it**, and its two failure classes — a symbol that does not
//! exist and a signature that does not match — were caught by a shell
//! script after a device build or not at all. A C ABI that only CI on a
//! phone can exercise has the same problem. This has no emulator, no
//! device, and no second language in it.
//!
//! What it deliberately does *not* do is call the Rust API and compare.
//! Every call below goes through the boundary, because the boundary is what
//! is being tested.

use std::ffi::{c_char, CString};
use std::path::PathBuf;

use chapbook_ffi::*;

fn fixture(rel: &str) -> CString {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel);
    CString::new(path.to_string_lossy().into_owned()).expect("fixture path has no interior NUL")
}

fn cstr(value: &str) -> CString {
    CString::new(value).expect("no interior NUL")
}

/// The vendored faces, all three axes pinned, so this suite means the same
/// thing on any machine — and so it does not depend on the host having
/// fonts at all.
fn fonts() -> *mut cb_font_source {
    let dir = fixture("fonts");
    let family = cstr("Crimson Text");
    let fonts = unsafe { cb_font_source_embedded(dir.as_ptr(), family.as_ptr()) };
    assert!(!fonts.is_null(), "embedded font source: {}", last_error());
    fonts
}

/// A config with a library dir of this test's own, so nothing here reads or
/// writes the machine's real library.
fn config(name: &str) -> *mut cb_config {
    let config = unsafe { cb_config_new(fonts()) };
    assert!(!config.is_null(), "config: {}", last_error());
    let dir = std::env::temp_dir().join(format!("chapbook-ffi-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("library dir is creatable");
    let dir = cstr(&dir.to_string_lossy());
    assert_eq!(
        unsafe { cb_config_set_library_dir(config, dir.as_ptr()) },
        cb_status::CB_OK
    );
    config
}

fn open(name: &str, rel: &str) -> *mut cb_session {
    let path = fixture(rel);
    let session = unsafe { cb_session_open_path(path.as_ptr(), config(name)) };
    assert!(!session.is_null(), "open {rel}: {}", last_error());
    session
}

/// The two-call string idiom, exercised as a host would have to write it.
fn read_string(
    mut call: impl FnMut(*mut c_char, usize, *mut usize) -> cb_status,
) -> Result<String, cb_status> {
    let mut needed: usize = 0;
    let probe = call(std::ptr::null_mut(), 0, &mut needed);
    assert_eq!(
        probe,
        cb_status::CB_ERR_BUFFER_TOO_SMALL,
        "a zero-capacity probe must report the size it wanted"
    );
    assert!(needed >= 1, "needed always counts the NUL");

    let mut buf = vec![0u8; needed];
    let rc = call(buf.as_mut_ptr() as *mut c_char, buf.len(), &mut needed);
    if rc != cb_status::CB_OK {
        return Err(rc);
    }
    assert_eq!(buf.pop(), Some(0), "the callee NUL-terminates");
    Ok(String::from_utf8(buf).expect("UTF-8 out"))
}

fn last_error() -> String {
    read_string(|buf, cap, needed| unsafe { cb_last_error_message(buf, cap, needed) })
        .unwrap_or_else(|_| "<no message>".into())
}

fn metrics() -> cb_metrics {
    cb_metrics {
        width: 600.0,
        height: 800.0,
        margin_top: 40.0,
        margin_right: 40.0,
        margin_bottom: 40.0,
        margin_left: 40.0,
        dpi_scale: 1.0,
        rotation: cb_rotation::CB_ROTATION_NONE,
    }
}

// ---- The shape of the ABI itself ----

#[test]
fn the_build_reports_what_it_can_actually_do() {
    // The Android spike shipped for weeks without a library and said
    // nothing, because a build compiled without a capability has nothing to
    // report an error about. This is the answer to that, so it is worth a
    // test that it tells the truth rather than always returning zero.
    let caps = cb_capabilities();
    assert_eq!(
        caps & cb_capability::CB_CAP_LIBRARY as u32 != 0,
        cfg!(feature = "library")
    );
    assert_eq!(
        caps & cb_capability::CB_CAP_PDF as u32 != 0,
        cfg!(feature = "pdf")
    );
    assert!(cb_abi_version() > 0);
}

#[test]
fn null_handles_are_reported_and_never_dereferenced() {
    // A host will pass null. It must get a code, not a segfault, and the
    // *_free calls must tolerate it so error paths need no cascade of ifs.
    let mut moved = false;
    assert_eq!(
        unsafe { cb_session_next_page(std::ptr::null_mut(), &mut moved) },
        cb_status::CB_ERR_NULL_ARGUMENT
    );
    assert_eq!(
        unsafe {
            cb_session_render_size(std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut())
        },
        cb_status::CB_ERR_NULL_ARGUMENT
    );
    unsafe {
        cb_session_close(std::ptr::null_mut());
        cb_config_free(std::ptr::null_mut());
        cb_font_source_free(std::ptr::null_mut());
    }
}

#[test]
fn a_null_out_pointer_is_refused_rather_than_written_through() {
    let session = open("null-out", "epub/illustrated.epub");
    assert_eq!(
        unsafe { cb_session_position(session, std::ptr::null_mut()) },
        cb_status::CB_ERR_NULL_ARGUMENT
    );
    unsafe { cb_session_close(session) };
}

#[test]
fn a_bad_utf8_path_is_a_code_and_not_a_crash() {
    // 0xFF is not valid UTF-8 anywhere, and a host with a mangled filename
    // should learn that rather than open something surprising.
    let bad = [0xFFu8, 0x00];
    let session =
        unsafe { cb_session_open_path(bad.as_ptr() as *const c_char, config("bad-utf8")) };
    assert!(session.is_null());
    assert!(
        last_error().contains("UTF-8"),
        "the message should name the problem: {}",
        last_error()
    );
}

#[test]
fn the_string_idiom_reports_the_size_it_needs() {
    let session = open("strings", "epub/illustrated.epub");
    let title =
        read_string(|buf, cap, needed| unsafe { cb_session_title(session, buf, cap, needed) })
            .expect("title");
    assert!(!title.is_empty());

    // One byte short is an error with `needed` set, not a truncation.
    let mut needed = 0usize;
    let mut small = vec![0u8; title.len()];
    let rc = unsafe {
        cb_session_title(
            session,
            small.as_mut_ptr() as *mut c_char,
            small.len(),
            &mut needed,
        )
    };
    assert_eq!(rc, cb_status::CB_ERR_BUFFER_TOO_SMALL);
    assert_eq!(needed, title.len() + 1, "needed counts the NUL");
    assert!(small.iter().all(|b| *b == 0), "nothing was written");
    unsafe { cb_session_close(session) };
}

#[test]
fn a_bad_font_source_fails_the_open_instead_of_reading_blank() {
    // A source resolving to no faces used to paginate every book to one
    // blank page — rendering, navigating and conforming the whole way. It
    // is an error at construction now, and this ABI must pass that through
    // rather than hand back a working-looking handle.
    let dir = cstr("/nonexistent-directory-for-this-test");
    let family = cstr("Nothing");
    let fonts = unsafe { cb_font_source_embedded(dir.as_ptr(), family.as_ptr()) };
    let config = unsafe { cb_config_new(fonts) };
    let path = fixture("epub/illustrated.epub");
    let session = unsafe { cb_session_open_path(path.as_ptr(), config) };
    assert!(session.is_null(), "a fontless session must not open");
    assert!(
        last_error().to_lowercase().contains("font"),
        "the message should name fonts: {}",
        last_error()
    );
}

#[test]
fn invalid_metrics_are_refused_including_nan() {
    let session = open("metrics", "epub/illustrated.epub");
    for bad in [f32::NAN, 0.0, -1.0, f32::INFINITY] {
        let mut m = metrics();
        m.width = bad;
        assert_eq!(
            unsafe { cb_session_set_metrics(session, m) },
            cb_status::CB_ERR_INVALID_ARGUMENT,
            "width {bad} must be refused"
        );
    }
    // And with no valid metrics ever set, a render size is unavailable
    // rather than a guess.
    let (mut w, mut h) = (0u32, 0u32);
    assert_eq!(
        unsafe { cb_session_render_size(session, &mut w, &mut h) },
        cb_status::CB_ERR_UNAVAILABLE
    );
    unsafe { cb_session_close(session) };
}

// ---- Reading, through the boundary ----

#[test]
fn a_book_opens_paginates_and_turns() {
    let session = open("read", "epub/illustrated.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );

    let mut kind = cb_book_kind::CB_BOOK_COMIC;
    assert_eq!(
        unsafe { cb_session_book_kind(session, &mut kind) },
        cb_status::CB_OK
    );
    assert_eq!(kind, cb_book_kind::CB_BOOK_EPUB);

    let mut faces = 0usize;
    assert_eq!(
        unsafe { cb_session_font_face_count(session, &mut faces) },
        cb_status::CB_OK
    );
    assert!(faces > 0, "the embedded source produced faces");

    let mut spine_len = 0usize;
    assert_eq!(
        unsafe { cb_session_spine_len(session, &mut spine_len) },
        cb_status::CB_OK
    );
    assert!(spine_len > 0);

    let mut start = cb_position { spine: 9, page: 9 };
    assert_eq!(
        unsafe { cb_session_position(session, &mut start) },
        cb_status::CB_OK
    );

    // Walk with the returned flag, never by comparing positions.
    let mut turns = 0;
    loop {
        let mut moved = false;
        assert_eq!(
            unsafe { cb_session_next_page(session, &mut moved) },
            cb_status::CB_OK
        );
        if !moved {
            break;
        }
        turns += 1;
        assert!(turns < 10_000, "the walk terminates");
    }
    assert!(turns > 0, "the book has more than one page");

    // The end stands still.
    let mut at_end = cb_position { spine: 0, page: 0 };
    assert_eq!(
        unsafe { cb_session_position(session, &mut at_end) },
        cb_status::CB_OK
    );
    let mut moved = true;
    assert_eq!(
        unsafe { cb_session_next_page(session, &mut moved) },
        cb_status::CB_OK
    );
    assert!(!moved, "a turn past the end does not move");
    let mut still = cb_position { spine: 0, page: 0 };
    assert_eq!(
        unsafe { cb_session_position(session, &mut still) },
        cb_status::CB_OK
    );
    assert_eq!(still, at_end);

    // And back is symmetric.
    let mut back = false;
    assert_eq!(
        unsafe { cb_session_prev_page(session, &mut back) },
        cb_status::CB_OK
    );
    assert!(back);
    unsafe { cb_session_close(session) };
}

#[test]
fn a_quarter_turn_swaps_the_buffer_and_leaves_the_pagination_alone() {
    // Until this test, `cb_rotation` was a knob no caller in any language
    // had ever turned across the boundary: every case in this file set
    // `CB_ROTATION_NONE`, and `cb_session_render_into` takes a different
    // path for a turned page — through a temporary, copied row by row.
    let session = open("rotate", "epub/minimal.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );

    let (mut w, mut h) = (0u32, 0u32);
    assert_eq!(
        unsafe { cb_session_render_size(session, &mut w, &mut h) },
        cb_status::CB_OK
    );
    assert_eq!((w, h), (600, 800), "the page box, unrotated");
    let mut upright_pages = 0usize;
    assert_eq!(
        unsafe { cb_session_page_count(session, &mut upright_pages) },
        cb_status::CB_OK
    );

    // Same page box, quarter turn. `width`/`height` stay in *reading*
    // orientation — that is what the header now says in as many words —
    // so this is deliberately the same 600x800 as above.
    let turned = cb_metrics {
        rotation: cb_rotation::CB_ROTATION_QUARTER,
        ..metrics()
    };
    assert_eq!(
        unsafe { cb_session_set_metrics(session, turned) },
        cb_status::CB_OK
    );
    assert_eq!(
        unsafe { cb_session_render_size(session, &mut w, &mut h) },
        cb_status::CB_OK
    );
    assert_eq!((w, h), (800, 600), "the axes swap on the way out");

    // And the text did not reflow to fit the panel: rotation is a
    // property of the output. A host that saw the page count move here
    // would be looking at a locator bug, not a paint one.
    let mut turned_pages = 0usize;
    assert_eq!(
        unsafe { cb_session_page_count(session, &mut turned_pages) },
        cb_status::CB_OK
    );
    assert_eq!(turned_pages, upright_pages, "a turn is not a relayout");

    let stride = w as usize * 4;
    let mut surface = vec![0u8; stride * h as usize];
    assert_eq!(
        unsafe {
            cb_session_render_into(session, surface.as_mut_ptr(), surface.len(), w, h, stride)
        },
        cb_status::CB_OK
    );
    assert!(
        surface.iter().any(|b| *b != 0),
        "the rotated copy path actually wrote something"
    );
    assert_eq!(
        &surface[0..4],
        &[255, 255, 255, 255],
        "top-left is still opaque paper after the turn"
    );

    // The buffer the *unrotated* size would have asked for is now the
    // wrong shape, and has to be refused rather than half-filled.
    let mut wrong = vec![0u8; 600 * 4 * 800];
    assert_eq!(
        unsafe { cb_session_render_into(session, wrong.as_mut_ptr(), wrong.len(), 600, 800, 2400) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );

    unsafe { cb_session_close(session) };
}

#[test]
fn pixels_come_back_in_a_buffer_the_caller_owns() {
    let session = open("pixels", "epub/illustrated.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );

    let (mut w, mut h) = (0u32, 0u32);
    assert_eq!(
        unsafe { cb_session_render_size(session, &mut w, &mut h) },
        cb_status::CB_OK
    );
    assert_eq!((w, h), (600, 800), "unrotated, at scale 1");

    let stride = w as usize * 4;
    let mut surface = vec![0u8; stride * h as usize];
    assert_eq!(
        unsafe {
            cb_session_render_into(session, surface.as_mut_ptr(), surface.len(), w, h, stride)
        },
        cb_status::CB_OK
    );
    assert!(
        surface.iter().any(|b| *b != 0),
        "something was actually drawn"
    );
    // Premultiplied RGBA8888: the default theme's paper is opaque white.
    assert_eq!(
        &surface[0..4],
        &[255, 255, 255, 255],
        "top-left is white paper"
    );

    // A surface of the wrong size is refused, not misdrawn into — and is
    // reported as a bad argument rather than as an unavailable page, so a
    // caller is sent to look at its own arithmetic.
    assert_eq!(
        unsafe {
            cb_session_render_into(
                session,
                surface.as_mut_ptr(),
                surface.len(),
                w - 1,
                h,
                stride,
            )
        },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    assert!(
        last_error().contains("render_size"),
        "the message should point at the fix: {}",
        last_error()
    );
    // A stride narrower than a row is caught before any write happens.
    assert_eq!(
        unsafe { cb_session_render_into(session, surface.as_mut_ptr(), surface.len(), w, h, 4) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    // As is a buffer too short for the stride it claims.
    assert_eq!(
        unsafe { cb_session_render_into(session, surface.as_mut_ptr(), 16, w, h, stride) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    unsafe { cb_session_close(session) };
}

#[test]
fn a_theme_change_reaches_the_pixels() {
    // Sepia is the one setting whose red and blue channels differ, so it is
    // what tells a premultiplied-RGBA buffer from a BGRA one. Black text on
    // white paper looks identical either way.
    let session = open("theme", "epub/illustrated.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );

    let mut settings = unsafe {
        let mut out = std::mem::zeroed::<cb_settings>();
        assert_eq!(cb_session_settings(session, &mut out), cb_status::CB_OK);
        out
    };
    assert_eq!(settings.theme, cb_theme::CB_THEME_LIGHT);

    settings.theme = cb_theme::CB_THEME_SEPIA;
    assert_eq!(
        unsafe {
            cb_session_set_settings(session, settings, cb_settings_scope::CB_SCOPE_THIS_BOOK)
        },
        cb_status::CB_OK
    );

    let (mut w, mut h) = (0u32, 0u32);
    assert_eq!(
        unsafe { cb_session_render_size(session, &mut w, &mut h) },
        cb_status::CB_OK
    );
    let stride = w as usize * 4;
    let mut surface = vec![0u8; stride * h as usize];
    assert_eq!(
        unsafe {
            cb_session_render_into(session, surface.as_mut_ptr(), surface.len(), w, h, stride)
        },
        cb_status::CB_OK
    );
    let (r, g, b) = (surface[0], surface[1], surface[2]);
    assert!(
        r > b && g > b,
        "sepia paper is warm: got r={r} g={g} b={b} — if b is highest the channels are swapped"
    );
    unsafe { cb_session_close(session) };
}

#[test]
fn a_reading_position_survives_a_close_and_reopen() {
    // The whole point of the library reaching the boundary at all. Uses one
    // library dir across two sessions, which is also the arrangement that
    // caught the restored-offset bug.
    //
    // Asked of the ABI rather than of `cfg!`, deliberately: this is the
    // question a host has to be able to ask, and a build without the
    // library remembers nothing *correctly*. Skipping on the answer is what
    // a host would do, so the test does it the same way.
    if cb_capabilities() & cb_capability::CB_CAP_LIBRARY as u32 == 0 {
        eprintln!("skipped: this build has no library, so nothing persists");
        return;
    }
    // `long.epub`, so six turns land somewhere a reopen has to actually
    // find again — in a two-page book the position restores correctly by
    // accident.
    let path = fixture("epub/long.epub");
    let dir = std::env::temp_dir().join(format!("chapbook-ffi-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("library dir");
    let dir_c = cstr(&dir.to_string_lossy());

    let open_one = || {
        let config = unsafe { cb_config_new(fonts()) };
        assert_eq!(
            unsafe { cb_config_set_library_dir(config, dir_c.as_ptr()) },
            cb_status::CB_OK
        );
        let session = unsafe { cb_session_open_path(path.as_ptr(), config) };
        assert!(!session.is_null(), "open: {}", last_error());
        assert_eq!(
            unsafe { cb_session_set_metrics(session, metrics()) },
            cb_status::CB_OK
        );
        session
    };

    let left_at = {
        let session = open_one();
        for _ in 0..6 {
            let mut moved = false;
            unsafe { cb_session_next_page(session, &mut moved) };
        }
        let mut at = cb_position { spine: 0, page: 0 };
        unsafe { cb_session_position(session, &mut at) };
        assert_eq!(unsafe { cb_session_suspend(session) }, cb_status::CB_OK);
        unsafe { cb_session_close(session) };
        at
    };
    assert!(left_at.spine > 0 || left_at.page > 0, "moved off the start");

    let session = open_one();
    let mut back = cb_position { spine: 0, page: 0 };
    unsafe { cb_session_position(session, &mut back) };
    assert_eq!(back.spine, left_at.spine, "reopened in the same unit");
    unsafe { cb_session_close(session) };
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_descriptor_book_keeps_its_place_across_the_boundary() {
    // The custody flow a phone runs, spelled in C: resolve, open a
    // descriptor, read, suspend, relaunch, resolve again, open again — and
    // come back where the reader left off. The engine adopts the book by
    // its bytes' fingerprint, so no path ever crosses.
    if cb_capabilities() & cb_capability::CB_CAP_LIBRARY as u32 == 0 {
        eprintln!("skipped: this build has no library, so nothing persists");
        return;
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/epub/long.epub");
    let dir = std::env::temp_dir().join(format!("chapbook-ffi-fd-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("library dir");
    let dir_c = cstr(&dir.to_string_lossy());

    let open_one = || {
        use std::os::fd::IntoRawFd;
        let fd = std::fs::File::open(&path)
            .expect("fixture opens")
            .into_raw_fd();
        let config = unsafe { cb_config_new(fonts()) };
        assert_eq!(
            unsafe { cb_config_set_library_dir(config, dir_c.as_ptr()) },
            cb_status::CB_OK
        );
        let session = unsafe { cb_session_open_fd(fd, cb_format::CB_FORMAT_GUESS, config) };
        assert!(!session.is_null(), "open fd: {}", last_error());
        assert_eq!(
            unsafe { cb_session_set_metrics(session, metrics()) },
            cb_status::CB_OK
        );
        session
    };

    let left_at = {
        let session = open_one();
        for _ in 0..6 {
            let mut moved = false;
            unsafe { cb_session_next_page(session, &mut moved) };
        }
        let mut at = cb_position { spine: 0, page: 0 };
        unsafe { cb_session_position(session, &mut at) };
        assert_eq!(unsafe { cb_session_suspend(session) }, cb_status::CB_OK);
        unsafe { cb_session_close(session) };
        at
    };
    assert!(left_at.spine > 0 || left_at.page > 0, "moved off the start");

    let session = open_one();
    let mut back = cb_position { spine: 0, page: 0 };
    unsafe { cb_session_position(session, &mut back) };
    assert_eq!(back.spine, left_at.spine, "reopened in the same unit");
    unsafe { cb_session_close(session) };
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn memory_calls_are_answerable_and_do_not_lose_the_place() {
    let session = open("memory", "epub/illustrated.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );
    let mut moved = false;
    unsafe { cb_session_next_page(session, &mut moved) };
    let mut before = cb_position { spine: 0, page: 0 };
    unsafe { cb_session_position(session, &mut before) };

    let (mut budget, mut used) = (0usize, 0usize);
    assert_eq!(
        unsafe { cb_session_cache_budget(session, &mut budget) },
        cb_status::CB_OK
    );
    assert!(budget > 0);
    assert_eq!(
        unsafe { cb_session_cache_bytes(session, &mut used) },
        cb_status::CB_OK
    );

    // onTrimMemory: give the caches up, keep the place.
    assert_eq!(
        unsafe { cb_session_release_caches(session) },
        cb_status::CB_OK
    );
    let mut after = cb_position { spine: 9, page: 9 };
    unsafe { cb_session_position(session, &mut after) };
    assert_eq!(after, before, "releasing caches is not navigation");

    // And the page still renders, rebuilt from nothing.
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { cb_session_render_size(session, &mut w, &mut h) };
    let stride = w as usize * 4;
    let mut surface = vec![0u8; stride * h as usize];
    assert_eq!(
        unsafe {
            cb_session_render_into(session, surface.as_mut_ptr(), surface.len(), w, h, stride)
        },
        cb_status::CB_OK
    );
    unsafe { cb_session_close(session) };
}

#[test]
fn a_session_moves_between_threads() {
    // `Send` and not `Sync` is the contract the header states. Moving one
    // to another thread and using it there must work; this is what a host
    // that opens on a worker and reads on the UI thread does.
    let session = open("threads", "epub/illustrated.epub") as usize;
    let handle = std::thread::spawn(move || {
        let session = session as *mut cb_session;
        assert_eq!(
            unsafe { cb_session_set_metrics(session, metrics()) },
            cb_status::CB_OK
        );
        let mut moved = false;
        assert_eq!(
            unsafe { cb_session_next_page(session, &mut moved) },
            cb_status::CB_OK
        );
        unsafe { cb_session_close(session) };
    });
    handle.join().expect("the worker did not panic");
}

#[test]
fn the_bytes_decide_the_format_not_the_name() {
    // The same claim the Android spike checked against a `content://` URI,
    // here with no Android in sight: hand over bytes with the format left
    // unstated and the sniffer gets it right.
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/epub/illustrated.epub");
    let bytes = std::fs::read(&path).expect("fixture is readable");
    let session = unsafe {
        cb_session_open_bytes(
            bytes.as_ptr(),
            bytes.len(),
            cb_format::CB_FORMAT_GUESS,
            config("sniff"),
        )
    };
    assert!(!session.is_null(), "open from bytes: {}", last_error());
    let mut kind = cb_book_kind::CB_BOOK_COMIC;
    unsafe { cb_session_book_kind(session, &mut kind) };
    assert_eq!(kind, cb_book_kind::CB_BOOK_EPUB, "sniffed as EPUB");
    unsafe { cb_session_close(session) };
}

// ---- Input ----

/// The tap helper every input test wants: what does a tap here mean.
fn tap(session: *const cb_session, x: f32, y: f32) -> cb_action {
    let mut action = cb_action::CB_ACTION_TOGGLE_MENU;
    assert_eq!(
        unsafe { cb_session_tap_action(session, x, y, &mut action) },
        cb_status::CB_OK,
        "tap at ({x}, {y}): {}",
        last_error()
    );
    action
}

#[test]
fn taps_resolve_through_the_book_and_not_the_shell() {
    // The same tap, on the same page box, means the opposite thing in an
    // RTL book — and the direction is nowhere in the calls a host makes,
    // which is the point: a shell that could pass a direction would pass
    // `Ltr` on every platform it ships to.
    let session = open("tap-ltr", "epub/minimal.epub");

    // Before metrics there are no thirds to land in.
    let mut action = cb_action::CB_ACTION_NONE;
    assert_eq!(
        unsafe { cb_session_tap_action(session, 100.0, 400.0, &mut action) },
        cb_status::CB_ERR_UNAVAILABLE
    );

    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );
    let mut direction = cb_reading_direction::CB_DIRECTION_RTL;
    assert_eq!(
        unsafe { cb_session_reading_direction(session, &mut direction) },
        cb_status::CB_OK
    );
    assert_eq!(direction, cb_reading_direction::CB_DIRECTION_LTR);

    assert_eq!(tap(session, 100.0, 400.0), cb_action::CB_ACTION_PREV_PAGE);
    assert_eq!(tap(session, 500.0, 400.0), cb_action::CB_ACTION_NEXT_PAGE);
    assert_eq!(tap(session, 300.0, 400.0), cb_action::CB_ACTION_TOGGLE_MENU);
    // NaN is not a coordinate, and must not quietly resolve to a band.
    assert_eq!(
        unsafe { cb_session_tap_action(session, f32::NAN, 400.0, &mut action) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    unsafe { cb_session_close(session) };

    let rtl = open("tap-rtl", "epub/page-direction.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(rtl, metrics()) },
        cb_status::CB_OK
    );
    let mut direction = cb_reading_direction::CB_DIRECTION_LTR;
    assert_eq!(
        unsafe { cb_session_reading_direction(rtl, &mut direction) },
        cb_status::CB_OK
    );
    assert_eq!(direction, cb_reading_direction::CB_DIRECTION_RTL);
    assert_eq!(tap(rtl, 100.0, 400.0), cb_action::CB_ACTION_NEXT_PAGE);
    assert_eq!(tap(rtl, 500.0, 400.0), cb_action::CB_ACTION_PREV_PAGE);
    unsafe { cb_session_close(rtl) };
}

#[test]
fn a_turned_panel_does_not_turn_the_tap_zones() {
    // Taps arrive in panel coordinates and the rotation is undone inside,
    // so a host drawing to a turned panel forwards what the touch event
    // carries. The failure this guards is a reader whose page turns are
    // ninety degrees out — which is what forwarding these coordinates
    // through unrotated zones would produce (300 of 800 is the middle).
    let session = open("tap-turned", "epub/minimal.epub");
    let mut turned = metrics();
    turned.width = 800.0;
    turned.height = 600.0;
    turned.rotation = cb_rotation::CB_ROTATION_QUARTER;
    assert_eq!(
        unsafe { cb_session_set_metrics(session, turned) },
        cb_status::CB_OK
    );
    // The panel is 600x800. Reading runs down it, so the bottom of the
    // panel is the next-page edge and the top the previous-page edge.
    assert_eq!(tap(session, 300.0, 700.0), cb_action::CB_ACTION_NEXT_PAGE);
    assert_eq!(tap(session, 300.0, 100.0), cb_action::CB_ACTION_PREV_PAGE);
    unsafe { cb_session_close(session) };
}

#[test]
fn the_middle_band_is_the_hosts_to_configure() {
    let session = open("tap-zones", "epub/minimal.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );

    // A host with its own menu gesture makes the middle inert.
    assert_eq!(
        unsafe { cb_session_set_tap_zones(session, 0.4, 0.4, cb_action::CB_ACTION_NONE) },
        cb_status::CB_OK
    );
    assert_eq!(tap(session, 300.0, 400.0), cb_action::CB_ACTION_NONE);
    assert_eq!(tap(session, 100.0, 400.0), cb_action::CB_ACTION_PREV_PAGE);

    // Or binds it to something else entirely.
    assert_eq!(
        unsafe { cb_session_set_tap_zones(session, 0.3, 0.3, cb_action::CB_ACTION_CYCLE_THEME) },
        cb_status::CB_OK
    );
    assert_eq!(tap(session, 300.0, 400.0), cb_action::CB_ACTION_CYCLE_THEME);

    // A fraction outside the page, or one that is not a number, is a
    // mistake worth hearing about, not a policy.
    assert_eq!(
        unsafe { cb_session_set_tap_zones(session, 1.5, 0.3, cb_action::CB_ACTION_NONE) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { cb_session_set_tap_zones(session, f32::NAN, 0.3, cb_action::CB_ACTION_NONE) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    // And the refused configuration did not half-apply.
    assert_eq!(tap(session, 300.0, 400.0), cb_action::CB_ACTION_CYCLE_THEME);
    unsafe { cb_session_close(session) };
}

#[test]
fn the_default_key_table_answers_without_a_session() {
    // The table is the value, not the mutability: it already knows the
    // bezel buttons on a Kobo and the volume keys an Android reader
    // borrows, with no session in sight.
    assert_eq!(
        cb_key_default_action(cb_key::CB_KEY_PAGE_DOWN),
        cb_action::CB_ACTION_NEXT_PAGE
    );
    assert_eq!(
        cb_key_default_action(cb_key::CB_KEY_TURN_PREV),
        cb_action::CB_ACTION_PREV_PAGE
    );
    assert_eq!(
        cb_key_default_action(cb_key::CB_KEY_VOLUME_UP),
        cb_action::CB_ACTION_PREV_PAGE
    );

    assert_eq!(
        cb_char_default_action('n' as u32),
        cb_action::CB_ACTION_NEXT_UNIT
    );
    // ASCII case is folded here, so a host need not care.
    assert_eq!(
        cb_char_default_action('N' as u32),
        cb_action::CB_ACTION_NEXT_UNIT
    );
    assert_eq!(
        cb_char_default_action('x' as u32),
        cb_action::CB_ACTION_NONE
    );
    // A surrogate is not a scalar value, and answers nothing rather than
    // panicking on the way to a char.
    assert_eq!(cb_char_default_action(0xD800), cb_action::CB_ACTION_NONE);
}

#[test]
fn apply_answers_repaint_and_consumed_separately() {
    let session = open("apply", "epub/illustrated.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );
    let mut outcome = cb_action_outcome::CB_OUTCOME_CHANGED;

    // The case the outcome type was built for, straight from the Android
    // device run: at unit 0 page 0 a previous-page does not move, and the
    // event is still the reader's — a host that forwards it gets the
    // system's volume slider drawn over the book.
    assert_eq!(
        unsafe { cb_session_apply(session, cb_action::CB_ACTION_PREV_PAGE, &mut outcome) },
        cb_status::CB_OK
    );
    assert_eq!(outcome, cb_action_outcome::CB_OUTCOME_UNCHANGED);

    assert_eq!(
        unsafe { cb_session_apply(session, cb_action::CB_ACTION_NEXT_PAGE, &mut outcome) },
        cb_status::CB_OK
    );
    assert_eq!(outcome, cb_action_outcome::CB_OUTCOME_CHANGED);

    // The engine has no chrome, and an empty back trail is the platform's
    // Back to take — both hand the event back to the host.
    assert_eq!(
        unsafe { cb_session_apply(session, cb_action::CB_ACTION_TOGGLE_MENU, &mut outcome) },
        cb_status::CB_OK
    );
    assert_eq!(outcome, cb_action_outcome::CB_OUTCOME_UNHANDLED);
    assert_eq!(
        unsafe { cb_session_apply(session, cb_action::CB_ACTION_BACK, &mut outcome) },
        cb_status::CB_OK
    );
    assert_eq!(outcome, cb_action_outcome::CB_OUTCOME_UNHANDLED);

    // NONE is a bug in the caller, not a quiet no-op.
    assert_eq!(
        unsafe { cb_session_apply(session, cb_action::CB_ACTION_NONE, &mut outcome) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    unsafe { cb_session_close(session) };
}

// ---- The text surface ----

/// Set metrics and force a layout the way a host does: by asking a
/// question whose answer needs one.
fn laid_out(name: &str, rel: &str) -> *mut cb_session {
    let session = open(name, rel);
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );
    let mut pages: usize = 0;
    assert_eq!(
        unsafe { cb_session_page_count(session, &mut pages) },
        cb_status::CB_OK
    );
    assert!(pages > 0);
    session
}

#[test]
fn text_runs_cross_the_boundary() {
    let session = laid_out("text-runs", "epub/minimal.epub");

    let mut count: usize = 0;
    assert_eq!(
        unsafe { cb_session_page_text_run_count(session, &mut count) },
        cb_status::CB_OK
    );
    assert!(count > 0, "a text page has runs");

    let mut run = cb_text_run {
        rect: cb_rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        },
        locator_start: 1,
        locator_end: 0,
    };
    assert_eq!(
        unsafe { cb_session_page_text_run(session, 0, &mut run) },
        cb_status::CB_OK
    );
    assert!(run.rect.w > 0.0 && run.rect.h > 0.0, "{run:?}");
    assert!(run.locator_end >= run.locator_start, "{run:?}");

    let text = read_string(|buf, cap, needed| unsafe {
        cb_session_page_text_run_text(session, 0, buf, cap, needed)
    })
    .expect("run text crosses");
    assert!(!text.is_empty());

    unsafe { cb_session_close(session) };
}

#[test]
fn words_and_speakable_text_cross() {
    let session = laid_out("text-words", "epub/minimal.epub");

    let speakable = read_string(|buf, cap, needed| unsafe {
        cb_session_page_speakable_text(session, buf, cap, needed)
    })
    .expect("speakable text crosses");
    assert!(!speakable.is_empty());

    let mut words: usize = 0;
    assert_eq!(
        unsafe { cb_session_page_word_count(session, &mut words) },
        cb_status::CB_OK
    );
    assert!(words > 0, "a text page has words");

    let mut span = cb_word_span {
        text_start: 0,
        text_end: 0,
        locator_start: 0,
        locator_end: 0,
    };
    assert_eq!(
        unsafe { cb_session_page_word(session, 0, &mut span) },
        cb_status::CB_OK
    );
    assert!(span.text_end > span.text_start, "{span:?}");
    assert!(span.locator_end > span.locator_start, "{span:?}");
    // The span indexes the string it was defined against.
    let chars = speakable.chars().count() as u32;
    assert!(span.text_end <= chars, "{span:?} over {chars} chars");

    unsafe { cb_session_close(session) };
}

#[test]
fn text_surface_is_unavailable_before_metrics() {
    let session = open("text-early", "epub/minimal.epub");
    let mut count: usize = 0;
    assert_eq!(
        unsafe { cb_session_page_text_run_count(session, &mut count) },
        cb_status::CB_ERR_UNAVAILABLE
    );
    let mut needed: usize = 0;
    assert_eq!(
        unsafe { cb_session_page_speakable_text(session, std::ptr::null_mut(), 0, &mut needed) },
        cb_status::CB_ERR_UNAVAILABLE
    );
    unsafe { cb_session_close(session) };
}

#[test]
fn text_run_index_out_of_range_is_invalid() {
    let session = laid_out("text-range", "epub/minimal.epub");
    let mut count: usize = 0;
    assert_eq!(
        unsafe { cb_session_page_text_run_count(session, &mut count) },
        cb_status::CB_OK
    );
    let mut run = cb_text_run {
        rect: cb_rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        },
        locator_start: 0,
        locator_end: 0,
    };
    assert_eq!(
        unsafe { cb_session_page_text_run(session, count, &mut run) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    let mut words: usize = 0;
    assert_eq!(
        unsafe { cb_session_page_word_count(session, &mut words) },
        cb_status::CB_OK
    );
    let mut span = cb_word_span {
        text_start: 0,
        text_end: 0,
        locator_start: 0,
        locator_end: 0,
    };
    assert_eq!(
        unsafe { cb_session_page_word(session, words, &mut span) },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );
    unsafe { cb_session_close(session) };
}

#[test]
fn range_rects_two_call_idiom() {
    let session = laid_out("text-rects", "epub/minimal.epub");

    // A range with geometry: the first run's own.
    let mut run = cb_text_run {
        rect: cb_rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        },
        locator_start: 0,
        locator_end: 0,
    };
    assert_eq!(
        unsafe { cb_session_page_text_run(session, 0, &mut run) },
        cb_status::CB_OK
    );

    // The sizing call, as a host writes it.
    let mut needed: usize = 0;
    assert_eq!(
        unsafe {
            cb_session_range_rects(
                session,
                run.locator_start,
                run.locator_end,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        },
        cb_status::CB_ERR_BUFFER_TOO_SMALL
    );
    assert!(needed > 0, "the first run has geometry");

    let mut rects = vec![
        cb_rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        needed
    ];
    assert_eq!(
        unsafe {
            cb_session_range_rects(
                session,
                run.locator_start,
                run.locator_end,
                rects.as_mut_ptr(),
                rects.len(),
                &mut needed,
            )
        },
        cb_status::CB_OK
    );
    assert_eq!(needed, rects.len());
    for rect in &rects {
        assert!(rect.w > 0.0 && rect.h > 0.0, "{rect:?}");
    }

    // An empty range sizes to zero, and a zero-capacity call for it is OK.
    assert_eq!(
        unsafe {
            cb_session_range_rects(
                session,
                run.locator_start,
                run.locator_start,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        },
        cb_status::CB_OK
    );
    assert_eq!(needed, 0);

    unsafe { cb_session_close(session) };
}

#[test]
fn word_at_answers_under_text() {
    let session = laid_out("text-word-at", "epub/minimal.epub");

    // Where the text sits depends on the fixture fonts, so sweep for it —
    // the same discipline the session tests use.
    let mut hit = None;
    'sweep: for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            let (mut start, mut end) = (0u32, 0u32);
            if unsafe { cb_session_word_at(session, x as f32, y as f32, &mut start, &mut end) }
                == cb_status::CB_OK
            {
                hit = Some((start, end));
                break 'sweep;
            }
        }
    }
    let (start, end) = hit.expect("some point on the page is a word");
    assert!(end > start);

    // The word has geometry, reachable by the same range.
    let mut needed: usize = 0;
    assert_eq!(
        unsafe {
            cb_session_range_rects(session, start, end, std::ptr::null_mut(), 0, &mut needed)
        },
        cb_status::CB_ERR_BUFFER_TOO_SMALL
    );
    assert!(needed > 0);

    unsafe { cb_session_close(session) };
}

/// The font family crosses this ABI on its own calls, because it is a
/// string and `cb_settings` is plain data a host holds by value.
///
/// The trap that shape creates, and the reason this test exists: a host
/// that reads the settings, changes the font *size*, and writes them back
/// must not silently lose the typeface on the way through. There is no
/// field for it in the struct, so it has to be carried across.
#[test]
fn a_chosen_font_survives_a_settings_round_trip() {
    let session = open("font-family", "epub/illustrated.epub");
    assert_eq!(
        unsafe { cb_session_set_metrics(session, metrics()) },
        cb_status::CB_OK
    );

    // Nothing chosen reads back as empty, not as an error — one branch for
    // a host showing "Publisher's font" in a picker.
    let initial = read_string(|buf, cap, needed| unsafe {
        cb_session_font_family(session, buf, cap, needed)
    })
    .expect("font family");
    assert_eq!(initial, "");

    // Offer what the session can actually match, then choose one.
    let mut count = 0usize;
    assert_eq!(
        unsafe { cb_session_font_family_count(session, &mut count) },
        cb_status::CB_OK
    );
    assert!(count > 0, "a picker needs something to offer");
    let first = read_string(|buf, cap, needed| unsafe {
        cb_session_font_family_at(session, 0, buf, cap, needed)
    })
    .expect("a family name");
    assert!(!first.is_empty());

    let chosen = std::ffi::CString::new(first.clone()).unwrap();
    assert_eq!(
        unsafe {
            cb_session_set_font_family(
                session,
                chosen.as_ptr(),
                cb_settings_scope::CB_SCOPE_THIS_BOOK,
            )
        },
        cb_status::CB_OK
    );
    let now = read_string(|buf, cap, needed| unsafe {
        cb_session_font_family(session, buf, cap, needed)
    })
    .expect("font family");
    assert_eq!(now, first);

    // Now the trap: an unrelated settings write.
    let mut settings = unsafe {
        let mut out = std::mem::zeroed::<cb_settings>();
        assert_eq!(cb_session_settings(session, &mut out), cb_status::CB_OK);
        out
    };
    settings.base_font_px += 2.0;
    assert_eq!(
        unsafe {
            cb_session_set_settings(session, settings, cb_settings_scope::CB_SCOPE_THIS_BOOK)
        },
        cb_status::CB_OK
    );
    let after = read_string(|buf, cap, needed| unsafe {
        cb_session_font_family(session, buf, cap, needed)
    })
    .expect("font family");
    assert_eq!(
        after, first,
        "changing the font size cleared the chosen typeface"
    );

    // Null returns the book to the publisher's font.
    assert_eq!(
        unsafe {
            cb_session_set_font_family(
                session,
                std::ptr::null(),
                cb_settings_scope::CB_SCOPE_THIS_BOOK,
            )
        },
        cb_status::CB_OK
    );
    let cleared = read_string(|buf, cap, needed| unsafe {
        cb_session_font_family(session, buf, cap, needed)
    })
    .expect("font family");
    assert_eq!(cleared, "");

    // Past the end is an argument error, not a crash. The count is read
    // again first, deliberately: a book's own `@font-face` families join
    // the database as units lay out, so the number captured before the
    // relayout above is already stale. That is documented behaviour and
    // this is what it looks like from a host.
    let mut grown = 0usize;
    assert_eq!(
        unsafe { cb_session_font_family_count(session, &mut grown) },
        cb_status::CB_OK
    );
    assert!(grown >= count, "the family list should only grow");
    assert_eq!(
        unsafe {
            cb_session_font_family_at(
                session,
                grown,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            )
        },
        cb_status::CB_ERR_INVALID_ARGUMENT
    );

    unsafe { cb_session_close(session) };
}
