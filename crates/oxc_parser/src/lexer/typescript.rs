use super::{Kind, Lexer, Token};

impl<'a> Lexer<'a> {
    /// Re-tokenize '<<' or '<=' or '<<=' to '<'
    pub(crate) fn re_lex_as_typescript_l_angle(&mut self, kind: Kind) -> Token {
        let offset = match kind {
            Kind::ShiftLeft | Kind::LtEq => 2,
            Kind::ShiftLeftEq => 3,
            _ => unreachable!(),
        };
        self.current.token.start = self.offset() - offset;
        // TODO: Is it better to use `set_offset()`?
        self.source.back(offset as usize - 1);
        let kind = Kind::LAngle;
        self.lookahead.clear();
        self.finish_next(kind)
    }
}
