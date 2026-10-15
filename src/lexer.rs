use crate::EngineError;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TokenKind {
    Identifier(String),
    Number(f64),
    String(String),
    Operator(String),
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    Semicolon,
    Comma,
    Eof,
}

#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub kind: TokenKind,
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

    pub fn tokenize(mut self) -> Result<Vec<Token>, EngineError> {
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
                b';' => self.single(TokenKind::Semicolon),
                b',' => self.single(TokenKind::Comma),
                b'\'' | b'"' => TokenKind::String(self.read_string(byte, line, column)?),
                b'0'..=b'9' => TokenKind::Number(self.read_number(line, column)?),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                    TokenKind::Identifier(self.read_identifier())
                }
                b'+' | b'-' | b'*' | b'/' | b'%' | b'!' | b'=' | b'<' | b'>' | b'&' | b'|'
                | b'?' => TokenKind::Operator(self.read_operator(line, column)?),
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

    fn single(&mut self, kind: TokenKind) -> TokenKind {
        self.advance();
        kind
    }

    fn skip_trivia(&mut self) -> Result<(), EngineError> {
        loop {
            match (self.peek(), self.peek_next()) {
                (Some(b' ' | b'\t' | b'\r' | b'\n'), _) => {
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

    fn read_identifier(&mut self) -> String {
        let start = self.offset;
        while matches!(
            self.peek(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
        ) {
            self.advance();
        }
        self.source[start..self.offset].to_owned()
    }

    fn read_operator(&mut self, line: usize, column: usize) -> Result<String, EngineError> {
        const OPERATORS: [&str; 14] = [
            "===", "!==", "==", "!=", "<=", ">=", "&&", "||", "??", "+", "-", "*", "/", "%",
        ];
        const OTHER: [&str; 4] = ["=", "<", ">", "!"];
        for operator in OPERATORS.iter().chain(OTHER.iter()) {
            if self.source[self.offset..].starts_with(operator) {
                for _ in operator.bytes() {
                    self.advance();
                }
                return Ok((*operator).to_owned());
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
