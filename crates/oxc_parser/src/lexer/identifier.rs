use super::{BytesIter, Kind, Lexer};
use crate::diagnostics;

use oxc_allocator::String;
use oxc_span::Span;
use oxc_syntax::identifier::{
    is_identifier_part_ascii_byte, is_identifier_part_unicode, is_identifier_start_ascii_byte,
    is_identifier_start_unicode,
};

impl<'a> Lexer<'a> {
    /// TODO: Make a wrapper type for bytes iterator.

    /// Handle identifier with ASCII start character.
    /// Returns text of the identifier, minus its first char.
    ///
    /// Start character should not be consumed from `self.current.chars` prior to calling this.
    ///
    /// This function is the "fast path" for the most common identifiers in JS code - purely
    /// consisting of ASCII characters: `a`-`z`, `A`-`Z`, `0`-`9`, `_`, `$`.
    /// JS syntax also allows Unicode identifiers and escapes (e.g. `\u{FF}`) in identifiers,
    /// but they are very rare in practice. So this fast path will handle 99% of JS code.
    ///
    /// SAFETY:
    /// * `self.current.chars` must not be exhausted (at least 1 char remaining).
    /// * Next char must be ASCII.
    ///
    /// Much of this function's code is duplicated in other functions below.
    /// This is not DRY, but is justified for 2 reasons:
    /// 1. Keeping all code for the fast path in a single function produces optimal performance.
    /// 2. Keeping the core logic of the fast path contained in one place makes it  easier to verify
    ///    the correctness of the unsafe code which is required for maximum speed.
    ///    The other identifier functions are more complex and therefore do not use unsafe code,
    ///    at the cost of speed, but they handle only rare cases anyway.
    #[allow(clippy::missing_safety_doc)] // Clippy is wrong!
    pub unsafe fn identifier_name_handler(&mut self) -> &'a str {
        // Create iterator over remaining bytes, but skipping the first byte.
        // Guaranteed slicing first byte off start will produce a valid UTF-8 string,
        // because caller guarantees current char is ASCII.
        let str_not_inc_first = self.remaining().get_unchecked(1..);
        let mut bytes = str_not_inc_first.as_bytes().iter();

        // Consume bytes from `bytes` iterator until reach a byte which can't be part of an identifier,
        // or reaching EOF.
        // Code paths for Unicode characters and `\` escapes marked `#[cold]` to hint to branch predictor
        // to expect ASCII chars only, which makes processing ASCII-only identifiers as fast as possible.
        // NB: `self.current.chars` is *not* advanced in this loop.
        while let Some(&b) = bytes.clone().next() {
            if is_identifier_part_ascii_byte(b) {
                bytes.next();
                continue;
            }
            if b == b'\\' {
                #[cold]
                fn backslash<'a>(lexer: &mut Lexer<'a>, bytes: BytesIter<'a>) -> &'a str {
                    &lexer.identifier_backslash(bytes, false)[1..]
                }
                return backslash(self, bytes);
            }
            if !b.is_ascii() {
                #[cold]
                fn unicode<'a>(lexer: &mut Lexer<'a>, bytes: BytesIter<'a>) -> &'a str {
                    &lexer.identifier_tail_unicode(bytes)[1..]
                }
                return unicode(self, bytes);
            }
            // ASCII char which is not part of identifier
            break;
        }

        // End of identifier found (which may be EOF).
        // Advance `self.current.chars` up to after end of identifier.
        let after_identifier = bytes.as_slice();
        self.current.chars = std::str::from_utf8_unchecked(after_identifier).chars();

        // Return identifier minus its first char.
        // We know `len` can't cut string in middle of a Unicode character sequence,
        // because we've only found ASCII bytes up to this point.
        let len = after_identifier.as_ptr() as usize - str_not_inc_first.as_ptr() as usize;
        str_not_inc_first.get_unchecked(..len)
    }

    /// Handle identifier after first char dealt with.
    /// First char can have been ASCII or Unicode, but cannot have been a `\` escape.
    /// First char should not be consumed from `self.current.chars` prior to calling this,
    /// but `bytes` iterator should be positioned *after* first char.
    /// TODO: Optimize this. And amend functions it calls not to return `&str`.
    pub fn identifier_tail_after_no_escape(&mut self, mut bytes: BytesIter<'a>) {
        // Find first byte which isn't valid ASCII identifier part
        let next_byte =
            if let Some(b) = Self::identifier_tail_consume_ascii_identifier_bytes(&mut bytes) {
                b
            } else {
                self.identifier_eof();
                return;
            };

        // Handle the byte which isn't ASCII identifier part.
        // Most likely we're at the end of the identifier, but handle `\` escape and Unicode chars.
        // Fast path for normal ASCII identifiers, by marking the 2 uncommon cases `#[cold]`.
        if next_byte == b'\\' {
            self.identifier_backslash(bytes, false);
        } else if !next_byte.is_ascii() {
            self.identifier_tail_unicode(bytes);
        } else {
            // End of identifier found.
            // Advance chars iterator to the byte we just found which isn't part of the identifier.
            self.identifier_end(&bytes);
        }
    }

    /// Consume bytes from `Bytes` iterator which are ASCII identifier part bytes.
    /// `bytes` iterator is left positioned on next non-matching byte.
    /// Returns next non-matching byte, or `None` if EOF.
    fn identifier_tail_consume_ascii_identifier_bytes(bytes: &mut BytesIter<'a>) -> Option<u8> {
        while let Some(&b) = bytes.clone().next() {
            if !is_identifier_part_ascii_byte(b) {
                return Some(b);
            }
            bytes.next();
        }
        None
    }

    /// End of identifier found.
    /// `bytes` iterator must be positioned on next byte after end of identifier.
    // `#[inline]` because we want this inlined into `identifier_tail_after_no_escape`,
    // which is on the fast path for common cases.
    #[inline]
    fn identifier_end(&mut self, bytes: &BytesIter<'a>) -> &'a str {
        let remaining = self.remaining();
        let len = bytes.as_slice().as_ptr() as usize - remaining.as_ptr() as usize;
        let (text, after_identifier) = remaining.split_at(len);
        self.current.chars = after_identifier.chars();
        text
    }

    /// Identifier end at EOF.
    /// Return text of identifier, and advance `self.current.chars` to end of file.
    // This could be replaced with `identifier_end` in `identifier_tail_after_no_escape`
    // but doing that causes a 3% drop in lexer benchmarks, for some reason.
    // TODO: Remove this function.
    fn identifier_eof(&mut self) -> &'a str {
        let text = self.remaining();
        self.current.chars = text[text.len()..].chars();
        text
    }

    /// Handle continuation of identifier after first byte of a multi-byte unicode char found.
    /// Any number of characters can have already been eaten from `bytes` iterator prior to it.
    /// `bytes` iterator should be positioned at start of Unicode character.
    /// Nothing should have been consumed from `self.current.chars` prior to calling this.
    // `#[cold]` to guide branch predictor that Unicode chars in identifiers are rare.
    // TODO: Remove `#[cold]`
    #[cold]
    fn identifier_tail_unicode(&mut self, mut bytes: BytesIter<'a>) -> &'a str {
        let at_end = self.identifier_consume_unicode_char_if_identifier_part(&mut bytes);
        if !at_end {
            let at_end = self.identifier_tail_consume_until_end_or_escape(&mut bytes);
            if !at_end {
                return self.identifier_backslash(bytes, false);
            }
        }

        self.identifier_end(&bytes)
    }

    /// Consume valid identifier bytes (ASCII or Unicode) from `bytes`
    /// until reach end of identifier or a `\`.
    /// Returns `true` if at end of identifier, or `false` if found `\`.
    fn identifier_tail_consume_until_end_or_escape(&mut self, bytes: &mut BytesIter<'a>) -> bool {
        loop {
            // Eat ASCII chars from `bytes`
            let next_byte =
                if let Some(b) = Self::identifier_tail_consume_ascii_identifier_bytes(bytes) {
                    b
                } else {
                    return true;
                };

            if next_byte.is_ascii() {
                return next_byte != b'\\';
            }

            // Unicode char
            let at_end = self.identifier_consume_unicode_char_if_identifier_part(bytes);
            if at_end {
                return true;
            }
            // Char was part of identifier. Keep eating.
        }
    }

    /// Consume unicode character from `bytes` if it's part of identifier.
    /// Returns `true` if at end of identifier (this character is not part of identifier)
    /// or `false` if character was consumed and potentially more of identifier still to come.
    fn identifier_consume_unicode_char_if_identifier_part(
        &self,
        bytes: &mut BytesIter<'a>,
    ) -> bool {
        let current_len = bytes.as_slice().as_ptr() as usize - self.remaining().as_ptr() as usize;
        let mut chars = self.remaining()[current_len..].chars();
        let c = chars.next().unwrap();
        if is_identifier_part_unicode(c) {
            // Advance `bytes` iterator past this character
            *bytes = chars.as_str().as_bytes().iter();
            false
        } else {
            // Reached end of identifier
            true
        }
    }

    /// Handle identifier after a `\` found.
    /// Any number of characters can have been eaten from `bytes` iterator prior to the `\`.
    /// `\` byte must not have been eaten from `bytes`.
    /// Nothing should have been consumed from `self.current.chars` prior to calling this.
    // `is_start` should be `true` if this is first char in the identifier,
    // and `false` otherwise.
    // `#[cold]` to guide branch predictor that escapes in identifiers are rare and keep a fast path
    // in `identifier_tail_after_no_escape` for the common case.
    // TODO: Remove `#[cold]` and mark callers as cold instead.
    #[cold]
    pub fn identifier_backslash(
        &mut self,
        mut bytes: BytesIter<'a>,
        mut is_start: bool,
    ) -> &'a str {
        // All the other identifier lexer functions only iterate through `bytes`,
        // leaving `self.current.chars` unchanged until the end of the identifier is found.
        // At this point, after finding an escape, we change approach.
        // In this function, the unescaped identifier is built up in an arena `String`.
        // Each time an escape is found, all the previous non-escaped bytes are pushed into the `String`
        // and `chars` iterator advanced to after the escape sequence.
        // We then search again for another run of unescaped bytes, and push them to the `String`
        // as a single chunk. If another escape is found, loop back and do same again.

        // Create an arena string to hold unescaped identifier.
        // We don't know how long identifier will end up being. Take a guess that total length
        // will be double what we've seen so far, or 16 minimum.
        const MIN_LEN: usize = 16;
        let mut len_to_push =
            bytes.as_slice().as_ptr() as usize - self.remaining().as_ptr() as usize;
        let capacity = (len_to_push * 2).max(MIN_LEN);
        let mut str = String::with_capacity_in(capacity, self.allocator);

        loop {
            // Add bytes before this escape to `str` and advance `chars` iterator to after the `\`
            str.push_str(&self.remaining()[0..len_to_push]);
            self.current.chars = self.remaining()[len_to_push + 1..].chars();

            // Consume escape sequence from `chars` and add char to `str`
            self.identifier_unicode_escape_sequence(&mut str, is_start);
            is_start = false;

            // Bring `bytes` iterator back into sync with `chars` iterator.
            // i.e. advance `bytes` to after the escape sequence.
            bytes = self.remaining().as_bytes().iter();

            // Consume bytes from `bytes` until reach end of identifier or another escape
            let at_end = self.identifier_tail_consume_until_end_or_escape(&mut bytes);
            if at_end {
                break;
            }
            // Found another `\` escape
            len_to_push = bytes.as_slice().as_ptr() as usize - self.remaining().as_ptr() as usize;
        }

        // Add bytes after last escape to `str`, and advance `chars` iterator to end of identifier
        let last_chunk = self.identifier_end(&bytes);
        str.push_str(last_chunk);

        // Convert to arena slice and save to `escaped_strings`
        let text = str.into_bump_str();
        self.save_string(true, text);
        text
    }

    pub fn private_identifier(&mut self) -> Kind {
        let mut bytes = self.remaining().as_bytes().iter();
        if let Some(&b) = bytes.clone().next() {
            if is_identifier_start_ascii_byte(b) {
                // Consume byte from `bytes`
                bytes.next();
                self.identifier_tail_after_no_escape(bytes);
                Kind::PrivateIdentifier
            } else {
                // Do not consume byte from `bytes`
                self.private_identifier_not_ascii_id(bytes)
            }
        } else {
            // EOF
            let start = self.offset();
            self.error(diagnostics::UnexpectedEnd(Span::new(start, start)));
            Kind::Undetermined
        }
    }

    #[cold]
    fn private_identifier_not_ascii_id(&mut self, bytes: BytesIter<'a>) -> Kind {
        let b = *bytes.clone().next().unwrap();
        if b == b'\\' {
            // Do not consume `\` byte from `bytes`
            self.identifier_backslash(bytes, true);
            return Kind::PrivateIdentifier;
        }

        if !b.is_ascii() {
            let mut chars = self.current.chars.clone();
            let c = chars.next().unwrap();
            if is_identifier_start_unicode(c) {
                // Eat char from `bytes` (but not from `self.current.chars`)
                let bytes = chars.as_str().as_bytes().iter();
                self.identifier_tail_after_no_escape(bytes);
                return Kind::PrivateIdentifier;
            }
        };

        // No identifier found
        let start = self.offset();
        let c = self.consume_char();
        self.error(diagnostics::InvalidCharacter(c, Span::new(start, self.offset())));
        Kind::Undetermined
    }
}
