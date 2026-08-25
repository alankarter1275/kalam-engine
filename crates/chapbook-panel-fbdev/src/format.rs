//! Turning chapbook's RGBA rows into whatever a framebuffer wants.
//!
//! This is the half of the backend that can be wrong in ways a test can
//! see, so it is kept free of file descriptors and mapped memory: plain
//! functions over slices, exercised against a `Vec` instead of a screen.
//! What is left in `lib.rs` — open, ioctl, mmap — either works or fails
//! loudly on the first call.

use chapbook_core::PanelRect;

/// Where one colour channel sits in a pixel, as the framebuffer reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Bitfield {
    pub offset: u32,
    pub length: u32,
}

/// How a framebuffer spells a pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// One luminance value per pixel, for a greyscale framebuffer.
    Grey,
    /// Channels placed by the device's own bitfields, which is what makes
    /// one code path cover RGB565, XRGB8888, BGRA8888 and the rest —
    /// nothing here needs to know which of those it is looking at.
    Channels {
        red: Bitfield,
        green: Bitfield,
        blue: Bitfield,
    },
}

/// Everything about the destination buffer's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub bytes_per_pixel: usize,
    /// Bytes per row, which is not always `width * bytes_per_pixel`:
    /// framebuffers pad rows to an alignment of their own choosing.
    pub line_length: usize,
    pub encoding: Encoding,
}

/// Fit an 8-bit channel into a field of `length` bits.
///
/// Narrowing drops low bits, which is what a 5-bit red field wants.
/// Widening shifts up rather than replicating: a framebuffer with more
/// than 8 bits per channel is rare enough that the extra fidelity is not
/// worth the arithmetic, and the error is below one 8-bit step either way.
fn fit(value: u8, length: u32) -> u32 {
    match length {
        0 => 0,
        1..=8 => u32::from(value) >> (8 - length),
        _ => u32::from(value) << (length - 8),
    }
}

/// Rec. 709 luminance, matching `chapbook_paint::quantize` so a greyscale
/// framebuffer and a greyscale panel agree about what a colour looks like.
fn luminance(r: u8, g: u8, b: u8) -> u8 {
    let lum = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    lum.round().clamp(0.0, 255.0) as u8
}

/// Write one pixel's low `bytes` bytes in the framebuffer's own order.
///
/// A framebuffer's bitfields describe positions inside a *native-endian*
/// word of `bits_per_pixel` bits, so the bytes reach memory the way this
/// machine spells that word — not always little-endian. On a big-endian
/// target the low bytes of a sub-word pixel sit at the end of the
/// representation, which is why this is not a plain truncation.
///
/// Every e-ink device is little-endian ARM, so nothing in the field
/// depends on the distinction; it is here because getting it silently
/// wrong would swap every channel on a target nobody thought to check.
fn write_pixel(dst: &mut [u8], word: u32, bytes: usize) {
    let repr = word.to_ne_bytes();
    let low = if cfg!(target_endian = "big") {
        &repr[4 - bytes..]
    } else {
        &repr[..bytes]
    };
    dst[..bytes].copy_from_slice(low);
}

/// Read a pixel back out of framebuffer memory — the inverse of
/// [`write_pixel`], for diagnostics.
pub(crate) fn read_native(bytes: &[u8]) -> u32 {
    let mut repr = [0u8; 4];
    let n = bytes.len().min(4);
    if cfg!(target_endian = "big") {
        repr[4 - n..].copy_from_slice(&bytes[..n]);
    } else {
        repr[..n].copy_from_slice(&bytes[..n]);
    }
    u32::from_ne_bytes(repr)
}

/// One source pixel as the framebuffer's own word.
pub(crate) fn pack(r: u8, g: u8, b: u8, encoding: Encoding) -> u32 {
    match encoding {
        Encoding::Grey => u32::from(luminance(r, g, b)),
        Encoding::Channels { red, green, blue } => {
            fit(r, red.length) << red.offset
                | fit(g, green.length) << green.offset
                | fit(b, blue.length) << blue.offset
        }
    }
}

/// Copy `rect` out of a panel-sized RGBA buffer into a framebuffer mapping.
///
/// Both ends are clipped rather than trusted. The rect comes from damage
/// arithmetic and the mapping's length comes from the kernel, and a blit
/// that runs off either end of mapped memory is a fault rather than a
/// wrong pixel — so the bounds are checked per row, once, and short rows
/// are skipped.
pub(crate) fn blit_into(
    dst: &mut [u8],
    layout: &Layout,
    rgba: &[u8],
    src_width: u32,
    rect: PanelRect,
) {
    let bpp = layout.bytes_per_pixel;
    if bpp == 0 || bpp > 4 || src_width == 0 {
        return;
    }
    let src_width = src_width as usize;
    let src_rows = rgba.len() / (src_width * 4);
    let dst_rows = dst.len().checked_div(layout.line_length).unwrap_or(0);

    let x0 = rect.x as usize;
    let x1 = (rect.max_x() as usize).min(src_width);
    let y0 = rect.y as usize;
    let y1 = (rect.max_y() as usize).min(src_rows).min(dst_rows);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    for y in y0..y1 {
        let src_row = y * src_width * 4;
        let dst_row = y * layout.line_length;
        for x in x0..x1 {
            let src = src_row + x * 4;
            let dst_at = dst_row + x * bpp;
            if dst_at + bpp > dst.len() {
                break;
            }
            let word = pack(rgba[src], rgba[src + 1], rgba[src + 2], layout.encoding);
            write_pixel(&mut dst[dst_at..dst_at + bpp], word, bpp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BGRA8888: Encoding = Encoding::Channels {
        red: Bitfield {
            offset: 16,
            length: 8,
        },
        green: Bitfield {
            offset: 8,
            length: 8,
        },
        blue: Bitfield {
            offset: 0,
            length: 8,
        },
    };
    const RGB565: Encoding = Encoding::Channels {
        red: Bitfield {
            offset: 11,
            length: 5,
        },
        green: Bitfield {
            offset: 5,
            length: 6,
        },
        blue: Bitfield {
            offset: 0,
            length: 5,
        },
    };

    /// The inverse of [`write_pixel`], so the tests assert the packing
    /// rather than the endianness of whatever ran them.
    fn read_pixel(bytes: &[u8]) -> u32 {
        let mut repr = [0u8; 4];
        if cfg!(target_endian = "big") {
            repr[4 - bytes.len()..].copy_from_slice(bytes);
        } else {
            repr[..bytes.len()].copy_from_slice(bytes);
        }
        u32::from_ne_bytes(repr)
    }

    fn layout(bpp: usize, line_length: usize, encoding: Encoding) -> Layout {
        Layout {
            bytes_per_pixel: bpp,
            line_length,
            encoding,
        }
    }

    #[test]
    fn channels_land_where_the_bitfields_say() {
        assert_eq!(pack(0xFF, 0x00, 0x00, BGRA8888), 0x00FF_0000);
        assert_eq!(pack(0x00, 0xFF, 0x00, BGRA8888), 0x0000_FF00);
        assert_eq!(pack(0x00, 0x00, 0xFF, BGRA8888), 0x0000_00FF);
        assert_eq!(pack(0x12, 0x34, 0x56, BGRA8888), 0x0012_3456);
    }

    #[test]
    fn narrow_fields_drop_low_bits() {
        // 565 keeps the top 5, 6 and 5 bits.
        assert_eq!(pack(0xFF, 0xFF, 0xFF, RGB565), 0xFFFF);
        assert_eq!(pack(0xFF, 0x00, 0x00, RGB565), 0xF800);
        assert_eq!(pack(0x00, 0xFF, 0x00, RGB565), 0x07E0);
        assert_eq!(pack(0x00, 0x00, 0xFF, RGB565), 0x001F);
        // 0x07 is 0b0000_0111: its top five bits are all zero, so a
        // 5-bit field keeps nothing. 0x08 sets the lowest surviving bit.
        assert_eq!(pack(0x07, 0x00, 0x00, RGB565), 0x0000);
        assert_eq!(pack(0x08, 0x00, 0x00, RGB565), 0x0800);
    }

    #[test]
    fn grey_is_luminance_and_agrees_with_the_paint_side() {
        assert_eq!(pack(0, 0, 0, Encoding::Grey), 0);
        assert_eq!(pack(255, 255, 255, Encoding::Grey), 255);
        // Green carries most of the luminance, blue almost none.
        assert!(pack(0, 255, 0, Encoding::Grey) > pack(255, 0, 0, Encoding::Grey));
        assert!(pack(255, 0, 0, Encoding::Grey) > pack(0, 0, 255, Encoding::Grey));
        // Already-quantized grey survives the round trip unchanged.
        for level in [0u8, 17, 68, 119, 187, 255] {
            assert_eq!(pack(level, level, level, Encoding::Grey), u32::from(level));
        }
    }

    #[test]
    fn a_blit_touches_only_its_rect() {
        let layout = layout(4, 4 * 4, BGRA8888);
        let mut dst = vec![0u8; 4 * 4 * 4];
        let rgba = [0xFFu8, 0xFF, 0xFF, 0xFF].repeat(4 * 4);
        blit_into(&mut dst, &layout, &rgba, 4, PanelRect::new(1, 1, 2, 2));

        for y in 0..4usize {
            for x in 0..4usize {
                let at = y * 16 + x * 4;
                let inside = (1..3).contains(&x) && (1..3).contains(&y);
                let word = read_pixel(&dst[at..at + 4]);
                assert_eq!(
                    word != 0,
                    inside,
                    "pixel ({x}, {y}) inside={inside} was {word:#010x}"
                );
            }
        }
    }

    #[test]
    fn row_padding_is_respected() {
        // A 4px row in a buffer whose rows are 32 bytes: the last 16 bytes
        // of each row are the framebuffer's own padding and must not be
        // treated as the next row's pixels.
        let layout = layout(4, 32, BGRA8888);
        let mut dst = vec![0u8; 32 * 2];
        let rgba = [0x10u8, 0x20, 0x30, 0xFF].repeat(4 * 2);
        blit_into(&mut dst, &layout, &rgba, 4, PanelRect::new(0, 0, 4, 2));

        for row in 0..2usize {
            for x in 0..4usize {
                let at = row * 32 + x * 4;
                assert_eq!(read_pixel(&dst[at..at + 4]), 0x0010_2030);
            }
            assert!(
                dst[row * 32 + 16..row * 32 + 32].iter().all(|b| *b == 0),
                "row {row} padding was written"
            );
        }
    }

    #[test]
    fn sixteen_bit_writes_two_bytes_per_pixel() {
        let layout = layout(2, 4 * 2, RGB565);
        let mut dst = vec![0u8; 4 * 2 * 2];
        let rgba = [0xFFu8, 0x00, 0x00, 0xFF].repeat(4 * 2);
        blit_into(&mut dst, &layout, &rgba, 4, PanelRect::new(0, 0, 4, 2));
        for pixel in dst.as_chunks::<2>().0 {
            assert_eq!(read_pixel(pixel), 0xF800);
        }
    }

    #[test]
    fn a_rect_past_the_edge_is_clipped_not_faulted() {
        let layout = layout(4, 4 * 4, BGRA8888);
        let mut dst = vec![0u8; 4 * 4 * 4];
        let rgba = [0xFFu8; 4 * 4 * 4];
        // Damage arithmetic and kernel geometry can disagree; running off
        // mapped memory is a fault, not a wrong pixel.
        blit_into(&mut dst, &layout, &rgba, 4, PanelRect::new(2, 2, 999, 999));
        blit_into(&mut dst, &layout, &rgba, 4, PanelRect::new(99, 99, 4, 4));
        blit_into(&mut dst, &layout, &rgba, 4, PanelRect::default());
        assert_ne!(read_pixel(&dst[48 + 8..48 + 12]), 0);
    }

    #[test]
    fn a_shorter_mapping_than_the_geometry_claims_is_survived() {
        // Only two rows are actually mapped; a four-row blit must stop.
        let layout = layout(4, 4 * 4, BGRA8888);
        let mut dst = vec![0u8; 2 * 16];
        let rgba = [0xFFu8; 4 * 4 * 4];
        blit_into(&mut dst, &layout, &rgba, 4, PanelRect::new(0, 0, 4, 4));
        assert!(dst.iter().any(|b| *b != 0), "nothing was written at all");
    }
}
