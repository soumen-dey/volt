mod lexer;
mod parser;

use lexer::Lexer;
use parser::{BinaryOp, Expr, Program, Stmt, UnaryOp};
use std::{cell::RefCell, collections::HashMap, fmt, rc::Rc};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
}

impl Value {
    pub fn to_js_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(value) if value.is_nan() => "NaN".to_owned(),
            Self::Number(value) if *value == f64::INFINITY => "Infinity".to_owned(),
            Self::Number(value) if *value == f64::NEG_INFINITY => "-Infinity".to_owned(),
            Self::Number(value) if *value == 0.0 => "0".to_owned(),
            Self::Number(value) => value.to_string(),
            Self::String(value) => value.clone(),
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
        }
    }

    fn to_number(&self) -> f64 {
        match self {
            Self::Undefined => f64::NAN,
            Self::Null => 0.0,
            Self::Boolean(value) => u8::from(*value) as f64,
            Self::Number(value) => *value,
            Self::String(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    0.0
                } else {
                    trimmed.parse().unwrap_or(f64::NAN)
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EngineError {
    pub name: &'static str,
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl EngineError {
    fn new(name: &'static str, message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            name,
            message: message.into(),
            line,
            column,
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {} ({}:{})",
            self.name, self.message, self.line, self.column
        )
    }
}

impl std::error::Error for EngineError {}

#[derive(Clone)]
struct Binding {
    value: Value,
    mutable: bool,
}

type ScopeRef = Rc<RefCell<Scope>>;

#[derive(Default)]
struct Scope {
    bindings: HashMap<String, Binding>,
    parent: Option<ScopeRef>,
}

impl Scope {
    fn child(parent: ScopeRef) -> ScopeRef {
        Rc::new(RefCell::new(Self {
            bindings: HashMap::new(),
            parent: Some(parent),
        }))
    }

    fn declare(
        scope: &ScopeRef,
        name: &str,
        value: Value,
        mutable: bool,
        line: usize,
        column: usize,
    ) -> Result<(), EngineError> {
        let mut current = scope.borrow_mut();
        if current.bindings.contains_key(name) {
            return Err(EngineError::new(
                "SyntaxError",
                format!("Identifier '{name}' has already been declared"),
                line,
                column,
            ));
        }
        current
            .bindings
            .insert(name.to_owned(), Binding { value, mutable });
        Ok(())
    }

    fn get(scope: &ScopeRef, name: &str) -> Option<Value> {
        let current = scope.borrow();
        current
            .bindings
            .get(name)
            .map(|binding| binding.value.clone())
            .or_else(|| {
                current
                    .parent
                    .as_ref()
                    .and_then(|parent| Self::get(parent, name))
            })
    }

    fn assign(
        scope: &ScopeRef,
        name: &str,
        value: Value,
        line: usize,
        column: usize,
    ) -> Result<Value, EngineError> {
        let parent = {
            let mut current = scope.borrow_mut();
            if let Some(binding) = current.bindings.get_mut(name) {
                if !binding.mutable {
                    return Err(EngineError::new(
                        "TypeError",
                        format!("Assignment to constant variable '{name}'"),
                        line,
                        column,
                    ));
                }
                binding.value = value.clone();
                return Ok(value);
            }
            current.parent.clone()
        };
        if let Some(parent) = parent {
            Self::assign(&parent, name, value, line, column)
        } else {
            Err(EngineError::new(
                "ReferenceError",
                format!("{name} is not defined"),
                line,
                column,
            ))
        }
    }
}

/// A from-scratch, headless interpreter for the subset listed in the README.
///
/// This is an early implementation milestone, not an ES2020-conformant engine.
pub struct Engine {
    global: ScopeRef,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            global: Rc::new(RefCell::new(Scope::default())),
        }
    }

    pub fn eval(&mut self, source: &str) -> Result<Value, EngineError> {
        let tokens = Lexer::new(source).tokenize()?;
        let program = parser::Parser::new(tokens).parse_program()?;
        self.execute_program(&program)
    }

    pub fn eval_to_string(&mut self, source: &str) -> Result<String, EngineError> {
        self.eval(source).map(|value| value.to_js_string())
    }

    fn execute_program(&mut self, program: &Program) -> Result<Value, EngineError> {
        let mut result = Value::Undefined;
        for statement in &program.statements {
            result = self.execute_statement(statement, self.global.clone())?;
        }
        Ok(result)
    }

    fn execute_statement(
        &mut self,
        statement: &Stmt,
        scope: ScopeRef,
    ) -> Result<Value, EngineError> {
        match statement {
            Stmt::Empty => Ok(Value::Undefined),
            Stmt::Expression(expression) => self.evaluate(expression, scope),
            Stmt::Declaration {
                name,
                initializer,
                mutable,
                line,
                column,
            } => {
                let value = if let Some(initializer) = initializer {
                    self.evaluate(initializer, scope.clone())?
                } else {
                    Value::Undefined
                };
                Scope::declare(&scope, name, value, *mutable, *line, *column)?;
                Ok(Value::Undefined)
            }
            Stmt::Block(statements) => {
                let block_scope = Scope::child(scope);
                let mut result = Value::Undefined;
                for statement in statements {
                    result = self.execute_statement(statement, block_scope.clone())?;
                }
                Ok(result)
            }
            Stmt::If {
                condition,
                consequent,
                alternate,
            } => {
                if self.evaluate(condition, scope.clone())?.is_truthy() {
                    self.execute_statement(consequent, scope)
                } else if let Some(alternate) = alternate {
                    self.execute_statement(alternate, scope)
                } else {
                    Ok(Value::Undefined)
                }
            }
            Stmt::While { condition, body } => {
                let mut result = Value::Undefined;
                while self.evaluate(condition, scope.clone())?.is_truthy() {
                    result = self.execute_statement(body, scope.clone())?;
                }
                Ok(result)
            }
        }
    }

    fn evaluate(&mut self, expression: &Expr, scope: ScopeRef) -> Result<Value, EngineError> {
        match expression {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Identifier { name, line, column } => Scope::get(&scope, name).ok_or_else(|| {
                EngineError::new(
                    "ReferenceError",
                    format!("{name} is not defined"),
                    *line,
                    *column,
                )
            }),
            Expr::Assignment {
                name,
                value,
                line,
                column,
            } => {
                let value = self.evaluate(value, scope.clone())?;
                Scope::assign(&scope, name, value, *line, *column)
            }
            Expr::Unary {
                operator,
                operand,
                line,
                column,
            } => {
                if *operator == UnaryOp::TypeOf
                    && matches!(operand.as_ref(), Expr::Identifier { name, .. } if Scope::get(&scope, name).is_none())
                {
                    return Ok(Value::String("undefined".to_owned()));
                }
                let operand = self.evaluate(operand, scope)?;
                match operator {
                    UnaryOp::Not => Ok(Value::Boolean(!operand.is_truthy())),
                    UnaryOp::Plus => Ok(Value::Number(operand.to_number())),
                    UnaryOp::Minus => Ok(Value::Number(-operand.to_number())),
                    UnaryOp::TypeOf => Ok(Value::String(
                        match operand {
                            Value::Undefined => "undefined",
                            Value::Null => "object",
                            Value::Boolean(_) => "boolean",
                            Value::Number(_) => "number",
                            Value::String(_) => "string",
                        }
                        .to_owned(),
                    )),
                    UnaryOp::Void => Ok(Value::Undefined),
                    UnaryOp::Delete => Err(EngineError::new(
                        "SyntaxError",
                        "delete is not supported in this runtime subset",
                        *line,
                        *column,
                    )),
                }
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.evaluate(left, scope.clone())?;
                if *operator == BinaryOp::And && !left.is_truthy() {
                    return Ok(left);
                }
                if *operator == BinaryOp::Or && left.is_truthy() {
                    return Ok(left);
                }
                if *operator == BinaryOp::Nullish && !matches!(left, Value::Null | Value::Undefined)
                {
                    return Ok(left);
                }
                let right = self.evaluate(right, scope)?;
                self.evaluate_binary(*operator, left, right)
            }
        }
    }

    fn evaluate_binary(
        &self,
        operator: BinaryOp,
        left: Value,
        right: Value,
    ) -> Result<Value, EngineError> {
        let number = |value: f64| Ok(Value::Number(value));
        let boolean = |value| Ok(Value::Boolean(value));
        match operator {
            BinaryOp::Add => {
                if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
                    Ok(Value::String(format!(
                        "{}{}",
                        left.to_js_string(),
                        right.to_js_string()
                    )))
                } else {
                    number(left.to_number() + right.to_number())
                }
            }
            BinaryOp::Subtract => number(left.to_number() - right.to_number()),
            BinaryOp::Multiply => number(left.to_number() * right.to_number()),
            BinaryOp::Divide => number(left.to_number() / right.to_number()),
            BinaryOp::Remainder => number(left.to_number() % right.to_number()),
            BinaryOp::StrictEqual => boolean(left == right),
            BinaryOp::StrictNotEqual => boolean(left != right),
            BinaryOp::Equal | BinaryOp::NotEqual => {
                let equal = loose_equal(&left, &right);
                boolean(if operator == BinaryOp::Equal {
                    equal
                } else {
                    !equal
                })
            }
            BinaryOp::LessThan
            | BinaryOp::LessThanOrEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterThanOrEqual => {
                let ordering = match (&left, &right) {
                    (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
                    _ => left.to_number().partial_cmp(&right.to_number()),
                };
                let answer = match (operator, ordering) {
                    (BinaryOp::LessThan, Some(ordering)) => ordering.is_lt(),
                    (BinaryOp::LessThanOrEqual, Some(ordering)) => !ordering.is_gt(),
                    (BinaryOp::GreaterThan, Some(ordering)) => ordering.is_gt(),
                    (BinaryOp::GreaterThanOrEqual, Some(ordering)) => !ordering.is_lt(),
                    _ => false,
                };
                boolean(answer)
            }
            BinaryOp::And | BinaryOp::Or | BinaryOp::Nullish => Ok(right),
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

fn loose_equal(left: &Value, right: &Value) -> bool {
    if std::mem::discriminant(left) == std::mem::discriminant(right) {
        return left == right;
    }
    match (left, right) {
        (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
        (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_)) => {
            left.to_number() == right.to_number()
        }
        (Value::Boolean(_), _) => loose_equal(&Value::Number(left.to_number()), right),
        (_, Value::Boolean(_)) => loose_equal(left, &Value::Number(right.to_number())),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Engine, Value};

    #[test]
    fn evaluates_precedence_and_persistent_bindings() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval_to_string("let answer = 6 * 7; answer").unwrap(),
            "42"
        );
        assert_eq!(engine.eval_to_string("answer + 1").unwrap(), "43");
    }

    #[test]
    fn evaluates_control_flow_and_block_scopes() {
        let mut engine = Engine::new();
        let result = engine
            .eval_to_string("let sum = 0; let i = 0; while (i < 4) { sum = sum + i; i = i + 1; } if (sum === 6) 'ok';")
            .unwrap();
        assert_eq!(result, "ok");
        assert!(engine.eval("hidden").is_err());
        engine.eval("{ let hidden = 1; }").unwrap();
        assert!(engine.eval("hidden").is_err());
    }

    #[test]
    fn supports_values_and_short_circuiting() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval("null ?? 'fallback'").unwrap(),
            Value::String("fallback".into())
        );
        assert_eq!(
            engine.eval("0 || 'truthy fallback'").unwrap(),
            Value::String("truthy fallback".into())
        );
        assert_eq!(
            engine.eval("typeof undefined").unwrap(),
            Value::String("undefined".into())
        );
        assert_eq!(
            engine.eval("typeof notDeclared").unwrap(),
            Value::String("undefined".into())
        );
        assert_eq!(engine.eval("false == null").unwrap(), Value::Boolean(false));
        assert_eq!(engine.eval("false == 0").unwrap(), Value::Boolean(true));
        assert_eq!(
            engine
                .eval("let touched = 0; false && (touched = 1); touched")
                .unwrap(),
            Value::Number(0.0)
        );
    }

    #[test]
    fn accepts_line_terminated_statements_and_rejects_same_line_adjacency() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string("let first = 1\nlet second = 2\nfirst + second")
                .unwrap(),
            "3"
        );
        assert_eq!(
            engine
                .eval("let first = 1 let second = 2")
                .unwrap_err()
                .name,
            "SyntaxError"
        );
    }

    #[test]
    fn reports_syntax_reference_and_const_errors() {
        let mut engine = Engine::new();
        assert_eq!(engine.eval("let = ;").unwrap_err().name, "SyntaxError");
        assert_eq!(engine.eval("missing").unwrap_err().name, "ReferenceError");
        engine.eval("const locked = 1;").unwrap();
        assert_eq!(engine.eval("locked = 2").unwrap_err().name, "TypeError");
    }
}
