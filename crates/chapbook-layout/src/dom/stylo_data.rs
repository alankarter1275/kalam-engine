//! Interior-mutable holder for stylo's per-element style data.
//!
//! Structure follows blitz-dom's proven `StyloData` wrapper: an `UnsafeCell`
//! encapsulated so access sites don't need raw `unsafe` blocks. Safety relies
//! on stylo's traversal model — `ensure_init`/`clear`/`unsafe_stylo_only_mut`
//! are only called while the style traversal has exclusive access to nodes.

use std::cell::UnsafeCell;
use std::fmt;

use style::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};

pub(crate) struct StyloData {
    inner: UnsafeCell<Option<ElementDataWrapper>>,
}

impl Default for StyloData {
    fn default() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }
}

impl fmt::Debug for StyloData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StyloData").finish_non_exhaustive()
    }
}

impl StyloData {
    pub fn has_data(&self) -> bool {
        unsafe { &*self.inner.get() }.is_some()
    }

    /// Borrow the element data immutably, if present.
    pub fn get(&self) -> Option<ElementDataRef<'_>> {
        unsafe { &*self.inner.get() }.as_ref().map(|w| w.borrow())
    }

    /// Get a mutable borrow. Only sound while stylo's traversal holds
    /// exclusive access to this node.
    pub unsafe fn unsafe_stylo_only_mut(&self) -> Option<ElementDataMut<'_>> {
        let opt = unsafe { &mut *self.inner.get() };
        opt.as_mut().map(|w| w.borrow_mut())
    }

    /// Initialize the element data if needed and return a mutable borrow.
    ///
    /// SAFETY: no outstanding borrows of this container may exist.
    pub unsafe fn ensure_init(&self) -> ElementDataMut<'_> {
        if !self.has_data() {
            unsafe { *self.inner.get() = Some(ElementDataWrapper::default()) };
        }
        unsafe { self.unsafe_stylo_only_mut() }.unwrap()
    }

    /// Clear back to the uninitialized state.
    ///
    /// SAFETY: no outstanding borrows of this container may exist.
    pub unsafe fn clear(&self) {
        unsafe { *self.inner.get() = None };
    }
}
