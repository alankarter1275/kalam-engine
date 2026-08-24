//! Report what a framebuffer says about itself, without drawing on it.
//!
//!     cargo run -p chapbook-panel-fbdev --example probe -- [/dev/fb0]
//!
//! The first thing to run when bringing up a device, and the only part of
//! this backend that can be checked without taking over the screen: if the
//! geometry here is wrong, the `#[repr(C)]` structs do not match the
//! kernel's and nothing below it can be trusted.

use chapbook_core::Panel;
use chapbook_panel_fbdev::FbdevPanel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/fb0".to_string());
    let panel = FbdevPanel::open_path(&device)?;
    let info = panel.info();
    println!("{device}: {}", panel.describe());
    println!(
        "  reports to chapbook as {}x{} {:?}",
        info.width, info.height, info.format
    );
    Ok(())
}
