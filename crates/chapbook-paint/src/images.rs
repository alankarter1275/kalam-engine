//! Decoded raster images, keyed by opaque producer ids (chapbook-layout
//! keys by DOM node tag; a comic producer would key by page index). Shared
//! between layout (intrinsic dimensions) and renderers (pixels).

use std::collections::HashMap;

#[derive(Default)]
pub struct ImageStore {
    images: HashMap<u64, StoredImage>,
}

/// Straight (non-premultiplied) RGBA8; backends premultiply as needed.
pub struct StoredImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ImageStore {
    pub fn insert(&mut self, id: u64, width: u32, height: u32, rgba: Vec<u8>) {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        self.images.insert(
            id,
            StoredImage {
                width,
                height,
                rgba,
            },
        );
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
}
