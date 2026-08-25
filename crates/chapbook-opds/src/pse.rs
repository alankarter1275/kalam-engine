//! OPDS-PSE streamed comics as [`Publication`]s.
//!
//! A PSE stream is a remote image-per-page comic: one HTTP fetch per page
//! through a `{pageNumber}` template (0-based; see docs/OPDS-INTEROP.md
//! §3). This is the producer the `Publication::unit_bytes` blocking
//! contract was written for — a page fetch can take seconds and fail with
//! `ChapbookError::Network`, and pages cache to disk so a re-read is
//! local. UI code must call it off the UI thread.
//!
//! The lazy-PSE pattern is handled at open: when a feed entry carries no
//! stream link, the complete entry behind its `rel="alternate"`
//! entry-document link is fetched and used instead.

use std::io::Write;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use chapbook_core::{BookKind, BookMetadata, Publication, Resource, Result, SpineItem, TocEntry};

use opds_client::{pse_page_url, Entry, Link, OpdsClient, OpdsError};

/// A remote comic backed by an OPDS-PSE stream link.
pub struct StreamedComic {
    client: OpdsClient,
    stream: Link,
    metadata: BookMetadata,
    spine: Vec<SpineItem>,
    toc: Vec<TocEntry>,
    cache_dir: PathBuf,
    /// `pse:lastRead` (1-based) from the stream link, for resume offers.
    last_read: Option<u32>,
}

impl StreamedComic {
    /// Open from a catalog URL: fetch the feed (or standalone entry), pick
    /// the first entry with a stream link — following the lazy-PSE
    /// alternate entry document when needed — and index its pages.
    /// Page bytes cache under `cache_root/<sha1 of stream href>/`.
    pub fn open(
        client: OpdsClient,
        url: &str,
        cache_root: &Path,
    ) -> std::result::Result<StreamedComic, OpdsError> {
        let feed = client.fetch(url)?;
        // Prefer an entry that already carries the stream.
        let direct = feed.entries.iter().find(|e| e.pse_stream().is_some());
        let entry: Entry = match direct {
            Some(entry) => entry.clone(),
            None => {
                // Lazy PSE: follow the first entry's complete-entry link.
                let entry = feed
                    .entries
                    .first()
                    .ok_or_else(|| OpdsError::Parse(format!("no entries in feed at {url}")))?;
                let alternate = entry
                    .links
                    .iter()
                    .find(|l| {
                        l.has_rel("alternate")
                            && l.media_type.as_ref().is_some_and(|t| t.is_entry_document())
                    })
                    .ok_or_else(|| {
                        OpdsError::Parse(format!(
                            "entry {:?} has neither a stream link nor a complete-entry link",
                            entry.title
                        ))
                    })?;
                let complete = client.fetch(&alternate.href)?;
                complete
                    .entries
                    .into_iter()
                    .find(|e| e.pse_stream().is_some())
                    .ok_or_else(|| {
                        OpdsError::Parse("complete entry carries no PSE stream link".into())
                    })?
            }
        };
        Self::from_entry(client, &entry, cache_root)
    }

    /// Build from an entry already known to carry a PSE stream link.
    pub fn from_entry(
        client: OpdsClient,
        entry: &Entry,
        cache_root: &Path,
    ) -> std::result::Result<StreamedComic, OpdsError> {
        let stream = entry
            .pse_stream()
            .ok_or_else(|| OpdsError::Parse("entry has no PSE stream link".into()))?
            .clone();
        // pse:count is required by convention: render nothing without it.
        let count = stream
            .pse_count
            .ok_or_else(|| OpdsError::Parse("PSE stream link lacks pse:count".into()))?;
        if count == 0 {
            return Err(OpdsError::Parse("PSE stream declares zero pages".into()));
        }
        let advisory_type = stream
            .media_type
            .as_ref()
            .map(|t| t.essence.clone())
            .unwrap_or_else(|| "image/jpeg".to_string());
        let spine: Vec<SpineItem> = (0..count)
            .map(|i| SpineItem {
                id: format!("page{i}"),
                href: pse_page_url(&stream, i, None),
                media_type: advisory_type.clone(),
                linear: true,
            })
            .collect();

        let cache_dir = cache_root.join(format!("{:x}", Sha1::digest(stream.href.as_bytes())));
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| OpdsError::Network(format!("cache dir: {e}")))?;

        Ok(StreamedComic {
            metadata: BookMetadata {
                title: Some(entry.title.clone()),
                authors: entry.authors.clone(),
                language: entry.language.clone(),
                identifier: entry.identifier.clone().or_else(|| Some(entry.id.clone())),
                description: entry.summary.clone(),
                format_version: "opds-pse".into(),
            },
            last_read: stream.pse_last_read,
            client,
            stream,
            spine,
            toc: Vec::new(),
            cache_dir,
        })
    }

    /// The 0-based page to offer resuming at, when the server reported
    /// reading progress (`pse:lastRead` is 1-based).
    pub fn resume_page(&self) -> Option<usize> {
        let last = self.last_read?;
        (last >= 1).then(|| (last as usize - 1).min(self.spine.len() - 1))
    }

    fn cache_path(&self, page: usize) -> PathBuf {
        self.cache_dir.join(format!("page{page}"))
    }
}

impl Publication for StreamedComic {
    fn kind(&self) -> BookKind {
        BookKind::Comic
    }

    fn metadata(&self) -> &BookMetadata {
        &self.metadata
    }

    fn spine(&self) -> &[SpineItem] {
        &self.spine
    }

    fn toc(&self) -> &[TocEntry] {
        &self.toc
    }

    /// Blocking fetch + cache, exactly per the trait contract: disk cache
    /// hit is local; a miss is one HTTP request (temp file + atomic rename,
    /// so concurrent prefetchers race benignly).
    fn unit_bytes(&self, spine_index: usize) -> Result<Vec<u8>> {
        self.spine_item(spine_index)?;
        let cached = self.cache_path(spine_index);
        if let Ok(data) = std::fs::read(&cached) {
            return Ok(data);
        }
        let (data, _content_type) = self
            .client
            .fetch_pse_page(&self.stream, spine_index as u32, None)
            .map_err(crate::to_chapbook_error)?;
        let tmp = cached.with_extension("part");
        let write = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&data)?;
            std::fs::rename(&tmp, &cached)
        })();
        if let Err(e) = write {
            // A failed cache write is not a failed read.
            let _ = std::fs::remove_file(&tmp);
            let _ = e;
        }
        Ok(data)
    }

    fn cover(&self) -> Result<Option<Resource>> {
        Ok(Some(Resource {
            media_type: self.spine[0].media_type.clone(),
            data: self.unit_bytes(0)?,
        }))
    }
}
