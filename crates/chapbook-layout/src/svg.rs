//! SVG rasterization into the [`chapbook_paint::ImageStore`].
//!
//! An SVG is decoded like any raster format — at `collect_images` time, into
//! straight RGBA at 1 image px = 1 CSS px of intrinsic size — so layout,
//! paint, and both renderers stay unaware the source was vector. The
//! intrinsic size comes from the SVG's own width/height (falling back to its
//! viewBox), capped so a pathological viewport cannot allocate gigabytes.
//!
//! `<image>` hrefs inside an SVG resolve through the same fetch closure as
//! everything else in the EPUB. usvg wants `Send + Sync` resolver closures
//! and the fetch closure is deliberately neither (it may sit on a `ReadSeek`
//! that is not `Sync`), so resolution is two-pass: a first parse records the
//! hrefs it asked for, the bytes are fetched outside usvg, and a second
//! parse serves them from an owned map. Data URIs resolve in-pass via
//! usvg's default resolver, so the second parse only happens for SVGs that
//! actually reference container files.
//!
//! Text inside SVG shapes with a fontdb copied from the session's own
//! `FontSystem` — never the host's font list — so what a diagram label
//! renders with is decided by the same `FontSource` policy as body text,
//! and goldens mean the same thing on every machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use resvg::usvg;

/// Intrinsic-size cap per axis, CSS px. A viewport beyond this rasterizes
/// scaled down to fit (dims in the store shrink with it: layout treats the
/// cap as the intrinsic size, which only matters for absurd inputs).
const MAX_DIM: u32 = 4096;

/// Cheap content sniff: does this look like an SVG document? Extension is
/// not consulted — a book is its bytes.
pub(crate) fn sniff(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    text.contains("<svg")
}

/// Copy the session's faces into a fontdb for usvg (the fontdb versions
/// differ, so the database cannot be shared). Font files are deduplicated
/// by data pointer — a multi-face file loads once.
pub(crate) fn svg_fontdb(fonts: &cosmic_text::FontSystem) -> Arc<usvg::fontdb::Database> {
    let src = fonts.db();
    let mut db = usvg::fontdb::Database::new();
    let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let ids: Vec<_> = src.faces().map(|f| f.id).collect();
    for id in ids {
        src.with_face_data(id, |data, _| {
            if seen.insert((data.as_ptr() as usize, data.len())) {
                db.load_font_data(data.to_vec());
            }
        });
    }
    db.set_serif_family(
        src.family_name(&cosmic_text::fontdb::Family::Serif)
            .to_string(),
    );
    db.set_sans_serif_family(
        src.family_name(&cosmic_text::fontdb::Family::SansSerif)
            .to_string(),
    );
    db.set_monospace_family(
        src.family_name(&cosmic_text::fontdb::Family::Monospace)
            .to_string(),
    );
    db.set_cursive_family(
        src.family_name(&cosmic_text::fontdb::Family::Cursive)
            .to_string(),
    );
    db.set_fantasy_family(
        src.family_name(&cosmic_text::fontdb::Family::Fantasy)
            .to_string(),
    );
    Arc::new(db)
}

/// Rasterize SVG bytes to `(width, height, straight RGBA8)`, `None` when
/// usvg rejects them. `fetch` resolves an `<image>` href (as written in the
/// SVG) to raw bytes; unresolvable references drop that image only.
pub(crate) fn rasterize(
    bytes: &[u8],
    fetch: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    fontdb: Arc<usvg::fontdb::Database>,
) -> Option<(u32, u32, Vec<u8>)> {
    // Pass one: parse with a recording resolver. If the SVG never asks for
    // an external href, this tree is final.
    let wanted: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let tree = {
        let recorder = usvg::ImageHrefResolver {
            resolve_string: Box::new(|href: &str, _opts: &usvg::Options| {
                wanted.lock().unwrap().push(href.to_string());
                None
            }),
            ..Default::default()
        };
        let options = usvg::Options {
            fontdb: fontdb.clone(),
            image_href_resolver: recorder,
            ..Default::default()
        };
        usvg::Tree::from_data(bytes, &options).ok()?
    };
    let wanted = wanted.into_inner().unwrap();

    let tree = if wanted.is_empty() {
        tree
    } else {
        // Pass two: same parse, hrefs served from prefetched bytes.
        let fetched: HashMap<String, Vec<u8>> = wanted
            .into_iter()
            .filter_map(|href| {
                let data = fetch(&href);
                if data.is_none() {
                    log::warn!("svg <image> href did not resolve: {href}");
                }
                Some((href.clone(), data?))
            })
            .collect();
        let inner_fontdb = fontdb.clone();
        let server = usvg::ImageHrefResolver {
            resolve_string: Box::new(move |href: &str, _opts: &usvg::Options| {
                image_kind(fetched.get(href)?.clone(), inner_fontdb.clone())
            }),
            ..Default::default()
        };
        let options = usvg::Options {
            fontdb,
            image_href_resolver: server,
            ..Default::default()
        };
        usvg::Tree::from_data(bytes, &options).ok()?
    };

    let size = tree.size();
    if !(size.width() > 0.0 && size.height() > 0.0) {
        return None;
    }
    let scale = (MAX_DIM as f32 / size.width())
        .min(MAX_DIM as f32 / size.height())
        .min(1.0);
    let w = (size.width() * scale).ceil().max(1.0) as u32;
    let h = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height()),
        &mut pixmap.as_mut(),
    );
    // The store holds straight (non-premultiplied) RGBA.
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some((w, h, rgba))
}

/// Classify fetched bytes into usvg's image kinds (content-sniffed —
/// extensions lie). Nested SVG parses without further external references.
fn image_kind(data: Vec<u8>, fontdb: Arc<usvg::fontdb::Database>) -> Option<usvg::ImageKind> {
    let arc = Arc::new(data);
    match arc.as_slice() {
        [0x89, b'P', b'N', b'G', ..] => Some(usvg::ImageKind::PNG(arc)),
        [0xFF, 0xD8, ..] => Some(usvg::ImageKind::JPEG(arc)),
        [b'G', b'I', b'F', b'8', ..] => Some(usvg::ImageKind::GIF(arc)),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => {
            Some(usvg::ImageKind::WEBP(arc))
        }
        _ if sniff(&arc) => {
            let options = usvg::Options {
                fontdb,
                ..Default::default()
            };
            usvg::Tree::from_data(&arc, &options)
                .ok()
                .map(usvg::ImageKind::SVG)
        }
        _ => None,
    }
}
