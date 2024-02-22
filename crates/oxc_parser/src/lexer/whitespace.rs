use super::{search::byte_search, Kind, Lexer};

struct NotWhitespaceMatcher;
impl NotWhitespaceMatcher {
    #[allow(clippy::unused_self)]
    #[inline]
    pub const fn use_table(&self) {}

    #[allow(clippy::unused_self)]
    #[inline]
    pub const fn is_table(&self) -> bool {
        false
    }

    #[allow(clippy::unused_self)]
    #[inline]
    pub const fn matches(&self, b: u8) -> bool {
        !matches!(b, b' ' | b'\t')
    }
}

impl<'a> Lexer<'a> {
    pub(super) fn line_break_handler(&mut self) -> Kind {
        self.token.is_on_new_line = true;

        // Indentation is common after a line break.
        // Consume it, along with any further line breaks.
        // Irregular line breaks and whitespace are not consumed.
        // They're uncommon, so leave them for the next call to `handle_byte` to take care of.
        byte_search! {
            lexer: self,
            table: NotWhitespaceMatcher,
            continue_if: |matched_byte, _pos| {
                // TODO: Branchlessly consume a following `\n`.
                // Common after `\r`, and double line breaks are also not so rare.
                matches!(matched_byte, b'\r' | b'\n')
            },
            handle_match: |_next_byte, _start| {
                Kind::Skip
            },
            handle_eof: |_start| {
                Kind::Skip
            },
        };
    }
}
