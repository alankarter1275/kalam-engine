//! The viewer proper, compiled on Windows only.
//!
//! Win32 reference viewer: the third shell over `chapbook_reader::Session`,
//! and the first one that pumps its own message loop. Everything
//! book-shaped lives in the session; this file is `user32` and `gdi32`
//! plumbing.
//!
//! Sources: an `.epub`, `.cbz` or `.pdf` path, or an OPDS URL
//! (page-streamed comic).
//!
//! Input goes through `chapbook_core::input` rather than being spelled out
//! here: this file turns a virtual-key code or a `WM_CHAR` into a `Key` and
//! a click into a point, and `KeyMap`/`TapZones` decide what either one
//! means. So the bindings are the engine's defaults — arrows/PageUp/
//! PageDown/space turn pages, n/p skip units, +/- size the text, t cycles
//! the theme, b and Backspace go back. Two of them are Windows' own:
//! `VK_BROWSER_BACK` is bound to going back, and the two thumb buttons on a
//! mouse arrive as `WM_XBUTTONDOWN` and are handed to `Key::TurnPrev` and
//! `Key::TurnNext` — the bezel-button vocabulary `KeyMap` has carried since
//! it was written and that no desktop shell had ever delivered.
//!
//! What stays this shell's own is what is not an `Action`: c copies the
//! selection, h highlights it, q/Escape quit. Press-drag over text selects,
//! a press on a link follows it, and a press on a stored highlight reports
//! it — this is the first shell in the workspace that runs all three hit
//! tests in the order `SHELLS.md` specifies.
//!
//! Rendering: the session rasterizes with tiny-skia at device pixels, and
//! the page reaches the window as a top-down 32bpp DIB through
//! `StretchDIBits`. No swapchain, no compositor, no third-party surface
//! crate — `BeginPaint`, one blit, `EndPaint`.
//!
//! Accessibility is `crate::uia`: `WM_GETOBJECT` hands out a document
//! element with a text pattern on it, and every paint re-captures the page
//! for it. A picture of text is unusable with a screen reader, and this is
//! the half of that job Windows asks for.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::core::{w, BSTR, HSTRING};
use windows::Win32::Foundation::{
    COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, StretchDIBits,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, NotificationKind_Other, NotificationProcessing_MostRecent,
    UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId, UiaClientsAreListening,
    UiaDisconnectProvider, UiaRaiseAutomationEvent, UiaRaiseNotificationEvent,
    UiaReturnRawElementProvider, UiaRootObjectId,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_BACK, VK_BROWSER_BACK, VK_DOWN, VK_ESCAPE, VK_LEFT,
    VK_NEXT, VK_PRIOR, VK_RIGHT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongPtrW, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, CREATESTRUCTW,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW, MSG, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_SHOW, WHEEL_DELTA, WINDOW_EX_STYLE, WM_APP, WM_CHAR, WM_CREATE, WM_DESTROY,
    WM_DPICHANGED, WM_ENDSESSION, WM_ERASEBKGND, WM_GETOBJECT, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_XBUTTONDOWN, WNDCLASSW,
    WS_OVERLAPPEDWINDOW, XBUTTON1,
};

use chapbook_core::{
    ActionOutcome, EdgeSizes, Key, KeyMap, PageMetrics, Rotation, Size, TapZones, Theme,
};
use chapbook_reader::{Session, SessionEvent};

use crate::uia::{apply_selection, PageProvider, PageSnapshot, WM_CHAPBOOK_SELECT};

/// The loader thread's wakeup, posted rather than sent.
///
/// `WM_APP` is the first value Windows reserves for an application's own
/// messages, and this shell has exactly one.
const WM_CHAPBOOK_WAKE: u32 = WM_APP;

/// `CF_UNICODETEXT`, as a plain `u32` because that is what
/// `SetClipboardData` takes. The constant itself lives in
/// `Win32::System::Ole::CF_UNICODETEXT` as a `CLIPBOARD_FORMAT`, which the
/// UIA vtables drag into this build anyway.
const CF_UNICODETEXT: u32 = windows::Win32::System::Ole::CF_UNICODETEXT.0 as u32;

/// A mouse drag that never exceeded this many logical pixels was a tap.
/// A mouse's slop, not a finger's — this shell is not driving a
/// touchscreen, and a shell that is wants its platform's own threshold.
const TAP_SLOP: f32 = 4.0;

pub fn run() -> std::process::ExitCode {
    // See chapbook-viewer: the engine reports through `log` and installs no
    // backend, so it is silent until a shell gives it somewhere to speak.
    chapbook_core::log_to_stderr();

    // Before any window exists, and before anything asks a DPI question.
    // Per-monitor v2 is what makes `GetDpiForWindow` tell the truth after a
    // drag between a laptop panel and an external monitor; without it
    // Windows lies to the process and bitmap-stretches the result, which on
    // a page of text is visible as blur and nothing else.
    //
    // It fails when the awareness is already set — a manifest, or a host
    // process that embedded us — and that is not this shell's business to
    // override.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let Some(source) = std::env::args().nth(1) else {
        eprintln!("usage: chapbook-viewer-win32 <book.epub|comic.cbz|doc.pdf|opds-url>");
        return std::process::ExitCode::from(2);
    };
    // The host's fonts and, for OPDS catalogs, the host's environment.
    // Windows has a real secret store behind `CredReadW`, and a shell that
    // wanted one would swap it in here and change nothing else — the
    // environment is what the other desktop shells use and what this one
    // inherits until somebody needs better.
    let config = chapbook_reader::SessionConfig::new(chapbook_core::FontSource::host())
        .with_credentials(std::sync::Arc::new(chapbook_core::EnvCredentials));
    let session = match Session::open_with(&source, config) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("chapbook-viewer-win32: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    match main_window(session) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chapbook-viewer-win32: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

/// Everything from the window class to the last `DispatchMessageW`.
fn main_window(session: Session) -> windows::core::Result<()> {
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let class = w!("ChapbookViewerWindow");

    let wc = WNDCLASSW {
        // Redraw the whole client area on a resize in either axis. The page
        // is repaginated when the box changes, so there is no part of it
        // that survives a resize and could have been kept.
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: class,
        // No background brush: `WM_ERASEBKGND` is answered below, because
        // letting Windows paint white behind a dark-theme page is a flash
        // of the wrong colour on every resize.
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(windows::core::Error::from_thread());
    }

    // The state the window procedure works through. Boxed so its address is
    // stable, and in a `RefCell` because a window procedure is re-entrant:
    // see `state_of`.
    let app = Box::new(RefCell::new(App {
        session,
        // This shell has no chrome, so there is nothing to hand
        // `ToggleMenu` to. Unbinding it is the honest version of leaving
        // `m` bound to a key press that does nothing.
        keys: {
            let mut keys = KeyMap::default();
            keys.unbind(Key::Char('m'));
            keys
        },
        zones: TapZones::default(),
        press: None,
        selecting: false,
        title: String::new(),
        dark: false,
        snapshot: Arc::new(Mutex::new(PageSnapshot::default())),
        provider: None,
        announced: Vec::new(),
        selected: None,
    }));
    // The book declares which edge it reads from, so the zones cannot be
    // built before the session exists. The middle band is inert: this shell
    // has no menu, so the band that would open one is bound to nothing
    // rather than to an action `apply` would refuse.
    {
        let mut state = app.borrow_mut();
        let direction = state.session.reading_direction();
        state.zones = TapZones {
            middle: None,
            ..TapZones::new(direction)
        };
    }
    let title = HSTRING::from(app.borrow().session.title());
    // The window procedure reaches the state through this address, and the
    // `Box` is what keeps it still. It stays owned here rather than going
    // through `into_raw`, so it is freed on every path out of this
    // function — including the one where `CreateWindowExW` fails and there
    // is no window procedure to hand ownership to.
    let app_ptr: *const RefCell<App> = &*app;

    // A logical 600x800 page, sized for the monitor the window opens on.
    // `AdjustWindowRectExForDpi` grows that by the frame and title bar at
    // that DPI, so the *client* area is the size asked for rather than the
    // size minus whatever the theme's chrome costs.
    let dpi = unsafe { GetDpiForSystem() };
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scaled(600, dpi),
        bottom: scaled(800, dpi),
    };
    unsafe {
        let _ = AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi,
        );
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            &title,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(instance),
            Some(app_ptr.cast::<c_void>()),
        )?
    };

    // The loader wakeup, and the reason this shell needs no timer.
    //
    // `HWND` is a raw pointer and so not `Send`, but `PostMessageW` is one
    // of the few user32 calls documented as callable from any thread: it
    // appends to the target thread's queue and returns, rather than
    // entering the window procedure. So only the *sender* crosses threads,
    // which is the same split the GTK shell makes with a channel — the
    // session stays on the thread that owns it, and this closure never
    // touches it.
    {
        struct Wake(HWND);
        // SAFETY: the handle is only ever passed to `PostMessageW`, which
        // MSDN documents as safe to call from a thread that does not own
        // the window. Posting to a destroyed window fails and is ignored,
        // which is the case this closure hits when the reader closes the
        // window while a page is still downloading.
        unsafe impl Send for Wake {}
        unsafe impl Sync for Wake {}
        impl Wake {
            // A method rather than a field read in the closure: edition
            // 2021 captures `wake.0` — a bare `HWND`, which is neither
            // `Send` nor `Sync` — where this captures the wrapper that
            // says it is safe to move.
            fn post(&self) {
                unsafe {
                    let _ = PostMessageW(Some(self.0), WM_CHAPBOOK_WAKE, WPARAM(0), LPARAM(0));
                }
            }
        }
        let wake = Wake(hwnd);
        app.borrow_mut().session.set_waker(move || wake.post());
    }

    {
        // The provider needs the window, so it cannot exist before it.
        let mut state = app.borrow_mut();
        let snapshot = state.snapshot.clone();
        state.provider = Some(PageProvider::element(hwnd, snapshot));
    }

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }

    let mut msg = MSG::default();
    // `GetMessageW` returns -1 on error, and `BOOL::as_bool` says that is
    // true, so the sign matters: a bare `.into()` here spins forever on a
    // handle that has gone bad.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            // Translation is what turns a key press into `WM_CHAR`, which
            // is where every `Key::Char` in this shell comes from — so the
            // keyboard layout's own answer is used rather than a virtual
            // key code mapped back to a letter by guesswork.
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // `app` drops here. The window is gone and the queue is drained, so no
    // further message can reach the procedure and nothing else holds the
    // address it was given.
    Ok(())
}

/// A logical pixel count at a given DPI.
fn scaled(logical: i32, dpi: u32) -> i32 {
    (logical as i64 * dpi as i64 / 96) as i32
}

struct App {
    session: Session,
    keys: KeyMap,
    zones: TapZones,
    /// Where the left button went down, in logical page coordinates, for
    /// the tap test in `WM_LBUTTONUP`. `None` when no button is down.
    press: Option<(f32, f32)>,
    /// Whether that press caught text and started a selection.
    selecting: bool,
    /// The title last handed to `SetWindowTextW`. Compared before setting,
    /// because `SetWindowTextW` sends `WM_SETTEXT` synchronously and a
    /// per-paint round trip through the window procedure is a cost with no
    /// benefit.
    title: String,
    /// Whether the title bar is currently drawn dark.
    dark: bool,
    /// What UI Automation reads. Shared with the provider, which answers
    /// out of it from whatever thread UIA asks on.
    snapshot: Arc<Mutex<PageSnapshot>>,
    /// The element behind `WM_GETOBJECT`. Built once, because UIA compares
    /// elements by COM identity and a client that is handed a new one every
    /// time believes the page was replaced every time.
    provider: Option<IRawElementProviderSimple>,
    /// The page text last given to the snapshot, so a repaint that changed
    /// no words raises no events. A selection repaint is the common case
    /// and it must not read the page out again.
    announced: Vec<char>,
    /// The selection the snapshot was built with, in locator space — the
    /// other half of "did anything a client cares about move".
    selected: Option<(u32, u32)>,
}

impl App {
    /// Push the session's state out to the parts of the window that are not
    /// the page: the title, and the colour of the frame around it.
    ///
    /// Called after `EndPaint` rather than during it, because both calls
    /// below re-enter the window procedure.
    fn sync_chrome(&mut self, hwnd: HWND) {
        let count = self.session.page_count().max(1);
        let title = format!(
            "{} — {} {}/{} p {}/{}",
            self.session.title(),
            match self.session.kind() {
                chapbook_core::BookKind::Epub => "ch",
                chapbook_core::BookKind::Comic | chapbook_core::BookKind::Pdf => "pg",
            },
            self.session.spine() + 1,
            self.session.spine_len(),
            self.session.page() + 1,
            count,
        );
        if title != self.title {
            let wide = HSTRING::from(title.as_str());
            self.title = title;
            unsafe {
                let _ = SetWindowTextW(hwnd, &wide);
            }
        }

        // The one piece of Windows chrome a reading theme reaches. Without
        // it, `t` into the dark theme leaves a white title bar bolted to
        // the top of a black page, which reads as a rendering bug rather
        // than as a shell that did not ask.
        let dark = self.session.settings().theme == Theme::Dark;
        if dark != self.dark {
            self.dark = dark;
            let flag = windows::core::BOOL::from(dark);
            unsafe {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE,
                    (&raw const flag).cast(),
                    std::mem::size_of_val(&flag) as u32,
                );
            }
        }
    }
}

impl App {
    /// Re-capture the page for UI Automation, and say what changed.
    ///
    /// Called after every paint, like `sync_chrome`, and for the same
    /// reason: a paint is the one place every content change funnels
    /// through. It compares before capturing, so a repaint that moved
    /// nothing costs one `speakable_page` and stops there.
    fn sync_accessibility(&mut self, scale: f64, force: bool) {
        // Nothing is listening: no client, and so no reason to walk the
        // page at all. This is what keeps the whole module free in the
        // ordinary case, which is a reader with no assistive technology
        // running.
        //
        // `force` is the case that check gets wrong on its own. A client
        // that attaches to a window already showing a page has missed
        // every paint there will be until the reader does something, so
        // the arrival itself — `WM_GETOBJECT` — has to be a capture. The
        // symptom otherwise is a screen reader that finds an empty
        // document until the next page turn, which reads as a page with no
        // text on it rather than as a race.
        if !force && !unsafe { UiaClientsAreListening() }.as_bool() {
            return;
        }
        let Some(provider) = self.provider.clone() else {
            return;
        };

        let text: Vec<char> = self
            .session
            .speakable_page()
            .map(|page| page.text.chars().collect())
            .unwrap_or_default();
        let selection = self.session.selected_range();
        let words_moved = text != self.announced;
        if !words_moved && selection == self.selected && !force {
            return;
        }
        self.selected = selection;

        let name = self.title.clone();
        let fresh = PageSnapshot::capture(&self.session, scale, name);
        if let Ok(mut snapshot) = self.snapshot.lock() {
            *snapshot = fresh;
        }

        // A client that just arrived is not being told anything changed;
        // it is being given something to read. Announcing here would mean
        // the page is spoken every time any automation tool attaches,
        // which is a different thing from a page turn and sounds like one.
        if force {
            self.announced = text;
            return;
        }

        if !words_moved {
            // Only the selection moved. A client tracking the caret wants
            // to know; a client reading the page does not want the page
            // read to it again.
            unsafe {
                let _ = UiaRaiseAutomationEvent(&provider, UIA_Text_TextSelectionChangedEventId);
            }
            return;
        }

        self.announced = text;
        // Advisory, both here and below: a client that misses one asks
        // again on its next query, and a failure is not something a reader
        // could act on.
        unsafe {
            let _ = UiaRaiseAutomationEvent(&provider, UIA_Text_TextChangedEventId);
        }

        // The turn made audible. `TextChanged` tells a client the words
        // moved; it does not make Narrator say them, and a reader that
        // turns the page in silence is not accessible however complete its
        // text pattern is. The GTK shell makes the same argument for
        // announcing a page over AT-SPI; this is the Windows call that
        // does it.
        //
        // `MostRecent` is what makes holding the page key down bearable:
        // pending announcements are dropped rather than queued, so someone
        // who turned six pages hears the sixth and not all six.
        let spoken: String = self.announced.iter().collect();
        if !spoken.is_empty() {
            unsafe {
                let _ = UiaRaiseNotificationEvent(
                    &provider,
                    NotificationKind_Other,
                    NotificationProcessing_MostRecent,
                    &BSTR::from(spoken),
                    &BSTR::from("chapbook.page"),
                );
            }
        }
    }
}

/// The state behind a window, or `None` before `WM_CREATE` has stored it.
fn state_of(hwnd: HWND) -> Option<&'static RefCell<App>> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<App>;
    // SAFETY: the pointer was stored by `WM_CREATE` from the `Box` that
    // `main_window` keeps alive until after the message loop has ended, so
    // it outlives every message dispatched to this window.
    unsafe { ptr.as_ref() }
}

/// The window procedure.
///
/// Two rules hold this together, and both are about re-entrancy. A window
/// procedure can be called again from inside itself — `SetWindowPos` sends
/// `WM_SIZE`, `SetWindowTextW` sends `WM_SETTEXT`, `DwmSetWindowAttribute`
/// asks for a frame repaint — so the state is a `RefCell` rather than a
/// bare `&mut`, and every handler borrows it for the shortest span that
/// does the work and drops the borrow before calling anything that can come
/// back through here. A message that finds the cell already borrowed falls
/// through to `DefWindowProcW`, which is the right answer for exactly the
/// messages that reach it that way.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_CREATE {
        // The `Box` address travelled here in the creation parameters; from
        // now on it is reachable from the handle alone.
        let create = lparam.0 as *const CREATESTRUCTW;
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize) };
        return LRESULT(0);
    }
    let Some(state) = state_of(hwnd) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };

    match msg {
        // Answered so Windows does not paint its own background first: the
        // page covers the whole client area, and an erase underneath it is
        // a flash of the wrong colour on every resize.
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd, state);
            LRESULT(0)
        }
        WM_CHAPBOOK_WAKE => {
            wake(hwnd, state);
            LRESULT(0)
        }
        WM_GETOBJECT => {
            // The one message a screen reader sends before anything else.
            // `lparam` names which object is being asked for; every value
            // but this one belongs to MSAA and goes to `DefWindowProcW`,
            // which answers for the window itself.
            if lparam.0 as i32 != UiaRootObjectId {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            let Ok(mut app) = state.try_borrow_mut() else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            // Capture before answering: see `sync_accessibility`.
            app.sync_accessibility(scale_of(hwnd) as f64, true);
            let Some(provider) = app.provider.clone() else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            drop(app);
            unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &provider) }
        }
        WM_CHAPBOOK_SELECT => {
            // A UIA client asked for a selection. It arrives here rather
            // than being applied where it was asked for, because the
            // session lives on this thread and nowhere else.
            if let Ok(mut app) = state.try_borrow_mut() {
                let (start, end) = (wparam.0 as u32, lparam.0 as u32);
                let App {
                    session, snapshot, ..
                } = &mut *app;
                if let Ok(snapshot) = snapshot.lock() {
                    apply_selection(session, &snapshot, start, end);
                }
                drop(app);
                invalidate(hwnd);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // Windows has already decided where the window should go on the
            // new monitor and passes the rect; taking it is what makes the
            // move look like one motion rather than a jump and a resize.
            let suggested = lparam.0 as *const RECT;
            let r = unsafe { *suggested };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            // The new scale reaches the session through `set_metrics` on
            // the repaint that follows, which is also where a resize's
            // does. `dpi_scale` is not a layout input, so this repaginates
            // nothing that a resize has not already repaginated.
            LRESULT(0)
        }
        WM_KEYDOWN => key_down(
            hwnd,
            state,
            VIRTUAL_KEY(wparam.0 as u16),
            msg,
            wparam,
            lparam,
        ),
        WM_CHAR => char_typed(hwnd, state, wparam.0 as u32, msg, wparam, lparam),
        WM_XBUTTONDOWN => {
            // The two thumb buttons, which is the closest a desktop comes to
            // a Kobo's bezel. `KeyMap` has bound them since it was written
            // and no shell had ever had a pair to deliver.
            let button = (wparam.0 >> 16) as u16;
            let key = if button == XBUTTON1 {
                Key::TurnPrev
            } else {
                Key::TurnNext
            };
            apply_key(hwnd, state, key);
            // An X button is documented as answered with TRUE, unlike the
            // other button messages.
            LRESULT(1)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) as u16 as i16) as i32;
            let action = if delta > 0 {
                chapbook_core::Action::PrevPage
            } else {
                chapbook_core::Action::NextPage
            };
            // One page per detent rather than per wheel unit: a free-
            // spinning wheel reports fractions of `WHEEL_DELTA`, and a book
            // that turned three pages on one flick is unreadable.
            if delta.abs() >= WHEEL_DELTA as i32 {
                let Ok(mut app) = state.try_borrow_mut() else {
                    return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
                };
                let outcome = app.session.apply(action);
                drop(app);
                if outcome.needs_redraw() {
                    invalidate(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            button_down(hwnd, state, lparam);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            mouse_move(hwnd, state, lparam);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            button_up(hwnd, state, lparam);
            LRESULT(0)
        }
        WM_ENDSESSION => {
            // Logging off or shutting down. This is Windows' version of the
            // callback `SHELLS.md` says is the only guaranteed one and is on
            // a clock, so it gets `suspend` — persist the position, close
            // the database, drop the caches — rather than `save_position`.
            // The session keeps working if the shutdown is cancelled.
            if wparam.0 != 0 {
                if let Ok(mut app) = state.try_borrow_mut() {
                    app.session.suspend();
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Every exit path funnels through here — the close button and
            // Alt+F4 through `DefWindowProcW`'s own `WM_CLOSE` handling,
            // and the `q` and Escape keys through their own `DestroyWindow`
            // — which is why the bookmark is left here and not in each of
            // them. `SHELLS.md` names the window's close button as the path
            // that is easy to forget; this is the one place it cannot be.
            if let Ok(mut app) = state.try_borrow_mut() {
                app.session.save_position();
                // Tell UI Automation the element is gone. Without this a
                // client can hold a reference to a provider whose window
                // has been destroyed, and every call it makes is answered
                // by a handle that resolves to nothing.
                if let Some(provider) = app.provider.take() {
                    unsafe {
                        let _ = UiaDisconnectProvider(&provider);
                    }
                }
            }
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Device pixels per logical pixel, from the monitor this window is on.
fn scale_of(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// A mouse message's coordinates, in the logical units `set_metrics` was
/// given.
///
/// Every hit test in the session takes these, not the raw event
/// coordinates — the same trap `SHELLS.md` names for Android's density,
/// with the same silent failure: on a 200% display every unscaled press
/// lands in the far band and always means "next page".
fn point_of(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xffff) as u16 as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as u16 as i16 as f32;
    let scale = scale_of(hwnd);
    (x / scale, y / scale)
}

fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// `WM_PAINT`: give the session the page box, take the pixels, blit them.
fn paint(hwnd: HWND, state: &RefCell<App>) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };

    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let (cw, ch) = (client.right - client.left, client.bottom - client.top);

    // Borrowed for the render and the blit, which do not re-enter, and
    // dropped before `sync_chrome`, which does. `painted` is false only
    // when the borrow failed: `BeginPaint` has validated the update region
    // by then, so the frame has to be asked for again or the window keeps
    // whatever was on it.
    let mut painted = cw <= 0 || ch <= 0;
    if !painted {
        if let Ok(mut app) = state.try_borrow_mut() {
            let scale = scale_of(hwnd);
            app.session.set_metrics(PageMetrics {
                // Logical, and margins in the same units. `dpi_scale` is
                // what makes the raster device-resolution, so the same book
                // paginates identically at 100% and 200%.
                size: Size::new(cw as f32 / scale, ch as f32 / scale),
                margins: EdgeSizes::uniform(40.0),
                dpi_scale: scale,
                rotation: Rotation::None,
            });
            match app.session.render() {
                Some(pixmap) => {
                    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
                    // Premultiplied RGBA → the BGRA byte order a 32bpp
                    // `BI_RGB` DIB means by "RGB". The page is opaque, so
                    // the alpha byte the blit ignores costs nothing.
                    let mut data = pixmap.take();
                    for px in data.as_chunks_mut::<4>().0 {
                        px.swap(0, 2);
                    }
                    let info = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: pw,
                            // Negative: top-down, which is the order the
                            // pixmap is in. A positive height here is the
                            // classic Win32 bug of a page rendered upside
                            // down.
                            biHeight: -ph,
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    // Stretched rather than blitted 1:1 because the raster
                    // is `round(logical * scale)` and the client rect is
                    // whatever Windows made it: the two agree to within a
                    // pixel, and a stretch of that is invisible where an
                    // unpainted sliver at the edge would not be.
                    unsafe {
                        StretchDIBits(
                            hdc,
                            0,
                            0,
                            cw,
                            ch,
                            0,
                            0,
                            pw,
                            ph,
                            Some(data.as_ptr().cast()),
                            &info,
                            DIB_RGB_COLORS,
                            SRCCOPY,
                        );
                    }
                    painted = true;
                }
                None => {
                    // No page yet — an image book still decoding, or a unit
                    // that failed. `None` means "nothing to draw", not
                    // "error", so the theme's ground is the honest thing to
                    // show under a placeholder.
                    let bg = app.session.settings().theme.background();
                    let brush = unsafe {
                        CreateSolidBrush(COLORREF(
                            u32::from(bg.r) | u32::from(bg.g) << 8 | u32::from(bg.b) << 16,
                        ))
                    };
                    unsafe {
                        FillRect(hdc, &client, brush);
                        let _ = DeleteObject(brush.into());
                    }
                    painted = true;
                }
            }
        }
    }
    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
    if !painted {
        invalidate(hwnd);
        return;
    }

    if let Ok(mut app) = state.try_borrow_mut() {
        // Chrome first: the title is what names the element, so a
        // snapshot taken before it would carry the previous page's name.
        app.sync_chrome(hwnd);
        app.sync_accessibility(scale_of(hwnd) as f64, false);
    }
}

/// The loader thread finished something.
fn wake(hwnd: HWND, state: &RefCell<App>) {
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    // `poll_loaded` answers "did the page on screen change" — a prefetched
    // unit landing deliberately does not count. The events answer the other
    // question: is there anything to tell the reader.
    let redraw = app.session.poll_loaded();
    for event in app.session.drain_events() {
        if let SessionEvent::UnitFailed { spine, message } = event {
            eprintln!(
                "chapbook-viewer-win32: page {} will not load: {message}",
                spine + 1
            );
        }
    }
    drop(app);
    if redraw {
        invalidate(hwnd);
    }
}

/// Named keys. Everything that produces a character arrives at
/// `char_typed` instead, through `TranslateMessage`.
fn key_down(
    hwnd: HWND,
    state: &RefCell<App>,
    vk: VIRTUAL_KEY,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Escape is the shell's before it is the engine's: it drops a live
    // selection if there is one, and otherwise closes the window.
    if vk == VK_ESCAPE {
        let Ok(mut app) = state.try_borrow_mut() else {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        };
        if app.session.selected_range().is_some() {
            app.session.selection_clear();
            drop(app);
            invalidate(hwnd);
        } else {
            drop(app);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        return LRESULT(0);
    }
    let key = match vk {
        VK_RIGHT => Key::ArrowRight,
        VK_LEFT => Key::ArrowLeft,
        VK_UP => Key::ArrowUp,
        VK_DOWN => Key::ArrowDown,
        VK_NEXT => Key::PageDown,
        VK_PRIOR => Key::PageUp,
        // Backspace produces a `WM_CHAR` of U+0008, but so does Ctrl+H, and
        // the engine's Back is a navigation key rather than a character.
        // Taking it here keeps the two apart.
        VK_BACK => Key::Backspace,
        // Windows' own back key — the fifth button on a keyboard that has
        // one, and where a browser-back gesture arrives. The engine has a
        // back stack and this is the key for it.
        VK_BROWSER_BACK => Key::Backspace,
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };
    if apply_key(hwnd, state, key) {
        LRESULT(0)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

/// Characters, as the keyboard layout produced them.
fn char_typed(
    hwnd: HWND,
    state: &RefCell<App>,
    unit: u32,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // A UTF-16 code unit. A surrogate half is not a `Key::Char` and no
    // reader binds one, so it falls through rather than being reassembled.
    let Some(c) = char::from_u32(unit) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    // Backspace and Escape reach here as control characters too; both were
    // answered in `key_down`, and answering them twice would turn one press
    // into two actions.
    if c.is_control() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // Shell-owned keys first, and they are exactly the ones that are not
    // reading actions: the clipboard, the annotation store and the window
    // belong to the app. Everything the engine can do for itself falls
    // through to the key map.
    let lower = c.to_ascii_lowercase();
    let outcome = {
        let Ok(mut app) = state.try_borrow_mut() else {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        };
        match lower {
            'c' => {
                let text = app.session.selected_text();
                drop(app);
                if let Some(text) = text {
                    copy_to_clipboard(hwnd, &text);
                }
                // Nothing on the page moved.
                return LRESULT(0);
            }
            'h' => {
                // The stored highlight replaces the selection that made it.
                app.session.add_highlight();
                app.session.selection_clear();
                ActionOutcome::Changed
            }
            'q' => {
                drop(app);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
            _ => {
                // Space is a `Key` of its own rather than `Char(' ')`, and
                // this is the only place it can be recognized: the layout,
                // not the virtual key, is what says a press produced one.
                let key = if lower == ' ' {
                    Key::Space
                } else {
                    Key::Char(lower)
                };
                match app.keys.action(key) {
                    Some(action) => app.session.apply(action),
                    None => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
                }
            }
        }
    };
    finish(hwnd, outcome, msg, wparam, lparam)
}

/// Put a `Key` through the map and the session. `false` when the engine
/// declined the event and the platform should have it.
fn apply_key(hwnd: HWND, state: &RefCell<App>, key: Key) -> bool {
    let Ok(mut app) = state.try_borrow_mut() else {
        return false;
    };
    let Some(action) = app.keys.action(key) else {
        return false;
    };
    let outcome = app.session.apply(action);
    drop(app);
    if outcome.needs_redraw() {
        invalidate(hwnd);
    }
    outcome.consumed()
}

/// The two halves of an `ActionOutcome`, which are not the same bit:
/// `needs_redraw` says whether to repaint, `consumed` says whether to tell
/// Windows we took the key. A bound key the engine declined — `Back` at the
/// bottom of its stack — reaches `DefWindowProcW` and from there whatever
/// is behind this window.
fn finish(hwnd: HWND, outcome: ActionOutcome, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if outcome.needs_redraw() {
        invalidate(hwnd);
    }
    if outcome.consumed() {
        LRESULT(0)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

/// A press: a link, then a highlight, then text, and a tap zone if it turns
/// out on release to have been a tap.
///
/// The order is the contract, not a preference. Links and highlights are
/// exact — both are `None` unless the press is inside the marked text — so
/// a miss falls through naturally, and asking the tap zones first would
/// swallow every link in the outer thirds of the page.
fn button_down(hwnd: HWND, state: &RefCell<App>, lparam: LPARAM) {
    let (x, y) = point_of(hwnd, lparam);
    // Capture, so a drag that leaves the window still reaches this shell.
    // Without it a selection stops growing at the window edge and the
    // button-up never arrives, which leaves the shell believing a button is
    // still down.
    unsafe {
        let _ = SetCapture(hwnd);
    }
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    app.press = Some((x, y));
    app.selecting = false;

    if let Some(href) = app.session.link_at(x, y) {
        if app.session.follow_link(&href) {
            app.press = None;
            drop(app);
            invalidate(hwnd);
            return;
        }
        // Not a reading position — an external URL. Handing it to the
        // browser is the shell's opportunity and this shell declines it,
        // rather than pretending the press was a page turn.
        app.press = None;
        drop(app);
        eprintln!("chapbook-viewer-win32: external link, not followed: {href}");
        return;
    }
    if let Some(id) = app.session.highlight_at(x, y) {
        // No annotation UI here, so there is nothing to open — but the hit
        // test runs in its documented place, and reporting it is what
        // proves the press did not fall through to a page turn.
        app.press = None;
        drop(app);
        eprintln!("chapbook-viewer-win32: highlight {id}");
        return;
    }
    // `selection_begin` returns false when there is no text under the
    // point, which is the cue to treat the press as something else — here,
    // a possible tap.
    app.selecting = app.session.selection_begin(x, y);
    drop(app);
    invalidate(hwnd);
}

fn mouse_move(hwnd: HWND, state: &RefCell<App>, lparam: LPARAM) {
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    if !app.selecting {
        return;
    }
    let (x, y) = point_of(hwnd, lparam);
    app.session.selection_drag(x, y);
    drop(app);
    invalidate(hwnd);
}

fn button_up(hwnd: HWND, state: &RefCell<App>, lparam: LPARAM) {
    unsafe {
        let _ = ReleaseCapture();
    }
    let (x, y) = point_of(hwnd, lparam);
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    let Some((px, py)) = app.press.take() else {
        return;
    };
    let dragged = (x - px).abs() > TAP_SLOP || (y - py).abs() > TAP_SLOP;
    if app.selecting {
        app.selecting = false;
        if dragged {
            drop(app);
            invalidate(hwnd);
            return;
        }
        // A press that anchored an empty selection and never moved was a
        // tap. Drop the anchor before turning, so it does not outlive the
        // page it was on.
        app.session.selection_clear();
    }
    if dragged {
        return;
    }
    // The metrics come back from the session rather than from this file's
    // own bookkeeping, which is what lets `action_at` undo a rotation the
    // shell never tracked.
    let Some(metrics) = app.session.metrics() else {
        return;
    };
    let Some(action) = app.zones.action_at(x, y, &metrics) else {
        drop(app);
        invalidate(hwnd);
        return;
    };
    let outcome = app.session.apply(action);
    drop(app);
    if outcome.needs_redraw() {
        invalidate(hwnd);
    }
}

/// Text onto the clipboard as `CF_UNICODETEXT`.
///
/// The allocation rule is the part that is easy to get wrong: on success
/// the clipboard owns the handle and freeing it is a double free, and on
/// failure the caller still owns it. So nothing here frees on the success
/// path, and the failure path lets the handle go with the process — a
/// leak of one string, on a path that means the clipboard was already
/// unavailable.
fn copy_to_clipboard(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = std::mem::size_of_val(wide.as_slice());
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            eprintln!("chapbook-viewer-win32: clipboard unavailable");
            return;
        }
        let result = (|| -> windows::core::Result<()> {
            EmptyClipboard()?;
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let dst = GlobalLock(handle);
            if dst.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst.cast::<u16>(), wide.len());
            let _ = GlobalUnlock(handle);
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(handle.0)))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        if let Err(e) = result {
            eprintln!("chapbook-viewer-win32: copy failed: {e}");
        }
    }
}
