//! Dictionary hyphenation: soft-hyphen insertion for `hyphens: auto`.
//!
//! cosmic-text has no soft-hyphen awareness of its own, but the two halves
//! it delegates to do the right thing already: `unicode-linebreak` treats
//! U+00AD as a break-after opportunity (UAX #14 class BA), and rustybuzz
//! renders it as a zero-width invisible (a default-ignorable code point).
//! So hyphenation is: insert soft hyphens at Knuth-Liang dictionary points
//! *before* shaping, let the line breaker use them, and have the paginator
//! append a visible hyphen glyph to any line that ends at one.
//!
//! Insertion happens at box-tree time — once per inline formatting context —
//! so every later shaping pass (measurement probes, indent splits, float
//! splits) sees the same text and byte offsets. Inserted soft hyphens carry
//! the locator offset of the character they precede, like generated content:
//! they are presentation, not source text.
//!
//! v1 scope: the embedded en-US dictionary only. Words already containing a
//! soft hyphen are left to the author's own break points (`hyphens: manual`
//! behavior, which also works without this pass).

use std::sync::OnceLock;

use hyphenation::{Hyphenator, Language, Load, Standard};

use crate::boxtree::InlineContent;

/// Words shorter than this are never hyphenated (dictionary minima still
/// apply on top).
const MIN_WORD_CHARS: usize = 5;

fn dictionary() -> Option<&'static Standard> {
    static DICT: OnceLock<Option<Standard>> = OnceLock::new();
    DICT.get_or_init(|| Standard::from_embedded(Language::EnglishUS).ok())
        .as_ref()
}

/// Insert soft hyphens at dictionary break points into every alphabetic
/// word of `inline`.
pub(crate) fn apply(inline: &mut InlineContent) {
    let Some(dict) = dictionary() else {
        return;
    };
    for run in &mut inline.runs {
        if run.text.chars().filter(|c| c.is_alphabetic()).count() < MIN_WORD_CHARS {
            continue;
        }
        hyphenate_run_text(&mut run.text, &mut run.offsets, dict);
    }
}

fn hyphenate_run_text(text: &mut String, offsets: &mut Vec<u32>, dict: &Standard) {
    let mut out = String::with_capacity(text.len() + 8);
    let mut out_offsets = Vec::with_capacity(offsets.len() + 8);
    let mut chars = text.char_indices().peekable();
    let mut char_idx = 0usize;
    // A soft hyphen counts as a word char so an author-hyphenated word is
    // recognized as one word (and then left alone).
    let word_char = |c: char| c.is_alphabetic() || c == '\u{AD}';

    while let Some(&(byte_start, first)) = chars.peek() {
        if !word_char(first) {
            out.push(first);
            out_offsets.push(offsets[char_idx]);
            chars.next();
            char_idx += 1;
            continue;
        }
        // Collect one maximal alphabetic word.
        let word_char_start = char_idx;
        let mut byte_end = byte_start;
        while let Some(&(i, c)) = chars.peek() {
            if word_char(c) {
                byte_end = i + c.len_utf8();
                chars.next();
                char_idx += 1;
            } else {
                break;
            }
        }
        let word = &text[byte_start..byte_end];
        let word_chars = char_idx - word_char_start;
        let breaks = if word_chars >= MIN_WORD_CHARS && !word.contains('\u{AD}') {
            dict.hyphenate(word).breaks
        } else {
            Vec::new()
        };
        // Emit the word, inserting a soft hyphen before the char at each
        // break byte. `breaks` are ascending byte indices into `word`.
        let mut next_break = breaks.iter().copied().peekable();
        for (wi, (wb, wc)) in word.char_indices().enumerate() {
            if next_break.peek() == Some(&wb) && wb > 0 {
                out.push('\u{AD}');
                out_offsets.push(offsets[word_char_start + wi]);
                next_break.next();
            }
            out.push(wc);
            out_offsets.push(offsets[word_char_start + wi]);
        }
    }

    *text = out;
    *offsets = out_offsets;
}

#[cfg(test)]
mod tests {
    #[test]
    fn inserts_soft_hyphens_with_duplicated_offsets() {
        let dict = super::dictionary().expect("embedded en-US dictionary");
        let mut text = "photography session".to_string();
        let mut offsets: Vec<u32> = (0..text.chars().count() as u32).collect();
        super::hyphenate_run_text(&mut text, &mut offsets, dict);
        assert!(text.contains('\u{AD}'), "got: {text:?}");
        assert_eq!(text.chars().count(), offsets.len());
        // Stripping the soft hyphens recovers the original text.
        assert_eq!(text.replace('\u{AD}', ""), "photography session");
        // Offsets stay monotonically non-decreasing.
        assert!(offsets.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn short_and_authored_words_left_alone() {
        let dict = super::dictionary().expect("embedded en-US dictionary");
        let mut text = "tiny au\u{AD}thored".to_string();
        let mut offsets: Vec<u32> = (0..text.chars().count() as u32).collect();
        let before = text.clone();
        super::hyphenate_run_text(&mut text, &mut offsets, dict);
        assert_eq!(text, before);
    }
}
