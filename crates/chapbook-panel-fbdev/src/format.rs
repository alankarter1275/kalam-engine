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
    /// One *bit* per pixel, eight to a byte — a bare SPI panel, and the
    /// hardware `PixelFormat::Grey { levels: 2 }` exists to feed.
    ///
    /// `one_is_white` comes from the kernel's visual (`FB_VISUAL_MONO10`
    /// says a set bit is white, `FB_VISUAL_MONO01` says it is black),
    /// because getting it backwards produces a perfectly formed negative
    /// image and no error.
    ///
    /// Bit order within the byte is MSB-first — the leftmost pixel in the
    /// high bit. Nothing in `fb_var_screeninfo` or `fb_fix_screeninfo`
    /// reports it; it is the convention every mono fbdev driver in the
    /// kernel follows, and the one `cfb_imageblit` assumes.
    Mono { one_is_white: bool },
}

/// Everything about the destination buffer's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    /// Bits, not bytes, because a mono framebuffer's pixel is smaller than
    /// the unit it is stored in and the byte path cannot describe it.
    pub bits_per_pixel: usize,
    /// Bytes per row, which is not always `width * bits / 8`: framebuffers
    /// pad rows to an alignment of their own choosing.
    pub line_length: usize,
    pub encoding: Encoding,
}

impl Layout {
    /// Whole bytes per pixel, or `None` when a pixel is narrower than one
    /// — which is the question every byte-addressed path here is really
    /// asking before it multiplies.
    pub fn bytes_per_pixel(&self) -> Option<usize> {
        (self.bits_per_pixel >= 8 && self.bits_per_pixel.is_multiple_of(8))
            .then_some(self.bits_per_pixel / 8)
    }
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
        // A pixel is one bit, so the only question is which side of the
        // middle it falls. The pipeline has normally already quantized to
        // two levels — `PixelFormat::Grey { levels: 2 }`, which is what a
        // mono panel asks for — in which case every value is 0 or 255 and
        // the threshold is not doing any rounding of its own.
        Encoding::Mono { one_is_white } => {
            let lit = luminance(r, g, b) >= 128;
            u32::from(lit == one_is_white)
        }
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
    if src_width == 0 {
        return;
    }
    let Some(bpp) = layout.bytes_per_pixel() else {
        if layout.bits_per_pixel == 1 {
            blit_mono(dst, layout, rgba, src_width, rect);
        }
        return;
    };
    if bpp > 4 {
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

/// The same blit, for a framebuffer whose pixel is one bit.
///
/// Two things make this not just the byte path with smaller numbers.
/// Eight pixels share a byte, so a partial byte at either end of a damage
/// rect has to be read, edited and written back or it takes its
/// neighbours with it — and damage rects are in no way byte-aligned,
/// since they come from glyph geometry. And a whole interior byte is
/// worth assembling in one go rather than eight read-modify-writes, which
/// is what the split below is for.
fn blit_mono(dst: &mut [u8], layout: &Layout, rgba: &[u8], src_width: u32, rect: PanelRect) {
    let Encoding::Mono { .. } = layout.encoding else {
        return;
    };
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
        let mut x = x0;
        while x < x1 {
            let byte_at = dst_row + x / 8;
            if byte_at >= dst.len() {
                break;
            }
            let first_bit = x % 8;
            // How much of this byte the rect actually covers.
            let bits = (8 - first_bit).min(x1 - x);
            let mut value = 0u8;
            for i in 0..bits {
                let src = src_row + (x + i) * 4;
                let bit = pack(rgba[src], rgba[src + 1], rgba[src + 2], layout.encoding);
                // MSB-first: the leftmost pixel is bit 7.
                value |= (bit as u8) << (7 - (first_bit + i));
            }
            if bits == 8 {
                dst[byte_at] = value;
            } else {
                // Keep the pixels outside the rect. `mask` is the span
                // this pass owns; everything else in the byte belongs to
                // whatever was there before.
                let mask = (((1u16 << bits) - 1) << (8 - first_bit - bits)) as u8;
                dst[byte_at] = (dst[byte_at] & !mask) | (value & mask);
            }
            x += bits;
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
            bits_per_pixel: bpp * 8,
            line_length,
            encoding,
        }
    }

    /// A 1bpp framebuffer `width` pixels across, rows padded to bytes.
    fn mono_layout(width: usize, one_is_white: bool) -> Layout {
        Layout {
            bits_per_pixel: 1,
            line_length: width.div_ceil(8),
            encoding: Encoding::Mono { one_is_white },
        }
    }

    /// An RGBA buffer from a bit pattern per row, 1 meaning white.
    fn mono_source(rows: &[&[u8]]) -> Vec<u8> {
        let mut rgba = Vec::new();
        for row in rows {
            for bit in *row {
                let v = if *bit == 1 { 0xFF } else { 0x00 };
                rgba.extend_from_slice(&[v, v, v, 0xFF]);
            }
        }
        rgba
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
    fn mono_packs_eight_pixels_to_a_byte_msb_first() {
        let layout = mono_layout(8, true);
        let mut dst = vec![0u8; 1];
        // Leftmost pixel white, the rest black: the high bit and nothing
        // else. Getting this backwards is the classic mono bug, and it
        // looks like a mirrored page rather than like an error.
        let rgba = mono_source(&[&[1, 0, 0, 0, 0, 0, 0, 0]]);
        blit_into(&mut dst, &layout, &rgba, 8, PanelRect::new(0, 0, 8, 1));
        assert_eq!(dst[0], 0b1000_0000);

        let rgba = mono_source(&[&[0, 0, 0, 0, 0, 0, 0, 1]]);
        blit_into(&mut dst, &layout, &rgba, 8, PanelRect::new(0, 0, 8, 1));
        assert_eq!(dst[0], 0b0000_0001);

        let rgba = mono_source(&[&[1, 0, 1, 1, 0, 0, 1, 0]]);
        blit_into(&mut dst, &layout, &rgba, 8, PanelRect::new(0, 0, 8, 1));
        assert_eq!(dst[0], 0b1011_0010);
    }

    #[test]
    fn mono_polarity_comes_from_the_device_not_from_a_guess() {
        let rgba = mono_source(&[&[1, 0, 0, 0, 0, 0, 0, 0]]);
        let mut lit = vec![0u8; 1];
        blit_into(
            &mut lit,
            &mono_layout(8, true),
            &rgba,
            8,
            PanelRect::new(0, 0, 8, 1),
        );
        let mut inverted = vec![0u8; 1];
        blit_into(
            &mut inverted,
            &mono_layout(8, false),
            &rgba,
            8,
            PanelRect::new(0, 0, 8, 1),
        );
        // The same page, on the two wirings, is the exact complement —
        // which is why the polarity has to come from `visual` and not
        // from whichever one somebody tried first.
        assert_eq!(lit[0], !inverted[0]);
        assert_eq!(lit[0], 0b1000_0000);
    }

    #[test]
    fn a_mono_damage_rect_leaves_the_rest_of_the_byte_alone() {
        let layout = mono_layout(8, true);
        // Eight pixels in one byte, all currently white.
        let mut dst = vec![0b1111_1111u8];
        // Repaint only pixels 2..5 — black. Damage rects come from glyph
        // geometry and are in no way byte-aligned, so the surrounding
        // pixels have to survive a read-modify-write.
        let rgba = mono_source(&[&[1, 1, 0, 0, 0, 1, 1, 1]]);
        blit_into(&mut dst, &layout, &rgba, 8, PanelRect::new(2, 0, 3, 1));
        assert_eq!(dst[0], 0b1100_0111);
    }

    #[test]
    fn a_mono_rect_spanning_bytes_writes_both_ends_and_the_middle() {
        // 20 pixels: three bytes a row, the last one only half used.
        let layout = mono_layout(20, true);
        let mut dst = vec![0u8; 3];
        let row: Vec<u8> = (0..20).map(|_| 1u8).collect();
        let rgba = mono_source(&[&row]);
        // Start mid-byte and end mid-byte, crossing a whole one.
        blit_into(&mut dst, &layout, &rgba, 20, PanelRect::new(3, 0, 14, 1));
        assert_eq!(dst[0], 0b0001_1111, "leading partial byte");
        assert_eq!(dst[1], 0b1111_1111, "whole interior byte");
        assert_eq!(dst[2], 0b1000_0000, "trailing partial byte");
    }

    #[test]
    fn mono_rows_follow_the_stride_not_the_width() {
        // 12 pixels wide, so a row is two bytes with four bits of padding.
        let layout = mono_layout(12, true);
        let mut dst = vec![0u8; 2 * 2];
        let rgba = mono_source(&[
            &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0],
        ]);
        blit_into(&mut dst, &layout, &rgba, 12, PanelRect::new(0, 0, 12, 2));
        assert_eq!(dst[0], 0b1000_0000);
        assert_eq!(dst[1], 0b0000_0000);
        assert_eq!(dst[2], 0b0000_0000);
        assert_eq!(dst[3], 0b1000_0000, "second row starts at the stride");
    }

    #[test]
    fn a_mono_blit_past_the_mapping_stops_rather_than_faulting() {
        let layout = mono_layout(16, true);
        // Only one row is mapped; a two-row blit must not run off it.
        let mut dst = vec![0u8; 2];
        let rgba = mono_source(&[&[1; 16], &[1; 16]]);
        blit_into(&mut dst, &layout, &rgba, 16, PanelRect::new(0, 0, 16, 2));
        assert_eq!(dst, vec![0xFF, 0xFF]);
        // And a rect entirely outside is simply nothing.
        let mut dst = vec![0u8; 2];
        blit_into(&mut dst, &layout, &rgba, 16, PanelRect::new(99, 99, 16, 2));
        assert_eq!(dst, vec![0, 0]);
    }

    #[test]
    fn mono_thresholds_at_the_middle_of_the_luminance_range() {
        // Not-yet-quantized colour still lands somewhere sensible: the
        // pipeline normally reduces to two levels first, but the blit is
        // the last line and must not depend on that.
        assert_eq!(pack(0, 0, 0, Encoding::Mono { one_is_white: true }), 0);
        assert_eq!(
            pack(255, 255, 255, Encoding::Mono { one_is_white: true }),
            1
        );
        // Green carries most of the luminance; pure blue is nearly none.
        assert_eq!(pack(0, 255, 0, Encoding::Mono { one_is_white: true }), 1);
        assert_eq!(pack(0, 0, 255, Encoding::Mono { one_is_white: true }), 0);
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
