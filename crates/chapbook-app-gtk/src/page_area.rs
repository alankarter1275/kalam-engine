//! The page as a widget with an accessible text surface.
//!
//! An adapted copy of the reference viewer's `PageArea`
//! (`chapbook-viewer-gtk`), which proved this shape against Orca over the
//! live AT-SPI bus — the viewers are demos to be read and copied, not
//! linked, so the copy is the intended reuse. The one difference: this
//! application opens and closes books while the window stays up, so the
//! widget holds a *slot* a session comes and goes from rather than a
//! session it is born with. Every accessor already answered `None` for "no
//! page yet"; an empty slot answers the same way.
//!
//! Offsets on this boundary are Unicode character offsets into the
//! speakable string, which is also what `WordSpan` carries — no second
//! offset space appears here. Geometry crosses in widget coordinates;
//! this shell paints at 1/scale and never rotates, so page space and
//! logical widget space are the same thing.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

use chapbook_app::chapbook_reader::{Session, SpeakablePage, WordSpan};

/// The slot a reader page lives in: `None` while the shelf is showing.
pub type SessionSlot = Rc<RefCell<Option<Session>>>;

glib::wrapper! {
    pub struct PageArea(ObjectSubclass<imp::PageArea>)
        @extends gtk::DrawingArea, gtk::Widget,
        @implements gtk::Accessible, gtk::AccessibleText, gtk::Buildable, gtk::ConstraintTarget;
}

impl PageArea {
    pub fn new(session: SessionSlot) -> Self {
        let area: Self = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::Paragraph)
            // Focusable, so the screen reader's locus of focus is the
            // text object and not the empty window around it — Orca
            // speaks what is focused, and an unfocusable canvas is
            // silence no matter what its Text interface says.
            .property("focusable", true)
            .build();
        area.imp().session.replace(Some(session));
        area
    }

    /// Tell assistive technology when the page's text has actually
    /// changed — a page turn, a relayout, a background load landing, a
    /// different book in the slot. Compares against the last announced
    /// text, so it is safe (and intended) to call after every draw; with
    /// the slot empty it retracts whatever was announced and says nothing.
    pub fn page_changed(&self) {
        let imp = self.imp();
        let now = imp
            .speakable()
            .map(|page| page.text)
            .filter(|text| !text.is_empty());
        if *imp.announced.borrow() == now {
            return;
        }
        let before = imp
            .announced
            .replace(now.clone())
            .map(|text| text.chars().count() as u32)
            .unwrap_or(0);
        if before > 0 {
            self.update_contents(gtk::AccessibleTextContentChange::Remove, 0, before);
        }
        if let Some(text) = now {
            let count = text.chars().count() as u32;
            self.update_contents(gtk::AccessibleTextContentChange::Insert, 0, count);
            self.announce(&text, gtk::AccessibleAnnouncementPriority::Medium);
        }
    }
}

/// A char-offset slice of `text` — the offset space of this whole
/// interface.
fn slice_chars(text: &str, start: u32, end: u32) -> String {
    text.chars()
        .skip(start as usize)
        .take(end.saturating_sub(start) as usize)
        .collect()
}

/// The word span containing `offset`, else the character under it — what
/// word-granularity navigation steps by.
fn word_around(words: &[WordSpan], offset: u32, len: u32) -> (u32, u32) {
    words
        .iter()
        .find(|w| w.text_start <= offset && offset < w.text_end)
        .map(|w| (w.text_start, w.text_end))
        .unwrap_or((offset.min(len), (offset + 1).min(len)))
}

/// The locator range covered by the words overlapping a char range of the
/// speakable text — how a text offset gets back to page geometry.
fn locator_range(words: &[WordSpan], start: u32, end: u32) -> Option<(u32, u32)> {
    let overlapping = words
        .iter()
        .filter(|w| w.text_start < end && start < w.text_end);
    overlapping
        .clone()
        .map(|w| w.locator_start)
        .min()
        .zip(overlapping.map(|w| w.locator_end).max())
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PageArea {
        pub session: RefCell<Option<SessionSlot>>,
        /// Text last announced to AT — the change detector that lets
        /// `page_changed` run after every draw and speak only on turns.
        pub announced: RefCell<Option<String>>,
    }

    impl PageArea {
        pub(super) fn speakable(&self) -> Option<SpeakablePage> {
            let slot = self.session.borrow();
            let page = slot.as_ref()?.borrow().as_ref()?.speakable_page();
            page
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PageArea {
        const NAME: &'static str = "ChapbookAppPageArea";
        type Type = super::PageArea;
        type ParentType = gtk::DrawingArea;
        type Interfaces = (gtk::AccessibleText,);
    }

    impl ObjectImpl for PageArea {}
    impl WidgetImpl for PageArea {}
    impl DrawingAreaImpl for PageArea {}

    // Every method is overridden rather than defaulted: the defaults
    // chain to a parent implementation this fresh interface does not
    // have, and would panic instead of answering.
    impl AccessibleTextImpl for PageArea {
        fn attributes(
            &self,
            _offset: u32,
        ) -> Vec<(gtk::AccessibleTextRange, glib::GString, glib::GString)> {
            Vec::new()
        }

        fn default_attributes(&self) -> Vec<(glib::GString, glib::GString)> {
            // Never empty: gtk4-rs 0.11 turns an empty list into NULL
            // arrays here, and GTK's AT-SPI bridge walks them to their
            // zero terminator without a NULL check — Orca's very first
            // GetDefaultAttributes call was a segfault against the
            // reference viewer. One honest attribute keeps the arrays
            // real. (The per-offset `attributes` path returns a checked
            // boolean and is safe empty.)
            let direction = {
                let slot = self.session.borrow();
                let direction = slot.as_ref().and_then(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|session| session.reading_direction())
                });
                match direction {
                    Some(chapbook_app::chapbook_reader::chapbook_core::ReadingDirection::Rtl) => {
                        "rtl"
                    }
                    _ => "ltr",
                }
            };
            vec![("direction".into(), direction.into())]
        }

        fn caret_position(&self) -> u32 {
            0
        }

        fn selection(&self) -> Vec<gtk::AccessibleTextRange> {
            Vec::new()
        }

        fn contents(&self, start: u32, end: u32) -> Option<glib::Bytes> {
            let page = self.speakable()?;
            Some(glib::Bytes::from_owned(
                slice_chars(&page.text, start, end).into_bytes(),
            ))
        }

        fn contents_at(
            &self,
            offset: u32,
            granularity: gtk::AccessibleTextGranularity,
        ) -> Option<(u32, u32, glib::Bytes)> {
            let page = self.speakable()?;
            let len = page.text.chars().count() as u32;
            let (start, end) = match granularity {
                gtk::AccessibleTextGranularity::Character => {
                    (offset.min(len), (offset + 1).min(len))
                }
                gtk::AccessibleTextGranularity::Word => word_around(&page.words, offset, len),
                // The page is one paragraph to AT; finer sentence/line
                // splits can come later without moving any boundary.
                _ => (0, len),
            };
            Some((
                start,
                end,
                glib::Bytes::from_owned(slice_chars(&page.text, start, end).into_bytes()),
            ))
        }

        fn extents(&self, start: u32, end: u32) -> Option<gtk::graphene::Rect> {
            let page = self.speakable()?;
            let (lo, hi) = locator_range(&page.words, start, end)?;
            let slot = self.session.borrow();
            let slot = slot.as_ref()?;
            let session = slot.borrow();
            let rects = session.as_ref()?.range_rects(lo, hi);
            let union = rects.into_iter().reduce(|a, b| a.union(&b))?;
            Some(gtk::graphene::Rect::new(
                union.origin.x,
                union.origin.y,
                union.size.w,
                union.size.h,
            ))
        }

        fn offset(&self, point: &gtk::graphene::Point) -> Option<u32> {
            // Word precision: the word under the point answers with its
            // first character, which is what word-oriented review reads.
            let slot = self.session.borrow();
            let (lo, hi) = slot
                .as_ref()?
                .borrow_mut()
                .as_mut()?
                .word_at(point.x(), point.y())?;
            let page = self.speakable()?;
            page.words
                .iter()
                .find(|w| (w.locator_start, w.locator_end) == (lo, hi))
                .map(|w| w.text_start)
        }
    }
}
