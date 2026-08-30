//! `render_into`: rasterizing into a caller-owned buffer.

mod common;
use chapbook_reader::Session;
use common::*;

/// A host's own buffer is a legitimate render target.
///
/// The point of `render_into` is that every platform already owns the
/// memory it wants the page in, and `render()` allocating a fresh `Pixmap`
/// per page turn meant a copy into that memory on every one. These check
/// the promise rather than the plumbing: same pixels, and the size a caller
/// is told to allocate is the size it gets.
mod render_into {
    use super::*;

    fn metrics() -> chapbook_core::PageMetrics {
        chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(20.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        }
    }

    fn ready(name: &str) -> Session {
        let mut session = open_isolated(name, &fixture("epub/minimal.epub"));
        session.set_metrics(metrics());
        session
    }

    #[test]
    fn render_size_matches_what_render_produces() {
        // `render_size` derives from the metrics; `render` sizes its pixmap
        // from the display list. A caller allocates from the first and the
        // engine rasterizes against the second, so if these ever diverge
        // `render_into` refuses rather than misdrawing — and this is what
        // notices the divergence.
        let mut session = ready("render-size");
        let (w, h) = session.render_size().expect("metrics are set");
        let pixmap = session.render().expect("something to draw");
        assert_eq!((pixmap.width(), pixmap.height()), (w, h));
    }

    #[test]
    fn a_borrowed_buffer_gets_the_same_pixels_as_an_allocated_one() {
        let mut owned = ready("into-owned");
        let expected = owned.render().expect("something to draw");
        let (w, h) = (expected.width(), expected.height());

        let mut borrowed = ready("into-borrowed");
        let mut dst = vec![0u8; (w * h * 4) as usize];
        assert!(borrowed.render_into(&mut dst, w, h, (w * 4) as usize));

        assert_eq!(
            dst,
            expected.data(),
            "byte-exact, or it is not the same page"
        );
    }

    #[test]
    fn a_padded_stride_lands_row_by_row_and_leaves_the_padding_alone() {
        // Android exposes a stride and does not promise it equals the row.
        // The slow path has to be correct even though it is not free.
        let mut owned = ready("stride-owned");
        let expected = owned.render().expect("something to draw");
        let (w, h) = (expected.width(), expected.height());
        let row = (w * 4) as usize;
        let stride = row + 64;

        let mut session = ready("stride-into");
        let mut dst = vec![0xABu8; stride * h as usize];
        assert!(session.render_into(&mut dst, w, h, stride));

        for y in 0..h as usize {
            let at = y * stride;
            assert_eq!(&dst[at..at + row], &expected.data()[y * row..(y + 1) * row]);
            assert!(
                dst[at + row..at + stride].iter().all(|b| *b == 0xAB),
                "row {y}: padding is the host's, not ours"
            );
        }
    }

    #[test]
    fn a_rotated_page_still_arrives_the_right_way_up() {
        let mut session = ready("rotated");
        session.set_metrics(chapbook_core::PageMetrics {
            rotation: chapbook_core::Rotation::Quarter,
            ..metrics()
        });
        let (w, h) = session.render_size().expect("metrics are set");
        // The quarter turn swaps the axes, and `render_size` says so.
        assert_eq!((w, h), (600, 400));

        let expected = session.render().expect("something to draw");
        assert_eq!((expected.width(), expected.height()), (w, h));

        let mut into = ready("rotated-into");
        into.set_metrics(chapbook_core::PageMetrics {
            rotation: chapbook_core::Rotation::Quarter,
            ..metrics()
        });
        let mut dst = vec![0u8; (w * h * 4) as usize];
        assert!(into.render_into(&mut dst, w, h, (w * 4) as usize));
        assert_eq!(dst, expected.data());
    }

    #[test]
    fn a_buffer_that_does_not_fit_is_refused_rather_than_overrun() {
        let mut session = ready("refused");
        let (w, h) = session.render_size().expect("metrics are set");
        let row = (w * 4) as usize;

        // Wrong dimensions.
        let mut dst = vec![0u8; row * h as usize];
        assert!(!session.render_into(&mut dst, w + 1, h, row));
        assert!(!session.render_into(&mut dst, w, h + 1, row));
        // A stride narrower than a row.
        assert!(!session.render_into(&mut dst, w, h, row - 1));
        // Right shape, too few bytes.
        let mut short = vec![0u8; row * h as usize - 1];
        assert!(!session.render_into(&mut short, w, h, row));
    }

    #[test]
    fn without_metrics_there_is_no_size_and_nothing_to_render_into() {
        let mut session = open_isolated("no-metrics", &fixture("epub/minimal.epub"));
        assert!(session.render_size().is_none());
        let mut dst = vec![0u8; 16];
        assert!(!session.render_into(&mut dst, 2, 2, 8));
    }
}
