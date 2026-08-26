//! What a book arrives as, said out loud.
//!
//! `open(source: &str)` sniffs a string for `http://` and then for a file
//! extension. That works on a desktop and on nothing else: Android hands an
//! app a `content://` URI resolved to a file descriptor, iOS hands it a
//! security-scoped bookmark, and a WASM build has no filesystem at all.
//! None of the three has a path, and two of the three have no extension
//! either — so the extension test is not merely unavailable, it is
//! answering a question the caller cannot ask.
//!
//! [`Source`] is that question asked properly, and [`Format::sniff`] is the
//! answer for the cases where nobody knows: the EPUB `mimetype` entry and
//! the `%PDF` header are in the bytes, and reading them is *better* than
//! trusting an extension even when there is one. A hostile or merely
//! misnamed `.epub` is a parse failure today; it opens correctly here.

use std::path::{Path, PathBuf};

/// A seekable byte source: what a file descriptor becomes.
///
/// `Read + Seek` is what the zip and PDF readers already need — neither can
/// work from a stream, because a zip's central directory is at the end.
///
/// `Send` because the loader thread owns the book. **Not `Sync`**, which is
/// the friendlier bound and deliberately weaker than what `rbook` asks for
/// under its `threadsafe` feature: chapbook wraps the reader to satisfy
/// that rather than pushing the requirement onto every host, since a
/// reader shimming a foreign runtime is far more likely to manage `Send`
/// than `Sync`.
pub trait ReadSeek: std::io::Read + std::io::Seek + Send {}

impl<T: std::io::Read + std::io::Seek + Send> ReadSeek for T {}

/// Which reader opens the bytes.
///
/// [`Guess`](Self::Guess) is the honest default for everything that is not
/// a path, and is worth preferring even for paths — see [`Self::sniff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// Decide from the bytes.
    #[default]
    Guess,
    Epub,
    Cbz,
    Pdf,
}

/// Bytes enough to identify a book container: a zip local header (30) plus
/// the `mimetype` name (8) plus the media type EPUB requires there (20).
///
/// Named for [`Format`] because `chapbook-cbz` has its own, smaller sniff
/// window for the *images inside* an archive, and the two are unrelated.
pub const FORMAT_SNIFF_BYTES: usize = 64;

impl Format {
    /// Identify a book from its leading bytes. `None` means the bytes match
    /// nothing chapbook reads.
    ///
    /// Give it at least [`FORMAT_SNIFF_BYTES`]; less still works, it just cannot
    /// tell an EPUB from a CBZ and will answer `Cbz` for both, since both
    /// are zips and only the EPUB says so inside itself.
    ///
    /// **EPUB before CBZ is the whole subtlety.** OCF requires that the
    /// first zip entry be an uncompressed file named `mimetype` whose
    /// contents are exactly `application/epub+zip`, which puts a known
    /// string at a known offset — byte 30 for the name, byte 38 for the
    /// value, when the entry has no extra field. That is the only thing
    /// distinguishing the two formats from the outside, and a zip lacking
    /// it is treated as a comic archive: a bag of images is what a CBZ is,
    /// and there is nothing else it could be.
    pub fn sniff(head: &[u8]) -> Option<Format> {
        if head.starts_with(b"%PDF") {
            return Some(Format::Pdf);
        }
        // Local file header. The empty-archive and spanned-archive magics
        // are deliberately not accepted: neither holds a book.
        if !head.starts_with(b"PK\x03\x04") {
            return None;
        }
        // Filename length and extra-field length are little-endian u16 at
        // offsets 26 and 28. A conforming OCF archive has no extra field
        // here, but reading the lengths rather than assuming them costs
        // two bytes of arithmetic and survives a zipper that adds one.
        let name_len = u16::from_le_bytes([*head.get(26)?, *head.get(27)?]) as usize;
        let extra_len = u16::from_le_bytes([*head.get(28)?, *head.get(29)?]) as usize;
        let name = head.get(30..30 + name_len)?;
        if name == b"mimetype" {
            let value_at = 30 + name_len + extra_len;
            let value = head.get(value_at..value_at + 20)?;
            if value == b"application/epub+zip" {
                return Some(Format::Epub);
            }
        }
        Some(Format::Cbz)
    }

    /// The format an extension claims, for the one case where that is all
    /// there is. Only ever a fallback: bytes outrank names.
    pub fn from_extension(path: &Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "epub" => Some(Format::Epub),
            "cbz" => Some(Format::Cbz),
            "pdf" => Some(Format::Pdf),
            _ => None,
        }
    }

    /// The name used in errors and in `FormatNotBuilt`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Guess => "unknown",
            Format::Epub => "EPUB",
            Format::Cbz => "CBZ",
            Format::Pdf => "PDF",
        }
    }
}

/// Where a book's bytes come from.
///
/// Only [`Path`](Self::Path) participates in the library — fingerprinting,
/// position restore, import — because the other two have no stable identity
/// to key on and no file to record. That is a real gap for a `content://`
/// URI, and closing it is the custody question in `docs/PLATFORM.md`
/// (persisting a bookmark, not a copy), not something a source type can
/// answer on its own.
pub enum Source {
    /// A local file. The desktop case, and the only one the library knows
    /// how to remember.
    Path(PathBuf),
    /// Bytes already in memory — a WASM `ArrayBuffer`, a download the host
    /// performed itself.
    Bytes { format: Format, bytes: Vec<u8> },
    /// An open, seekable handle: an Android `ParcelFileDescriptor`, an iOS
    /// security-scoped file, anything a host can hand over without a path.
    Reader {
        format: Format,
        reader: Box<dyn ReadSeek>,
    },
    /// An OPDS catalog URL, resolved to a PSE page stream.
    Url(String),
}

impl Source {
    /// Bytes with the format left to the sniffer.
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Source {
        Source::Bytes {
            format: Format::Guess,
            bytes: bytes.into(),
        }
    }

    /// A handle with the format left to the sniffer.
    pub fn reader(reader: impl ReadSeek + 'static) -> Source {
        Source::Reader {
            format: Format::Guess,
            reader: Box::new(reader),
        }
    }

    /// Pin the format, for a host that already knows — a `content://` URI
    /// whose MIME type the resolver reported, say. Ignored by
    /// [`Source::Path`] and [`Source::Url`], which have their own answers.
    pub fn with_format(self, format: Format) -> Source {
        match self {
            Source::Bytes { bytes, .. } => Source::Bytes { format, bytes },
            Source::Reader { reader, .. } => Source::Reader { format, reader },
            other => other,
        }
    }

    /// The file this came from, when there is one. `None` is what makes a
    /// source invisible to the library.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Source::Path(path) => Some(path),
            _ => None,
        }
    }
}

/// The string form every existing caller passes, preserved exactly: an
/// `http(s)://` prefix means a catalog, anything else is a path.
///
/// Kept because a command line really does hand over a string, and because
/// it is the one context where the old sniffing is right rather than merely
/// habitual.
impl From<&str> for Source {
    fn from(source: &str) -> Source {
        if source.starts_with("http://") || source.starts_with("https://") {
            Source::Url(source.to_string())
        } else {
            Source::Path(PathBuf::from(source))
        }
    }
}

impl From<&String> for Source {
    fn from(source: &String) -> Source {
        Source::from(source.as_str())
    }
}

impl From<String> for Source {
    fn from(source: String) -> Source {
        Source::from(source.as_str())
    }
}

impl From<&Path> for Source {
    fn from(path: &Path) -> Source {
        Source::Path(path.to_path_buf())
    }
}

impl From<PathBuf> for Source {
    fn from(path: PathBuf) -> Source {
        Source::Path(path)
    }
}

/// Redacted where it needs to be: a URL may carry a per-user API key in its
/// path, and the bytes of a book are not worth printing.
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Path(path) => f.debug_tuple("Path").field(path).finish(),
            Source::Bytes { format, bytes } => f
                .debug_struct("Bytes")
                .field("format", format)
                .field("len", &bytes.len())
                .finish(),
            Source::Reader { format, .. } => {
                f.debug_struct("Reader").field("format", format).finish()
            }
            Source::Url(_) => f.debug_tuple("Url").field(&"<redacted>").finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip local header for one entry: name, no extra field, stored.
    fn zip_head(name: &[u8], contents: &[u8]) -> Vec<u8> {
        let mut out = Vec::from(b"PK\x03\x04".as_slice());
        out.extend_from_slice(&[0; 22]); // version..uncompressed size
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(name);
        out.extend_from_slice(contents);
        out
    }

    #[test]
    fn an_epub_says_so_in_its_first_entry() {
        let head = zip_head(b"mimetype", b"application/epub+zip");
        assert_eq!(Format::sniff(&head), Some(Format::Epub));
    }

    #[test]
    fn a_zip_that_is_not_an_epub_is_a_comic() {
        let head = zip_head(b"001.jpg", b"\xff\xd8\xff\xe0____________");
        assert_eq!(Format::sniff(&head), Some(Format::Cbz));
    }

    #[test]
    fn a_mimetype_entry_naming_something_else_is_not_an_epub() {
        let head = zip_head(b"mimetype", b"application/zip______");
        assert_eq!(Format::sniff(&head), Some(Format::Cbz));
    }

    #[test]
    fn an_extra_field_does_not_hide_the_media_type() {
        // Same archive, but the zipper inserted a 4-byte extra field.
        let mut head = Vec::from(b"PK\x03\x04".as_slice());
        head.extend_from_slice(&[0; 22]);
        head.extend_from_slice(&8u16.to_le_bytes());
        head.extend_from_slice(&4u16.to_le_bytes());
        head.extend_from_slice(b"mimetype");
        head.extend_from_slice(b"\x00\x00\x00\x00");
        head.extend_from_slice(b"application/epub+zip");
        assert_eq!(Format::sniff(&head), Some(Format::Epub));
    }

    #[test]
    fn a_pdf_is_its_header() {
        assert_eq!(Format::sniff(b"%PDF-1.7\n%..."), Some(Format::Pdf));
    }

    #[test]
    fn nothing_recognisable_is_none() {
        assert_eq!(Format::sniff(b"<html><body>no"), None);
        assert_eq!(Format::sniff(b""), None);
        // A zip end-of-central-directory is a zip, but not a book: it is
        // what an *empty* archive starts with.
        assert_eq!(Format::sniff(b"PK\x05\x06"), None);
    }

    #[test]
    fn a_truncated_zip_header_does_not_panic() {
        let full = zip_head(b"mimetype", b"application/epub+zip");
        for cut in 0..full.len() {
            let _ = Format::sniff(&full[..cut]);
        }
    }

    #[test]
    fn strings_still_split_into_urls_and_paths() {
        assert!(matches!(
            Source::from("https://cat.example.com/opds/"),
            Source::Url(_)
        ));
        assert!(matches!(Source::from("/books/a.epub"), Source::Path(_)));
        // Not a URL scheme chapbook fetches, so it is a (strange) path.
        assert!(matches!(Source::from("ftp://host/a.epub"), Source::Path(_)));
    }

    #[test]
    fn extensions_are_a_fallback_and_case_insensitive() {
        assert_eq!(
            Format::from_extension(Path::new("/b/A.EPUB")),
            Some(Format::Epub)
        );
        assert_eq!(
            Format::from_extension(Path::new("/b/a.cbz")),
            Some(Format::Cbz)
        );
        assert_eq!(Format::from_extension(Path::new("/b/a.txt")), None);
        assert_eq!(Format::from_extension(Path::new("/b/noext")), None);
    }

    #[test]
    fn debug_prints_no_secret_and_no_payload() {
        let url = Source::Url("https://cat.example.com/opds/abc123secret/".into());
        let shown = format!("{url:?}");
        assert!(!shown.contains("abc123secret"), "{shown}");

        let bytes = Source::bytes(vec![0u8; 4096]);
        let shown = format!("{bytes:?}");
        assert!(shown.contains("4096"), "the length is useful: {shown}");
    }

    #[test]
    fn a_cursor_is_a_reader() {
        // The bound has to admit the obvious in-memory case, or every test
        // and every WASM host needs a newtype.
        let source = Source::reader(std::io::Cursor::new(vec![1u8, 2, 3]));
        assert!(matches!(
            source,
            Source::Reader {
                format: Format::Guess,
                ..
            }
        ));
    }
}
