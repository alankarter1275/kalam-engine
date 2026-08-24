//! Micro-bench for the panel conversion, which the stage timings put at a
//! third of a page turn's cost.
//!
//!     cargo run --release -p chapbook-paint --example quantbench

use std::time::Instant;

use chapbook_core::{PixelFormat, Rotation};

fn page(w: u32, h: u32) -> Vec<u8> {
    // Text-like: mostly light, with darker runs, so dithering has real
    // residuals to push around rather than a flat field.
    (0..w * h)
        .flat_map(|i| {
            let x = i % w;
            let v = if (x / 7).is_multiple_of(3) {
                40u8
            } else {
                235u8
            };
            let v = v.wrapping_add((i % 11) as u8);
            [v, v.wrapping_sub(3), v.wrapping_add(5), 255]
        })
        .collect()
}

fn bench(name: &str, reps: u32, mut f: impl FnMut()) {
    f(); // warm
    let t = Instant::now();
    for _ in 0..reps {
        f();
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / f64::from(reps);
    println!("  {name:<34} {per:>7.3} ms");
}

fn main() {
    for (label, w, h) in [
        ("Pi 7in  800x480", 800u32, 480u32),
        ("Clara   1448x1072", 1448, 1072),
    ] {
        let src = page(w, h);
        println!(
            "\n{label}  ({} px, {:.1} MiB RGBA)",
            w * h,
            src.len() as f64 / 1048576.0
        );
        let mut buf = src.clone();

        bench("quantize 16-level, dither", 50, || {
            buf.copy_from_slice(&src);
            chapbook_paint::quantize(
                &mut buf,
                w,
                h,
                PixelFormat::Grey {
                    levels: 16,
                    dither: true,
                },
            );
        });
        bench("quantize 16-level, no dither", 50, || {
            buf.copy_from_slice(&src);
            chapbook_paint::quantize(
                &mut buf,
                w,
                h,
                PixelFormat::Grey {
                    levels: 16,
                    dither: false,
                },
            );
        });
        bench("quantize Rgba (no-op)", 50, || {
            buf.copy_from_slice(&src);
            chapbook_paint::quantize(&mut buf, w, h, PixelFormat::Rgba);
        });
        bench("rotate None (full copy)", 50, || {
            std::hint::black_box(chapbook_paint::rotate(&src, w, h, Rotation::None));
        });
        bench("rotate Quarter", 50, || {
            std::hint::black_box(chapbook_paint::rotate(&src, w, h, Rotation::Quarter));
        });
        bench("baseline: memcpy the buffer", 50, || {
            buf.copy_from_slice(&src);
        });
    }
}
