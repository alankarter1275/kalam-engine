//! Headless session dump: render the current page (optionally with a
//! drag-selection) to a PNG. A dev tool for eyeballing shell-free session
//! behavior: `cargo run -p chapbook-reader --example dump -- <source> <out.png> [x0 y0 x1 y1]`

use chapbook_reader::chapbook_core::{EdgeSizes, PageMetrics, Size};
use chapbook_reader::Session;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (source, out) = (&args[0], &args[1]);
    let mut session = Session::open(source).unwrap();
    session.set_metrics(PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
    });
    session.render().unwrap();
    if let [x0, y0, x1, y1] = args[2..]
        .iter()
        .map(|a| a.parse::<f32>().unwrap())
        .collect::<Vec<_>>()[..]
    {
        session.selection_begin(x0, y0);
        session.selection_drag(x1, y1);
        eprintln!("selection: {:?}", session.selected_range());
    }
    session.render().unwrap().save_png(out).unwrap();
}
