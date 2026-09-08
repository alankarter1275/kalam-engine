//! `ComicInfo.xml` — the metadata sidecar comic archives actually carry.
//!
//! The format came out of ComicRack and is now specified by the Anansi
//! Project; every tagger in the ecosystem writes it and it is the only
//! manifest a CBZ ever has. It is a flat document of text elements plus a
//! `<Pages>` list, and nothing in it is required, so every field here is
//! optional and a malformed file degrades to the filename-stem metadata a
//! CBZ had before.
//!
//! `<Page Bookmark="...">` is the one place a comic names its own
//! divisions: chapter breaks in a collected volume, stories in an anthology.
//! `Image` indexes the archive's page images in reading order, which is
//! exactly what a spine index is.

use quick_xml::events::{BytesRef, Event};
use quick_xml::Reader;

/// The fields of a `ComicInfo.xml` a reader has any use for.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ComicInfo {
    pub title: Option<String>,
    pub series: Option<String>,
    pub number: Option<String>,
    pub summary: Option<String>,
    pub language: Option<String>,
    /// Creator credits in the order the roles are listed below, deduped.
    pub credits: Vec<String>,
    /// `(image index, label)` for each page carrying a `Bookmark`.
    pub bookmarks: Vec<(usize, String)>,
}

/// Credit roles, most responsible first. Each may hold a comma-separated
/// list — the schema says so, and taggers use it.
const CREDIT_ROLES: [&str; 6] = [
    "Writer",
    "Penciller",
    "Inker",
    "Colorist",
    "Letterer",
    "CoverArtist",
];

impl ComicInfo {
    /// A single display name, since `BookMetadata` has one title slot and a
    /// comic's identity is usually split across `Series` and `Number`. Most
    /// tagged issues carry a series and a number and no title at all.
    pub(crate) fn display_title(&self) -> Option<String> {
        match (&self.series, &self.number, &self.title) {
            (Some(s), Some(n), Some(t)) => Some(format!("{s} #{n}: {t}")),
            (Some(s), Some(n), None) => Some(format!("{s} #{n}")),
            (Some(s), None, Some(t)) => Some(format!("{s}: {t}")),
            (Some(s), None, None) => Some(s.clone()),
            (None, _, title) => title.clone(),
        }
    }
}

/// Parse a `ComicInfo.xml`. Returns `None` if the bytes are not one, or are
/// too broken to read — the caller falls back to what it had.
pub(crate) fn parse(bytes: &[u8]) -> Option<ComicInfo> {
    // No `trim_text`: an entity splits the text around it into separate
    // events, and trimming each of those eats the spaces on either side of
    // an `&amp;`. Element values get trimmed once, whole, below.
    let mut reader = Reader::from_reader(bytes);

    let mut info = ComicInfo::default();
    let mut credits: Vec<(usize, String)> = Vec::new();
    let mut saw_root = false;
    let mut element = String::new();
    let mut text = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Err(_) => return None,
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.local_name().as_ref().to_string();
                if name == "ComicInfo" {
                    saw_root = true;
                } else if name == "Page" {
                    // `Bookmark` without `Image` has nothing to point at.
                    let mut image = None;
                    let mut label = None;
                    for attr in e.attributes().flatten() {
                        let Ok(value) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        else {
                            continue;
                        };
                        match attr.key.local_name().as_ref() {
                            "Image" => image = value.trim().parse::<usize>().ok(),
                            "Bookmark" => {
                                label = Some(value.trim().to_string()).filter(|s| !s.is_empty())
                            }
                            _ => {}
                        }
                    }
                    if let (Some(image), Some(label)) = (image, label) {
                        info.bookmarks.push((image, label));
                    }
                }
                element = name;
                text.clear();
            }
            Ok(Event::Text(e)) => text.push_str(&e.xml_content(quick_xml::XmlVersion::Implicit1_0)),
            Ok(Event::CData(e)) => text.push_str(&e.into_inner()),
            Ok(Event::GeneralRef(e)) => push_entity(&mut text, e),
            Ok(Event::End(_)) => {
                let value = text.trim();
                if !value.is_empty() {
                    let value = value.to_string();
                    match element.as_str() {
                        "Title" => info.title = Some(value),
                        "Series" => info.series = Some(value),
                        "Number" => info.number = Some(value),
                        "Summary" => info.summary = Some(value),
                        "LanguageISO" => info.language = Some(value),
                        role => {
                            if let Some(rank) = CREDIT_ROLES.iter().position(|r| *r == role) {
                                for name in value.split(',') {
                                    let name = name.trim();
                                    if !name.is_empty() {
                                        credits.push((rank, name.to_string()));
                                    }
                                }
                            }
                        }
                    }
                }
                element.clear();
                text.clear();
            }
            Ok(_) => {}
        }
        buf.clear();
    }

    if !saw_root {
        return None;
    }
    // Stable sort keeps within-role order while ranking Writer above art.
    credits.sort_by_key(|(rank, _)| *rank);
    for (_, name) in credits {
        if !info.credits.contains(&name) {
            info.credits.push(name);
        }
    }
    Some(info)
}

/// quick-xml reports `&amp;` and friends as an event of their own and
/// splits the surrounding text around them, so a parser that ignores that
/// event drops every entity it meets: `Adventures &amp; Escapes` would come
/// back as `Adventures  Escapes`.
fn push_entity(out: &mut String, entity: BytesRef) {
    if let Ok(Some(c)) = entity.resolve_char_ref() {
        out.push(c);
        return;
    }
    // The five predefined entities. Anything else needs a DTD to define it,
    // and ComicInfo.xml does not carry one.
    out.push_str(match entity.into_inner().as_ref() {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    });
}

#[cfg(test)]
mod tests {
    use super::{parse, ComicInfo};

    const FULL: &[u8] = br#"<?xml version="1.0"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <Series>Adventures &amp; Escapes</Series>
  <Number>7</Number>
  <Summary>A summary.</Summary>
  <Writer>Ada Lovelace, Grace Hopper</Writer>
  <Penciller>Ada Lovelace</Penciller>
  <Colorist>Jean Bartik</Colorist>
  <LanguageISO>en</LanguageISO>
  <PageCount>3</PageCount>
  <Pages>
    <Page Image="0" Type="FrontCover" />
    <Page Image="1" Bookmark="Chapter One" />
    <Page Image="2" Bookmark="Chapter Two" Type="Story" />
  </Pages>
</ComicInfo>"#;

    #[test]
    fn reads_the_fields_a_reader_uses() {
        let info = parse(FULL).unwrap();
        assert_eq!(info.series.as_deref(), Some("Adventures & Escapes"));
        assert_eq!(info.number.as_deref(), Some("7"));
        assert_eq!(info.summary.as_deref(), Some("A summary."));
        assert_eq!(info.language.as_deref(), Some("en"));
        assert_eq!(info.display_title().unwrap(), "Adventures & Escapes #7");
    }

    #[test]
    fn entities_survive_the_text_they_sit_in() {
        let info =
            parse(br#"<ComicInfo><Title>A &amp; B &#38; C &lt;3</Title></ComicInfo>"#).unwrap();
        assert_eq!(info.title.as_deref(), Some("A & B & C <3"));
    }

    #[test]
    fn credits_rank_by_role_and_dedupe() {
        let info = parse(FULL).unwrap();
        // Writer before art; Ada appears twice in the file and once here.
        assert_eq!(
            info.credits,
            ["Ada Lovelace", "Grace Hopper", "Jean Bartik"]
        );
    }

    #[test]
    fn only_pages_with_a_bookmark_become_entries() {
        let info = parse(FULL).unwrap();
        assert_eq!(
            info.bookmarks,
            [
                (1, "Chapter One".to_string()),
                (2, "Chapter Two".to_string())
            ]
        );
    }

    #[test]
    fn title_composition_covers_the_shapes_taggers_write() {
        let with = |xml: &str| parse(format!("<ComicInfo>{xml}</ComicInfo>").as_bytes()).unwrap();
        assert_eq!(
            with("<Series>S</Series><Number>2</Number><Title>T</Title>")
                .display_title()
                .unwrap(),
            "S #2: T"
        );
        assert_eq!(with("<Series>S</Series>").display_title().unwrap(), "S");
        assert_eq!(with("<Title>T</Title>").display_title().unwrap(), "T");
        assert_eq!(with("<Notes>n</Notes>").display_title(), None);
    }

    #[test]
    fn a_document_that_is_not_comicinfo_is_declined() {
        assert_eq!(parse(b"<container><rootfiles/></container>"), None);
        assert_eq!(parse(b"not xml at all"), None);
    }

    #[test]
    fn an_empty_comicinfo_parses_to_nothing_in_particular() {
        assert_eq!(parse(b"<ComicInfo/>"), Some(ComicInfo::default()));
    }
}
