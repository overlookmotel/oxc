use oxc_index::assert_eq_size;

use super::{Atom, INLINE_FLAG, MAX_LEN_INLINE};

/// A buffer stored on the stack whose size is equal to the stack size of `String`
#[repr(transparent)]
pub struct InlineBuffer(pub [u8; MAX_LEN_INLINE]);

assert_eq_size!(InlineBuffer, Atom);

impl InlineBuffer {
    /// Construct a new [`InlineBuffer`]. A short string stored inline.
    ///
    /// # Safety
    /// Caller must ensure length of `text` is less than or equal to [`MAX_LEN_INLINE`].
    pub unsafe fn new(text: &str) -> Self {
        debug_assert!(text.len() <= MAX_LEN_INLINE);

        let len = text.len();
        let mut buffer = [0u8; MAX_LEN_INLINE];

        // Set length in the last byte
        buffer[MAX_LEN_INLINE - 1] = len as u8 | INLINE_FLAG;

        // Copy the string into buffer.
        //
        // Note: In the case where len == MAX_LEN_INLINE, we'll overwrite the len, but that's OK
        // because when reading the length we can detect that the last byte is part of UTF-8
        // and return a length of MAX_LEN_INLINE.
        //
        // SAFETY:
        // * src (`text`) is valid for `len` bytes because `len` comes from `text`.
        // * dst (`buffer`) is valid for `len` bytes if safety invariant upheld by caller.
        // * src and dst don't overlap because we created dst.
        std::ptr::copy_nonoverlapping(text.as_ptr(), buffer.as_mut_ptr(), len);

        InlineBuffer(buffer)
    }

    /// Construct a new [`InlineBuffer`] at compile time.
    ///
    /// # Panic
    /// Panics if length of `text` is greater than [`MAX_LEN_INLINE`] bytes.
    pub const fn new_const(text: &str) -> Self {
        assert!(
            text.len() <= MAX_LEN_INLINE,
            "Provided string has a length greater than MAX_LEN_INLINE"
        );

        let len = text.len();
        let mut buffer = [0u8; MAX_LEN_INLINE];

        // Set length in the last byte
        buffer[MAX_LEN_INLINE - 1] = len as u8 | INLINE_FLAG;

        // Note: `for` loops aren't allowed in `const fn`, hence the while.
        // Note: Iterating forward results in badly optimized code, because the compiler tries to
        //       unroll the loop.
        let text = text.as_bytes();
        let mut i = len;
        while i > 0 {
            buffer[i - 1] = text[i - 1];
            i -= 1;
        }

        InlineBuffer(buffer)
    }
}
