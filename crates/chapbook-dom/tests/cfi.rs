//! CFI ↔ locator-offset conversion: generation shape, spec-style
//! resolution, ID-assertion correction, and full round-trips.

use chapbook_core::Cfi;
use chapbook_dom::{cfi_for_offset, locator_text, offset_for_cfi, parse_xhtml};

fn doc(html: &str) -> chapbook_dom::Document {
    parse_xhtml(html.as_bytes(), "test.xhtml").unwrap()
}

/// Five paragraphs, no inter-element whitespace: predictable indices.
const FIVE_PARAS: &str = "<html><head><title>t</title></head><body id=\"body01\">\
<p id=\"para01\">one</p><p id=\"para02\">two</p><p id=\"para03\">three</p>\
<p id=\"para04\">four</p><p id=\"para05\">xxx yyy zzz</p></body></html>";

#[test]
fn generates_canonical_steps_with_id_assertions() {
    let d = doc(FIVE_PARAS);
    let text = locator_text(&d);
    // Offset 10 into para05's text ("zz|z" boundary region).
    let base = text.find("xxx yyy zzz").unwrap() as u32;
    let cfi = cfi_for_offset(&d, 3, base + 10).unwrap();
    // body is the second element child of html (head first) -> /4; para05
    // the fifth element of body -> /10; its text is span /1.
    assert_eq!(cfi.to_string(), "epubcfi(/6/8!/4[body01]/10[para05]/1:10)");
    assert_eq!(cfi.spine_index(), Some(3));
}

#[test]
fn resolves_spec_style_cfi() {
    let d = doc(FIVE_PARAS);
    let text = locator_text(&d);
    let cfi = Cfi::parse("epubcfi(/6/8!/4[body01]/10[para05]/1:4)").unwrap();
    let offset = offset_for_cfi(&d, &cfi).unwrap();
    let expected = text.find("yyy").unwrap() as u32;
    assert_eq!(offset, expected);
}

#[test]
fn id_assertion_corrects_stale_index() {
    let d = doc(FIVE_PARAS);
    let text = locator_text(&d);
    // Index says the FIRST paragraph (/2) but the assertion names para03:
    // the assertion wins, per spec.
    let cfi = Cfi::parse("epubcfi(/6/2!/4[body01]/2[para03]/1:0)").unwrap();
    let offset = offset_for_cfi(&d, &cfi).unwrap();
    assert_eq!(offset, text.find("three").unwrap() as u32);
}

#[test]
fn element_terminus_resolves_to_content_start() {
    let d = doc(FIVE_PARAS);
    let text = locator_text(&d);
    let cfi = Cfi::parse("epubcfi(/6/2!/4/6[para03])").unwrap();
    assert_eq!(
        offset_for_cfi(&d, &cfi),
        Some(text.find("three").unwrap() as u32)
    );
}

#[test]
fn round_trips_every_offset_of_a_gnarly_document() {
    // Inline nesting, br, a comment mid-span, an excluded script, and
    // inter-element whitespace text nodes.
    let html = "<html><head><script>ignored()</script></head><body>\n\
        <h1>Title Here</h1>\n\
        <p>Plain <em>emphatic <strong>very</strong></em> text<br/>after break.</p>\n\
        <p>Second <!-- note --> paragraph with <a href=\"#\">a link</a> inside.</p>\n\
        </body></html>";
    let d = doc(html);
    let total = locator_text(&d).chars().count() as u32;
    assert!(total > 40);
    for offset in 0..=total {
        let cfi = cfi_for_offset(&d, 0, offset).unwrap();
        let back = offset_for_cfi(&d, &cfi)
            .unwrap_or_else(|| panic!("offset {offset} produced unresolvable CFI {cfi}"));
        assert_eq!(back, offset, "round trip failed at {offset} via {cfi}");
        // And the string form survives reparsing.
        let reparsed = Cfi::parse(&cfi.to_string()).unwrap();
        assert_eq!(offset_for_cfi(&d, &reparsed), Some(offset));
    }
}

#[test]
fn out_of_range_offsets_clamp_to_document_end() {
    let d = doc(FIVE_PARAS);
    let total = locator_text(&d).chars().count() as u32;
    let cfi = cfi_for_offset(&d, 0, total + 1000).unwrap();
    assert_eq!(offset_for_cfi(&d, &cfi), Some(total));
}
