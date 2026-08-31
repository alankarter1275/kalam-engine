//! Web Annotation Protocol sync for chapbook's highlights, notes and
//! bookmarks.
//!
//! Two halves, and they are useful separately:
//!
//! - [`model`] and [`mapping`] are the **format**: the Web Annotation Data
//!   Model as a reading app needs it, and chapbook's layered locator
//!   serialized into a selector stack. No networking, no library.
//! - [`container`] is the **protocol**: create, update, delete and list
//!   against a Web Annotation Protocol container, over the same injected
//!   `HttpClient` the catalog uses.
//!
//! # Two rules this crate is built around
//!
//! **Never destroy what you did not model.** A container is shared: Thorium,
//! a browser extension and chapbook can all write to one. Every type here
//! keeps unrecognized members and hands them back unchanged, so a chapbook
//! sync round trip is not a data-loss event for whoever else is in there.
//!
//! **Never trust an offset you did not take.** A `TextPositionSelector`
//! means nothing without knowing which extraction produced it, so chapbook
//! writes its `chapbook:locatorVersion` alongside and refuses to read one
//! that does not match. The quote layer is what actually crosses between
//! clients — see [`mapping`]'s docs.

pub mod container;
pub mod mapping;
pub mod model;

pub use container::{AnnotationContainer, ContainerError, StoredAnnotation};
pub use mapping::{from_annotation, to_annotation, Mark};
pub use model::{Annotation, Selector, Target, CFI_CONFORMS_TO, CONTEXT, MEDIA_TYPE};

/// The link relation an annotation service is discovered by, on an OPDS
/// catalog entry.
pub const REL_ANNOTATION_SERVICE: &str = "http://www.w3.org/ns/oa#annotationService";
