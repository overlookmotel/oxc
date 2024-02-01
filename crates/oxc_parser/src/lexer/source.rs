#![allow(clippy::unnecessary_safety_comment)]

use crate::MAX_LEN;

use std::{marker::PhantomData, slice, str};

#[derive(Clone)]
pub(super) struct Source<'a> {
    /// Pointer to start of source string. Never altered after initialization.
    start: *const u8,
    /// Pointer to end of source string. Never altered after initialization.
    end: *const u8,
    /// Pointer to current position in source string
    ptr: *const u8,
    /// Marker for immutable borrow of source string
    _marker: PhantomData<&'a str>,
}

impl<'a> Source<'a> {
    /// Create `Source` from `&str`.
    pub(super) fn new(mut source: &'a str) -> Self {
        // If source exceeds size limit, substitute a short source which will fail to parse.
        // `Parser::parse` will convert error to `diagnostics::OverlongSource`.
        if source.len() > MAX_LEN {
            source = "\0";
        }

        let start = source.as_ptr();
        // SAFETY: Adding `source.len()` to the starting pointer gives a pointer
        // at the end of `source`. `end` will never be dereferenced, only checked
        // for direct pointer equality with `current` to check if at end of file.
        let end = unsafe { start.add(source.len()) };

        Self { start, end, ptr: start, _marker: PhantomData }
    }

    /// Get entire source as `&str`.
    #[inline]
    pub(super) fn whole(&self) -> &'a str {
        // SAFETY: `start` and `end` are created from a `&str` in `Source::new`,
        // so guaranteed to be start and end of a valid UTF-8 string.
        unsafe {
            let len = self.end as usize - self.start as usize;
            let slice = slice::from_raw_parts(self.start, len);
            str::from_utf8_unchecked(slice)
        }
    }

    /// Get remaining source as `&str`.
    #[inline]
    pub(super) fn remaining(&self) -> &'a str {
        // SAFETY:
        // `start` and `end` are created from a `&str` in `Source::new` so span a single allocation.
        // Contract of `Source` is that `ptr` is always `>= start` and `<= end`,
        // so a slice spanning `ptr` to `end` will always be part of of a single allocation.
        // Contract of `Source` is that `ptr` is always on a UTF-8 character boundary,
        // so slice from `ptr` to `end` will always be a valid UTF-8 string.
        unsafe {
            let len = self.end as usize - self.ptr as usize;
            let slice = slice::from_raw_parts(self.ptr, len);
            str::from_utf8_unchecked(slice)
        }
    }

    // Return if at end of source.
    #[inline]
    pub(super) fn is_eof(&self) -> bool {
        self.ptr == self.end
    }

    /// Get source position.
    /// The `SourcePosition` returned is guaranteed to be within bounds of `&str` that `Source`
    /// was created from, and on a UTF-8 character boundary, so can be used by caller
    /// to later move current position of this `Source` using `Source::set_position`.
    #[inline]
    pub(super) fn position(&self) -> SourcePosition<'a> {
        SourcePosition { ptr: self.ptr, _marker: PhantomData }
    }

    /// Move current position in source.
    // TODO: Should this be unsafe? It's possible to create a `SourcePosition` from a *different*
    // `Source`, which would violate `Source`'s invariants.
    #[inline]
    pub(super) fn set_position(&mut self, pos: SourcePosition) {
        // `SourcePosition` always upholds the invariants of `Source`
        self.ptr = pos.ptr;
    }

    /// Get current position in source, relative to start of source.
    #[allow(clippy::cast_possible_truncation)]
    #[inline]
    pub(super) fn offset(&self) -> u32 {
        // Cannot overflow because of `MAX_LEN` check in `Source::new`
        (self.ptr as usize - self.start as usize) as u32
    }

    /// Move current position in source to an offset.
    #[inline]
    pub(super) fn set_offset(&mut self, offset: u32) {
        let offset = offset as usize;
        let len = self.end as usize - self.start as usize;
        assert!(offset <= len);
        if offset == len {
            // Moving to end, so by definition on a UTF-8 character boundary
            self.ptr = self.end;
        } else {
            // SAFETY: `start + offset` is < `end`, so `new_ptr` is in bounds of original `&str`
            let new_ptr = unsafe { self.start.add(offset) };
            // SAFETY: `new_ptr` is in bounds of original `&str`, and not at the end,
            // so valid to read a byte
            let byte = unsafe { new_ptr.read() };
            // Enforce invariant that `ptr` must be positioned on a UTF-8 character boundary
            // (128 - 191 are UTF-8 continuation bytes i.e. not on a UTF-8 character boundary)
            assert!(!(128..192).contains(&byte));
            self.ptr = new_ptr;
        }
    }

    /// Get next char and move `current` on to after it.
    #[inline]
    pub(super) fn next_char(&mut self) -> Option<char> {
        self.next_code_point().map(|ch| {
            // SAFETY:
            // `Source` is created from a `&str`, so between `start` and `end` must be valid UTF-8.
            // Invariant of `Source` is that `ptr` must always be positioned on a UTF-8 character boundary.
            // Therefore `ch` must be a valid Unicode Scalar Value.
            unsafe { char::from_u32_unchecked(ch) }
        })
    }

    /// Get next code point.
    /// Copied from implementation of `std::str::Chars`.
    /// https://doc.rust-lang.org/src/core/str/validations.rs.html#36
    #[allow(clippy::cast_lossless)]
    #[inline]
    fn next_code_point(&mut self) -> Option<u32> {
        // Decode UTF-8.
        // SAFETY: If next byte is not ASCII, this function consumes further bytes until end of UTF-8
        // character sequence, leaving `ptr` positioned on next UTF-8 character boundary, or at EOF.
        let x = unsafe { self.next_byte() }?;
        if x < 128 {
            return Some(x as u32);
        }

        // `x` is not a UTF-8 continuation byte, and is a valid UTF-8 byte value
        debug_assert!((192..248).contains(&x));

        // Multibyte case follows
        // Decode from a byte combination out of: [[[x y] z] w]
        // NOTE: Performance is sensitive to the exact formulation here
        let init = utf8_first_byte(x, 2);
        // SAFETY: `Source` contains a valid UTF-8 string, and 1st byte is not ASCII,
        // so guaranteed there is a further byte to be consumed.
        let y = unsafe { self.next_byte_unchecked() };
        let mut ch = utf8_acc_cont_byte(init, y);
        if x >= 0xE0 {
            // [[x y z] w] case
            // 5th bit in 0xE0 .. 0xEF is always clear, so `init` is still valid
            // SAFETY: `Source` contains a valid UTF-8 string, and 1st byte indicates it is start
            // of a 3 or 4-byte sequence, so guaranteed there is a further byte to be consumed.
            let z = unsafe { self.next_byte_unchecked() };
            let y_z = utf8_acc_cont_byte((y & CONT_MASK) as u32, z);
            ch = init << 12 | y_z;
            if x >= 0xF0 {
                // [x y z w] case
                // use only the lower 3 bits of `init`
                // SAFETY: `Source` contains a valid UTF-8 string, and 1st byte indicates it is start
                // of a 4-byte sequence, so guaranteed there is a further byte to be consumed.
                let w = unsafe { self.next_byte_unchecked() };
                ch = (init & 7) << 18 | utf8_acc_cont_byte(y_z, w);
            }
        }

        Some(ch)
    }

    /// Get next byte of source, if not at EOF.
    ///
    /// SAFETY:
    /// This function may leave `self.ptr` in middle of a UTF-8 character sequence.
    /// It is caller's responsibility to ensure that either the byte returned is ASCII,
    /// or make further calls to `next_byte()` or `next_byte_unchecked()` until `self.ptr`
    /// is positioned on a UTF-8 character boundary.
    #[inline]
    unsafe fn next_byte(&mut self) -> Option<u8> {
        if self.ptr == self.end {
            // TODO: Mark this branch `#[cold]`?
            None
        } else {
            // SAFETY: Safe to read from `ptr` as we just checked it's not out of bounds
            Some(self.next_byte_unchecked())
        }
    }

    /// Get next byte of source, without bounds-check.
    ///
    /// SAFETY:
    /// 1. Caller must ensure `ptr < end` i.e. not at end of file.
    /// 2. This function may leave `self.ptr` in middle of a UTF-8 character sequence.
    ///    It is caller's responsibility to ensure that either the byte returned is ASCII,
    ///    or make further calls to `next_byte()` or `next_byte_unchecked()` until the end of
    ///    the UTF-8 character sequence is reached.
    #[inline]
    unsafe fn next_byte_unchecked(&mut self) -> u8 {
        let byte = self.ptr.read();
        self.ptr = self.ptr.add(1);
        byte
    }

    /// Get next char, without consuming it.
    #[inline]
    pub(super) fn peek_char(&self) -> Option<char> {
        self.peek_code_point().map(|ch| {
            // SAFETY:
            // `Source` is created from a `&str` so between `start` and `end` must be valid UTF-8.
            // Invariant of `Source` is that `ptr` must always be positioned on start
            // of a UTF-8 character sequence.
            // Therefore `ch` must be a valid Unicode Scalar Value.
            unsafe { char::from_u32_unchecked(ch) }
        })
    }

    /// Peek next code point, without consuming it.
    /// Copied from implementation of `std::str::Chars`.
    /// https://doc.rust-lang.org/src/core/str/validations.rs.html#36
    #[allow(clippy::cast_lossless)]
    #[inline]
    fn peek_code_point(&self) -> Option<u32> {
        // Decode UTF-8
        let x = self.peek_byte()?;
        if x < 128 {
            return Some(x as u32);
        }

        // `x` is not a UTF-8 continuation byte, and is a valid UTF-8 byte value
        debug_assert!((192..248).contains(&x));

        let mut ptr = self.ptr;
        #[allow(clippy::items_after_statements)]
        #[inline]
        unsafe fn advance_and_read(ptr: &mut *const u8) -> u8 {
            *ptr = ptr.add(1);
            ptr.read()
        }

        // Multibyte case follows
        // Decode from a byte combination out of: [[[x y] z] w]
        // NOTE: Performance is sensitive to the exact formulation here
        let init = utf8_first_byte(x, 2);
        // SAFETY: `Source` contains a valid UTF-8 string, and 1st byte is not ASCII,
        // so guaranteed there is a further byte to be read.
        let y = unsafe { advance_and_read(&mut ptr) };
        let mut ch = utf8_acc_cont_byte(init, y);
        if x >= 0xE0 {
            // [[x y z] w] case
            // 5th bit in 0xE0 .. 0xEF is always clear, so `init` is still valid
            // SAFETY: `Source` contains a valid UTF-8 string, and 1st byte indicates it is start
            // of a 3 or 4-byte sequence, so guaranteed there is a further byte to be read.
            let z = unsafe { advance_and_read(&mut ptr) };
            let y_z = utf8_acc_cont_byte((y & CONT_MASK) as u32, z);
            ch = init << 12 | y_z;
            if x >= 0xF0 {
                // [x y z w] case
                // use only the lower 3 bits of `init`
                // SAFETY: `Source` contains a valid UTF-8 string, and 1st byte indicates it is start
                // of a 4-byte sequence, so guaranteed there is a further byte to be read.
                let w = unsafe { advance_and_read(&mut ptr) };
                ch = (init & 7) << 18 | utf8_acc_cont_byte(y_z, w);
            }
        }

        Some(ch)
    }

    /// Peek next byte of source without consuming it, if not at EOF.
    #[inline]
    pub(super) fn peek_byte(&self) -> Option<u8> {
        if self.ptr == self.end {
            // TODO: Mark this branch `#[cold]`?
            None
        } else {
            // SAFETY: Safe to read from `ptr` as we just checked it's not out of bounds
            Some(unsafe { self.peek_byte_unchecked() })
        }
    }

    /// Peek next byte of source without consuming it, without bounds-check.
    ///
    /// SAFETY: Caller must ensure `ptr < end` i.e. not at end of file.
    #[inline]
    pub(super) unsafe fn peek_byte_unchecked(&self) -> u8 {
        self.ptr.read()
    }
}

/// Wrapper around a pointer to a position in `Source`.
#[derive(Debug, Clone, Copy)]
pub struct SourcePosition<'a> {
    ptr: *const u8,
    _marker: PhantomData<&'a str>,
}

/// Mask of the value bits of a continuation byte.
/// Copied from implementation of `std::str::Chars`.
/// https://doc.rust-lang.org/src/core/str/validations.rs.html#274
const CONT_MASK: u8 = 0b0011_1111;

/// Returns the initial codepoint accumulator for the first byte.
/// The first byte is special, only want bottom 5 bits for width 2, 4 bits
/// for width 3, and 3 bits for width 4.
/// Copied from implementation of `std::str::Chars`.
/// https://doc.rust-lang.org/src/core/str/validations.rs.html#11
#[inline]
const fn utf8_first_byte(byte: u8, width: u32) -> u32 {
    (byte & (0x7F >> width)) as u32
}

/// Returns the value of `ch` updated with continuation byte `byte`.
/// Copied from implementation of `std::str::Chars`.
/// https://doc.rust-lang.org/src/core/str/validations.rs.html#17
#[inline]
const fn utf8_acc_cont_byte(ch: u32, byte: u8) -> u32 {
    (ch << 6) | (byte & CONT_MASK) as u32
}
