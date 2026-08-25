//! Bring-up self-test: write known colours, read them back, and check the
//! packing against what the device itself says its pixels look like.
//!
//!     cargo run -p chapbook-panel-fbdev --example selftest -- [/dev/fb0]
//!
//! Unit tests can only assert the bitfield constants someone typed while
//! guessing at hardware. This asserts against the numbers the kernel
//! reports for the panel actually present, which is the only way to find
//! out that a format nobody owns was packed wrong. It is the second thing
//! to run on a new device, after `probe`.
//!
//! It writes to the *bottom* row and leaves it there. Not the top: a
//! framebuffer console draws in the top-left, and a console redrawing
//! between the write and the read back makes this fail intermittently for
//! a reason that has nothing to do with the packing. A shell owns the
//! panel on a real device, but a bring-up tool should not assume it.

use chapbook_core::{Panel, PanelRect};
use chapbook_panel_fbdev::FbdevPanel;

/// What a colour must pack to, worked out from the device's own channel
/// placement rather than from anything this crate believes.
fn expected(r: u8, g: u8, b: u8, channels: [(u32, u32); 3]) -> u32 {
    let mut word = 0u32;
    for (value, (offset, length)) in [r, g, b].into_iter().zip(channels) {
        let fitted = match length {
            0 => 0,
            1..=8 => u32::from(value) >> (8 - length),
            _ => u32::from(value) << (length - 8),
        };
        word |= fitted << offset;
    }
    word
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/fb0".to_string());
    let mut panel = FbdevPanel::open_path(&device)?;
    println!("{device}: {}", panel.describe());

    let cases: [(&str, [u8; 3]); 7] = [
        ("black", [0x00, 0x00, 0x00]),
        ("white", [0xFF, 0xFF, 0xFF]),
        ("red", [0xFF, 0x00, 0x00]),
        ("green", [0x00, 0xFF, 0x00]),
        ("blue", [0x00, 0x00, 0xFF]),
        ("mid grey", [0x80, 0x80, 0x80]),
        ("odd mix", [0x12, 0x34, 0x56]),
    ];

    let info = panel.info();
    let (w, h) = (info.width, info.height);
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    // One case per pixel along the bottom row.
    let row = h - 1;
    for (i, (_, [r, g, b])) in cases.iter().enumerate() {
        let at = (row as usize * w as usize + i) * 4;
        rgba[at..at + 4].copy_from_slice(&[*r, *g, *b, 0xFF]);
    }
    panel.blit(&rgba, PanelRect::new(0, row, cases.len() as u32, 1))?;

    let channels = panel.channels();
    println!("channels: {channels:?}");
    let mut failures = 0;
    for (i, (name, [r, g, b])) in cases.iter().enumerate() {
        let got = panel.read_pixel(i as u32, row).expect("pixel is on screen");
        match channels {
            Some(channels) => {
                let want = expected(*r, *g, *b, channels);
                let ok = got == want;
                failures += usize::from(!ok);
                println!(
                    "  {:>8}  wrote {r:02x}{g:02x}{b:02x}  read {got:#010x}  want {want:#010x}  {}",
                    name,
                    if ok { "ok" } else { "MISMATCH" }
                );
            }
            None => {
                // Greyscale or mono: no channel placement to check
                // against, so only the ordering property is meaningful.
                // On a 1bpp panel `read_pixel` returns the bit itself,
                // so the interesting reading is 0 versus 1.
                println!(
                    "  {name:>8}  wrote {r:02x}{g:02x}{b:02x}  read {got:#010x}  \
                     (no channel placement)"
                );
            }
        }
    }

    if channels.is_some() {
        // Distinct colours must not collapse onto one word: a stride or
        // byte-order error often shows up as everything reading alike.
        let mut seen: Vec<u32> = (0..cases.len())
            .filter_map(|i| panel.read_pixel(i as u32, row))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        if seen.len() < cases.len() {
            println!(
                "  DISTINCTNESS: {} of {} words differ",
                seen.len(),
                cases.len()
            );
            failures += 1;
        }
    }

    if failures == 0 {
        println!("PASS");
        Ok(())
    } else {
        println!("FAIL: {failures} check(s)");
        std::process::exit(1)
    }
}
