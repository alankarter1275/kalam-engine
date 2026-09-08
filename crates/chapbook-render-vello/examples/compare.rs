//! Render one fixture page through both backends and report how they differ.
//! `cargo run -p chapbook-render-vello --example compare -- <book> [out-dir]`

use chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::Session;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = &args[0];
    let out = args.get(1).cloned().unwrap_or_else(|| ".".to_string());

    let mut s = Session::open(source, chapbook_core::FontSource::host()).unwrap();
    s.set_metrics(PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: std::env::var("SCALE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0),
        rotation: Rotation::None,
    });
    for _ in 0..200 {
        s.render();
        if !s.has_pending_loads() {
            break;
        }
        s.poll_loaded();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    s.poll_loaded();

    let cpu = s.render().expect("cpu render");
    cpu.save_png(format!("{out}/cpu.png")).unwrap();

    let frame = s.frame().expect("frame");
    let ops = frame.list.ops.len();
    let glyph_runs = frame
        .list
        .ops
        .iter()
        .filter(|op| matches!(op, chapbook_paint::DisplayOp::GlyphRun { .. }))
        .count();
    let glyphs: usize = frame
        .list
        .ops
        .iter()
        .map(|op| match op {
            chapbook_paint::DisplayOp::GlyphRun { glyphs, .. } => glyphs.len(),
            _ => 0,
        })
        .sum();

    for op in &frame.list.ops {
        match op {
            chapbook_paint::DisplayOp::FillRect { rect, color } => println!(
                "fill    y {:8.3}..{:8.3}  x {:7.2}..{:7.2}  rgba {:?}",
                rect.min_y(),
                rect.max_y(),
                rect.min_x(),
                rect.max_x(),
                (color.r, color.g, color.b, color.a)
            ),
            chapbook_paint::DisplayOp::Image { dest, .. } => println!(
                "image   y {:8.3}..{:8.3}  x {:7.2}..{:7.2}",
                dest.min_y(),
                dest.max_y(),
                dest.min_x(),
                dest.max_x()
            ),
            chapbook_paint::DisplayOp::GlyphRun {
                origin, font_size, ..
            } => println!(
                "glyphs  baseline y {:8.3}  x {:7.2}  size {font_size}",
                origin.y, origin.x
            ),
        }
    }

    let mut vello = chapbook_render_vello::VelloRenderer::new().expect("vello");

    // ISOLATE=n renders the page ground plus the nth glyph run only, and
    // reports where each backend puts its ink relative to the baseline the
    // op declares. That is the arbiter: the op says where the baseline is.
    if let Ok(n) = std::env::var("ISOLATE") {
        let n: usize = n.parse().unwrap();
        let mut runs = frame
            .list
            .ops
            .iter()
            .filter(|op| matches!(op, chapbook_paint::DisplayOp::GlyphRun { .. }));
        let run = runs.nth(n).expect("no such glyph run").clone();
        let baseline = match &run {
            chapbook_paint::DisplayOp::GlyphRun { origin, .. } => origin.y,
            _ => unreachable!(),
        };
        let only = chapbook_paint::DisplayList {
            size: frame.list.size,
            ops: vec![frame.list.ops[0].clone(), run],
        };
        let mut cpu_only = tiny_skia::Pixmap::new(600, 800).unwrap();
        let (fonts, images) = s.paint_resources();
        chapbook_render_tinyskia::Renderer::new().render(
            &only,
            fonts,
            images,
            1.0,
            &mut cpu_only.as_mut(),
        );
        let gpu_only = vello.render(&only, fonts, images, 1.0).expect("gpu");

        let bounds = |rgba: &[u8], w: u32| {
            let (mut first, mut last) = (None, None);
            for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
                if px[0] < 200 {
                    let y = (i as u32 / w) as i32;
                    first.get_or_insert(y);
                    last = Some(y);
                }
            }
            (first, last)
        };
        if let chapbook_paint::DisplayOp::GlyphRun {
            glyphs, font_size, ..
        } = &only.ops[1]
        {
            println!("font_size {font_size}, {} glyphs", glyphs.len());
            for g in glyphs.iter().take(6) {
                println!("  glyph id {:5} x {:8.3} y {:8.3}", g.id, g.x, g.y);
            }
        }
        println!("baseline y = {baseline:.3}");
        println!("cpu ink rows {:?}", bounds(cpu_only.data(), 600));
        println!("gpu ink rows {:?}", bounds(&gpu_only.rgba, 600));
        return;
    }

    let (fonts, images) = s.paint_resources();
    let scale = std::env::var("SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let gpu = vello
        .render(&frame.list, fonts, images, scale)
        .expect("gpu");

    // Write the GPU page out as a PNG through tiny-skia, so both files are
    // produced the same way.
    let mut pixmap = tiny_skia::Pixmap::new(gpu.width, gpu.height).unwrap();
    pixmap.data_mut().copy_from_slice(&gpu.rgba);
    pixmap.save_png(format!("{out}/gpu.png")).unwrap();

    let dark = |rgba: &[u8]| {
        rgba.as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] < 128)
            .count()
    };
    println!("ops {ops}, glyph runs {glyph_runs}, glyphs {glyphs}");
    println!("cpu dark pixels: {}", dark(cpu.data()));
    println!("gpu dark pixels: {}", dark(&gpu.rgba));

    let centroid = |rgba: &[u8], w: u32| {
        let (mut m, mut mx, mut my) = (0.0f64, 0.0f64, 0.0f64);
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let lum =
                (0.2126 * f32::from(px[0]) + 0.7152 * f32::from(px[1]) + 0.0722 * f32::from(px[2]))
                    / 255.0;
            let d = f64::from(1.0 - lum);
            let (x, y) = ((i as u32 % w) as f64, (i as u32 / w) as f64);
            m += d;
            mx += d * x;
            my += d * y;
        }
        (mx / m, my / m, m)
    };
    let c = centroid(cpu.data(), cpu.width());
    let g = centroid(&gpu.rgba, gpu.width);
    println!("cpu centroid ({:.3}, {:.3}) mass {:.1}", c.0, c.1, c.2);
    println!("gpu centroid ({:.3}, {:.3}) mass {:.1}", g.0, g.1, g.2);
    println!("delta ({:+.3}, {:+.3})", g.0 - c.0, g.1 - c.1);

    // Where do they disagree? Per-row darkness difference, worst first.
    let row_mass = |rgba: &[u8], w: u32, h: u32| {
        let mut rows = vec![0.0f64; h as usize];
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let lum =
                (0.2126 * f32::from(px[0]) + 0.7152 * f32::from(px[1]) + 0.0722 * f32::from(px[2]))
                    / 255.0;
            rows[(i as u32 / w) as usize] += f64::from(1.0 - lum);
        }
        rows
    };
    let cr = row_mass(cpu.data(), cpu.width(), cpu.height());
    let gr = row_mass(&gpu.rgba, gpu.width, gpu.height);
    let mut diffs: Vec<(usize, f64)> = cr
        .iter()
        .zip(&gr)
        .enumerate()
        .map(|(y, (a, b))| (y, b - a))
        .collect();
    diffs.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
    if let Ok(range) = std::env::var("ROWS") {
        let (a, b) = range.split_once("..").unwrap();
        let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
        println!("row profile {a}..{b} (cpu | gpu):");
        for y in a..b.min(cr.len()) {
            println!("  y={y:4}  {:8.2} | {:8.2}", cr[y], gr[y]);
        }
    }
    println!("worst rows (y: gpu-cpu darkness):");
    for (y, d) in diffs.iter().take(8) {
        println!(
            "  y={y:4} {d:+8.2}  (cpu {:7.2}, gpu {:7.2})",
            cr[*y], gr[*y]
        );
    }
}
