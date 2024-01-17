use oxc_index::assert_eq_size;

use super::{Atom, HEAP_FLAG_USIZE, MAX_LEN, MAX_LEN_INLINE};

const USIZE_SIZE: usize = std::mem::size_of::<usize>();

#[repr(C)]
pub struct HeapBuffer {
    // Pointer to string content
    // TODO: `compact_str` uses `NonNull<u8>` here. Is there an advantage to that?
    pub ptr: *const u8,
    // Length stored as a `usize` but as little-endian bytes.
    // Combined with `MAX_LEN` restriction, this ensures top 3 bits of last byte
    // are not used for length, and can contain flag bits.
    pub len: [u8; USIZE_SIZE],
}

assert_eq_size!(HeapBuffer, Atom);

impl HeapBuffer {
    /// Construct a new [`HeapBuffer`].
    ///
    /// Caller must ensure length is greater than [`MAX_LEN_INLINE`].
    /// Failing to do so will not cause UB, but may cause equality comparisons
    /// to return wrong result.
    ///
    /// # Panic
    /// Panics if length of string is greater than [`MAX_LEN`].
    pub const fn new(text: &str) -> Self {
        // Caller must ensure length is greater than `MAX_LEN_INLINE`.
        // This means all short strings are always stored inline, so the same string will always
        // be represented by the same bits, making equality comparisons cheap.
        debug_assert!(text.len() > MAX_LEN_INLINE);

        // Strings larger than `MAX_LEN` cannot be stored, as they would require more than
        // `size_of::<usize>() * 8 - 3` bits. The top 3 bits of last byte are reserved for flag bits.
        // TODO: Should this assertion be removed on 64-bit systems? Losing the check would be a slight
        // speed improvement, and it's infeasible to construct a string larger than 2 exabytes anyway.
        assert!(text.len() <= MAX_LEN);

        // Store length in little-endian order so most significant byte is last byte.
        // Add `HEAP_FLAG` to that last byte.
        let len = (text.len() | HEAP_FLAG_USIZE).to_le_bytes();

        Self { ptr: text.as_ptr(), len }
    }
}
