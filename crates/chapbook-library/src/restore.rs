//! The cross-file/edition position restore chain (docs/LOCATORS.md §3) —
//! the orchestration layer above `chapbook_core::resolve_in_text`.
//!
//! Degrades to "right page-ish", never "gone": exact offset → quote re-find
//! in the stored chapter → quote search in neighboring chapters (page-count
//! and splitting drift between editions) → fraction within the chapter.

use chapbook_core::{resolve_in_text, LayeredLocator, Locator, ResolvedOffset, SpineItem};

/// How a position was recovered. Anything but `Exact` should be rewritten
/// (self-healing migration): re-capture at the recovered offset and store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreTier {
    Exact,
    Quote,
    Fraction,
    /// Nothing matched anywhere useful: chapter start by progression.
    ChapterStart,
}

/// How many chapters on each side of the target to try the quote layer in
/// when the edition changed (content shifts between adjacent spine items
/// when a new edition re-splits chapters).
const NEIGHBOR_SEARCH_RADIUS: usize = 2;

/// Restore a stored position against a (possibly different edition of a)
/// book. `same_edition` is the fingerprint comparison; `locator_text_of`
/// produces a spine item's locator text (None for unreadable items).
pub fn restore_position(
    stored: &LayeredLocator,
    same_edition: bool,
    spine: &[SpineItem],
    mut locator_text_of: impl FnMut(usize) -> Option<String>,
) -> (Locator, RestoreTier) {
    if spine.is_empty() {
        return (Locator::default(), RestoreTier::ChapterStart);
    }

    // Target spine item: by href first (survives spine reordering), then by
    // stored index.
    let by_href = spine.iter().position(|s| s.href == stored.spine_href);
    let target = by_href.unwrap_or(stored.spine_index).min(spine.len() - 1);
    let href_matches = by_href.is_some();

    if let Some(text) = locator_text_of(target) {
        let trusted = same_edition && href_matches;
        match resolve_in_text(&text, stored, trusted) {
            ResolvedOffset::Exact(offset) => {
                return (Locator::new(target, offset), RestoreTier::Exact)
            }
            ResolvedOffset::Quote(offset) => {
                return (Locator::new(target, offset), RestoreTier::Quote)
            }
            ResolvedOffset::Fraction(offset) => {
                if same_edition {
                    // Same file: the fraction is as good as it gets.
                    return (Locator::new(target, offset), RestoreTier::Fraction);
                }
                // Different edition: before settling, try the quote in
                // neighboring chapters.
                for distance in 1..=NEIGHBOR_SEARCH_RADIUS {
                    for candidate in [
                        target.checked_sub(distance),
                        Some(target + distance).filter(|i| *i < spine.len()),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if let Some(neighbor_text) = locator_text_of(candidate) {
                            if let ResolvedOffset::Quote(offset) =
                                resolve_in_text(&neighbor_text, stored, false)
                            {
                                return (Locator::new(candidate, offset), RestoreTier::Quote);
                            }
                        }
                    }
                }
                return (Locator::new(target, offset), RestoreTier::Fraction);
            }
        }
    }

    // Chapter unreadable: land at the progression-implied chapter start.
    let approx = (stored.book_progression * spine.len() as f64).round() as usize;
    (
        Locator::chapter_start(approx.min(spine.len() - 1)),
        RestoreTier::ChapterStart,
    )
}
