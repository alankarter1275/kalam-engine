//! Container-internal href resolution.
//!
//! Hrefs inside content documents ("../images/cover.png") are relative to the
//! document that references them; manifest lookups want a normalized path
//! from the container root. No `url` base is available for `zip:` content, so
//! this is plain segment arithmetic.

/// Resolve `href` relative to the container-root path of the document it
/// appears in (`base`), returning a normalized container-root path.
///
/// `base` is the referencing document itself (e.g. `OEBPS/text/ch1.xhtml`),
/// not its directory. Fragments and query strings are stripped; leading `/`
/// means container-root-absolute. Percent-escapes are left as written —
/// manifest hrefs are compared raw first, with decoding as a lookup fallback.
pub fn resolve_href(base: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or("");

    let mut segments: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        let mut s: Vec<&str> = base.split('/').collect();
        s.pop(); // drop the document filename, keep its directory
        s
    };

    for seg in href.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

/// Decode `%XX` escapes; used as a manifest-lookup fallback when the raw
/// form misses (some packages list hrefs decoded, some encoded).
pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = hex_pair(bytes[i + 1], bytes[i + 2]) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(a: u8, b: u8) -> Option<u8> {
    let hi = (a as char).to_digit(16)?;
    let lo = (b as char).to_digit(16)?;
    Some((hi * 16 + lo) as u8)
}

#[cfg(test)]
mod tests {
    use super::resolve_href;

    #[test]
    fn sibling() {
        assert_eq!(
            resolve_href("OEBPS/ch1.xhtml", "ch2.xhtml"),
            "OEBPS/ch2.xhtml"
        );
    }

    #[test]
    fn parent_dir() {
        assert_eq!(
            resolve_href("OEBPS/text/ch1.xhtml", "../images/a.png"),
            "OEBPS/images/a.png"
        );
    }

    #[test]
    fn fragment_and_query_stripped() {
        assert_eq!(
            resolve_href("OEBPS/ch1.xhtml", "ch2.xhtml#part2"),
            "OEBPS/ch2.xhtml"
        );
        assert_eq!(
            resolve_href("OEBPS/ch1.xhtml", "ch2.xhtml?x=1"),
            "OEBPS/ch2.xhtml"
        );
    }

    #[test]
    fn root_absolute() {
        assert_eq!(
            resolve_href("OEBPS/text/ch1.xhtml", "/styles/a.css"),
            "styles/a.css"
        );
    }

    #[test]
    fn percent_escapes_preserved() {
        assert_eq!(
            resolve_href("OEBPS/ch1.xhtml", "My%20Cover.png"),
            "OEBPS/My%20Cover.png"
        );
    }

    #[test]
    fn dot_segments_at_root() {
        assert_eq!(resolve_href("ch1.xhtml", "./a/./b.css"), "a/b.css");
        assert_eq!(resolve_href("ch1.xhtml", "../../a.css"), "a.css");
    }
}
