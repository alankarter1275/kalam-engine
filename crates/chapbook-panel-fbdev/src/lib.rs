//! A [`Panel`] over a plain Linux framebuffer.
//!
//! This backend has no electrophoretic display controller behind it, so
//! it cannot honour an [`UpdateClass`]: there is no waveform to choose
//! and no update to wait for. Pixels written to the mapping are simply on
//! screen. That is the whole point of it.
//!
//! It exists to give the panel seam a second implementor, the way the
//! vello backend did for the display list — and it can run today, on a
//! virtual machine or a Raspberry Pi, which no e-ink backend can. `mxcfb`
//! is not a generic e-ink API but NXP's driver for one SoC family, its
//! struct layouts and waveform constants differ per vendor and per model,
//! and the quirks that matter were all found by someone holding the
//! device. Writing that layer without hardware would produce code that
//! runs, which is not the same as code that is right.
//!
//! So: everything above `Panel` gets exercised end to end here — the
//! display list, quantization, rotation, damage, and
//! [`chapbook_core::PanelDriver`]'s collision and cleanup rules — against
//! a screen that actually shows the result. What is left for a device is
//! only the part that genuinely needs one.

mod format;

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;

use chapbook_core::{
    ChapbookError, Panel, PanelInfo, PanelRect, PixelFormat, Result, UpdateClass, UpdateToken,
};

use format::{Bitfield, Encoding, Layout};

// linux/fb.h. Plain constants rather than _IOR-derived: the framebuffer
// ioctls predate the encoding scheme and are literal numbers.
const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    kind: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

impl Default for FbFixScreeninfo {
    fn default() -> Self {
        // Safety: every field is a plain integer or an array of them.
        unsafe { std::mem::zeroed() }
    }
}

/// The structs above are transcriptions of `linux/fb.h`, and a
/// transcription that drifts does not fail — it reads plausible garbage
/// out of the wrong offsets. `--example probe` catches that on whatever
/// machine runs it; these catch it on every target the crate is ever
/// compiled for, including ones nobody has.
///
/// `fb_var_screeninfo` is all `__u32`, so its layout is the same
/// everywhere. `fb_fix_screeninfo` embeds two `unsigned long`, which is
/// eight bytes on a 64-bit target and four on a 32-bit one — and every
/// e-ink device is 32-bit or 64-bit ARM, so both arms of this are real.
mod layout_matches_the_kernel {
    use super::{FbFixScreeninfo, FbVarScreeninfo};
    use std::mem::{offset_of, size_of};

    const _: () = {
        assert!(offset_of!(FbVarScreeninfo, xres) == 0);
        assert!(offset_of!(FbVarScreeninfo, yres) == 4);
        assert!(offset_of!(FbVarScreeninfo, xres_virtual) == 8);
        assert!(offset_of!(FbVarScreeninfo, yres_virtual) == 12);
        assert!(offset_of!(FbVarScreeninfo, bits_per_pixel) == 24);
        assert!(offset_of!(FbVarScreeninfo, grayscale) == 28);
        assert!(offset_of!(FbVarScreeninfo, red) == 32);
        assert!(offset_of!(FbVarScreeninfo, green) == 44);
        assert!(offset_of!(FbVarScreeninfo, blue) == 56);
        assert!(offset_of!(FbVarScreeninfo, transp) == 68);
        assert!(offset_of!(FbVarScreeninfo, rotate) == 136);
        assert!(size_of::<FbVarScreeninfo>() == 160);
    };

    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(offset_of!(FbFixScreeninfo, smem_start) == 16);
        assert!(offset_of!(FbFixScreeninfo, smem_len) == 24);
        assert!(offset_of!(FbFixScreeninfo, visual) == 36);
        assert!(offset_of!(FbFixScreeninfo, line_length) == 48);
        assert!(offset_of!(FbFixScreeninfo, mmio_start) == 56);
        assert!(size_of::<FbFixScreeninfo>() == 80);
    };

    #[cfg(target_pointer_width = "32")]
    const _: () = {
        assert!(offset_of!(FbFixScreeninfo, smem_start) == 16);
        assert!(offset_of!(FbFixScreeninfo, smem_len) == 20);
        assert!(offset_of!(FbFixScreeninfo, visual) == 32);
        assert!(offset_of!(FbFixScreeninfo, line_length) == 44);
        assert!(offset_of!(FbFixScreeninfo, mmio_start) == 48);
        assert!(size_of::<FbFixScreeninfo>() == 68);
    };
}

fn errno(what: &str) -> ChapbookError {
    ChapbookError::Panel(format!("{what}: {}", std::io::Error::last_os_error()))
}

/// A framebuffer device, mapped and ready to draw into.
#[derive(Debug)]
pub struct FbdevPanel {
    // Held open for the lifetime of the mapping.
    _file: File,
    map: *mut u8,
    map_len: usize,
    layout: Layout,
    width: u32,
    height: u32,
    format: PixelFormat,
    next_token: u32,
}

// Safety: the mapping is owned exclusively by this struct, which hands it
// out only through `&mut self`. Nothing else holds the pointer.
unsafe impl Send for FbdevPanel {}

impl FbdevPanel {
    /// Open the default framebuffer, `/dev/fb0`.
    pub fn open() -> Result<Self> {
        FbdevPanel::open_path("/dev/fb0")
    }

    /// Open a specific framebuffer device.
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| ChapbookError::Panel(format!("open {}: {e}", path.display())))?;
        let fd = file.as_raw_fd();

        let mut var = FbVarScreeninfo::default();
        let mut fix = FbFixScreeninfo::default();
        // Safety: both ioctls fill a caller-owned struct of the size the
        // kernel expects, and the fd is a framebuffer we just opened.
        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var) < 0 {
                return Err(errno("FBIOGET_VSCREENINFO"));
            }
            if libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix) < 0 {
                return Err(errno("FBIOGET_FSCREENINFO"));
            }
        }

        if var.bits_per_pixel == 0 || var.bits_per_pixel % 8 != 0 || var.bits_per_pixel > 32 {
            return Err(ChapbookError::Panel(format!(
                "{} reports {} bits per pixel, which this backend cannot pack",
                path.display(),
                var.bits_per_pixel
            )));
        }
        if var.xres == 0 || var.yres == 0 {
            return Err(ChapbookError::Panel(format!(
                "{} reports a {}x{} screen",
                path.display(),
                var.xres,
                var.yres
            )));
        }

        let line_length = if fix.line_length > 0 {
            fix.line_length as usize
        } else {
            var.xres as usize * (var.bits_per_pixel as usize / 8)
        };
        // `smem_len` is what is actually mappable; the visible geometry can
        // be smaller than the virtual one, and a pan offset lives in the
        // difference.
        let map_len = if fix.smem_len > 0 {
            fix.smem_len as usize
        } else {
            line_length * var.yres_virtual.max(var.yres) as usize
        };

        // Safety: mapping a framebuffer we hold open, with a length the
        // kernel just reported for it.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(errno(&format!("mmap {}", path.display())));
        }

        let grey = var.grayscale != 0
            || (var.red.length == 0 && var.green.length == 0 && var.blue.length == 0);
        let encoding = if grey {
            Encoding::Grey
        } else {
            Encoding::Channels {
                red: Bitfield {
                    offset: var.red.offset,
                    length: var.red.length,
                },
                green: Bitfield {
                    offset: var.green.offset,
                    length: var.green.length,
                },
                blue: Bitfield {
                    offset: var.blue.offset,
                    length: var.blue.length,
                },
            }
        };

        Ok(FbdevPanel {
            _file: file,
            map: map.cast::<u8>(),
            map_len,
            layout: Layout {
                bytes_per_pixel: var.bits_per_pixel as usize / 8,
                line_length,
                encoding,
            },
            width: var.xres,
            height: var.yres,
            // Full colour by default even on a greyscale framebuffer: it
            // can show 256 levels, so asking the pipeline to quantize to
            // e-ink's sixteen would throw away quality for nothing. Use
            // `with_format` to emulate a shallower panel deliberately.
            format: PixelFormat::Rgba,
            next_token: 1,
        })
    }

    /// Report a different [`PixelFormat`] than the device's own, so the
    /// pipeline quantizes as if for a shallower panel.
    ///
    /// The point is being able to see what a 16-level e-ink page will look
    /// like without owning one.
    pub fn with_format(mut self, format: PixelFormat) -> Self {
        self.format = format;
        self
    }

    /// A description of the device, for a shell that wants to say what it
    /// found — the same reason the GPU backend names its adapter.
    pub fn describe(&self) -> String {
        let depth = self.layout.bytes_per_pixel * 8;
        let encoding = match self.layout.encoding {
            Encoding::Grey => "grey".to_string(),
            // Offsets as well as widths: a wrong `#[repr(C)]` shows up
            // here as nonsense, where a swapped channel order would
            // otherwise only be visible as wrong colours on the glass.
            Encoding::Channels { red, green, blue } => format!(
                "R{}@{} G{}@{} B{}@{}",
                red.length, red.offset, green.length, green.offset, blue.length, blue.offset
            ),
        };
        format!(
            "{}x{} {depth}bpp {encoding}, stride {}",
            self.width, self.height, self.layout.line_length
        )
    }

    fn mapping(&mut self) -> &mut [u8] {
        // Safety: `map` came from a successful mmap of `map_len` bytes and
        // is unmapped only in `Drop`, so it is valid for the whole life of
        // this borrow, and `&mut self` makes the slice exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.map, self.map_len) }
    }
}

impl Drop for FbdevPanel {
    fn drop(&mut self) {
        // Safety: unmapping exactly the region this struct mapped, once.
        unsafe {
            libc::munmap(self.map.cast::<libc::c_void>(), self.map_len);
        }
    }
}

impl Panel for FbdevPanel {
    fn info(&self) -> PanelInfo {
        PanelInfo {
            width: self.width,
            height: self.height,
            format: self.format,
        }
    }

    fn blit(&mut self, rgba: &[u8], rect: PanelRect) -> Result<()> {
        let expected = self.width as usize * self.height as usize * 4;
        if rgba.len() != expected {
            return Err(ChapbookError::Panel(format!(
                "buffer is {} bytes, panel needs {expected}",
                rgba.len()
            )));
        }
        let (layout, width) = (self.layout, self.width);
        format::blit_into(self.mapping(), &layout, rgba, width, rect);
        Ok(())
    }

    /// No-op beyond handing back a token.
    ///
    /// A framebuffer has no update to request: the pixels went on screen
    /// during [`Panel::blit`]. The class is accepted and ignored, which is
    /// honest — this is the backend that shows what the seam looks like
    /// when the panel underneath cannot do anything clever.
    fn submit(&mut self, _rect: PanelRect, _class: UpdateClass) -> Result<UpdateToken> {
        let token = UpdateToken(self.next_token);
        self.next_token = self.next_token.wrapping_add(1).max(1);
        Ok(token)
    }

    /// Returns immediately: nothing was ever in flight.
    fn wait(&mut self, _token: UpdateToken) -> Result<()> {
        Ok(())
    }
}
