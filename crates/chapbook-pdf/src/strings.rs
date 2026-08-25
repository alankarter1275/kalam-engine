//! Decoding PDF *text strings*.
//!
//! A PDF string object is a byte string; what those bytes mean depends on
//! where the string sits. In the places a reader cares about — the Info
//! dictionary, outline titles — the spec calls it a text string, and it is
//! either UTF-16BE (with a `FE FF` byte-order mark), UTF-8 (`EF BB BF`,
//! PDF 2.0), or PDFDocEncoding. `String::from_utf8_lossy` gets the pure-ASCII
//! case right and mangles every other one: a UTF-16BE title arrives as text
//! interleaved with NULs, and a Latin-1 author loses their accents to
//! replacement characters.

/// Decode a PDF text string, per PDF 32000-1 §7.9.2.2.
pub(crate) fn text_string(bytes: &[u8]) -> String {
    match bytes {
        [0xFE, 0xFF, rest @ ..] => {
            // UTF-16BE. An odd trailing byte is malformed; drop it rather
            // than lose the whole title.
            char::decode_utf16(
                rest.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| u16::from_be_bytes(*p)),
            )
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => bytes.iter().map(|&b| pdf_doc(b)).collect(),
    }
}

/// PDFDocEncoding is Latin-1 except for the ranges below (PDF 32000-1
/// Annex D.2), which carry the punctuation and ligatures producers actually
/// emit — em dashes, curly quotes, `fi`. Undefined codes map to the
/// replacement character rather than silently becoming a Latin-1 control.
fn pdf_doc(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}',
        '\u{2044}', '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}',
        '\u{201D}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}',
        '\u{0141}', '\u{0152}', '\u{0160}', '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}',
        '\u{0153}', '\u{0161}', '\u{017E}', '\u{FFFD}',
    ];
    const ACCENTS: [char; 8] = [
        '\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}',
        '\u{02DC}',
    ];
    match byte {
        0x18..=0x1F => ACCENTS[byte as usize - 0x18],
        0x7F | 0xAD => char::REPLACEMENT_CHARACTER,
        0x80..=0x9F => HIGH[byte as usize - 0x80],
        0xA0 => '\u{20AC}', // Euro, where Latin-1 has a no-break space.
        _ => byte as char,  // Latin-1 and the ASCII range agree with Unicode.
    }
}

#[cfg(test)]
mod tests {
    use super::text_string;

    #[test]
    fn ascii_passes_through() {
        assert_eq!(text_string(b"Minimal Fixture"), "Minimal Fixture");
    }

    #[test]
    fn utf16be_is_decoded_not_interleaved_with_nuls() {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "Frühling — 春".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(text_string(&bytes), "Frühling — 春");
    }

    #[test]
    fn utf16be_surrogate_pairs_survive() {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "a🎈b".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(text_string(&bytes), "a🎈b");
    }

    #[test]
    fn pdfdocencoding_keeps_latin1_and_the_punctuation_block() {
        // 0xE9 is Latin-1 'é'; 0x84 is an em dash, which Latin-1 does not
        // have and from_utf8_lossy would have turned into U+FFFD.
        assert_eq!(text_string(&[b'C', b'a', b'f', 0xE9]), "Café");
        assert_eq!(text_string(&[b'a', 0x84, b'b']), "a—b");
        // The block is ordered by PostScript glyph name, so the curly
        // double quotes sit at 0x8D/0x8E and the ligatures at 0x93/0x94.
        assert_eq!(text_string(&[0x8D, b'q', 0x8E]), "“q”");
        assert_eq!(text_string(&[0x93, 0x94]), "ﬁﬂ");
    }

    #[test]
    fn a0_is_a_euro_sign_not_a_no_break_space() {
        assert_eq!(text_string(&[0xA0, b'9']), "€9");
    }

    #[test]
    fn an_odd_utf16_tail_loses_the_stray_byte_not_the_string() {
        let bytes = [0xFE, 0xFF, 0x00, b'h', 0x00, b'i', 0x00];
        assert_eq!(text_string(&bytes), "hi");
    }
}
