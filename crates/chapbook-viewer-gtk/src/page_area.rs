//! The page as a widget with an accessible text surface.
//!
//! A rasterized page is a picture, and a picture of text is unusable with
//! a screen reader. This subclass puts `Session`'s text surface —
//! [`chapbook_reader::Session::speakable_page`] for the words,
//! [`chapbook_reader::Session::range_rects`] for the geometry — behind
//! GTK's `AccessibleText` interface, which is what Orca reads over
//! AT-SPI. It is the workspace's first GObject subclass, and it exists as
//! much to prove the accessor's shape against a real assistive stack as
//! to serve this shell.
//!
//! Offsets on this boundary are Unicode character offsets into the
//! speakable string, which is also what [`chapbook_reader::WordSpan`]
//! carries — no second offset space appears here. Geometry crosses in
//! widget coordinates; this shell paints at 1/scale and never rotates, so
//! page space and logical widget space are the same thing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

use chapbook_reader::{Session, SpeakablePage, WordSpan};

glib::wrapper! {
    pub struct PageArea(ObjectSubclass<imp::PageArea>)
        @extends gtk::DrawingArea, gtk::Widget,
        @implements gtk::Accessible, gtk::AccessibleText, gtk::Buildable, gtk::ConstraintTarget;
}

impl PageArea {
    pub fn new(session: Rc<RefCell<Session>>) -> Self {
        let area: Self = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::Paragraph)
            .build();
        area.imp().session.replace(Some(session));
        area
    }

    /// Tell assistive technology the page's text changed wholesale — a
    /// page turn, a relayout, a background load landing. AT has no other
    /// way to know: it caches what `contents` last said.
    pub fn page_changed(&self) {
        let imp = self.imp();
        let now = imp
            .speakable()
            .map(|page| page.text.chars().count() as u32)
            .unwrap_or(0);
        let before = imp.announced.replace(now);
        if before > 0 {
            self.update_contents(gtk::AccessibleTextContentChange::Remove, 0, before);
        }
        if now > 0 {
            self.update_contents(gtk::AccessibleTextContentChange::Insert, 0, now);
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
        pub session: RefCell<Option<Rc<RefCell<Session>>>>,
        /// Chars last announced to AT, so a page turn can retract them.
        pub announced: Cell<u32>,
    }

    impl PageArea {
        pub(super) fn speakable(&self) -> Option<SpeakablePage> {
            let session = self.session.borrow();
            let page = session.as_ref()?.borrow().speakable_page();
            page
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PageArea {
        const NAME: &'static str = "ChapbookPageArea";
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
            Vec::new()
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
            let session = self.session.borrow();
            let rects = session.as_ref()?.borrow().range_rects(lo, hi);
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
            let session = self.session.borrow();
            let (lo, hi) = session
                .as_ref()?
                .borrow_mut()
                .word_at(point.x(), point.y())?;
            let page = self.speakable()?;
            page.words
                .iter()
                .find(|w| (w.locator_start, w.locator_end) == (lo, hi))
                .map(|w| w.text_start)
        }
    }
}
