use super::{
    search::{byte_search, safe_byte_match_table, SafeByteMatchTable},
    Kind, Lexer, SourcePosition,
};

static NOT_WHITESPACE_OR_REGULAR_LINE_BREAK_TABLE: SafeByteMatchTable =
    safe_byte_match_table!(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'));

impl<'a> Lexer<'a> {
    pub(super) fn line_break_handler(&mut self) -> Kind {
        self.token.is_on_new_line = true;

        // Indentation is common after a line break.
        // Consume it, along with any further line breaks.
        // Irregular line breaks are not handled here.
        // They're uncommon, so leave them for the next call to `handle_byte` to take care of.
        byte_search! {
            lexer: self,
            table: NOT_WHITESPACE_OR_REGULAR_LINE_BREAK_TABLE,
            ret_type: Kind,
            handle_match: |_lexer, next_byte, _after_first| {
                Kind::Skip
            },
            handle_eof: |_lexer, _after_first| {
                Kind::Eof
            },
        };
    }
}
