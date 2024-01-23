use super::{Kind, Lexer, SurrogatePair};
use crate::diagnostics;
use std::str::Bytes;

use oxc_allocator::String;
use oxc_span::Span;
use oxc_syntax::identifier::{
    is_identifier_part, is_identifier_part_ascii_byte, is_identifier_part_unicode,
    is_identifier_start, is_identifier_start_ascii_byte, is_identifier_start_unicode,
};

#[allow(clippy::unnecessary_safety_comment)]
/// Entry point from ASCII byte handlers.
///
/// Handle identifier starting with an ASCII char.
/// Start character should not be consumed from `lexer.current.chars` prior to calling this.
///
/// This is designed to have a fast path for the common case of identifiers which are entirely ASCII
/// characters with no escapes.
///
/// SAFETY:
/// * Must not be at EOF.
/// * Next char in `lexer.current.chars` must be ASCII.
//
// TODO: Can we get a gain by avoiding returning slice if it's not used (IDT handler)?
pub unsafe fn handle_ascii_start<'a>(lexer: &mut Lexer<'a>) -> &'a str {
    // Advance `bytes` forward 1 byte, past the 1st char, which caller guarantees is ASCII
    let bytes = lexer.remaining().get_unchecked(1..).bytes();
    let mut id_lexer = IdentifierLexer { lexer, bytes };
    let text = id_lexer.tail_unescaped();

    // Return identifier minus its first character.
    // Caller guaranteed first char was ASCII.
    // Everything we've done since guarantees this is safe.
    // TODO: Write this comment better!
    text.get_unchecked(1..)
}

/// Entry point from Unicode byte handler.
///
/// Handle identifier starting with a Unicode char.
/// Start character should not be consumed from `lexer.current.chars` prior to calling this.
pub fn handle_unicode_start(lexer: &mut Lexer) {
    // `bytes` is positioned after this char
    let mut chars = lexer.current.chars.clone();
    chars.next();
    let bytes = chars.as_str().bytes();
    let mut id_lexer = IdentifierLexer { lexer, bytes };
    id_lexer.tail_unescaped();
}

/// Entry point from `\` byte handler.
///
/// Handle identifier starting with an escape.
/// Next char should be `\`.
/// It should not be consumed from `lexer.current.chars` prior to calling this.
pub fn handle_backslash<'a>(lexer: &mut Lexer<'a>) -> &'a str {
    let bytes = lexer.remaining().bytes();
    let mut id_lexer = IdentifierLexer { lexer, bytes };
    id_lexer.backslash(true)
}

/// Entry point for private identifier.
///
/// Handle private identifier.
/// `#` character should be consumed before calling this. Next char can be anything.
pub fn handle_private(lexer: &mut Lexer) -> Kind {
    let bytes = lexer.remaining().bytes();
    let mut id_lexer = IdentifierLexer { lexer, bytes };
    id_lexer.private()
}

struct IdentifierLexer<'a, 'b> {
    lexer: &'b mut Lexer<'a>,
    bytes: Bytes<'a>,
}

impl<'a, 'b> IdentifierLexer<'a, 'b> {
    // ---------- Fast path ---------- //

    /// Handle identifier after 1st char dealt with.
    ///
    /// 1st char can have been ASCII or Unicode, but cannot have been a `\` escape.
    /// 1st character should not be consumed from `lexer.current.chars` prior to calling this,
    /// but `bytes` iterator should be positioned *after* 1st char.
    ///
    /// `#[inline]` because we want this inlined into `handle_ascii_start`,
    /// which is the fast path for common case.
    #[inline]
    fn tail_unescaped(&mut self) -> &'a str {
        // Find first byte which isn't valid ASCII identifier part
        let next_byte = match self.consume_ascii_identifier_bytes() {
            Some(b) => b,
            None => {
                return self.eof();
            }
        };

        // Handle the byte which isn't ASCII identifier part.
        // Most likely we're at the end of the identifier, but handle `\` escape and Unicode chars.
        // Fast path for normal ASCII identifiers, by marking the 2 uncommon cases `#[cold]`.
        if next_byte == b'\\' {
            self.backslash(false)
        } else if !next_byte.is_ascii() {
            self.tail_unicode_byte()
        } else {
            // End of identifier found.
            // Advance chars iterator to the byte we just found which isn't part of the identifier.
            self.end()
        }
    }

    /// Consume bytes from `Bytes` iterator which are ASCII identifier part bytes.
    /// `bytes` iterator is left positioned on next non-matching byte.
    /// Returns next non-matching byte, or `None` if EOF.
    ///
    /// `#[inline]` because we want this inlined into `tail_unescaped`,
    /// which is on the fast path for common cases.
    #[inline]
    fn consume_ascii_identifier_bytes(&mut self) -> Option<u8> {
        loop {
            match self.peek_byte() {
                Some(b) => {
                    if !is_identifier_part_ascii_byte(b) {
                        return Some(b);
                    }
                    // TODO: Would `.unwrap_unchecked()` help here?
                    self.bytes.next();
                }
                None => {
                    return None;
                }
            }
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.clone().next()
    }

    /// End of identifier found.
    /// `bytes` iterator must be positioned on next byte after end of identifier.
    ///
    /// `#[inline]` because we want this inlined into `tail_unescaped`,
    /// which is on the fast path for common case.
    #[inline]
    fn end(&mut self) -> &'a str {
        let remaining = self.remaining();
        let len = remaining.len() - self.bytes.len();
        // SAFETY: Only safe if `lexer.remaining().as_bytes()[lexer.remaining.len() - bytes.len()]`
        // is a UTF-8 character boundary, and within bounds of `self.remaining()`
        // TODO: Explain better why this is safe!
        // TODO: Or maybe only use unsafe in `tail_unescaped` where correctness is easier to prove?
        unsafe {
            self.lexer.current.chars = remaining.get_unchecked(len..).chars();
            remaining.get_unchecked(..len)
        }
    }

    // ---------- Slow path ---------- //

    /// Identifier end at EOF.
    /// Return text of identifier, and advance `lexer.current.chars` to end of file.
    ///
    /// NB: This could be replaced with `end()` in `tail_unescaped`, but doing that
    /// causes a 3% drop in lexer benchmarks. Maybe because `end()` is marked `#[inline]`
    /// and we don't want this inlined because it's a rare case?
    fn eof(&mut self) -> &'a str {
        let text = self.remaining();
        self.lexer.current.chars = text[text.len()..].chars();
        text
    }

    /// Handle continuation of identifier when a Unicode char found.
    ///
    /// Any number of characters can have already been eaten from `bytes` iterator prior to this.
    /// `bytes` iterator should be positioned at *start* of Unicode character.
    /// Nothing should have been consumed from `lexer.current.chars` prior to calling this.
    ///
    /// `#[cold]` to guide branch predictor that Unicode chars in identifiers are rare.
    #[cold]
    fn tail_unicode_byte(&mut self) -> &'a str {
        let at_end = self.consume_unicode_char_if_identifier_part();
        if !at_end {
            let at_end = self.consume_until_end_or_escape();
            if !at_end {
                return self.backslash(false);
            }
        }

        self.end()
    }

    /// Consume unicode character from `bytes` if it's part of identifier.
    /// Returns `true` if at end of identifier (this character is not part of identifier)
    /// or `false` if character was consumed and potentially more of identifier still to come.
    fn consume_unicode_char_if_identifier_part(&mut self) -> bool {
        let remaining = &self.remaining()[self.remaining().len() - self.bytes.len()..];
        let mut chars = remaining.chars();
        let c = chars.next().unwrap();
        if is_identifier_part_unicode(c) {
            // Advance `bytes` iterator past this character
            self.bytes = chars.as_str().bytes();
            return false;
        }
        // Reached end of identifier
        true
    }

    /// Consume valid identifier bytes (ASCII or Unicode) from `bytes`
    /// until reach end of identifier or a `\`.
    /// Returns `true` if at end of identifier, or `false` if found `\`.
    fn consume_until_end_or_escape(&mut self) -> bool {
        loop {
            // Eat ASCII identifier bytes from `bytes`
            let next_byte = match self.consume_ascii_identifier_bytes() {
                Some(b) => b,
                None => {
                    return true;
                }
            };

            if next_byte.is_ascii() {
                // ASCII byte which either isn't part of identifier, or is a `\`
                return next_byte != b'\\';
            }

            // Unicode char
            let at_end = self.consume_unicode_char_if_identifier_part();
            if at_end {
                return true;
            }
            // Char was part of identifier. Keep eating.
        }
    }

    /// Handle identifier after a `\` found.
    ///
    /// Any number of characters (or none) can have been eaten from `bytes` iterator prior to the `\`.
    /// `\` byte must not have been eaten from `bytes`.
    ///
    /// None of the identifier should have been consumed from `lexer.current.chars` prior to calling this.
    /// `is_start` should be `true` if this is 1st char of identifier, `false` otherwise.
    ///
    /// `#[cold]` to guide branch predictor that escapes in identifiers are rare and keep a fast path
    /// in `tail_unescaped` for the common case.
    #[cold]
    fn backslash(&mut self, mut is_start: bool) -> &'a str {
        // All the other identifier lexer functions only iterate through `bytes`,
        // leaving `lexer.current.chars` unchanged until the end of the identifier is found.
        // But at this point, after finding an escape, we change approach.
        //
        // In this function, the unescaped identifier is built up in an arena `String`.
        // Each time an escape is found, all the previous non-escaped bytes are pushed into the `String`
        // and `chars` iterator advanced to after the escape sequence.
        // We then search again for another run of unescaped bytes, and push them to the `String`.
        // If another escape is found, loop back to the start and do same again.
        // At the end, push any non-escaped bytes after the last escape to the `String`.

        // Create an arena string to hold unescaped identifier.
        // We don't know how long identifier will end up being. Take a guess that total length
        // will be double what we've seen so far, or 16 minimum.
        const MIN_LEN: usize = 16;
        let len = self.remaining().len() - self.bytes.len();
        let capacity = (len * 2).max(MIN_LEN);
        let mut str = String::with_capacity_in(capacity, self.lexer.allocator);

        loop {
            // Add bytes before this escape to `str` and advance `chars` iterator to after the `\`
            let len = self.remaining().len() - self.bytes.len();
            str.push_str(&self.remaining()[0..len]);
            self.lexer.current.chars = self.remaining()[len + 1..].chars();

            // Consume escape sequence from `chars` and add char to `str`
            self.identifier_unicode_escape_sequence(&mut str, is_start);
            is_start = false;

            // Bring `bytes` iterator back into sync with `chars` iterator.
            // i.e. advance `bytes` to after the escape sequence.
            self.bytes = self.remaining().bytes();

            // Consume bytes until reach end of identifier or another escape
            let at_end = self.consume_until_end_or_escape();
            if at_end {
                break;
            }
            // Found another `\` escape
        }

        // Add rest of identifier after last escape to `str`, and advance `chars` iterator to end of identifier
        let last_chunk = self.end();
        str.push_str(last_chunk);

        // Convert to arena slice and save to `escaped_strings`
        let text = str.into_bump_str();
        self.lexer.save_string(true, text);
        text
    }

    /// Identifier `UnicodeEscapeSequence`
    ///   \u `Hex4Digits`
    ///   \u{ `CodePoint` }
    fn identifier_unicode_escape_sequence(
        &mut self,
        str: &mut String,
        check_identifier_start: bool,
    ) {
        let start = self.offset();
        if self.lexer.current.chars.next() != Some('u') {
            let range = Span::new(start, self.offset());
            self.lexer.error(diagnostics::UnicodeEscapeSequence(range));
            return;
        }

        let value = match self.lexer.peek() {
            Some('{') => self.lexer.unicode_code_point(),
            _ => self.lexer.surrogate_pair(),
        };

        let Some(value) = value else {
            let range = Span::new(start, self.offset());
            self.lexer.error(diagnostics::UnicodeEscapeSequence(range));
            return;
        };

        // For Identifiers, surrogate pair is an invalid grammar, e.g. `var \uD800\uDEA7`.
        let ch = match value {
            SurrogatePair::Astral(..) | SurrogatePair::HighLow(..) => {
                let range = Span::new(start, self.offset());
                self.lexer.error(diagnostics::UnicodeEscapeSequence(range));
                return;
            }
            SurrogatePair::CodePoint(code_point) => {
                if let Ok(ch) = char::try_from(code_point) {
                    ch
                } else {
                    let range = Span::new(start, self.offset());
                    self.lexer.error(diagnostics::UnicodeEscapeSequence(range));
                    return;
                }
            }
        };

        let is_valid =
            if check_identifier_start { is_identifier_start(ch) } else { is_identifier_part(ch) };

        if !is_valid {
            self.lexer.error(diagnostics::InvalidCharacter(ch, self.lexer.current_offset()));
            return;
        }

        str.push(ch);
    }

    // ---------- Forward calls to lexer ---------- //

    fn remaining(&self) -> &'a str {
        self.lexer.remaining()
    }

    fn offset(&self) -> u32 {
        self.lexer.offset()
    }

    // ---------- Private identifiers ---------- //

    fn private(&mut self) -> Kind {
        if let Some(b) = self.peek_byte() {
            if is_identifier_start_ascii_byte(b) {
                // Consume byte from `bytes`
                self.bytes.next();
                self.tail_unescaped();
                Kind::PrivateIdentifier
            } else {
                // Do not consume byte from `bytes`
                self.private_not_valid_ascii_first_byte()
            }
        } else {
            let start = self.offset();
            self.lexer.error(diagnostics::UnexpectedEnd(Span::new(start, start)));
            Kind::Undetermined
        }
    }

    #[cold]
    fn private_not_valid_ascii_first_byte(&mut self) -> Kind {
        let b = self.peek_byte().unwrap();
        if b == b'\\' {
            // Do not consume `\` byte from `bytes`
            self.backslash(true);
            return Kind::PrivateIdentifier;
        }

        if !b.is_ascii() {
            let mut chars = self.lexer.current.chars.clone();
            let c = chars.next().unwrap();
            if is_identifier_start_unicode(c) {
                // Char has been eaten from `bytes` (but not from `lexer.current.chars`)
                self.bytes = chars.as_str().bytes();
                self.tail_unescaped();
                return Kind::PrivateIdentifier;
            }
        };

        let start = self.offset();
        let c = self.lexer.consume_char();
        self.lexer.error(diagnostics::InvalidCharacter(c, Span::new(start, self.offset())));
        Kind::Undetermined
    }
}
