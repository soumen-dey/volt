use crate::lexer::{Token, TokenKind};
use crate::{EngineError, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub(crate) enum UnaryOp {
    Not,
    Plus,
    Minus,
    BitNot,
    TypeOf,
    Void,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub(crate) enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponent,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
    StrictEqual,
    StrictNotEqual,
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    In,
    InstanceOf,
    And,
    Or,
    Nullish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeclarationKind {
    Let,
    Const,
    Var,
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
        operator: Option<BinaryOp>,
    },
    Conditional {
        test: Box<Expr>,
        consequent: Box<Expr>,
        alternate: Box<Expr>,
    },
    Update {
        target: Box<Expr>,
        increment: bool,
        prefix: bool,
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
        strict: bool,
    },
    Object(Vec<(String, Expr)>),
    Array(Vec<Option<Expr>>),
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
        kind: DeclarationKind,
        line: usize,
        column: usize,
    },
    FunctionDeclaration {
        name: String,
        parameters: Vec<String>,
        body: Vec<Stmt>,
        strict: bool,
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
    For {
        initializer: Option<Box<Stmt>>,
        condition: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    ForEach {
        name: String,
        kind: DeclarationKind,
        iterable: Expr,
        body: Box<Stmt>,
        is_of: bool,
    },
    DoWhile {
        body: Box<Stmt>,
        condition: Expr,
    },
    Break,
    Continue,
}

#[derive(Clone)]
pub(crate) struct Program {
    pub statements: Vec<Stmt>,
    pub strict: bool,
}

pub(crate) struct Parser<'source> {
    tokens: Vec<Token<'source>>,
    current: usize,
    function_depth: usize,
    loop_depth: usize,
    strict_context: bool,
}

impl<'source> Parser<'source> {
    pub fn new(tokens: Vec<Token<'source>>) -> Self {
        Self {
            tokens,
            current: 0,
            function_depth: 0,
            loop_depth: 0,
            strict_context: false,
        }
    }

    pub fn parse_program(mut self) -> Result<Program, EngineError> {
        let mut statements = Vec::new();
        let mut directive_prologue = true;
        while !self.is_at_end() {
            let statement = self.statement()?;
            if directive_prologue {
                if let Some(directive) = string_directive(&statement) {
                    if directive == "use strict" {
                        self.strict_context = true;
                    }
                } else {
                    directive_prologue = false;
                }
            }
            statements.push(statement);
        }
        let strict = self.strict_context;
        if strict {
            validate_strict_statements(&statements)?;
        }
        Ok(Program { statements, strict })
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
            let (name, _, _) = self.consume_binding_identifier("Expected a function name")?;
            let (parameters, body, strict) = self.function_tail()?;
            return Ok(Stmt::FunctionDeclaration {
                name,
                parameters,
                body,
                strict,
            });
        }
        if self.matches_keyword("if") {
            return self.if_statement();
        }
        if self.matches_keyword("while") {
            return self.while_statement();
        }
        if self.matches_keyword("for") {
            return self.for_statement();
        }
        if self.matches_keyword("do") {
            return self.do_while_statement();
        }
        if self.matches_keyword("break") {
            if self.loop_depth == 0 {
                return Err(self.error_here("'break' is only valid inside a loop"));
            }
            self.consume_statement_end()?;
            return Ok(Stmt::Break);
        }
        if self.matches_keyword("continue") {
            if self.loop_depth == 0 {
                return Err(self.error_here("'continue' is only valid inside a loop"));
            }
            self.consume_statement_end()?;
            return Ok(Stmt::Continue);
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
        self.declaration_with_terminator(true, false)
    }

    fn declaration_with_terminator(
        &mut self,
        consume_terminator: bool,
        allow_missing_const_initializer: bool,
    ) -> Result<Stmt, EngineError> {
        let keyword = (*self.previous()).clone();
        let kind = match &keyword.kind {
            TokenKind::Identifier(name) if *name == "const" => DeclarationKind::Const,
            TokenKind::Identifier(name) if *name == "var" => DeclarationKind::Var,
            _ => DeclarationKind::Let,
        };
        let (name, line, column) = self.consume_binding_identifier("Expected a binding name")?;
        let initializer = if self.matches_operator("=") {
            Some(self.assignment()?)
        } else {
            None
        };
        if kind == DeclarationKind::Const
            && initializer.is_none()
            && !allow_missing_const_initializer
        {
            return Err(self.error_at(&keyword, "A const declaration requires an initializer"));
        }
        if consume_terminator {
            self.consume_statement_end()?;
        }
        Ok(Stmt::Declaration {
            name,
            initializer,
            kind,
            line,
            column,
        })
    }

    fn if_statement(&mut self) -> Result<Stmt, EngineError> {
        self.consume(&TokenKind::LeftParen, "Expected '(' after if")?;
        let condition = self.expression()?;
        self.consume(&TokenKind::RightParen, "Expected ')' after if condition")?;
        let consequent = Box::new(self.statement_body()?);
        let alternate = if self.matches_keyword("else") {
            Some(Box::new(self.statement_body()?))
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
        self.loop_depth += 1;
        let body = self.statement_body();
        self.loop_depth -= 1;
        let body = Box::new(body?);
        Ok(Stmt::While { condition, body })
    }

    fn for_statement(&mut self) -> Result<Stmt, EngineError> {
        self.consume(&TokenKind::LeftParen, "Expected '(' after for")?;
        let initializer = if self.matches(&TokenKind::Semicolon) {
            None
        } else if self.matches_keyword("let")
            || self.matches_keyword("const")
            || self.matches_keyword("var")
        {
            let declaration = self.declaration_with_terminator(false, true)?;
            if self.matches_keyword("of") || self.matches_keyword("in") {
                let is_of = matches!(
                    &self.previous().kind,
                    TokenKind::Identifier(operator) if *operator == "of"
                );
                let (name, kind) = match declaration {
                    Stmt::Declaration {
                        name,
                        kind,
                        initializer,
                        ..
                    } => {
                        if initializer.is_some() {
                            return Err(
                                self.error_here("A for-in/of binding cannot have an initializer")
                            );
                        }
                        (name, kind)
                    }
                    _ => unreachable!("declaration parser returns a declaration"),
                };
                let iterable = self.expression()?;
                self.consume(&TokenKind::RightParen, "Expected ')' after for-in/of head")?;
                self.loop_depth += 1;
                let body = self.statement_body();
                self.loop_depth -= 1;
                return Ok(Stmt::ForEach {
                    name,
                    kind,
                    iterable,
                    body: Box::new(body?),
                    is_of,
                });
            }
            if matches!(
                &declaration,
                Stmt::Declaration {
                    kind: DeclarationKind::Const,
                    initializer: None,
                    ..
                }
            ) {
                return Err(self.error_here("A const declaration requires an initializer"));
            }
            self.consume(&TokenKind::Semicolon, "Expected ';' after for initializer")?;
            Some(Box::new(declaration))
        } else {
            let expression = self.expression()?;
            self.consume(&TokenKind::Semicolon, "Expected ';' after for initializer")?;
            Some(Box::new(Stmt::Expression(expression)))
        };
        let condition = if self.matches(&TokenKind::Semicolon) {
            None
        } else {
            let condition = self.expression()?;
            self.consume(&TokenKind::Semicolon, "Expected ';' after for condition")?;
            Some(condition)
        };
        let update = if self.check(&TokenKind::RightParen) {
            None
        } else {
            Some(self.expression()?)
        };
        self.consume(&TokenKind::RightParen, "Expected ')' after for clauses")?;
        self.loop_depth += 1;
        let body = self.statement_body();
        self.loop_depth -= 1;
        Ok(Stmt::For {
            initializer,
            condition,
            update,
            body: Box::new(body?),
        })
    }

    fn do_while_statement(&mut self) -> Result<Stmt, EngineError> {
        self.loop_depth += 1;
        let body = self.statement_body();
        self.loop_depth -= 1;
        if !self.matches_keyword("while") {
            return Err(self.error_here("Expected 'while' after do-while body"));
        }
        self.consume(&TokenKind::LeftParen, "Expected '(' after do-while")?;
        let condition = self.expression()?;
        self.consume(
            &TokenKind::RightParen,
            "Expected ')' after do-while condition",
        )?;
        self.matches(&TokenKind::Semicolon);
        Ok(Stmt::DoWhile {
            body: Box::new(body?),
            condition,
        })
    }

    fn try_statement(&mut self) -> Result<Stmt, EngineError> {
        self.consume(&TokenKind::LeftBrace, "Expected '{' after try")?;
        let body = Box::new(self.block()?);
        let (catch_name, catch_body) = if self.matches_keyword("catch") {
            let binding = if self.matches(&TokenKind::LeftParen) {
                let (name, _, _) = self.consume_binding_identifier("Expected catch binding")?;
                self.consume(&TokenKind::RightParen, "Expected ')' after catch binding")?;
                Some(name)
            } else {
                None
            };
            self.consume(&TokenKind::LeftBrace, "Expected '{' after catch clause")?;
            (binding, Some(Box::new(self.block()?)))
        } else {
            (None, None)
        };
        let finally_body = if self.matches_keyword("finally") {
            self.consume(&TokenKind::LeftBrace, "Expected '{' after finally")?;
            Some(Box::new(self.block()?))
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

    fn statement_body(&mut self) -> Result<Stmt, EngineError> {
        let statement = self.statement()?;
        if matches!(
            &statement,
            Stmt::Declaration {
                kind: DeclarationKind::Let | DeclarationKind::Const,
                ..
            } | Stmt::FunctionDeclaration { .. }
        ) {
            return Err(self.error_here(
                "Lexical declarations and function declarations require a block in this position",
            ));
        }
        Ok(statement)
    }

    fn block(&mut self) -> Result<Stmt, EngineError> {
        let mut statements = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.is_at_end() {
            statements.push(self.statement()?);
        }
        self.consume(&TokenKind::RightBrace, "Expected '}' after block")?;
        Ok(Stmt::Block(statements))
    }

    fn function_tail(&mut self) -> Result<(Vec<String>, Vec<Stmt>, bool), EngineError> {
        self.consume(&TokenKind::LeftParen, "Expected '(' after function name")?;
        let mut parameters = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            loop {
                parameters.push(
                    self.consume_binding_identifier("Expected parameter name")?
                        .0,
                );
                if !self.matches(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.consume(&TokenKind::RightParen, "Expected ')' after parameters")?;
        self.consume(&TokenKind::LeftBrace, "Expected '{' before function body")?;
        let outer_strict = self.strict_context;
        let inherited_strict = outer_strict;
        let outer_loop_depth = self.loop_depth;
        self.loop_depth = 0;
        self.strict_context = inherited_strict;
        self.function_depth += 1;
        let mut statements = Vec::new();
        let mut directive_prologue = true;
        let body = (|| {
            while !self.check(&TokenKind::RightBrace) && !self.is_at_end() {
                let statement = self.statement()?;
                if directive_prologue {
                    if let Some(directive) = string_directive(&statement) {
                        if directive == "use strict" {
                            self.strict_context = true;
                        }
                    } else {
                        directive_prologue = false;
                    }
                }
                statements.push(statement);
            }
            self.consume(&TokenKind::RightBrace, "Expected '}' after function body")
        })();
        self.function_depth -= 1;
        self.loop_depth = outer_loop_depth;
        let strict = self.strict_context;
        self.strict_context = outer_strict;
        body?;
        if strict {
            validate_strict_parameters(&parameters)?;
            validate_strict_statements(&statements)?;
        }
        Ok((parameters, statements, strict))
    }

    fn expression(&mut self) -> Result<Expr, EngineError> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expr, EngineError> {
        let expression = self.conditional()?;
        let operator = match &self.peek().kind {
            TokenKind::Operator(operator) => match *operator {
                "=" => Some(None),
                "+=" => Some(Some(BinaryOp::Add)),
                "-=" => Some(Some(BinaryOp::Subtract)),
                "*=" => Some(Some(BinaryOp::Multiply)),
                "/=" => Some(Some(BinaryOp::Divide)),
                "%=" => Some(Some(BinaryOp::Remainder)),
                "**=" => Some(Some(BinaryOp::Exponent)),
                "&=" => Some(Some(BinaryOp::BitAnd)),
                "|=" => Some(Some(BinaryOp::BitOr)),
                "^=" => Some(Some(BinaryOp::BitXor)),
                "<<=" => Some(Some(BinaryOp::ShiftLeft)),
                ">>=" => Some(Some(BinaryOp::ShiftRight)),
                ">>>=" => Some(Some(BinaryOp::UnsignedShiftRight)),
                "&&=" => Some(Some(BinaryOp::And)),
                "||=" => Some(Some(BinaryOp::Or)),
                "??=" => Some(Some(BinaryOp::Nullish)),
                _ => None,
            },
            _ => None,
        };
        if let Some(operator) = operator {
            self.advance();
            let equals = (*self.previous()).clone();
            let value = self.assignment()?;
            if matches!(
                expression,
                Expr::Identifier { .. } | Expr::Member(_, _) | Expr::Index(_, _)
            ) {
                return Ok(Expr::Assignment {
                    target: Box::new(expression),
                    value: Box::new(value),
                    operator,
                });
            }
            return Err(self.error_at(&equals, "Invalid assignment target"));
        }
        Ok(expression)
    }

    fn conditional(&mut self) -> Result<Expr, EngineError> {
        let test = self.binary(1)?;
        if self.matches_operator("?") {
            let consequent = self.assignment()?;
            self.consume(&TokenKind::Colon, "Expected ':' in conditional expression")?;
            let alternate = self.assignment()?;
            Ok(Expr::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            })
        } else {
            Ok(test)
        }
    }

    fn binary(&mut self, minimum_precedence: u8) -> Result<Expr, EngineError> {
        let mut expression = self.unary()?;
        while let Some((operator, precedence)) = self.current_binary_operator() {
            if precedence < minimum_precedence {
                break;
            }
            self.advance();
            let right_precedence = if operator == BinaryOp::Exponent {
                precedence
            } else {
                precedence + 1
            };
            let right = self.binary(right_precedence)?;
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
            let unary = match *operator {
                "!" => Some(UnaryOp::Not),
                "+" => Some(UnaryOp::Plus),
                "-" => Some(UnaryOp::Minus),
                "~" => Some(UnaryOp::BitNot),
                _ => None,
            };
            if let Some(operator) = unary {
                self.advance();
                let token = (*self.previous()).clone();
                return Ok(Expr::Unary {
                    operator,
                    operand: Box::new(self.unary()?),
                    line: token.line,
                    column: token.column,
                });
            }
        }
        if self.matches_operator("++") || self.matches_operator("--") {
            let increment = matches!(
                &self.previous().kind,
                TokenKind::Operator(operator) if *operator == "++"
            );
            let target = self.unary()?;
            if !matches!(
                target,
                Expr::Identifier { .. } | Expr::Member(_, _) | Expr::Index(_, _)
            ) {
                return Err(self.error_here("Invalid update target"));
            }
            return Ok(Expr::Update {
                target: Box::new(target),
                increment,
                prefix: true,
            });
        }
        if self.matches_keyword("typeof")
            || self.matches_keyword("void")
            || self.matches_keyword("delete")
        {
            let token = (*self.previous()).clone();
            let operator = match &token.kind {
                TokenKind::Identifier(name) if *name == "typeof" => UnaryOp::TypeOf,
                TokenKind::Identifier(name) if *name == "void" => UnaryOp::Void,
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
            } else if self.matches_operator("++") || self.matches_operator("--") {
                let increment = matches!(
                    &self.previous().kind,
                    TokenKind::Operator(operator) if *operator == "++"
                );
                if self.peek().line > self.previous().line {
                    self.current -= 1;
                    break;
                }
                if !matches!(
                    expression,
                    Expr::Identifier { .. } | Expr::Member(_, _) | Expr::Index(_, _)
                ) {
                    return Err(self.error_here("Invalid update target"));
                }
                expression = Expr::Update {
                    target: Box::new(expression),
                    increment,
                    prefix: false,
                };
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
            let mut arguments = Vec::new();
            if self.matches(&TokenKind::LeftParen) {
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
            }
            return Ok(Expr::Construct(Box::new(callee), arguments));
        }
        if self.matches_keyword("function") {
            let name = if matches!(self.peek().kind, TokenKind::Identifier(_)) {
                Some(self.consume_binding_identifier("Expected function name")?.0)
            } else {
                None
            };
            let (parameters, body, strict) = self.function_tail()?;
            return Ok(Expr::Function {
                name,
                parameters,
                body,
                strict,
            });
        }
        if self.matches(&TokenKind::LeftBrace) {
            return self.object_literal();
        }
        if self.matches(&TokenKind::LeftBracket) {
            return self.array_literal();
        }
        self.advance();
        let token = (*self.previous()).clone();
        match token.kind {
            TokenKind::Number(value) => Ok(Expr::Literal(Value::Number(value))),
            TokenKind::String(value) => Ok(Expr::Literal(Value::String(value))),
            TokenKind::Identifier("true") => Ok(Expr::Literal(Value::Boolean(true))),
            TokenKind::Identifier("false") => Ok(Expr::Literal(Value::Boolean(false))),
            TokenKind::Identifier("null") => Ok(Expr::Literal(Value::Null)),
            TokenKind::Identifier("undefined") => Ok(Expr::Literal(Value::Undefined)),
            TokenKind::Identifier("this") => Ok(Expr::This),
            TokenKind::Identifier(name) => Ok(Expr::Identifier {
                name: name.to_owned(),
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
            self.advance();
            let key = match self.previous().clone() {
                Token {
                    kind: TokenKind::Identifier(key),
                    ..
                } => key.to_owned(),
                Token {
                    kind: TokenKind::String(key),
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

    fn array_literal(&mut self) -> Result<Expr, EngineError> {
        let mut elements = Vec::new();
        while !self.check(&TokenKind::RightBracket) && !self.is_at_end() {
            if self.matches(&TokenKind::Comma) {
                elements.push(None);
                continue;
            }
            elements.push(Some(self.assignment()?));
            if !self.matches(&TokenKind::Comma) {
                break;
            }
        }
        self.consume(&TokenKind::RightBracket, "Expected ']' after array literal")?;
        Ok(Expr::Array(elements))
    }

    fn current_binary_operator(&self) -> Option<(BinaryOp, u8)> {
        let operator = match &self.peek().kind {
            TokenKind::Operator(operator) | TokenKind::Identifier(operator) => *operator,
            _ => return None,
        };
        Some(match operator {
            "??" => (BinaryOp::Nullish, 1),
            "||" => (BinaryOp::Or, 2),
            "&&" => (BinaryOp::And, 3),
            "|" => (BinaryOp::BitOr, 4),
            "^" => (BinaryOp::BitXor, 5),
            "&" => (BinaryOp::BitAnd, 6),
            "==" => (BinaryOp::Equal, 7),
            "!=" => (BinaryOp::NotEqual, 7),
            "===" => (BinaryOp::StrictEqual, 7),
            "!==" => (BinaryOp::StrictNotEqual, 7),
            "<" => (BinaryOp::LessThan, 8),
            "<=" => (BinaryOp::LessThanOrEqual, 8),
            ">" => (BinaryOp::GreaterThan, 8),
            ">=" => (BinaryOp::GreaterThanOrEqual, 8),
            "in" => (BinaryOp::In, 8),
            "instanceof" => (BinaryOp::InstanceOf, 8),
            "<<" => (BinaryOp::ShiftLeft, 9),
            ">>" => (BinaryOp::ShiftRight, 9),
            ">>>" => (BinaryOp::UnsignedShiftRight, 9),
            "+" => (BinaryOp::Add, 10),
            "-" => (BinaryOp::Subtract, 10),
            "*" => (BinaryOp::Multiply, 11),
            "/" => (BinaryOp::Divide, 11),
            "%" => (BinaryOp::Remainder, 11),
            "**" => (BinaryOp::Exponent, 12),
            _ => return None,
        })
    }

    fn consume_identifier(&mut self, message: &str) -> Result<(String, usize, usize), EngineError> {
        if let TokenKind::Identifier(name) = &self.peek().kind {
            let name = (*name).to_owned();
            let line = self.peek().line;
            let column = self.peek().column;
            self.advance();
            return Ok((name, line, column));
        }
        Err(self.error_here(message))
    }

    fn consume_binding_identifier(
        &mut self,
        message: &str,
    ) -> Result<(String, usize, usize), EngineError> {
        let (name, line, column) = self.consume_identifier(message)?;
        if matches!(
            name.as_str(),
            "break"
                | "case"
                | "catch"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "export"
                | "extends"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "new"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
                | "yield"
                | "class"
                | "enum"
                | "let"
                | "await"
                | "null"
                | "true"
                | "false"
        ) {
            return Err(EngineError::new(
                "SyntaxError",
                format!("'{name}' is a reserved word"),
                line,
                column,
            ));
        }
        Ok((name, line, column))
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
        if matches!(&self.peek().kind, TokenKind::Identifier(name) if *name == keyword) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn matches_operator(&mut self, operator: &str) -> bool {
        if matches!(&self.peek().kind, TokenKind::Operator(value) if *value == operator) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn matches(&mut self, kind: &TokenKind<'source>) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, kind: &TokenKind<'source>) -> bool {
        std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind)
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    fn advance(&mut self) {
        if !self.is_at_end() {
            self.current += 1;
        }
    }

    fn peek(&self) -> &Token<'source> {
        &self.tokens[self.current]
    }

    fn previous(&self) -> &Token<'source> {
        &self.tokens[self.current.saturating_sub(1)]
    }

    fn error_here(&self, message: &str) -> EngineError {
        self.error_at(self.peek(), message)
    }

    fn error_at(&self, token: &Token<'source>, message: &str) -> EngineError {
        EngineError::new("SyntaxError", message, token.line, token.column)
    }
}

fn string_directive(statement: &Stmt) -> Option<&str> {
    let Stmt::Expression(Expr::Literal(Value::String(value))) = statement else {
        return None;
    };
    Some(value)
}

fn validate_strict_parameters(parameters: &[String]) -> Result<(), EngineError> {
    let mut seen = std::collections::HashSet::new();
    for parameter in parameters {
        if is_strict_restricted_name(parameter) {
            return Err(EngineError::new(
                "SyntaxError",
                format!("Invalid strict-mode parameter name '{parameter}'"),
                1,
                1,
            ));
        }
        if !seen.insert(parameter) {
            return Err(EngineError::new(
                "SyntaxError",
                format!("Duplicate parameter name '{parameter}' in strict mode"),
                1,
                1,
            ));
        }
    }
    Ok(())
}

fn validate_strict_statements(statements: &[Stmt]) -> Result<(), EngineError> {
    for statement in statements {
        match statement {
            Stmt::Declaration {
                name, initializer, ..
            } => {
                validate_strict_name(name)?;
                if let Some(initializer) = initializer {
                    validate_strict_expression(initializer)?;
                }
            }
            Stmt::FunctionDeclaration {
                name,
                parameters,
                body,
                ..
            } => {
                validate_strict_name(name)?;
                validate_strict_parameters(parameters)?;
                validate_strict_statements(body)?;
            }
            Stmt::Block(statements) => validate_strict_statements(statements)?,
            Stmt::If {
                condition,
                consequent,
                alternate,
            } => {
                validate_strict_expression(condition)?;
                validate_strict_statements(std::slice::from_ref(consequent))?;
                if let Some(alternate) = alternate {
                    validate_strict_statements(std::slice::from_ref(alternate))?;
                }
            }
            Stmt::While { condition, body } => {
                validate_strict_expression(condition)?;
                validate_strict_statements(std::slice::from_ref(body))?;
            }
            Stmt::For {
                initializer,
                condition,
                update,
                body,
            } => {
                if let Some(initializer) = initializer {
                    validate_strict_statements(std::slice::from_ref(initializer))?;
                }
                if let Some(condition) = condition {
                    validate_strict_expression(condition)?;
                }
                if let Some(update) = update {
                    validate_strict_expression(update)?;
                }
                validate_strict_statements(std::slice::from_ref(body))?;
            }
            Stmt::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                validate_strict_name(name)?;
                validate_strict_expression(iterable)?;
                validate_strict_statements(std::slice::from_ref(body))?;
            }
            Stmt::DoWhile { body, condition } => {
                validate_strict_statements(std::slice::from_ref(body))?;
                validate_strict_expression(condition)?;
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    validate_strict_expression(value)?;
                }
            }
            Stmt::Throw(value) => validate_strict_expression(value)?,
            Stmt::Try {
                body,
                catch_name,
                catch_body,
                finally_body,
            } => {
                validate_strict_statements(std::slice::from_ref(body))?;
                if let Some(name) = catch_name {
                    validate_strict_name(name)?;
                }
                if let Some(body) = catch_body {
                    validate_strict_statements(std::slice::from_ref(body))?;
                }
                if let Some(body) = finally_body {
                    validate_strict_statements(std::slice::from_ref(body))?;
                }
            }
            Stmt::Empty | Stmt::Break | Stmt::Continue => {}
            Stmt::Expression(expression) => validate_strict_expression(expression)?,
        }
    }
    Ok(())
}

fn validate_strict_expression(expression: &Expr) -> Result<(), EngineError> {
    match expression {
        Expr::Identifier { .. } => Ok(()),
        Expr::Assignment { target, value, .. } => {
            if matches!(target.as_ref(), Expr::Identifier { name, .. } if is_strict_restricted_name(name))
            {
                return Err(EngineError::new(
                    "SyntaxError",
                    "Assignment to 'eval' or 'arguments' is forbidden in strict mode",
                    1,
                    1,
                ));
            }
            validate_strict_expression(target)?;
            validate_strict_expression(value)
        }
        Expr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            validate_strict_expression(test)?;
            validate_strict_expression(consequent)?;
            validate_strict_expression(alternate)
        }
        Expr::Update { target, .. } => validate_strict_expression(target),
        Expr::Unary {
            operator: UnaryOp::Delete,
            operand,
            ..
        } if matches!(operand.as_ref(), Expr::Identifier { .. }) => Err(EngineError::new(
            "SyntaxError",
            "Deleting an unqualified identifier is forbidden in strict mode",
            1,
            1,
        )),
        Expr::Unary { operand, .. } => validate_strict_expression(operand),
        Expr::Binary { left, right, .. } => {
            validate_strict_expression(left)?;
            validate_strict_expression(right)
        }
        Expr::Function {
            parameters, body, ..
        } => {
            validate_strict_parameters(parameters)?;
            validate_strict_statements(body)
        }
        Expr::Object(properties) => {
            for (_, value) in properties {
                validate_strict_expression(value)?;
            }
            Ok(())
        }
        Expr::Array(elements) => {
            for expression in elements.iter().flatten() {
                validate_strict_expression(expression)?;
            }
            Ok(())
        }
        Expr::Member(object, _) => validate_strict_expression(object),
        Expr::Index(object, property) => {
            validate_strict_expression(object)?;
            validate_strict_expression(property)
        }
        Expr::Call(callee, arguments) | Expr::Construct(callee, arguments) => {
            validate_strict_expression(callee)?;
            for argument in arguments {
                validate_strict_expression(argument)?;
            }
            Ok(())
        }
        Expr::Literal(_) | Expr::This => Ok(()),
    }
}

fn validate_strict_name(name: &str) -> Result<(), EngineError> {
    if is_strict_restricted_name(name) {
        Err(EngineError::new(
            "SyntaxError",
            format!("Invalid strict-mode binding name '{name}'"),
            1,
            1,
        ))
    } else {
        Ok(())
    }
}

fn is_strict_restricted_name(name: &str) -> bool {
    matches!(
        name,
        "eval"
            | "arguments"
            | "implements"
            | "interface"
            | "let"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "static"
            | "yield"
    )
}
