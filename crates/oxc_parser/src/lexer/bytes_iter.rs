//! Iterator over string bytes.
//!
//! Unlike `std::str::Bytes`, this type allows converting back to a `&[u8]` slice,
//! a `&str` slice, or a `std::str::Chars` iterator.
//!
//! It also produces more efficient codegen than `std::str::Bytes` in some circumstances
//! (unclear why).

use std::{
    slice,
    str::{self, Chars},
};

#[derive(Clone)]
pub struct BytesIter<'a>(slice::Iter<'a, u8>);

impl<'a> From<&'a str> for BytesIter<'a> {
    #[inline]
    fn from(s: &'a str) -> Self {
        Self(s.as_bytes().iter())
    }
}

impl<'a> From<Chars<'a>> for BytesIter<'a> {
    #[inline]
    fn from(chars: Chars<'a>) -> Self {
        Self::from(chars.as_str())
    }
}

impl<'a> From<&Chars<'a>> for BytesIter<'a> {
    #[inline]
    fn from(chars: &Chars<'a>) -> Self {
        Self::from(chars.as_str())
    }
}

impl<'a> BytesIter<'a> {
    #[inline]
    pub fn next(&mut self) -> Option<u8> {
        self.0.next().copied()
    }

    #[inline]
    pub fn next_char(&mut self) -> Option<char> {
        let mut chars = match self.chars() {
            Some(chars) => chars,
            None => {
                return None;
            }
        };

        chars.next().map(|c| {
            self.0 = chars.as_str().as_bytes().iter();
            c
        })
    }

    #[inline]
    pub fn peek(&self) -> Option<u8> {
        self.clone().next()
    }

    #[inline]
    pub unsafe fn peek_unchecked(&self) -> u8 {
        *self.as_slice().get_unchecked(0)
    }

    #[inline]
    pub fn peek_char(&self) -> Option<char> {
        self.chars().and_then(|mut chars| chars.next())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    #[inline]
    pub fn as_slice(&self) -> &'a [u8] {
        self.0.as_slice()
    }

    #[inline]
    fn is_on_utf8_char_boundary(&self) -> bool {
        // Byte values 0b10xxxxxx (128-191) are UTF-8 continuation bytes.
        // Byte values above 0b11110111 (> 247) do not occur in UTF-8 strings.
        // All other byte values are start of a UTF-8 character sequence.
        // NB: End of string is a valid char boundary.
        self.0.clone().next().map_or(true, |b| !(128..192).contains(b))
    }

    #[inline]
    pub fn as_str(&self) -> Option<&'a str> {
        if self.is_on_utf8_char_boundary() {
            // SAFETY: `BytesIter` is always created from a valid UTF-8 string.
            // It takes an immutable ref, so the bytes cannot be modified.
            // Therefore no need to check validity of all bytes the way `str::from_utf8()` does.
            // We only need to make sure the iterator is positioned on a UTF-8 character boundary,
            // which we just did.
            Some(unsafe { str::from_utf8_unchecked(self.as_slice()) })
        } else {
            None
        }
    }

    #[inline]
    pub unsafe fn as_str_unchecked(&self) -> &'a str {
        str::from_utf8_unchecked(self.as_slice())
    }

    #[inline]
    pub fn chars(&self) -> Option<Chars<'a>> {
        self.as_str().map(|s| s.chars())
    }

    #[inline]
    pub unsafe fn chars_unchecked(&self) -> Chars<'a> {
        self.as_str_unchecked().chars()
    }

    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.as_slice().as_ptr()
    }

    /// SAFETY: Slice must contain the bytes of a valid UTF-8 string.
    #[inline]
    pub unsafe fn from_slice(slice: &'a [u8]) -> Self {
        Self::from(std::str::from_utf8_unchecked(slice))
    }
}
