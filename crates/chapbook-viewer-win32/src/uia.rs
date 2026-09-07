//! The page behind UI Automation, so Narrator can read it.
//!
//! A rasterized page is a picture, and a picture of text is unusable with
//! a screen reader. This module puts `Session`'s text surface —
//! [`chapbook_reader::Session::speakable_page`] for the words,
//! [`chapbook_reader::Session::page_text_runs`] for the lines,
//! [`chapbook_reader::Session::range_rects`] for the geometry — behind
//! `ITextProvider`, which is what Narrator reads. It is the Windows half
//! of the job `chapbook-viewer-gtk`'s `AccessibleText` surface does over
//! AT-SPI, and it exists as much to put the accessor's shape in front of a
//! second real assistive stack as to serve this shell.
//!
//! # Why a snapshot and not the session
//!
//! Every other consumer of the text surface in this workspace reads it
//! live, because every other consumer is called on the thread that owns
//! the session. UI Automation is not: a server-side provider is called
//! from UIA's own RPC threads unless the process opts into COM threading
//! and is a well-behaved STA, and `Session` is `Send` but not `Sync` and
//! mutates through `&mut self`. Reaching into it from a provider callback
//! would be a data race that happens to work until a client asks a
//! question while the reader is turning a page.
//!
//! So the shell captures a [`PageSnapshot`] — plain data, no engine types
//! that are not `Copy` — whenever the page's text changes, and the
//! provider answers out of that behind a `Mutex`. It costs one capture per
//! page turn and buys a provider with no thread affinity at all. The one
//! thing a client can ask for that a snapshot cannot serve is a
//! *mutation*: `ITextRangeProvider::Select` posts back to the window
//! instead, which is the same "only the sender crosses threads" move the
//! loader wakeup makes.
//!
//! # Offsets
//!
//! Every offset on this boundary is a Unicode **character** offset into
//! the speakable string, which is the space [`chapbook_reader::WordSpan`]
//! already carries and the one the GTK shell uses. Locator offsets appear
//! only inside [`PageSnapshot::capture`] and in the selection that goes
//! back to the session.
//!
//! # Units
//!
//! `Character`, `Word` and `Line` are real. `Format`, `Paragraph`, `Page`
//! and `Document` all resolve to the whole page, which is UIA's own
//! documented fallback — a provider that cannot honour a unit uses the
//! next larger one it can. The reason `Paragraph` is not real is worth
//! stating rather than leaving as an omission: the speakable page collapses
//! whitespace and carries no paragraph structure, and the only way to
//! recover one from what *is* carried would be to call a gap in locator
//! offsets between two lines a paragraph break, which is a threshold
//! dressed as a fact. A screen reader told "that paragraph is the page" is
//! reading something true and too big; one told a guessed boundary is
//! reading something false. `Line` is the unit review commands actually
//! use, and it is exact.

use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};

use windows::core::{implement, IUnknown, IUnknownImpl, Interface, Ref, BSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, VARIANT_FALSE, VARIANT_TRUE, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_BSTR, VT_I4, VT_R8,
    VT_UNKNOWN,
};
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, IRawElementProviderSimple_Impl, ITextProvider, ITextProvider_Impl,
    ITextRangeProvider, ITextRangeProvider_Impl, ProviderOptions,
    ProviderOptions_ServerSideProvider, SupportedTextSelection, SupportedTextSelection_Single,
    TextPatternRangeEndpoint, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
    TextUnit, TextUnit_Character, TextUnit_Line, TextUnit_Word, UIA_ControlTypePropertyId,
    UIA_DocumentControlTypeId, UIA_HasKeyboardFocusPropertyId, UIA_IsContentElementPropertyId,
    UIA_IsControlElementPropertyId, UIA_IsKeyboardFocusablePropertyId, UIA_NamePropertyId,
    UIA_TextPatternId, UiaGetReservedNotSupportedValue, UiaHostProviderFromHwnd, UiaPoint,
    UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_TEXTATTRIBUTE_ID,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use chapbook_core::Rect;
use chapbook_reader::Session;

/// A selection a UIA client asked for, on its way back to the window.
///
/// The endpoints travel in the message: `wparam` is the start and `lparam`
/// the end, both character offsets into the snapshot the client was
/// reading. The window converts them to locator space against its own
/// session — which may by then be showing a different page, in which case
/// the offsets do not resolve and nothing happens, which is the right
/// answer for a selection aimed at a page that has gone.
pub const WM_CHAPBOOK_SELECT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// One word: where it sits in the speakable string, in locator space, and
/// on the page.
#[derive(Debug, Clone, Copy)]
pub struct Word {
    pub start: u32,
    pub end: u32,
    pub locator_start: u32,
    pub locator_end: u32,
    /// Page-space bounding rect, logical pixels.
    pub rect: Rect,
}

/// One visual line, as a range of the speakable string.
///
/// The range runs from this line's first word to the *next* line's first
/// word, so every character belongs to exactly one line — including the
/// punctuation and spacing between words, which no `WordSpan` covers and
/// which a reader asked to read a line still expects to hear.
#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub start: u32,
    pub end: u32,
    pub rect: Rect,
}

/// Everything UI Automation is allowed to know about the page.
#[derive(Debug, Default)]
pub struct PageSnapshot {
    /// The speakable text as characters, because every offset on this
    /// boundary indexes it and a screen reader asks for many small ranges.
    /// A `String` would make each one a UTF-8 walk.
    pub text: Vec<char>,
    pub words: Vec<Word>,
    pub lines: Vec<Line>,
    /// The reader's selection, in character offsets.
    pub selection: Option<(u32, u32)>,
    /// Device pixels per logical pixel: page rects are logical, and UIA
    /// wants physical screen coordinates.
    pub scale: f64,
    /// What the element is called — the book and where in it we are.
    pub name: String,
}

impl PageSnapshot {
    /// Capture the current page. Called on the thread that owns the
    /// session, and on no other.
    ///
    /// Everything it calls reads laid-out state and never the loader
    /// thread, so this is safe where the shell already is: after a paint.
    pub fn capture(session: &Session, scale: f64, name: String) -> PageSnapshot {
        let mut snapshot = PageSnapshot {
            scale,
            name,
            ..PageSnapshot::default()
        };
        let Some(page) = session.speakable_page() else {
            return snapshot;
        };
        snapshot.text = page.text.chars().collect();

        // One `range_rects` per word. That is a page's glyphs walked once
        // per word rather than once, and it is still the cheap option: it
        // happens on a page turn, not on a query, and the alternative is a
        // provider that hands a screen reader the whole line's rectangle
        // when it asks where one word is.
        snapshot.words = page
            .words
            .iter()
            .map(|word| Word {
                start: word.text_start,
                end: word.text_end,
                locator_start: word.locator_start,
                locator_end: word.locator_end,
                rect: union(session.range_rects(word.locator_start, word.locator_end)),
            })
            .collect();

        // Lines come from the layout's own visual lines; the text range of
        // each is decided by which words fall inside its locator range.
        let runs = session.page_text_runs().unwrap_or_default();
        let mut starts: Vec<(u32, Rect)> = Vec::new();
        for run in &runs {
            let first = snapshot.words.iter().find(|w| {
                w.locator_start >= run.locator_start && w.locator_start < run.locator_end
            });
            if let Some(word) = first {
                starts.push((word.start, run.rect));
            }
        }
        // The first line owns anything before its first word, and the last
        // owns everything after its own.
        if let Some(first) = starts.first_mut() {
            first.0 = 0;
        }
        let len = snapshot.text.len() as u32;
        snapshot.lines = starts
            .iter()
            .enumerate()
            .map(|(i, &(start, rect))| Line {
                start,
                end: starts.get(i + 1).map_or(len, |next| next.0),
                rect,
            })
            .collect();

        snapshot.selection = session
            .selected_range()
            .and_then(|(lo, hi)| snapshot.text_range(lo, hi));
        snapshot
    }

    /// Locator range → character range, through the word table. `None`
    /// when no word overlaps it — a selection on a page that has since
    /// turned, or one covering only whitespace.
    fn text_range(&self, lo: u32, hi: u32) -> Option<(u32, u32)> {
        let mut range: Option<(u32, u32)> = None;
        for word in self
            .words
            .iter()
            .filter(|w| w.locator_start < hi && lo < w.locator_end)
        {
            range = Some(match range {
                None => (word.start, word.end),
                Some((a, b)) => (a.min(word.start), b.max(word.end)),
            });
        }
        range
    }

    /// Character range → locator range, the same way round.
    pub fn locator_range(&self, start: u32, end: u32) -> Option<(u32, u32)> {
        let mut range: Option<(u32, u32)> = None;
        for word in self.words.iter().filter(|w| w.start < end && start < w.end) {
            range = Some(match range {
                None => (word.locator_start, word.locator_end),
                Some((a, b)) => (a.min(word.locator_start), b.max(word.locator_end)),
            });
        }
        range
    }

    fn len(&self) -> u32 {
        self.text.len() as u32
    }

    fn slice(&self, start: u32, end: u32) -> String {
        let (start, end) = (start.min(self.len()), end.min(self.len()));
        if end <= start {
            return String::new();
        }
        self.text[start as usize..end as usize].iter().collect()
    }

    /// The word containing `offset`, or the one before it when `offset`
    /// sits in the space between two words.
    fn word_at(&self, offset: u32) -> Option<&Word> {
        self.words
            .iter()
            .rev()
            .find(|word| word.start <= offset && offset < word.end)
            .or_else(|| self.words.iter().rev().find(|word| word.end <= offset))
    }

    fn line_at(&self, offset: u32) -> Option<&Line> {
        self.lines
            .iter()
            .find(|line| line.start <= offset && offset < line.end)
            .or_else(|| self.lines.last())
    }
}

/// The union of some rects, or an empty one.
fn union(rects: Vec<Rect>) -> Rect {
    rects
        .into_iter()
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Rect::ZERO)
}

/// The window, for the two things a provider does with one: turn page
/// coordinates into screen coordinates, and post a request back.
///
/// # Safety
///
/// `HWND` is a raw pointer and so neither `Send` nor `Sync`, and this type
/// asserts both. It is sound because of what is done with the handle and
/// nothing else: `ClientToScreen`, `ScreenToClient`, `UiaHostProviderFromHwnd`
/// and `PostMessageW` are all callable from a thread that does not own the
/// window, and nothing here ever enters the window procedure. A handle that
/// has been destroyed makes each of them fail, which is what a client
/// holding a range across a window close gets, and is not a crash.
#[derive(Clone, Copy)]
struct Win(HWND);
unsafe impl Send for Win {}
unsafe impl Sync for Win {}

/// The page element: one document, with a text pattern on it.
#[implement(IRawElementProviderSimple, ITextProvider)]
pub struct PageProvider {
    snapshot: Arc<Mutex<PageSnapshot>>,
    window: Win,
}

impl PageProvider {
    /// The element for a window's page, as the interface a shell holds.
    ///
    /// It hands back `IRawElementProviderSimple` rather than `Self`
    /// because there is nothing to do with a `PageProvider` that is not
    /// through an interface: `WM_GETOBJECT` returns one, and COM identity
    /// is what makes it the same element the next time a client asks.
    pub fn element(hwnd: HWND, snapshot: Arc<Mutex<PageSnapshot>>) -> IRawElementProviderSimple {
        PageProvider {
            snapshot,
            window: Win(hwnd),
        }
        .into()
    }
}

impl PageProvider_Impl {
    /// A range over this page, carrying the element it came from.
    ///
    /// The element travels with the range rather than being rebuilt by
    /// `GetEnclosingElement`, because UIA compares elements by COM
    /// identity: a fresh provider with the same contents is a *different*
    /// element to a client, and a client that sees the enclosing element
    /// change under a range stops trusting the range.
    fn range(&self, start: u32, end: u32) -> ITextRangeProvider {
        PageRange {
            snapshot: self.snapshot.clone(),
            element: self.to_interface(),
            window: self.window,
            span: Mutex::new((start, end)),
        }
        .into()
    }
}

impl IRawElementProviderSimple_Impl for PageProvider_Impl {
    fn ProviderOptions(&self) -> windows::core::Result<ProviderOptions> {
        // Deliberately *not* `ProviderOptions_UseComThreading`. That flag
        // is how a provider asks UIA to marshal every call onto the thread
        // that created it, and it is the right answer for a provider that
        // reaches into thread-affine state. This one reaches into a
        // snapshot behind a `Mutex`, so it can be answered wherever UIA
        // happens to ask — and not asking for marshalling means not
        // depending on this process being a well-behaved STA, which a
        // shell that never calls `CoInitializeEx` is not.
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> windows::core::Result<IUnknown> {
        if pattern == UIA_TextPatternId {
            return Ok(self.to_interface::<ITextProvider>().into());
        }
        // A null `IUnknown` is how this call says "not that one"; there is
        // no error for it.
        Err(windows::core::Error::empty())
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> windows::core::Result<VARIANT> {
        Ok(match property {
            // A book's page is a document, which is what makes Narrator
            // offer its reading commands rather than treat this as a
            // nameless canvas.
            p if p == UIA_ControlTypePropertyId => variant_i32(UIA_DocumentControlTypeId.0),
            p if p == UIA_NamePropertyId => {
                variant_bstr(&self.snapshot.lock().expect("snapshot lock").name)
            }
            // Content and control both: this is the only thing in the
            // window, so a client filtering for either must find it.
            p if p == UIA_IsContentElementPropertyId || p == UIA_IsControlElementPropertyId => {
                variant_bool(true)
            }
            // The window takes the keyboard, and the page is the only
            // thing in it — a screen reader speaks what is focused, and an
            // element that says it can never be focused is silence no
            // matter what its text pattern says.
            p if p == UIA_IsKeyboardFocusablePropertyId => variant_bool(true),
            p if p == UIA_HasKeyboardFocusPropertyId => variant_bool(true),
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> windows::core::Result<IRawElementProviderSimple> {
        // The host provider is what supplies everything this module does
        // not: the element's place in the tree, its bounding rectangle,
        // its window-derived properties. Handing it back is what lets this
        // be a *simple* provider — no fragment navigation to implement,
        // because the HWND already has a place in the tree and UIA merges
        // the two.
        unsafe { UiaHostProviderFromHwnd(self.window.0) }
    }
}

impl ITextProvider_Impl for PageProvider_Impl {
    fn GetSelection(&self) -> windows::core::Result<*mut SAFEARRAY> {
        let selection = self.snapshot.lock().expect("snapshot lock").selection;
        match selection {
            Some((start, end)) => ranges_array(&[self.range(start, end)]),
            // An empty array, not a null one: "there is no selection" is a
            // different answer from "this control does not do selections",
            // and a client that gets null for the second reason will stop
            // asking.
            None => ranges_array(&[]),
        }
    }

    fn GetVisibleRanges(&self) -> windows::core::Result<*mut SAFEARRAY> {
        // A page is exactly what is visible. That is the whole reason this
        // engine paginates rather than scrolls, and it is the one place in
        // this file where that shows up as a simplification.
        let len = self.snapshot.lock().expect("snapshot lock").len();
        ranges_array(&[self.range(0, len)])
    }

    fn RangeFromChild(
        &self,
        _child: Ref<IRawElementProviderSimple>,
    ) -> windows::core::Result<ITextRangeProvider> {
        // The page has no child elements — no embedded controls, no
        // images exposed as objects — so there is no child to have a range.
        Err(windows::core::Error::empty())
    }

    fn RangeFromPoint(&self, point: &UiaPoint) -> windows::core::Result<ITextRangeProvider> {
        let snapshot = self.snapshot.lock().expect("snapshot lock");
        let (x, y) = self.window.page_point(point.x, point.y, snapshot.scale);
        // Word precision, like the GTK shell's `offset`: the word under
        // the point answers with its own range, which is what word-oriented
        // review reads. A degenerate range at the nearest character would
        // be more literal and less useful.
        let word = snapshot
            .words
            .iter()
            .find(|word| contains(&word.rect, x, y))
            .map(|word| (word.start, word.end));
        let (start, end) = match word {
            Some(span) => span,
            // Off any word: the line under the point, collapsed to its
            // start. A point in a margin is still somewhere on the page.
            None => snapshot
                .lines
                .iter()
                .find(|line| contains(&line.rect, x, y))
                .map_or((0, 0), |line| (line.start, line.start)),
        };
        drop(snapshot);
        Ok(self.range(start, end))
    }

    fn DocumentRange(&self) -> windows::core::Result<ITextRangeProvider> {
        let len = self.snapshot.lock().expect("snapshot lock").len();
        Ok(self.range(0, len))
    }

    fn SupportedTextSelection(&self) -> windows::core::Result<SupportedTextSelection> {
        // One selection, which is what `Session` has: `selection_begin`
        // replaces whatever was there.
        Ok(SupportedTextSelection_Single)
    }
}

impl Win {
    /// A screen point in page coordinates — logical pixels, origin at the
    /// client area's top left.
    ///
    /// This shell never rotates, so page space and client space differ by
    /// the scale factor and nothing else. A shell that did rotate would put
    /// the result through `PageMetrics::panel_to_page` here, the same way
    /// every other hit test in the reader does.
    fn page_point(&self, x: f64, y: f64, scale: f64) -> (f32, f32) {
        let mut point = POINT {
            x: x as i32,
            y: y as i32,
        };
        let _ = unsafe { ScreenToClient(self.0, &mut point) };
        (
            (point.x as f64 / scale) as f32,
            (point.y as f64 / scale) as f32,
        )
    }

    /// A page rect as UIA wants it: physical pixels, screen origin.
    fn screen_rect(&self, rect: &Rect, scale: f64) -> [f64; 4] {
        let mut origin = POINT {
            x: (rect.origin.x as f64 * scale) as i32,
            y: (rect.origin.y as f64 * scale) as i32,
        };
        let _ = unsafe { ClientToScreen(self.0, &mut origin) };
        [
            origin.x as f64,
            origin.y as f64,
            rect.size.w as f64 * scale,
            rect.size.h as f64 * scale,
        ]
    }
}

fn contains(rect: &Rect, x: f32, y: f32) -> bool {
    x >= rect.origin.x
        && x < rect.origin.x + rect.size.w
        && y >= rect.origin.y
        && y < rect.origin.y + rect.size.h
}

/// A range of the page, as a client holds it.
///
/// Its endpoints move under it — `ExpandToEnclosingUnit`, `Move` and the
/// `MoveEndpoint*` pair all mutate through `&self`, because that is the
/// COM signature — so they live behind a `Mutex` rather than a `Cell`. The
/// span is clamped to the snapshot on every read, which is what keeps a
/// range held across a page turn from indexing off the end of a shorter
/// page: it degenerates rather than panicking.
#[implement(ITextRangeProvider)]
struct PageRange {
    snapshot: Arc<Mutex<PageSnapshot>>,
    /// The element this range is over, held so `GetEnclosingElement` can
    /// hand back the same one every time.
    element: IRawElementProviderSimple,
    window: Win,
    span: Mutex<(u32, u32)>,
}

impl PageRange {
    fn get(&self) -> (u32, u32) {
        let len = self.snapshot.lock().expect("snapshot lock").len();
        let (start, end) = *self.span.lock().expect("span lock");
        let start = start.min(len);
        (start, end.clamp(start, len))
    }

    fn set(&self, start: u32, end: u32) {
        *self.span.lock().expect("span lock") = (start, end.max(start));
    }

    /// The endpoints of `unit` around an offset. `None` for the units this
    /// provider resolves to the whole page.
    fn unit_around(&self, offset: u32, unit: TextUnit) -> Option<(u32, u32)> {
        let snapshot = self.snapshot.lock().expect("snapshot lock");
        let len = snapshot.len();
        match unit {
            u if u == TextUnit_Character => Some((offset.min(len), (offset + 1).min(len))),
            u if u == TextUnit_Word => snapshot.word_at(offset).map(|w| (w.start, w.end)),
            u if u == TextUnit_Line => snapshot.line_at(offset).map(|l| (l.start, l.end)),
            _ => None,
        }
    }

    /// The boundaries of `unit` across the page, as a sorted list of start
    /// offsets plus the end of the last one. This is what `Move` and
    /// `MoveEndpointByUnit` step along.
    fn boundaries(&self, unit: TextUnit) -> Vec<(u32, u32)> {
        let snapshot = self.snapshot.lock().expect("snapshot lock");
        let len = snapshot.len();
        match unit {
            u if u == TextUnit_Character => (0..len).map(|i| (i, i + 1)).collect(),
            u if u == TextUnit_Word => snapshot.words.iter().map(|w| (w.start, w.end)).collect(),
            u if u == TextUnit_Line => snapshot.lines.iter().map(|l| (l.start, l.end)).collect(),
            // Everything larger than a line is the page, so there is one
            // of them and nowhere to step.
            _ => vec![(0, len)],
        }
    }
}

impl ITextRangeProvider_Impl for PageRange_Impl {
    fn Clone(&self) -> windows::core::Result<ITextRangeProvider> {
        let (start, end) = self.get();
        Ok(PageRange {
            snapshot: self.snapshot.clone(),
            element: self.element.clone(),
            window: self.window,
            span: Mutex::new((start, end)),
        }
        .into())
    }

    fn Compare(
        &self,
        other: Ref<ITextRangeProvider>,
    ) -> windows::core::Result<windows::core::BOOL> {
        let Some(other) = other.as_ref() else {
            return Ok(false.into());
        };
        // Same endpoints *and* the same page: two ranges from different
        // snapshots are not the same range even when their numbers match.
        let Ok(other) = other.cast_object_ref::<PageRange>() else {
            return Ok(false.into());
        };
        let same_page = Arc::ptr_eq(&self.snapshot, &other.snapshot);
        Ok((same_page && self.get() == other.get()).into())
    }

    fn CompareEndpoints(
        &self,
        endpoint: TextPatternRangeEndpoint,
        other: Ref<ITextRangeProvider>,
        other_endpoint: TextPatternRangeEndpoint,
    ) -> windows::core::Result<i32> {
        let Some(other) = other.as_ref() else {
            return Err(windows::core::Error::empty());
        };
        let Ok(other) = other.cast_object_ref::<PageRange>() else {
            return Err(windows::core::Error::empty());
        };
        let mine = endpoint_of(self.get(), endpoint);
        let theirs = endpoint_of(other.get(), other_endpoint);
        Ok(mine as i32 - theirs as i32)
    }

    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> windows::core::Result<()> {
        let (start, _) = self.get();
        match self.unit_around(start, unit) {
            Some((a, b)) => self.set(a, b),
            None => {
                let len = self.snapshot.lock().expect("snapshot lock").len();
                self.set(0, len);
            }
        }
        Ok(())
    }

    fn FindAttribute(
        &self,
        _attribute: UIA_TEXTATTRIBUTE_ID,
        _value: &VARIANT,
        _backward: windows::core::BOOL,
    ) -> windows::core::Result<ITextRangeProvider> {
        // No attribute is reported as anything but "not supported" below,
        // so there is nothing here to search for. Saying so is the honest
        // answer; returning the document range would be a lie a client
        // would act on.
        Err(windows::core::Error::empty())
    }

    fn FindText(
        &self,
        text: &BSTR,
        backward: windows::core::BOOL,
        ignorecase: windows::core::BOOL,
    ) -> windows::core::Result<ITextRangeProvider> {
        let needle: Vec<char> = if ignorecase.as_bool() {
            text.to_string()
                .chars()
                .flat_map(char::to_lowercase)
                .collect()
        } else {
            text.to_string().chars().collect()
        };
        if needle.is_empty() {
            return Err(windows::core::Error::empty());
        }
        let (start, end) = self.get();
        let snapshot = self.snapshot.lock().expect("snapshot lock");
        let hay: Vec<char> = if ignorecase.as_bool() {
            snapshot.text[start as usize..end as usize]
                .iter()
                .flat_map(|c| c.to_lowercase())
                .collect()
        } else {
            snapshot.text[start as usize..end as usize].to_vec()
        };
        // Case folding by `to_lowercase` can change the character count,
        // so a match is only reported when it did not — the alternative is
        // an offset that points somewhere else in the page, which is worse
        // than not finding the word.
        if hay.len() != (end - start) as usize {
            return Err(windows::core::Error::empty());
        }
        let positions = 0..=hay.len().saturating_sub(needle.len());
        let found = if backward.as_bool() {
            positions
                .rev()
                .find(|&i| hay[i..i + needle.len()] == needle[..])
        } else {
            positions
                .clone()
                .find(|&i| hay[i..i + needle.len()] == needle[..])
        };
        let Some(at) = found.filter(|_| hay.len() >= needle.len()) else {
            return Err(windows::core::Error::empty());
        };
        drop(snapshot);
        Ok(PageRange {
            snapshot: self.snapshot.clone(),
            element: self.element.clone(),
            window: self.window,
            span: Mutex::new((start + at as u32, start + (at + needle.len()) as u32)),
        }
        .into())
    }

    fn GetAttributeValue(
        &self,
        _attribute: UIA_TEXTATTRIBUTE_ID,
    ) -> windows::core::Result<VARIANT> {
        // The reserved "not supported" object, which is what UIA wants
        // here and is not the same as an empty variant: a client reading an
        // empty variant sees a font size of zero rather than an absent one.
        //
        // Text attributes are not absent from the engine — a `GlyphRun`
        // knows its face and size — but they are absent from the *text
        // surface*, which is deliberately a much smaller thing to hold
        // still than the paint vocabulary. Exposing them means widening
        // that accessor, which is a chapbook-reader change and not a
        // Windows one.
        let unsupported = unsafe { UiaGetReservedNotSupportedValue()? };
        Ok(variant_unknown(unsupported))
    }

    fn GetBoundingRectangles(&self) -> windows::core::Result<*mut SAFEARRAY> {
        let (start, end) = self.get();
        let snapshot = self.snapshot.lock().expect("snapshot lock");
        // One rectangle per line the range touches, which is what UIA
        // specifies and what a screen reader's highlight draws. Each is the
        // union of the range's words on that line, so a range covering half
        // a line boxes half a line.
        let mut rects: Vec<[f64; 4]> = Vec::new();
        for line in &snapshot.lines {
            if line.end <= start || end <= line.start {
                continue;
            }
            let covered = snapshot
                .words
                .iter()
                .filter(|w| w.start >= line.start && w.end <= line.end)
                .filter(|w| w.start < end && start < w.end)
                .map(|w| w.rect)
                .reduce(|a, b| a.union(&b));
            let rect = covered.unwrap_or(line.rect);
            if rect.size.w > 0.0 && rect.size.h > 0.0 {
                rects.push(self.window.screen_rect(&rect, snapshot.scale));
            }
        }
        drop(snapshot);
        doubles_array(&rects.concat())
    }

    fn GetEnclosingElement(&self) -> windows::core::Result<IRawElementProviderSimple> {
        Ok(self.element.clone())
    }

    fn GetText(&self, max: i32) -> windows::core::Result<BSTR> {
        let (start, mut end) = self.get();
        if max >= 0 {
            end = end.min(start.saturating_add(max as u32));
        }
        let text = self
            .snapshot
            .lock()
            .expect("snapshot lock")
            .slice(start, end);
        Ok(BSTR::from(text))
    }

    fn Move(&self, unit: TextUnit, count: i32) -> windows::core::Result<i32> {
        if count == 0 {
            return Ok(0);
        }
        let units = self.boundaries(unit);
        if units.is_empty() {
            return Ok(0);
        }
        let (start, _) = self.get();
        // Where the range is now, as an index into the units.
        let at = units.iter().rposition(|&(a, _)| a <= start).unwrap_or(0) as i32;
        let last = units.len() as i32 - 1;
        let target = (at + count).clamp(0, last);
        let (a, b) = units[target as usize];
        self.set(a, b);
        // The count actually moved, which is not the count asked for when
        // the range ran into an end of the page. A client uses the
        // difference to know it has arrived.
        Ok(target - at)
    }

    fn MoveEndpointByUnit(
        &self,
        endpoint: TextPatternRangeEndpoint,
        unit: TextUnit,
        count: i32,
    ) -> windows::core::Result<i32> {
        if count == 0 {
            return Ok(0);
        }
        let units = self.boundaries(unit);
        if units.is_empty() {
            return Ok(0);
        }
        let (start, end) = self.get();
        let moving = endpoint_of((start, end), endpoint);
        let at = units.iter().rposition(|&(a, _)| a <= moving).unwrap_or(0) as i32;
        let last = units.len() as i32 - 1;
        let target = (at + count).clamp(0, last);
        let (a, b) = units[target as usize];
        let moved = if endpoint == TextPatternRangeEndpoint_Start {
            a
        } else {
            b
        };
        // An endpoint pushed past the other one takes it along, which is
        // what UIA says a degenerate range is.
        if endpoint == TextPatternRangeEndpoint_Start {
            self.set(moved, end.max(moved));
        } else {
            self.set(start.min(moved), moved);
        }
        Ok(target - at)
    }

    fn MoveEndpointByRange(
        &self,
        endpoint: TextPatternRangeEndpoint,
        other: Ref<ITextRangeProvider>,
        other_endpoint: TextPatternRangeEndpoint,
    ) -> windows::core::Result<()> {
        let Some(other) = other.as_ref() else {
            return Err(windows::core::Error::empty());
        };
        let Ok(other) = other.cast_object_ref::<PageRange>() else {
            return Err(windows::core::Error::empty());
        };
        let target = endpoint_of(other.get(), other_endpoint);
        let (start, end) = self.get();
        if endpoint == TextPatternRangeEndpoint_Start {
            self.set(target, end.max(target));
        } else {
            self.set(start.min(target), target);
        }
        Ok(())
    }

    fn Select(&self) -> windows::core::Result<()> {
        // The one thing a snapshot cannot answer, because it is a change
        // rather than a question. It goes back to the window instead, and
        // the window applies it against the live session on the thread that
        // owns one — the same split the loader wakeup makes, for the same
        // reason.
        let (start, end) = self.get();
        unsafe {
            PostMessageW(
                Some(self.window.0),
                WM_CHAPBOOK_SELECT,
                WPARAM(start as usize),
                LPARAM(end as isize),
            )?;
        }
        Ok(())
    }

    fn AddToSelection(&self) -> windows::core::Result<()> {
        // `SupportedTextSelection_Single` already said so; this is the
        // call that has to agree with it.
        Err(windows::core::Error::empty())
    }

    fn RemoveFromSelection(&self) -> windows::core::Result<()> {
        Err(windows::core::Error::empty())
    }

    fn ScrollIntoView(&self, _align_to_top: windows::core::BOOL) -> windows::core::Result<()> {
        // Every range this provider can hand out is on the page that is on
        // the screen, so there is never anything to scroll — which is
        // pagination's answer to a question a scrolling document has to
        // work at.
        Ok(())
    }

    fn GetChildren(&self) -> windows::core::Result<*mut SAFEARRAY> {
        // No embedded elements, so an empty array rather than an error.
        empty_array(VT_UNKNOWN)
    }
}

fn endpoint_of((start, end): (u32, u32), which: TextPatternRangeEndpoint) -> u32 {
    if which == TextPatternRangeEndpoint_End {
        end
    } else {
        start
    }
}

// ---- SAFEARRAY and VARIANT plumbing ----
//
// Both are owned by the caller on the way out, which is the rule that
// decides every `ManuallyDrop` below: the value is handed over, not
// dropped here.

fn empty_array(vt: VARENUM) -> windows::core::Result<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(vt, 0, 0) };
    if array.is_null() {
        return Err(windows::core::Error::from_thread());
    }
    Ok(array)
}

fn ranges_array(ranges: &[ITextRangeProvider]) -> windows::core::Result<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(VT_UNKNOWN, 0, ranges.len() as u32) };
    if array.is_null() {
        return Err(windows::core::Error::from_thread());
    }
    for (i, range) in ranges.iter().enumerate() {
        let index = i as i32;
        // `SafeArrayPutElement` AddRefs what it stores, so the reference
        // held by `ranges` is still ours to drop.
        if let Err(e) = unsafe { SafeArrayPutElement(array, &index, range.as_raw()) } {
            unsafe { SafeArrayDestroy(array).ok() };
            return Err(e);
        }
    }
    Ok(array)
}

fn doubles_array(values: &[f64]) -> windows::core::Result<*mut SAFEARRAY> {
    let array = unsafe { SafeArrayCreateVector(VT_R8, 0, values.len() as u32) };
    if array.is_null() {
        return Err(windows::core::Error::from_thread());
    }
    for (i, value) in values.iter().enumerate() {
        let index = i as i32;
        if let Err(e) = unsafe { SafeArrayPutElement(array, &index, (value as *const f64).cast()) }
        {
            unsafe { SafeArrayDestroy(array).ok() };
            return Err(e);
        }
    }
    Ok(array)
}

/// A `VARIANT` of one tag and one payload.
///
/// Written as a value rather than as a sequence of writes through
/// `Anonymous.Anonymous`: those fields sit behind a `ManuallyDrop` inside a
/// union, and assigning through one runs a destructor on whatever the
/// zeroed memory is currently claiming to be. Building the whole thing at
/// once has no such step. Constructing a union is safe; only reading one
/// back is not, and the caller here is UIA, which reads it by the tag.
fn variant(vt: VARENUM, value: VARIANT_0_0_0) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: value,
            }),
        },
    }
}

fn variant_i32(value: i32) -> VARIANT {
    variant(VT_I4, VARIANT_0_0_0 { lVal: value })
}

fn variant_bool(value: bool) -> VARIANT {
    let value = if value { VARIANT_TRUE } else { VARIANT_FALSE };
    variant(VT_BOOL, VARIANT_0_0_0 { boolVal: value })
}

/// The string is handed over, not borrowed: UIA frees it with
/// `VariantClear`, which is why it goes in wrapped rather than dropped
/// here.
fn variant_bstr(value: &str) -> VARIANT {
    variant(
        VT_BSTR,
        VARIANT_0_0_0 {
            bstrVal: ManuallyDrop::new(BSTR::from(value)),
        },
    )
}

fn variant_unknown(value: IUnknown) -> VARIANT {
    variant(
        VT_UNKNOWN,
        VARIANT_0_0_0 {
            punkVal: ManuallyDrop::new(Some(value)),
        },
    )
}

/// Apply a selection a UIA client asked for, against the live session.
///
/// Called from the window procedure, on the thread that owns the session —
/// the other end of [`WM_CHAPBOOK_SELECT`].
pub fn apply_selection(session: &mut Session, snapshot: &PageSnapshot, start: u32, end: u32) {
    match snapshot.locator_range(start, end) {
        Some((lo, hi)) => session.select_range(lo, hi),
        // A degenerate range, or one aimed at a page that has turned. A
        // caret is the closest thing this engine has to an empty
        // selection, and it does not have one, so nothing happens.
        None => session.selection_clear(),
    }
}
