//! Decoded raster images, keyed by opaque producer ids (chapbook-layout
//! keys by DOM node tag; a comic producer would key by page index). Shared
//! between layout (intrinsic dimensions) and renderers (pixels).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Store identity for renderer-side caches (see [`ImageStore::id`]).
static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(1);

pub struct ImageStore {
    id: u64,
    images: HashMap<u64, StoredImage>,
}

impl Default for ImageStore {
    fn default() -> Self {
        ImageStore {
            id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
            images: HashMap::new(),
        }
    }
}

/// Premultiplied RGBA8, converted once at [`ImageStore::insert`] — a frame
/// composites images without touching the pixels again (tiny-skia reads
/// them in place; vello marks the upload premultiplied).
pub struct StoredImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ImageStore {
    /// Store straight (non-premultiplied) RGBA8 — what decoders produce.
    /// Premultiplication happens here, once, instead of per frame in every
    /// backend. Opaque images (comics, PDF pages) pass through unchanged.
    pub fn insert(&mut self, id: u64, width: u32, height: u32, mut rgba: Vec<u8>) {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        for px in rgba.as_chunks_mut::<4>().0 {
            let a = u16::from(px[3]);
            if a < 255 {
                px[0] = (u16::from(px[0]) * a / 255) as u8;
                px[1] = (u16::from(px[1]) * a / 255) as u8;
                px[2] = (u16::from(px[2]) * a / 255) as u8;
            }
        }
        self.images.insert(
            id,
            StoredImage {
                width,
                height,
                rgba,
            },
        );
    }

    /// A process-unique identity, changing with every new store. What a
    /// renderer-side cache (e.g. vello's image blobs) keys on to know its
    /// entries describe *this* store — resource ids alone repeat across
    /// chapters, since they come from per-document arenas.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn get(&self, id: u64) -> Option<&StoredImage> {
        self.images.get(&id)
    }

    /// Intrinsic size in CSS px (1 image px = 1 CSS px).
    pub fn dims(&self, id: u64) -> Option<(u32, u32)> {
        self.images.get(&id).map(|i| (i.width, i.height))
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Bytes of decoded pixels held here.
    ///
    /// Exact, and the term that matters: a 1600x2400 comic page is 15.4 MB
    /// of RGBA whatever the display can show, so this is what a session's
    /// cache budget is mostly spending.
    pub fn bytes(&self) -> usize {
        self.images.values().map(|i| i.rgba.len()).sum()
    }
}
