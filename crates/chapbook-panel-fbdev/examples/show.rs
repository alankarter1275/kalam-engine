//! Read a book on a Linux framebuffer.
//!
//!     cargo run -p chapbook-panel-fbdev --example show -- <book> [/dev/fb0]
//!
//! Deliberately written the way a device shell would be, rather than the
//! short way. It goes through `Session::frame` and rasterizes for itself,
//! so the whole seam is exercised: display list, panel policy, damage,
//! intent, and `PanelDriver`'s rules — not just `Session::render`, which
//! would hide all of it behind one call.
//!
//! Needs write access to the framebuffer (`video` group, or root), and a
//! console that is not being redrawn underneath it — `chvt` to a free one,
//! or run it in a VM.

use std::time::Duration;

use chapbook_core::{EdgeSizes, PageMetrics, Panel, PanelDriver, PanelRect, Rotation, Size};
use chapbook_panel_fbdev::FbdevPanel;
use chapbook_reader::{chapbook_paint, chapbook_render_tinyskia, tiny_skia, Session};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(book) = args.next() else {
        eprintln!("usage: show <book.epub|comic.cbz|doc.pdf> [/dev/fb0]");
        std::process::exit(2);
    };
    let device = args.next().unwrap_or_else(|| "/dev/fb0".to_string());

    let panel = FbdevPanel::open_path(&device)?;
    // Say what was found, for the same reason the GPU backend names its
    // adapter: "it drew something" is not the same as "it drew it right".
    eprintln!("show: {device} — {}", panel.describe());
    let info = panel.info();
    let mut driver = PanelDriver::new(panel);

    let mut session = Session::open(&book)?;
    // The page is laid out in reading orientation; rotation happens on the
    // way to the panel, so an unrotated shell hands over its own size.
    let page = Size::new(info.width as f32, info.height as f32);
    let rotation = Rotation::None;
    session.set_metrics(PageMetrics {
        size: page,
        margins: EdgeSizes::uniform(32.0),
        dpi_scale: 1.0,
        rotation,
    });
    session.set_pixel_format(info.format);

    let mut renderer = chapbook_render_tinyskia::Renderer::new();
    let mut show = |session: &mut Session, driver: &mut PanelDriver<FbdevPanel>| {
        let Some(frame) = session.frame() else {
            return Ok(());
        };
        let (w, h) = (page.w as u32, page.h as u32);
        let Some(mut pixmap) = tiny_skia::Pixmap::new(w, h) else {
            return Ok(());
        };
        let (fonts, images) = session.paint_resources();
        renderer.render(&frame.list, fonts, images, 1.0, &mut pixmap);

        // Panel policy, exactly as any other backend applies it. The
        // display list says where dithering belongs — over images, not
        // over text — which is the one part of this a shell cannot work
        // out from the pixels it was handed.
        let dithered = frame.list.dither_regions(1.0);
        chapbook_paint::quantize_regions(pixmap.data_mut(), w, h, info.format, &dithered);
        let rgba = chapbook_paint::rotate(pixmap.data(), w, h, rotation);

        let damage = frame
            .damage
            .map(|rect| PanelRect::from_page(rect, page, 1.0, rotation));
        driver
            .present(&rgba, damage, frame.intent.update_class())
            .map(|_| ())
    };

    show(&mut session, &mut driver)?;
    // A few page turns, so the damage and cadence paths get walked rather
    // than just the first full paint.
    for _ in 0..3 {
        std::thread::sleep(Duration::from_secs(2));
        // `next_page` answers "did it move" itself, which is the whole
        // reason it returns anything. Deriving it here is what broke this
        // loop the first time: it compared the page alone, and a turn off
        // the end of a unit crosses into the next one by resetting the
        // page to 0, so a successful move read as "did not move". Most
        // books open on a single-page cover, so it stopped on turn one.
        // `chapbook_reader::conformance` now asserts this for any shell.
        if !session.next_page() {
            break;
        }
        show(&mut session, &mut driver)?;
    }
    // Nothing is ever in flight on a framebuffer, but a shell owes this
    // before it tears the panel down and the habit should be visible here.
    driver.flush()?;
    Ok(())
}
