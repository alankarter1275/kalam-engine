//! Non-strict URL resolution.
//!
//! Every href in a feed resolves against the request URL, but hrefs are
//! never strict-URI-parsed: PSE stream templates contain literal
//! `{pageNumber}`/`{maxWidth}` braces that `url::Url` would percent-encode
//! into garbage. This is string-level RFC 3986 merging that treats the href
//! as opaque text.

/// Resolve `href` against the absolute `base` request URL.
pub fn resolve_url(base: &str, href: &str) -> String {
    if href.is_empty() {
        return base.to_string();
    }
    if has_scheme(href) {
        return href.to_string();
    }
    let (scheme, rest) = match base.split_once("://") {
        Some((s, r)) => (s, r),
        None => return href.to_string(), // relative base: give href back untouched
    };
    let (authority, base_path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    // Strip query/fragment from the base path before merging.
    let base_path = base_path
        .split(['?', '#'])
        .next()
        .unwrap_or("/")
        .to_string();

    if let Some(net_path) = href.strip_prefix("//") {
        return format!("{scheme}://{net_path}");
    }
    if href.starts_with('/') {
        return format!("{scheme}://{authority}{}", normalize_path_opaque(href));
    }

    // Relative reference: merge with the base path's directory. Keep the
    // href's query/fragment attached to its final segment.
    let (href_path, href_tail) = split_tail(href);
    let dir = match base_path.rfind('/') {
        Some(idx) => &base_path[..=idx],
        None => "/",
    };
    let merged = format!("{dir}{href_path}");
    format!(
        "{scheme}://{authority}{}{href_tail}",
        normalize_path_opaque(&merged)
    )
}

fn has_scheme(href: &str) -> bool {
    let Some(colon) = href.find(':') else {
        return false;
    };
    let scheme = &href[..colon];
    let mut chars = scheme.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        // A '/' before the ':' means the colon was inside a path.
        && !scheme.contains('/')
}

/// Split an href into (path, "?query#fragment" tail).
fn split_tail(href: &str) -> (&str, &str) {
    match href.find(['?', '#']) {
        Some(idx) => (&href[..idx], &href[idx..]),
        None => (href, ""),
    }
}

/// Remove `.`/`..` segments; every other segment is opaque bytes (braces
/// and all).
fn normalize_path_opaque(path: &str) -> String {
    let (path, tail) = split_tail(path);
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut joined = out.join("/");
    if !joined.starts_with('/') {
        joined.insert(0, '/');
    }
    if path.ends_with('/') && !joined.ends_with('/') {
        joined.push('/');
    }
    format!("{joined}{tail}")
}

#[cfg(test)]
mod tests {
    use super::resolve_url;

    const BASE: &str = "https://cat.example.com/opds/feed/new?page=2";

    #[test]
    fn absolute_href_passes_through() {
        assert_eq!(
            resolve_url(BASE, "https://cdn.example.net/c.jpg"),
            "https://cdn.example.net/c.jpg"
        );
        assert_eq!(
            resolve_url(BASE, "mailto:admin@example.com"),
            "mailto:admin@example.com"
        );
    }

    #[test]
    fn root_relative() {
        assert_eq!(
            resolve_url(BASE, "/covers/gopl.jpg"),
            "https://cat.example.com/covers/gopl.jpg"
        );
    }

    #[test]
    fn document_relative_with_query() {
        assert_eq!(
            resolve_url(BASE, "next?page=3"),
            "https://cat.example.com/opds/feed/next?page=3"
        );
    }

    #[test]
    fn dot_segments() {
        assert_eq!(
            resolve_url(BASE, "../opensearch.xml"),
            "https://cat.example.com/opds/opensearch.xml"
        );
    }

    #[test]
    fn protocol_relative() {
        assert_eq!(
            resolve_url(BASE, "//other.example.com/x"),
            "https://other.example.com/x"
        );
    }

    #[test]
    fn pse_template_braces_survive() {
        let got = resolve_url(
            BASE,
            "/opds/page/chapter/demo/c12?page={pageNumber}&width={maxWidth}",
        );
        assert_eq!(
            got,
            "https://cat.example.com/opds/page/chapter/demo/c12?page={pageNumber}&width={maxWidth}"
        );
    }

    #[test]
    fn opaque_ids_with_slashes_and_colons_in_paths() {
        // Comic-server chapter paths — never normalized away.
        assert_eq!(
            resolve_url(BASE, "/dl/series%2Fvol.1/c12.cbz"),
            "https://cat.example.com/dl/series%2Fvol.1/c12.cbz"
        );
    }
}
