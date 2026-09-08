//! Headless session dump: render the current page (optionally with a
//! drag-selection) to a PNG. A dev tool for eyeballing shell-free session
//! behavior: `cargo run -p chapbook-reader --example dump -- <source> <out.png> [page] [x0 y0 x1 y1]`

use chapbook_reader::chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::Session;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (source, out) = (&args[0], &args[1]);
    let mut session = Session::open(source, chapbook_core::FontSource::host()).unwrap();
    session.set_metrics(PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
        rotation: Rotation::None,
    });
    render_loaded(&mut session);
    let mut rest: Vec<f32> = args[2..].iter().map(|a| a.parse().unwrap()).collect();
    if rest.len() % 4 == 1 {
        for _ in 0..rest.remove(0) as usize {
            session.next_page();
            render_loaded(&mut session);
        }
    }
    if let [x0, y0, x1, y1] = rest[..] {
        session.selection_begin(x0, y0);
        session.selection_drag(x1, y1);
        eprintln!("selection: {:?}", session.selected_range());
    }
    render_loaded(&mut session).save_png(out).unwrap();
}

fn render_loaded(session: &mut Session) -> chapbook_reader::tiny_skia::Pixmap {
    loop {
        let pixmap = session.render().unwrap();
        if !session.has_pending_loads() {
            session.poll_loaded();
            return session.render().unwrap();
        }
        session.poll_loaded();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = pixmap;
    }
}
