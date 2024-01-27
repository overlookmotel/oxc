use super::{BytesIter, Kind, Lexer};
use crate::diagnostics;

use oxc_allocator::String;
use oxc_span::Span;
use oxc_syntax::identifier::{
    is_identifier_part_ascii_byte, is_identifier_part_unicode, is_identifier_start_ascii_byte,
    is_identifier_start_unicode,
};

const MIN_ESCAPED_STR_LEN: usize = 16;

impl<'a> Lexer<'a> {
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
    /// 2. Keeping the core logic of the fast path contained in one place makes it easier to verify
    ///    the correctness of the unsafe code which is required for maximum speed.
    ///    The other identifier functions are more complex and therefore do not use unsafe code,
    ///    at the cost of speed, but they handle only rare cases anyway.
    #[allow(clippy::missing_safety_doc)] // Clippy is wrong!
    pub unsafe fn identifier_name_handler(&mut self) -> &'a str {
        const BATCH_SIZE: usize = 32;

        // Create iterator over remaining bytes, but skipping the first byte.
        // Guaranteed slicing first byte off start will produce a valid UTF-8 string,
        // because caller guarantees current char is ASCII.
        let after_first = self.remaining().as_ptr().add(1);
        let mut curr = after_first;
        // TODO: This is unsound. If `self.end_ptr() as usize` < `BATCH_SIZE`, it will wrap around.
        // Then will read a batch which is out of bounds.
        // `self.end_ptr().saturating_sub(BATCH_SIZE)` solves the problem, but lexer benchmarks drop 2%.
        // Could store this as property of Lexer, as it never changes.
        let batching_end = self.end_ptr() as usize - BATCH_SIZE;

        // Consume bytes which are ASCII identifier part.
        // Process in batches, to avoid bounds check on each turn of the loop.
        // If not enough bytes remaining for a batch, de-opt and process bytes by byte.
        // TODO: Could alternatively substitute an "end buffer" when we get to that point, so always
        // enough bytes for a batch. But would require setting `curr` back to point to corresponding
        // point in original buffer afterwards for slicing strings at the end to work.
        // NB: `self.current.chars` is *not* advanced in this loop.
        #[allow(unused_assignments)]
        let mut next_byte = 0;
        'outer: loop {
            if curr as usize > batching_end {
                // Not enough bytes remaining to process as a batch.
                // This branch marked `#[cold]` as should be very uncommon in normal-length JS files.
                // Very short JS files will be penalized, but they'll be very fast to parse anyway.
                // TODO: Could extend very short files during parser initialization with a bunch of `\n`s
                // to remove that problem.
                return self.identifier_name_handler_unbatched(curr);
            }

            // The compiler will unroll this loop.
            // TODO: Try repeating this manually or with a macro to make sure it's unrolled.
            for _i in 0..BATCH_SIZE {
                next_byte = curr.read();
                if !is_identifier_part_ascii_byte(next_byte) {
                    break 'outer;
                }
                curr = curr.add(1);
            }
        }

        // Check for uncommon cases
        if !next_byte.is_ascii() {
            #[cold]
            unsafe fn unicode<'a>(lexer: &mut Lexer<'a>, curr: *const u8) -> &'a str {
                let bytes = BytesIter::from_ptr_pair(curr, lexer.end_ptr());
                &lexer.identifier_tail_unicode(bytes)[1..]
            }
            return unicode(self, curr);
        }
        if next_byte == b'\\' {
            #[cold]
            unsafe fn backslash<'a>(lexer: &mut Lexer<'a>, curr: *const u8) -> &'a str {
                let bytes = BytesIter::from_ptr_pair(curr, lexer.end_ptr());
                &lexer.identifier_backslash(bytes, false)[1..]
            }
            return backslash(self, curr);
        }

        // End of identifier found.
        // Advance `self.current.chars` up to after end of identifier.
        // `curr` must be positioned on a UTF-8 character boundary, as we've only consumed ASCII bytes.
        self.current.chars = str_from_start_and_end(curr, self.end_ptr()).chars();

        // Return identifier minus its first char.
        // Caller guarantees 1st char is ASCII, so `after_first` must be on a UTF-8 character boundary.
        // `curr` must be positioned on a UTF-8 character boundary, as we've only consumed ASCII bytes.
        str_from_start_and_end(after_first, curr)
    }

    #[cold]
    unsafe fn identifier_name_handler_unbatched(&mut self, mut curr: *const u8) -> &'a str {
        let end = self.end_ptr();
        #[allow(unused_assignments)]
        let mut next_byte = 0;
        loop {
            if curr == end {
                // EOF.
                // Get identifier minus first char.
                // Caller guarantees first char is ASCII.
                let id_without_first = self.remaining().get_unchecked(1..);

                // Advance `self.current.chars` up to EOF.
                // End of string cannot be in middle of a Unicode byte sequence.
                self.current.chars = str_from_start_and_end(end, end).chars();
                return id_without_first;
            }

            next_byte = curr.read();
            if !is_identifier_part_ascii_byte(next_byte) {
                break;
            }
            curr = curr.add(1);
        }

        // Check for uncommon cases
        if !next_byte.is_ascii() {
            #[cold]
            unsafe fn unicode<'a>(
                lexer: &mut Lexer<'a>,
                curr: *const u8,
                end: *const u8,
            ) -> &'a str {
                let bytes = BytesIter::from_ptr_pair(curr, end);
                &lexer.identifier_tail_unicode(bytes)[1..]
            }
            return unicode(self, curr, end);
        }
        if next_byte == b'\\' {
            #[cold]
            unsafe fn backslash<'a>(
                lexer: &mut Lexer<'a>,
                curr: *const u8,
                end: *const u8,
            ) -> &'a str {
                let bytes = BytesIter::from_ptr_pair(curr, end);
                &lexer.identifier_backslash(bytes, false)[1..]
            }
            return backslash(self, curr, end);
        }

        // End of identifier found.
        // `self.current.chars` has not been advanced, so guarantee caller of `identifier_name_handler`
        // made that at least a 1 char remains in source still holds. Therefore `.add(1)` is in bounds.
        let after_first = self.remaining().as_ptr().add(1);

        // Advance `self.current.chars` up to after end of identifier.
        // `curr` must be positioned on a UTF-8 character boundary, as we've only consumed ASCII
        // bytes from it. `self.end_ptr()` is end of string so also on a char boundary.
        self.current.chars = str_from_start_and_end(curr, end).chars();

        // Return identifier minus its first char.
        // `after_first` and `curr` are part of same allocation.
        // `curr` must be positioned on a UTF-8 character boundary (see above).
        str_from_start_and_end(after_first, curr)
    }

    /// Handle identifier after first char dealt with.
    /// First char can have been ASCII or Unicode, but cannot have been a `\` escape.
    /// First char should not be consumed from `self.current.chars` prior to calling this,
    /// but `bytes` iterator should be positioned *after* first char.
    /// TODO: Optimize this. And amend functions it calls not to return `&str`.
    /// TODO: This is called when ASCII byte as first char of a private identifier
    /// or after a Unicode char. We want to make path for 1st case fast for ASCII,
    /// but if first char of an identifier is unicode, can't assume others won't be too.
    /// So needs 2 separate implementations which handle those 2 cases with unicode branch
    /// either `#[cold]` or not.
    pub fn identifier_tail_after_no_escape(&mut self, mut bytes: BytesIter<'a>) {
        // Find first byte which isn't valid ASCII identifier part
        let next_byte = if let Some(b) = Self::identifier_tail_consume_ascii(&mut bytes) {
            b
        } else {
            self.identifier_eof();
            return;
        };

        // Handle the byte which isn't ASCII identifier part.
        // Most likely we're at the end of the identifier, but handle `\` escape and Unicode chars.
        // Fast path for normal ASCII identifiers, by marking the 2 uncommon cases `#[cold]`.
        if !next_byte.is_ascii() {
            self.identifier_tail_unicode(bytes);
        } else if next_byte == b'\\' {
            self.identifier_backslash(bytes, false);
        } else {
            // End of identifier found.
            // Advance chars iterator to the byte we just found which isn't part of the identifier.
            self.identifier_end(&bytes);
        }
    }

    /// Consume bytes from `Bytes` iterator which are ASCII identifier part bytes.
    /// `bytes` iterator is left positioned on next non-matching byte.
    /// Returns next non-matching byte, or `None` if EOF.
    fn identifier_tail_consume_ascii(bytes: &mut BytesIter) -> Option<u8> {
        while let Some(b) = bytes.peek() {
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
        let len = bytes.as_ptr() as usize - remaining.as_ptr() as usize;
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
        let at_end = Self::identifier_consume_unicode_char_if_identifier_part(&mut bytes);
        if !at_end {
            let at_end = Self::identifier_tail_consume_until_end_or_escape(&mut bytes);
            if !at_end {
                return self.identifier_backslash(bytes, false);
            }
        }

        self.identifier_end(&bytes)
    }

    /// Consume valid identifier bytes (ASCII or Unicode) from `bytes`
    /// until reach end of identifier or a `\`.
    /// Returns `true` if at end of identifier, or `false` if found `\`.
    fn identifier_tail_consume_until_end_or_escape(bytes: &mut BytesIter<'a>) -> bool {
        loop {
            // Eat ASCII chars from `bytes`
            let next_byte = if let Some(b) = Self::identifier_tail_consume_ascii(bytes) {
                b
            } else {
                return true;
            };

            if next_byte.is_ascii() {
                return next_byte != b'\\';
            }

            // Unicode char
            let at_end = Self::identifier_consume_unicode_char_if_identifier_part(bytes);
            if at_end {
                return true;
            }
            // Char was part of identifier. Keep eating.
        }
    }

    /// Consume unicode character from `bytes` if it's part of identifier.
    /// Returns `true` if at end of identifier (this character is not part of identifier)
    /// or `false` if character was consumed and potentially more of identifier still to come.
    fn identifier_consume_unicode_char_if_identifier_part(bytes: &mut BytesIter) -> bool {
        let c = bytes.peek_char().unwrap();
        if is_identifier_part_unicode(c) {
            // Advance `bytes` iterator past this character
            bytes.next_char().unwrap();
            false
        } else {
            // Reached end of identifier
            true
        }
    }

    // All the other identifier lexer functions only iterate through `bytes`,
    // leaving `self.current.chars` unchanged until the end of the identifier is found.
    // We change our approach after finding an escape.
    // In these functions, the unescaped identifier is built up in an arena string.
    // Each time an escape is found, all the previous non-escaped bytes are pushed into the `String`
    // and `chars` iterator advanced to after the escape sequence.
    // We then search again for another run of unescaped bytes, and push them to the `String`
    // as a single chunk. If another escape is found, loop back and do same again.

    pub fn identifier_backslash_handler(&mut self) -> &'a str {
        // Create arena string to hold unescaped identifier.
        // We don't know how long identifier will end up being, so guess.
        let str = String::with_capacity_in(MIN_ESCAPED_STR_LEN, self.allocator);

        // Consume `\`
        self.consume_char();

        // Process escape and get rest of identifier
        self.identifier_after_backslash(str, true)
    }

    #[cold]
    #[allow(clippy::needless_pass_by_value)] // TODO: Test if faster to pass `bytes` as mut ref
    fn identifier_backslash(&mut self, bytes: BytesIter<'a>, is_start: bool) -> &'a str {
        // Create arena string to hold unescaped identifier.
        // We don't know how long identifier will end up being. Take a guess that total length
        // will be double what we've seen so far, or `MIN_ESCAPED_STR_LEN` minimum.
        let len_so_far = bytes.as_ptr() as usize - self.remaining().as_ptr() as usize;
        let capacity = (len_so_far * 2).max(MIN_ESCAPED_STR_LEN);
        let mut str = String::with_capacity_in(capacity, self.allocator);

        // Push identifier up this point into `str`
        // `bumpalo::collections::string::String::push_str` is currently expensive due to
        // inefficiency in bumpalo's implementation. But best we have right now.
        str.push_str(&self.remaining()[0..len_so_far]);

        // Advance `self.current.chars` to after backslash
        self.current.chars = self.remaining()[len_so_far + 1..].chars();

        // Process escape and get rest of identifier
        self.identifier_after_backslash(str, is_start)
    }

    /// Process rest of identifier after a `\` found.
    ///
    /// `self.current.chars` should be positioned after the `\`,
    /// and `str` contain the identifier up to before the escape.
    ///
    /// `is_start` should be `true` if this is first char in the identifier,
    /// and `false` otherwise.
    fn identifier_after_backslash(&mut self, mut str: String<'a>, mut is_start: bool) -> &'a str {
        loop {
            // Consume escape sequence from `chars` and add char to `str`.
            // This advances `self.current.chars` to after end of escape sequence.
            // TODO: Move `identifier_unicode_escape_sequence` into this module?
            self.identifier_unicode_escape_sequence(&mut str, is_start);
            is_start = false;

            // Consume bytes until reach end of identifier or another escape.
            // NB: This does not advance `self.current.chars`, only `bytes`.
            let mut bytes = self.bytes_iter();
            let at_end = Self::identifier_tail_consume_until_end_or_escape(&mut bytes);
            if at_end {
                // Add bytes after last escape to `str`, and advance `chars` iterator to end of identifier.
                // `bumpalo::collections::string::String::push_str` is currently expensive due to
                // inefficiency in bumpalo's implementation. But best we have right now.
                let last_chunk = self.identifier_end(&bytes);
                str.push_str(last_chunk);
                break;
            }

            // Found another `\`.
            // Add bytes before this escape to `str` and advance `chars` iterator to after the `\`.
            let chunk_len = bytes.as_ptr() as usize - self.remaining().as_ptr() as usize;
            str.push_str(&self.remaining()[0..chunk_len]);
            self.current.chars = self.remaining()[chunk_len + 1..].chars();
        }

        // Convert `str` to arena slice and save to `escaped_strings`
        let text = str.into_bump_str();
        self.save_string(true, text);
        text
    }

    pub fn private_identifier(&mut self) -> Kind {
        let mut bytes = self.bytes_iter();
        if let Some(b) = bytes.peek() {
            if is_identifier_start_ascii_byte(b) {
                // Consume byte from `bytes`
                bytes.next();
                self.identifier_tail_after_no_escape(bytes);
                Kind::PrivateIdentifier
            } else {
                // Do not consume byte from `bytes`
                self.private_identifier_not_ascii_id()
            }
        } else {
            // EOF
            let start = self.offset();
            self.error(diagnostics::UnexpectedEnd(Span::new(start, start)));
            Kind::Undetermined
        }
    }

    #[cold]
    fn private_identifier_not_ascii_id(&mut self) -> Kind {
        let mut bytes = self.bytes_iter();
        let b = bytes.peek().unwrap();
        if !b.is_ascii() {
            let c = bytes.peek_char().unwrap();
            if is_identifier_start_unicode(c) {
                bytes.next_char().unwrap();
                self.identifier_tail_after_no_escape(bytes);
                return Kind::PrivateIdentifier;
            }
        } else if b == b'\\' {
            // Assume Unicode characters are more common than `\` escapes, so this branch `#[cold]`
            #[cold]
            fn backslash(lexer: &mut Lexer) -> Kind {
                lexer.identifier_backslash_handler();
                Kind::PrivateIdentifier
            }
            return backslash(self);
        }

        // No identifier found
        let start = self.offset();
        let c = self.consume_char();
        self.error(diagnostics::InvalidCharacter(c, Span::new(start, self.offset())));
        Kind::Undetermined
    }
}

#[allow(clippy::inline_always)]
#[inline(always)]
unsafe fn str_from_start_and_end<'a>(start: *const u8, end: *const u8) -> &'a str {
    let slice = std::slice::from_raw_parts(start, end as usize - start as usize);
    std::str::from_utf8_unchecked(slice)
}
