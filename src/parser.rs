use crate::lexer::{Token, TokenKind};
use crate::{EngineError, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Not,
    Plus,
    Minus,
    TypeOf,
    Void,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    StrictEqual,
    StrictNotEqual,
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    And,
    Or,
    Nullish,
}

#[derive(Clone)]
pub(crate) enum Expr {
    Literal(Value),
    Identifier {
        name: String,
        line: usize,
        column: usize,
    },
    This,
    Assignment {
        target: Box<Expr>,
        value: Box<Expr>,
        line: usize,
        column: usize,
    },
    Unary {
        operator: UnaryOp,
        operand: Box<Expr>,
        line: usize,
        column: usize,
    },
    Binary {
        operator: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Function {
        name: Option<String>,
        parameters: Vec<String>,
        body: Vec<Stmt>,
    },
    Object(Vec<(String, Expr)>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Construct(Box<Expr>, Vec<Expr>),
}

#[derive(Clone)]
pub(crate) enum Stmt {
    Empty,
    Expression(Expr),
    Declaration {
        name: String,
        initializer: Option<Expr>,
        mutable: bool,
        line: usize,
        column: usize,
    },
    FunctionDeclaration {
        name: String,
        parameters: Vec<String>,
        body: Vec<Stmt>,
    },
    Block(Vec<Stmt>),
    If {
        condition: Expr,
        consequent: Box<Stmt>,
        alternate: Option<Box<Stmt>>,
    },
    While {
        condition: Expr,
        body: Box<Stmt>,
    },
    Return(Option<Expr>),
    Throw(Expr),
    Try {
        body: Box<Stmt>,
        catch_name: Option<String>,
        catch_body: Option<Box<Stmt>>,
        finally_body: Option<Box<Stmt>>,
    },
}

#[derive(Clone)]
pub(crate) struct Program {
    pub statements: Vec<Stmt>,
}

pub(crate) struct Parser {
    tokens: Vec<Token>,
    current: usize,
    function_depth: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            current: 0,
            function_depth: 0,
        }
    }

    pub fn parse_program(mut self) -> Result<Program, EngineError> {
        let mut statements = Vec::new();
        while !self.is_at_end() {
            statements.push(self.statement()?);
        }
        Ok(Program { statements })
    }

    fn statement(&mut self) -> Result<Stmt, EngineError> {
        if self.matches(&TokenKind::Semicolon) {
            return Ok(Stmt::Empty);
        }
        if self.matches_keyword("let")
            || self.matches_keyword("const")
            || self.matches_keyword("var")
        {
            return self.declaration();
        }
        if self.matches_keyword("function") {
            let (name, _, _) = self.consume_identifier("Expected a function name")?;
            let (parameters, body) = self.function_tail()?;
            return Ok(Stmt::FunctionDeclaration {
                name,
                parameters,
                body,
            });
        }
        if self.matches_keyword("if") {
            return self.if_statement();
        }
        if self.matches_keyword("while") {
            return self.while_statement();
        }
        if self.matches_keyword("return") {
            if self.function_depth == 0 {
                return Err(self.error_here("'return' is only valid inside a function"));
            }
            let value = if self.check(&TokenKind::Semicolon)
                || self.check(&TokenKind::RightBrace)
                || self.is_at_end()
                || self.peek().line > self.previous().line
            {
                None
            } else {
                Some(self.expression()?)
            };
            self.consume_statement_end()?;
            return Ok(Stmt::Return(value));
        }
        if self.matches_keyword("throw") {
            if self.peek().line > self.previous().line {
                return Err(self.error_here("A line break is not allowed after 'throw'"));
            }
            let value = self.expression()?;
            self.consume_statement_end()?;
            return Ok(Stmt::Throw(value));
        }
        if self.matches_keyword("try") {
            return self.try_statement();
        }
        if self.matches(&TokenKind::LeftBrace) {
            return self.block();
        }
        let expression = self.expression()?;
        self.consume_statement_end()?;
        Ok(Stmt::Expression(expression))
    }

    fn declaration(&mut self) -> Result<Stmt, EngineError> {
        let keyword = self.previous().clone();
        let mutable = !matches!(&keyword.kind, TokenKind::Identifier(name) if name == "const");
        let (name, line, column) = self.consume_identifier("Expected a binding name")?;
        let initializer = if self.matches_operator("=") {
            Some(self.assignment()?)
        } else {
            None
        };
        if !mutable && initializer.is_none() {
            return Err(self.error_at(&keyword, "A const declaration requires an initializer"));
        }
        self.consume_statement_end()?;
        Ok(Stmt::Declaration {
            name,
            initializer,
            mutable,
            line,
            column,
        })
    }

    fn if_statement(&mut self) -> Result<Stmt, EngineError> {
        self.consume(&TokenKind::LeftParen, "Expected '(' after if")?;
        let condition = self.expression()?;
        self.consume(&TokenKind::RightParen, "Expected ')' after if condition")?;
        let consequent = Box::new(self.statement()?);
        let alternate = if self.matches_keyword("else") {
            Some(Box::new(self.statement()?))
        } else {
            None
        };
        Ok(Stmt::If {
            condition,
            consequent,
            alternate,
        })
    }

    fn while_statement(&mut self) -> Result<Stmt, EngineError> {
        self.consume(&TokenKind::LeftParen, "Expected '(' after while")?;
        let condition = self.expression()?;
        self.consume(&TokenKind::RightParen, "Expected ')' after while condition")?;
        let body = Box::new(self.statement()?);
        Ok(Stmt::While { condition, body })
    }

    fn try_statement(&mut self) -> Result<Stmt, EngineError> {
        let body = Box::new(self.statement()?);
        let (catch_name, catch_body) = if self.matches_keyword("catch") {
            let binding = if self.matches(&TokenKind::LeftParen) {
                let (name, _, _) = self.consume_identifier("Expected catch binding")?;
                self.consume(&TokenKind::RightParen, "Expected ')' after catch binding")?;
                Some(name)
            } else {
                None
            };
            (binding, Some(Box::new(self.statement()?)))
        } else {
            (None, None)
        };
        let finally_body = if self.matches_keyword("finally") {
            Some(Box::new(self.statement()?))
        } else {
            None
        };
        if catch_body.is_none() && finally_body.is_none() {
            return Err(self.error_here("Expected 'catch' or 'finally' after try block"));
        }
        Ok(Stmt::Try {
            body,
            catch_name,
            catch_body,
            finally_body,
        })
    }

    fn block(&mut self) -> Result<Stmt, EngineError> {
        let mut statements = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.is_at_end() {
            statements.push(self.statement()?);
        }
        self.consume(&TokenKind::RightBrace, "Expected '}' after block")?;
        Ok(Stmt::Block(statements))
    }

    fn function_tail(&mut self) -> Result<(Vec<String>, Vec<Stmt>), EngineError> {
        self.consume(&TokenKind::LeftParen, "Expected '(' after function name")?;
        let mut parameters = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            loop {
                parameters.push(self.consume_identifier("Expected parameter name")?.0);
                if !self.matches(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.consume(&TokenKind::RightParen, "Expected ')' after parameters")?;
        self.consume(&TokenKind::LeftBrace, "Expected '{' before function body")?;
        self.function_depth += 1;
        let body = self.block();
        self.function_depth -= 1;
        match body? {
            Stmt::Block(statements) => Ok((parameters, statements)),
            _ => unreachable!(),
        }
    }

    fn expression(&mut self) -> Result<Expr, EngineError> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expr, EngineError> {
        let expression = self.binary(1)?;
        if self.matches_operator("=") {
            let equals = self.previous().clone();
            let value = self.assignment()?;
            if matches!(
                expression,
                Expr::Identifier { .. } | Expr::Member(_, _) | Expr::Index(_, _)
            ) {
                return Ok(Expr::Assignment {
                    target: Box::new(expression),
                    value: Box::new(value),
                    line: equals.line,
                    column: equals.column,
                });
            }
            return Err(self.error_at(&equals, "Invalid assignment target"));
        }
        Ok(expression)
    }

    fn binary(&mut self, minimum_precedence: u8) -> Result<Expr, EngineError> {
        let mut expression = self.unary()?;
        while let Some((operator, precedence)) = self.current_binary_operator() {
            if precedence < minimum_precedence {
                break;
            }
            self.advance();
            let right = self.binary(precedence + 1)?;
            expression = Expr::Binary {
                operator,
                left: Box::new(expression),
                right: Box::new(right),
            };
        }
        Ok(expression)
    }

    fn unary(&mut self) -> Result<Expr, EngineError> {
        if let TokenKind::Operator(operator) = &self.peek().kind {
            let unary = match operator.as_str() {
                "!" => Some(UnaryOp::Not),
                "+" => Some(UnaryOp::Plus),
                "-" => Some(UnaryOp::Minus),
                _ => None,
            };
            if let Some(operator) = unary {
                let token = self.advance().clone();
                return Ok(Expr::Unary {
                    operator,
                    operand: Box::new(self.unary()?),
                    line: token.line,
                    column: token.column,
                });
            }
        }
        if self.matches_keyword("typeof")
            || self.matches_keyword("void")
            || self.matches_keyword("delete")
        {
            let token = self.previous().clone();
            let operator = match &token.kind {
                TokenKind::Identifier(name) if name == "typeof" => UnaryOp::TypeOf,
                TokenKind::Identifier(name) if name == "void" => UnaryOp::Void,
                _ => UnaryOp::Delete,
            };
            return Ok(Expr::Unary {
                operator,
                operand: Box::new(self.unary()?),
                line: token.line,
                column: token.column,
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, EngineError> {
        let mut expression = self.primary()?;
        loop {
            if self.matches(&TokenKind::Dot) {
                let (name, _, _) = self.consume_identifier("Expected a property name after '.'")?;
                expression = Expr::Member(Box::new(expression), name);
            } else if self.matches(&TokenKind::LeftBracket) {
                let property = self.expression()?;
                self.consume(
                    &TokenKind::RightBracket,
                    "Expected ']' after property expression",
                )?;
                expression = Expr::Index(Box::new(expression), Box::new(property));
            } else if self.matches(&TokenKind::LeftParen) {
                let mut arguments = Vec::new();
                if !self.check(&TokenKind::RightParen) {
                    loop {
                        arguments.push(self.assignment()?);
                        if !self.matches(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.consume(&TokenKind::RightParen, "Expected ')' after arguments")?;
                expression = Expr::Call(Box::new(expression), arguments);
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn primary(&mut self) -> Result<Expr, EngineError> {
        if self.matches_keyword("new") {
            let (name, line, column) =
                self.consume_identifier("Expected a constructor after 'new'")?;
            let mut callee = Expr::Identifier { name, line, column };
            while self.matches(&TokenKind::Dot) {
                let (property, _, _) =
                    self.consume_identifier("Expected a property name after '.'")?;
                callee = Expr::Member(Box::new(callee), property);
            }
            self.consume(&TokenKind::LeftParen, "Expected '(' after constructor")?;
            let mut arguments = Vec::new();
            if !self.check(&TokenKind::RightParen) {
                loop {
                    arguments.push(self.assignment()?);
                    if !self.matches(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.consume(
                &TokenKind::RightParen,
                "Expected ')' after constructor arguments",
            )?;
            return Ok(Expr::Construct(Box::new(callee), arguments));
        }
        if self.matches_keyword("function") {
            let name = if matches!(self.peek().kind, TokenKind::Identifier(_)) {
                Some(self.consume_identifier("Expected function name")?.0)
            } else {
                None
            };
            let (parameters, body) = self.function_tail()?;
            return Ok(Expr::Function {
                name,
                parameters,
                body,
            });
        }
        if self.matches(&TokenKind::LeftBrace) {
            return self.object_literal();
        }
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Number(value) => Ok(Expr::Literal(Value::Number(value))),
            TokenKind::String(value) => Ok(Expr::Literal(Value::String(value))),
            TokenKind::Identifier(name) if name == "true" => {
                Ok(Expr::Literal(Value::Boolean(true)))
            }
            TokenKind::Identifier(name) if name == "false" => {
                Ok(Expr::Literal(Value::Boolean(false)))
            }
            TokenKind::Identifier(name) if name == "null" => Ok(Expr::Literal(Value::Null)),
            TokenKind::Identifier(name) if name == "undefined" => {
                Ok(Expr::Literal(Value::Undefined))
            }
            TokenKind::Identifier(name) if name == "this" => Ok(Expr::This),
            TokenKind::Identifier(name) => Ok(Expr::Identifier {
                name,
                line: token.line,
                column: token.column,
            }),
            TokenKind::LeftParen => {
                let expression = self.expression()?;
                self.consume(&TokenKind::RightParen, "Expected ')' after expression")?;
                Ok(expression)
            }
            _ => Err(self.error_at(&token, "Expected an expression")),
        }
    }

    fn object_literal(&mut self) -> Result<Expr, EngineError> {
        let mut properties = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.is_at_end() {
            let key = match self.advance().clone() {
                Token {
                    kind: TokenKind::Identifier(key) | TokenKind::String(key),
                    ..
                } => key,
                Token {
                    kind: TokenKind::Number(number),
                    ..
                } => number.to_string(),
                token => return Err(self.error_at(&token, "Expected an object property name")),
            };
            self.consume(&TokenKind::Colon, "Expected ':' after object property name")?;
            properties.push((key, self.assignment()?));
            if !self.matches(&TokenKind::Comma) {
                break;
            }
        }
        self.consume(&TokenKind::RightBrace, "Expected '}' after object literal")?;
        Ok(Expr::Object(properties))
    }

    fn current_binary_operator(&self) -> Option<(BinaryOp, u8)> {
        let TokenKind::Operator(operator) = &self.peek().kind else {
            return None;
        };
        Some(match operator.as_str() {
            "??" => (BinaryOp::Nullish, 1),
            "||" => (BinaryOp::Or, 2),
            "&&" => (BinaryOp::And, 3),
            "==" => (BinaryOp::Equal, 4),
            "!=" => (BinaryOp::NotEqual, 4),
            "===" => (BinaryOp::StrictEqual, 4),
            "!==" => (BinaryOp::StrictNotEqual, 4),
            "<" => (BinaryOp::LessThan, 5),
            "<=" => (BinaryOp::LessThanOrEqual, 5),
            ">" => (BinaryOp::GreaterThan, 5),
            ">=" => (BinaryOp::GreaterThanOrEqual, 5),
            "+" => (BinaryOp::Add, 6),
            "-" => (BinaryOp::Subtract, 6),
            "*" => (BinaryOp::Multiply, 7),
            "/" => (BinaryOp::Divide, 7),
            "%" => (BinaryOp::Remainder, 7),
            _ => return None,
        })
    }

    fn consume_identifier(&mut self, message: &str) -> Result<(String, usize, usize), EngineError> {
        if let TokenKind::Identifier(name) = &self.peek().kind {
            let name = name.clone();
            let token = self.advance();
            return Ok((name, token.line, token.column));
        }
        Err(self.error_here(message))
    }

    fn consume(&mut self, kind: &TokenKind, message: &str) -> Result<(), EngineError> {
        if self.check(kind) {
            self.advance();
            Ok(())
        } else {
            Err(self.error_here(message))
        }
    }

    fn consume_statement_end(&mut self) -> Result<(), EngineError> {
        if self.matches(&TokenKind::Semicolon)
            || self.check(&TokenKind::RightBrace)
            || self.is_at_end()
            || self.peek().line > self.previous().line
        {
            Ok(())
        } else {
            Err(self.error_here("Expected ';' or a line break after statement"))
        }
    }

    fn matches_keyword(&mut self, keyword: &str) -> bool {
        if matches!(&self.peek().kind, TokenKind::Identifier(name) if name == keyword) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn matches_operator(&mut self, operator: &str) -> bool {
        if matches!(&self.peek().kind, TokenKind::Operator(value) if value == operator) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn matches(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind)
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.current += 1;
        }
        self.previous()
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.current.saturating_sub(1)]
    }

    fn error_here(&self, message: &str) -> EngineError {
        self.error_at(self.peek(), message)
    }

    fn error_at(&self, token: &Token, message: &str) -> EngineError {
        EngineError::new("SyntaxError", message, token.line, token.column)
    }
}
