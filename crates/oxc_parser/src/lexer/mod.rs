//! An Ecma-262 Lexer / Tokenizer
//! Prior Arts:
//!     * [jsparagus](https://github.com/mozilla-spidermonkey/jsparagus/blob/master/crates/parser/src)
//!     * [rome](https://github.com/rome/tools/tree/main/crates/rome_js_parser/src/lexer)
//!     * [rustc](https://github.com/rust-lang/rust/blob/master/compiler/rustc_lexer/src)
//!     * [v8](https://v8.dev/blog/scanner)

mod byte_handlers;
mod comment;
mod identifier;
mod jsx;
mod kind;
mod number;
mod numeric;
mod punctuation;
mod regex;
mod source;
mod string;
mod string_builder;
mod template;
mod token;
mod trivia_builder;
mod typescript;
mod unicode;

use rustc_hash::FxHashMap;
use std::collections::VecDeque;

use oxc_allocator::Allocator;
use oxc_ast::ast::RegExpFlags;
use oxc_diagnostics::Error;
use oxc_span::{SourceType, Span};

use self::{
    byte_handlers::handle_byte,
    source::{Source, SourcePosition},
    string_builder::AutoCow,
    trivia_builder::TriviaBuilder,
};
pub use self::{
    kind::Kind,
    number::{parse_big_int, parse_float, parse_int},
    token::Token,
};
use crate::diagnostics;

#[derive(Debug, Clone, Copy)]
pub struct LexerCheckpoint<'a> {
    /// Current position in source
    position: SourcePosition<'a>,

    token: Token,

    errors_pos: usize,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum LexerContext {
    Regular,
    /// Lex the next token, returns `JsxString` or any other token
    JsxAttributeValue,
}

/// Wrapper around `Token`.
/// TODO: This serves no purpose and can be replaced with `Token`.
struct LexerCurrent {
    token: Token,
}

pub struct Lexer<'a> {
    allocator: &'a Allocator,

    // Wrapper around source text. Must not be changed after initialization.
    source: Source<'a>,

    source_type: SourceType,

    current: LexerCurrent,

    pub(crate) errors: Vec<Error>,

    lookahead: VecDeque<LexerCheckpoint<'a>>,

    context: LexerContext,

    pub(crate) trivia_builder: TriviaBuilder,

    /// Data store for escaped strings, indexed by [Token::start] when [Token::escaped] is true
    pub escaped_strings: FxHashMap<u32, &'a str>,

    /// Data store for escaped templates, indexed by [Token::start] when [Token::escaped] is true
    /// `None` is saved when the string contains an invalid escape sequence.
    pub escaped_templates: FxHashMap<u32, Option<&'a str>>,
}

#[allow(clippy::unused_self)]
impl<'a> Lexer<'a> {
    pub fn new(allocator: &'a Allocator, source_text: &'a str, source_type: SourceType) -> Self {
        let source = Source::new(source_text);

        // The first token is at the start of file, so is allows on a new line
        let token = Token::new_on_new_line();
        let current = LexerCurrent { token };
        Self {
            allocator,
            source,
            source_type,
            current,
            errors: vec![],
            lookahead: VecDeque::with_capacity(4), // 4 is the maximum lookahead for TypeScript
            context: LexerContext::Regular,
            trivia_builder: TriviaBuilder::default(),
            escaped_strings: FxHashMap::default(),
            escaped_templates: FxHashMap::default(),
        }
    }

    /// Remaining string from `Chars`
    pub fn remaining(&self) -> &'a str {
        self.source.remaining()
    }

    /// Creates a checkpoint storing the current lexer state.
    /// Use `rewind` to restore the lexer to the state stored in the checkpoint.
    pub fn checkpoint(&self) -> LexerCheckpoint<'a> {
        LexerCheckpoint {
            position: self.source.position(),
            token: self.current.token,
            errors_pos: self.errors.len(),
        }
    }

    /// Rewinds the lexer to the same state as when the passed in `checkpoint` was created.
    pub fn rewind(&mut self, checkpoint: LexerCheckpoint<'a>) {
        self.errors.truncate(checkpoint.errors_pos);
        self.source.set_position(checkpoint.position);
        self.current.token = checkpoint.token;
        self.lookahead.clear();
    }

    /// Find the nth lookahead token lazily
    pub fn lookahead(&mut self, n: u8) -> Token {
        let n = n as usize;
        debug_assert!(n > 0);

        if self.lookahead.len() > n - 1 {
            return self.lookahead[n - 1].token;
        }

        let token = self.current.token;
        let position = self.source.position();

        if let Some(checkpoint) = self.lookahead.back() {
            self.source.set_position(checkpoint.position);
        }

        // Reset the current token for `read_next_token`
        // TODO: Is this still required?
        self.current.token = Token::default();

        let mut peeked = Token::default();
        for _i in self.lookahead.len()..n {
            let kind = self.read_next_token();
            peeked = self.finish_next(kind);
            self.lookahead.push_back(LexerCheckpoint {
                position: self.source.position(),
                token: peeked,
                errors_pos: self.errors.len(),
            });
        }

        self.current.token = token;
        self.source.set_position(position);

        peeked
    }

    /// Set context
    pub fn set_context(&mut self, context: LexerContext) {
        self.context = context;
    }

    /// Main entry point
    pub fn next_token(&mut self) -> Token {
        if let Some(checkpoint) = self.lookahead.pop_front() {
            self.source.set_position(checkpoint.position);
            return checkpoint.token;
        }
        let kind = self.read_next_token();
        self.finish_next(kind)
    }

    fn finish_next(&mut self, kind: Kind) -> Token {
        self.current.token.kind = kind;
        self.current.token.end = self.offset();
        debug_assert!(self.current.token.start <= self.current.token.end);
        let token = self.current.token;
        self.current.token = Token::default();
        token
    }

    // ---------- Private Methods ---------- //
    fn error<T: Into<Error>>(&mut self, error: T) {
        self.errors.push(error.into());
    }

    /// Get the length offset from the source, in UTF-8 bytes
    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn offset(&self) -> u32 {
        self.source.offset()
    }

    /// Get the current unterminated token range
    fn unterminated_range(&self) -> Span {
        Span::new(self.current.token.start, self.offset())
    }

    /// Consume the current char if not at EOF
    #[inline]
    fn next_char(&mut self) -> Option<char> {
        self.source.next_char()
    }

    /// Consume the current char
    #[inline]
    fn consume_char(&mut self) -> char {
        self.source.next_char().unwrap()
    }

    /// Peek the next char without advancing the position
    #[inline]
    fn peek(&self) -> Option<char> {
        self.source.peek_char()
    }

    /// Peek the next next char without advancing the position
    #[inline]
    fn peek2(&self) -> Option<char> {
        let mut source = self.source.clone();
        source.next_char();
        source.next_char()
    }

    /// Peek the next character, and advance the current position if it matches
    #[inline]
    fn next_eq(&mut self, c: char) -> bool {
        let matched = self.peek() == Some(c);
        if matched {
            self.source.next_char().unwrap();
        }
        matched
    }

    fn current_offset(&self) -> Span {
        let offset = self.offset();
        Span::new(offset, offset)
    }

    /// Return `IllegalCharacter` Error or `UnexpectedEnd` if EOF
    fn unexpected_err(&mut self) {
        let offset = self.current_offset();
        match self.peek() {
            Some(c) => self.error(diagnostics::InvalidCharacter(c, offset)),
            None => self.error(diagnostics::UnexpectedEnd(offset)),
        }
    }

    /// Read each char and set the current token
    /// Whitespace and line terminators are skipped
    fn read_next_token(&mut self) -> Kind {
        loop {
            let offset = self.offset();
            self.current.token.start = offset;

            let byte = if let Some(byte) = self.source.peek_byte() {
                byte
            } else {
                return Kind::Eof;
            };

            // SAFETY: `byte` is byte value at current position in source
            let kind = unsafe { handle_byte(byte, self) };
            if kind != Kind::Skip {
                return kind;
            }
        }
    }
}
