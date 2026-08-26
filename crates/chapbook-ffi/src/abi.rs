//! The two conversions every entry point needs: a C string in, and a Rust
//! string out into a buffer the caller owns.

use std::ffi::{c_char, CStr};

use crate::error::{cb_status, fail};

/// Borrow a `const char*` as `&str`.
///
/// `None` means the pointer was null or the bytes were not UTF-8, and the
/// last-error string already says which.
pub(crate) unsafe fn str_in<'a>(ptr: *const c_char, what: &str) -> Option<&'a str> {
    if ptr.is_null() {
        fail(cb_status::CB_ERR_NULL_ARGUMENT, format!("{what} is null"));
        return None;
    }
    // SAFETY: the caller's contract, stated in the header: a NUL-terminated
    // string that outlives the call.
    match unsafe { CStr::from_ptr(ptr) }.to_str() {
        Ok(value) => Some(value),
        Err(_) => {
            fail(
                cb_status::CB_ERR_INVALID_UTF8,
                format!("{what} is not valid UTF-8"),
            );
            None
        }
    }
}

/// Write `value` into the caller's buffer as a NUL-terminated C string, and
/// report how many bytes that took.
///
/// The one string-returning shape in this ABI. `needed` is set on *every*
/// path including the too-small one, so the standard two-call idiom works:
/// ask with a zero-capacity buffer, allocate, ask again. `buf` may be null
/// exactly when `cap` is zero, which is what makes the first call legal.
///
/// Returning the length rather than a pointer is what removes
/// `cb_free_string` from this header, and with it the most common defect in
/// hand-written C ABIs — a host freeing Rust's allocation with the wrong
/// allocator. Nothing here crosses owned.
pub(crate) unsafe fn str_out(
    value: &str,
    buf: *mut c_char,
    cap: usize,
    needed: *mut usize,
) -> cb_status {
    // An interior NUL would silently truncate the value on the host side,
    // which is worse than refusing it.
    if value.as_bytes().contains(&0) {
        return fail(
            cb_status::CB_ERR_INVALID_ARGUMENT,
            "value contains an interior NUL and cannot cross as a C string",
        );
    }
    let want = value.len() + 1;
    if !needed.is_null() {
        // SAFETY: caller-provided out-pointer, checked non-null.
        unsafe { *needed = want };
    }
    if cap == 0 {
        return if buf.is_null() {
            // The sizing call. Not an error worth a message: it is how the
            // idiom starts.
            cb_status::CB_ERR_BUFFER_TOO_SMALL
        } else {
            fail(
                cb_status::CB_ERR_BUFFER_TOO_SMALL,
                "buffer capacity is zero",
            )
        };
    }
    if buf.is_null() {
        return fail(
            cb_status::CB_ERR_NULL_ARGUMENT,
            "buffer is null but capacity is not zero",
        );
    }
    if cap < want {
        return fail(
            cb_status::CB_ERR_BUFFER_TOO_SMALL,
            format!("buffer holds {cap} bytes, {want} needed"),
        );
    }
    // SAFETY: `cap >= want` was just checked, and the two regions cannot
    // overlap — `value` is owned by this crate, `buf` by the caller.
    unsafe {
        std::ptr::copy_nonoverlapping(value.as_ptr(), buf as *mut u8, value.len());
        *buf.add(value.len()) = 0;
    }
    cb_status::CB_OK
}
