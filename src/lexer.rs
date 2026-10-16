use crate::EngineError;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TokenKind<'source> {
    Identifier(&'source str),
    Number(f64),
    String(String),
    Operator(&'source str),
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Dot,
    Colon,
    Semicolon,
    Comma,
    Eof,
}

#[derive(Clone, Debug)]
pub(crate) struct Token<'source> {
    pub kind: TokenKind<'source>,
    pub line: usize,
    pub column: usize,
}

pub(crate) struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    offset: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token<'a>>, EngineError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia()?;
            let line = self.line;
            let column = self.column;
            let Some(byte) = self.peek() else {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    line,
                    column,
                });
                return Ok(tokens);
            };

            let kind = match byte {
                b'(' => self.single(TokenKind::LeftParen),
                b')' => self.single(TokenKind::RightParen),
                b'{' => self.single(TokenKind::LeftBrace),
                b'}' => self.single(TokenKind::RightBrace),
                b'[' => self.single(TokenKind::LeftBracket),
                b']' => self.single(TokenKind::RightBracket),
                b'.' => self.single(TokenKind::Dot),
                b':' => self.single(TokenKind::Colon),
                b';' => self.single(TokenKind::Semicolon),
                b',' => self.single(TokenKind::Comma),
                b'\'' | b'"' => TokenKind::String(self.read_string(byte, line, column)?),
                b'0'..=b'9' => TokenKind::Number(self.read_number(line, column)?),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                    TokenKind::Identifier(self.read_identifier())
                }
                b'+' | b'-' | b'*' | b'/' | b'%' | b'!' | b'=' | b'<' | b'>' | b'&' | b'|'
                | b'?' | b'^' | b'~' => TokenKind::Operator(self.read_operator(line, column)?),
                _ => {
                    return Err(EngineError::new(
                        "SyntaxError",
                        format!("Unexpected character '{}'", byte as char),
                        line,
                        column,
                    ));
                }
            };
            tokens.push(Token { kind, line, column });
        }
    }

    fn single(&mut self, kind: TokenKind<'a>) -> TokenKind<'a> {
        self.advance();
        kind
    }

    fn skip_trivia(&mut self) -> Result<(), EngineError> {
        loop {
            match (self.peek(), self.peek_next()) {
                (Some(b' ' | b'\t'), _) => {
                    let length = horizontal_whitespace_prefix(&self.bytes[self.offset..]);
                    self.offset += length;
                    self.column += length;
                }
                (Some(b'\r' | b'\n'), _) => {
                    self.advance();
                }
                (Some(b'/'), Some(b'/')) => {
                    while !matches!(self.peek(), None | Some(b'\n')) {
                        self.advance();
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    let line = self.line;
                    let column = self.column;
                    self.advance();
                    self.advance();
                    loop {
                        match (self.peek(), self.peek_next()) {
                            (Some(b'*'), Some(b'/')) => {
                                self.advance();
                                self.advance();
                                break;
                            }
                            (None, _) => {
                                return Err(EngineError::new(
                                    "SyntaxError",
                                    "Unterminated block comment",
                                    line,
                                    column,
                                ));
                            }
                            _ => {
                                self.advance();
                            }
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn read_string(
        &mut self,
        quote: u8,
        line: usize,
        column: usize,
    ) -> Result<String, EngineError> {
        self.advance();
        let mut result = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n' | b'\r') => {
                    return Err(EngineError::new(
                        "SyntaxError",
                        "Unterminated string literal",
                        line,
                        column,
                    ));
                }
                Some(byte) if byte == quote => {
                    self.advance();
                    return Ok(result);
                }
                Some(b'\\') => {
                    self.advance();
                    let Some(escaped) = self.peek() else {
                        return Err(EngineError::new(
                            "SyntaxError",
                            "Unterminated escape sequence",
                            line,
                            column,
                        ));
                    };
                    self.advance();
                    result.push(match escaped {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'\\' => '\\',
                        b'\'' => '\'',
                        b'"' => '"',
                        other => other as char,
                    });
                }
                Some(byte) if byte.is_ascii() => {
                    result.push(byte as char);
                    self.advance();
                }
                Some(_) => {
                    let character = self.source[self.offset..].chars().next().unwrap();
                    result.push(character);
                    self.advance_char(character);
                }
            }
        }
    }

    fn read_number(&mut self, line: usize, column: usize) -> Result<f64, EngineError> {
        let start = self.offset;
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
        ) {
            let current = self.peek().unwrap();
            if (current == b'+' || current == b'-')
                && !matches!(
                    self.bytes.get(self.offset.wrapping_sub(1)),
                    Some(b'e' | b'E')
                )
            {
                break;
            }
            self.advance();
        }
        let text = &self.source[start..self.offset];
        text.parse().map_err(|_| {
            EngineError::new(
                "SyntaxError",
                format!("Invalid numeric literal '{text}'"),
                line,
                column,
            )
        })
    }

    fn read_identifier(&mut self) -> &'a str {
        let start = self.offset;
        let length = identifier_continue_prefix(&self.bytes[start..]);
        self.offset += length;
        self.column += length;
        &self.source[start..self.offset]
    }

    fn read_operator(&mut self, line: usize, column: usize) -> Result<&'a str, EngineError> {
        const OPERATORS: [&str; 30] = [
            ">>>=", "===", "!==", "&&=", "||=", "??=", "**=", ">>>", ">>=", "<<=", "**", "==",
            "!=", "<=", ">=", "&&", "||", "??", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=",
            "|=", "^=", "<<", ">>",
        ];
        const OTHER: [&str; 10] = ["=", "<", ">", "!", "+", "-", "*", "&", "|", "?"];
        let start = self.offset;
        for operator in OPERATORS.iter().chain(OTHER.iter()) {
            if self.source[self.offset..].starts_with(operator) {
                for _ in operator.bytes() {
                    self.advance();
                }
                return Ok(&self.source[start..self.offset]);
            }
        }
        Err(EngineError::new(
            "SyntaxError",
            "Unsupported operator",
            line,
            column,
        ))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn peek_next(&self) -> Option<u8> {
        self.bytes.get(self.offset + 1).copied()
    }

    fn advance(&mut self) {
        if let Some(byte) = self.peek() {
            self.offset += 1;
            if byte == b'\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
    }

    fn advance_char(&mut self, character: char) {
        self.offset += character.len_utf8();
        self.column += 1;
    }
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

fn horizontal_whitespace_prefix(bytes: &[u8]) -> usize {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: runtime feature detection guarantees AVX2 support.
        return unsafe { horizontal_whitespace_prefix_avx2(bytes) };
    }

    #[cfg(target_arch = "aarch64")]
    {
        // AArch64 guarantees Advanced SIMD (NEON) support.
        return unsafe { horizontal_whitespace_prefix_neon(bytes) };
    }

    #[allow(unreachable_code)]
    bytes
        .iter()
        .take_while(|&&byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn identifier_continue_prefix(bytes: &[u8]) -> usize {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: runtime feature detection guarantees AVX2 support.
        return unsafe { identifier_continue_prefix_avx2(bytes) };
    }

    #[cfg(target_arch = "aarch64")]
    {
        // AArch64 guarantees Advanced SIMD (NEON) support.
        return unsafe { identifier_continue_prefix_neon(bytes) };
    }

    #[allow(unreachable_code)]
    bytes
        .iter()
        .take_while(|&&byte| is_identifier_continue(byte))
        .count()
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn identifier_continue_prefix_avx2(bytes: &[u8]) -> usize {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    let mut offset = 0;
    let lower_bound = _mm256_set1_epi8((b'a' - 1) as i8);
    let lower_limit = _mm256_set1_epi8((b'z' + 1) as i8);
    let upper_bound = _mm256_set1_epi8((b'A' - 1) as i8);
    let upper_limit = _mm256_set1_epi8((b'Z' + 1) as i8);
    let digit_bound = _mm256_set1_epi8((b'0' - 1) as i8);
    let digit_limit = _mm256_set1_epi8((b'9' + 1) as i8);
    let underscore = _mm256_set1_epi8(b'_' as i8);
    let dollar = _mm256_set1_epi8(b'$' as i8);

    while offset + 32 <= bytes.len() {
        // SAFETY: the loop ensures at least 32 readable bytes remain.
        let input = unsafe { _mm256_loadu_si256(bytes.as_ptr().add(offset).cast()) };
        let lowercase = _mm256_and_si256(
            _mm256_cmpgt_epi8(input, lower_bound),
            _mm256_cmpgt_epi8(lower_limit, input),
        );
        let uppercase = _mm256_and_si256(
            _mm256_cmpgt_epi8(input, upper_bound),
            _mm256_cmpgt_epi8(upper_limit, input),
        );
        let digits = _mm256_and_si256(
            _mm256_cmpgt_epi8(input, digit_bound),
            _mm256_cmpgt_epi8(digit_limit, input),
        );
        let punctuation = _mm256_or_si256(
            _mm256_cmpeq_epi8(input, underscore),
            _mm256_cmpeq_epi8(input, dollar),
        );
        let valid = _mm256_or_si256(
            _mm256_or_si256(lowercase, uppercase),
            _mm256_or_si256(digits, punctuation),
        );
        let valid_mask = _mm256_movemask_epi8(valid) as u32;
        if valid_mask != u32::MAX {
            return offset + (!valid_mask).trailing_zeros() as usize;
        }
        offset += 32;
    }

    offset
        + bytes[offset..]
            .iter()
            .take_while(|&&byte| is_identifier_continue(byte))
            .count()
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn horizontal_whitespace_prefix_avx2(bytes: &[u8]) -> usize {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    let mut offset = 0;
    let space = _mm256_set1_epi8(b' ' as i8);
    let tab = _mm256_set1_epi8(b'\t' as i8);

    while offset + 32 <= bytes.len() {
        // SAFETY: the loop ensures at least 32 readable bytes remain.
        let input = unsafe { _mm256_loadu_si256(bytes.as_ptr().add(offset).cast()) };
        let whitespace = _mm256_or_si256(
            _mm256_cmpeq_epi8(input, space),
            _mm256_cmpeq_epi8(input, tab),
        );
        let whitespace_mask = _mm256_movemask_epi8(whitespace) as u32;
        if whitespace_mask != u32::MAX {
            return offset + (!whitespace_mask).trailing_zeros() as usize;
        }
        offset += 32;
    }

    offset
        + bytes[offset..]
            .iter()
            .take_while(|&&byte| matches!(byte, b' ' | b'\t'))
            .count()
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn horizontal_whitespace_prefix_neon(bytes: &[u8]) -> usize {
    use std::arch::aarch64::*;

    let mut offset = 0;
    let space = vdupq_n_u8(b' ');
    let tab = vdupq_n_u8(b'\t');
    let mut lanes = [0_u8; 16];

    while offset + 16 <= bytes.len() {
        // SAFETY: the loop ensures at least 16 readable bytes remain.
        let input = unsafe { vld1q_u8(bytes.as_ptr().add(offset)) };
        let whitespace = vorrq_u8(vceqq_u8(input, space), vceqq_u8(input, tab));
        // SAFETY: `lanes` has space for exactly 16 bytes.
        unsafe { vst1q_u8(lanes.as_mut_ptr(), whitespace) };
        if let Some(invalid_lane) = lanes.iter().position(|&lane| lane == 0) {
            return offset + invalid_lane;
        }
        offset += 16;
    }

    offset
        + bytes[offset..]
            .iter()
            .take_while(|&&byte| matches!(byte, b' ' | b'\t'))
            .count()
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn identifier_continue_prefix_neon(bytes: &[u8]) -> usize {
    use std::arch::aarch64::*;

    let mut offset = 0;
    let lower_bound = vdupq_n_u8(b'a');
    let lower_limit = vdupq_n_u8(b'z');
    let upper_bound = vdupq_n_u8(b'A');
    let upper_limit = vdupq_n_u8(b'Z');
    let digit_bound = vdupq_n_u8(b'0');
    let digit_limit = vdupq_n_u8(b'9');
    let underscore = vdupq_n_u8(b'_');
    let dollar = vdupq_n_u8(b'$');
    let mut lanes = [0_u8; 16];

    while offset + 16 <= bytes.len() {
        // SAFETY: the loop ensures at least 16 readable bytes remain.
        let input = unsafe { vld1q_u8(bytes.as_ptr().add(offset)) };
        let lowercase = vandq_u8(vcgeq_u8(input, lower_bound), vcleq_u8(input, lower_limit));
        let uppercase = vandq_u8(vcgeq_u8(input, upper_bound), vcleq_u8(input, upper_limit));
        let digits = vandq_u8(vcgeq_u8(input, digit_bound), vcleq_u8(input, digit_limit));
        let punctuation = vorrq_u8(vceqq_u8(input, underscore), vceqq_u8(input, dollar));
        let valid = vorrq_u8(
            vorrq_u8(lowercase, uppercase),
            vorrq_u8(digits, punctuation),
        );
        // SAFETY: `lanes` has space for exactly 16 bytes.
        unsafe { vst1q_u8(lanes.as_mut_ptr(), valid) };
        if let Some(invalid_lane) = lanes.iter().position(|&lane| lane == 0) {
            return offset + invalid_lane;
        }
        offset += 16;
    }

    offset
        + bytes[offset..]
            .iter()
            .take_while(|&&byte| is_identifier_continue(byte))
            .count()
}

#[cfg(test)]
mod tests {
    use super::{Lexer, TokenKind, horizontal_whitespace_prefix, identifier_continue_prefix};

    #[test]
    fn identifier_scanner_stops_at_every_vector_boundary() {
        for stop in 0..96 {
            let mut bytes = vec![b'a'; 96];
            bytes[stop] = b'@';
            assert_eq!(identifier_continue_prefix(&bytes), stop);
        }
        let bytes = [b"aZ9_$".repeat(40), b"!".to_vec()].concat();
        assert_eq!(identifier_continue_prefix(&bytes), 200);
        assert_eq!(identifier_continue_prefix(b"volt_engine42$tail!"), 18);
        assert_eq!(identifier_continue_prefix("nameé".as_bytes()), 4);
        let unicode_after_vector = format!("{}é", "a".repeat(31));
        assert_eq!(
            identifier_continue_prefix(unicode_after_vector.as_bytes()),
            31
        );
    }

    #[test]
    fn whitespace_scanner_stops_at_every_vector_boundary() {
        for stop in 0..96 {
            let mut bytes = vec![b' '; 96];
            bytes[stop] = b'x';
            assert_eq!(horizontal_whitespace_prefix(&bytes), stop);
        }
        assert_eq!(horizontal_whitespace_prefix(b" \t \tname"), 4);
    }

    #[test]
    fn identifier_and_operator_tokens_borrow_the_source() {
        let source = "volt+=speed";
        let tokens = Lexer::new(source).tokenize().unwrap();
        let TokenKind::Identifier(identifier) = &tokens[0].kind else {
            panic!("expected identifier token");
        };
        let TokenKind::Operator(operator) = &tokens[1].kind else {
            panic!("expected operator token");
        };

        assert_eq!(*identifier, "volt");
        assert_eq!(*operator, "+=");
        assert!(std::ptr::eq(identifier.as_ptr(), source.as_ptr()));
        assert!(std::ptr::eq(operator.as_ptr(), source[4..].as_ptr()));
    }
}
