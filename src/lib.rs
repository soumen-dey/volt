mod lexer;
mod parser;

use lexer::Lexer;
use parser::{BinaryOp, Expr, Stmt, UnaryOp};
use std::{
    collections::{HashMap, HashSet},
    fmt,
};

type ObjectId = usize;
type ScopeId = usize;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Object(ObjectId),
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
            Self::Object(_) => "[object Object]".to_owned(),
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
            Self::Object(_) => true,
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
            Self::Object(_) => f64::NAN,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EngineError {
    pub name: &'static str,
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub thrown: Option<Value>,
}

impl EngineError {
    fn new(name: &'static str, message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            name,
            message: message.into(),
            line,
            column,
            thrown: None,
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

#[derive(Default)]
struct Scope {
    bindings: HashMap<String, Binding>,
    parent: Option<ScopeId>,
}

#[derive(Clone)]
enum NativeFunction {
    Object,
    ObjectCreate,
    ObjectGetPrototypeOf,
    ObjectSetPrototypeOf,
}

#[derive(Clone)]
enum Function {
    User {
        parameters: Vec<String>,
        body: Vec<Stmt>,
        closure: ScopeId,
    },
    Native(NativeFunction),
}

#[derive(Default)]
struct Object {
    properties: HashMap<String, Value>,
    prototype: Option<ObjectId>,
    function: Option<Function>,
}

enum Completion {
    Normal(Value),
    Return(Value),
}

/// A from-scratch, headless JavaScript interpreter for the subset listed in
/// the README. Objects and lexical environments are held in engine-owned
/// arenas and are reclaimed when the engine is dropped.
pub struct Engine {
    global: ScopeId,
    scopes: Vec<Scope>,
    objects: Vec<Object>,
    object_prototype: ObjectId,
}

impl Engine {
    pub fn new() -> Self {
        let mut engine = Self {
            global: 0,
            scopes: vec![Scope::default()],
            objects: Vec::new(),
            object_prototype: 0,
        };
        engine.object_prototype = engine.alloc_object(None);
        let object_constructor =
            engine.alloc_native(NativeFunction::Object, Some(engine.object_prototype));
        let create =
            engine.alloc_native(NativeFunction::ObjectCreate, Some(engine.object_prototype));
        let get_prototype = engine.alloc_native(
            NativeFunction::ObjectGetPrototypeOf,
            Some(engine.object_prototype),
        );
        let set_prototype = engine.alloc_native(
            NativeFunction::ObjectSetPrototypeOf,
            Some(engine.object_prototype),
        );
        engine.set_property(
            object_constructor,
            "prototype",
            Value::Object(engine.object_prototype),
        );
        engine.set_property(object_constructor, "create", Value::Object(create));
        engine.set_property(
            object_constructor,
            "getPrototypeOf",
            Value::Object(get_prototype),
        );
        engine.set_property(
            object_constructor,
            "setPrototypeOf",
            Value::Object(set_prototype),
        );
        engine.scopes[engine.global].bindings.insert(
            "Object".to_owned(),
            Binding {
                value: Value::Object(object_constructor),
                mutable: false,
            },
        );
        Self::declare_global(&mut engine, "undefined", Value::Undefined, false);
        Self::declare_global(&mut engine, "NaN", Value::Number(f64::NAN), false);
        Self::declare_global(&mut engine, "Infinity", Value::Number(f64::INFINITY), false);
        engine
    }

    fn declare_global(&mut self, name: &str, value: Value, mutable: bool) {
        self.scopes[self.global]
            .bindings
            .insert(name.to_owned(), Binding { value, mutable });
    }

    fn alloc_object(&mut self, prototype: Option<ObjectId>) -> ObjectId {
        let id = self.objects.len();
        self.objects.push(Object {
            prototype,
            ..Object::default()
        });
        id
    }

    fn alloc_native(&mut self, native: NativeFunction, prototype: Option<ObjectId>) -> ObjectId {
        let id = self.alloc_object(prototype);
        self.objects[id].function = Some(Function::Native(native));
        id
    }

    fn alloc_user_function(
        &mut self,
        parameters: Vec<String>,
        body: Vec<Stmt>,
        closure: ScopeId,
    ) -> ObjectId {
        let id = self.alloc_object(Some(self.object_prototype));
        self.objects[id].function = Some(Function::User {
            parameters,
            body,
            closure,
        });
        let prototype = self.alloc_object(Some(self.object_prototype));
        self.set_property(prototype, "constructor", Value::Object(id));
        self.set_property(id, "prototype", Value::Object(prototype));
        id
    }

    fn new_scope(&mut self, parent: ScopeId) -> ScopeId {
        let id = self.scopes.len();
        self.scopes.push(Scope {
            bindings: HashMap::new(),
            parent: Some(parent),
        });
        id
    }

    fn declare(
        &mut self,
        scope: ScopeId,
        name: &str,
        value: Value,
        mutable: bool,
        line: usize,
        column: usize,
    ) -> Result<(), EngineError> {
        if self.scopes[scope].bindings.contains_key(name) {
            return Err(EngineError::new(
                "SyntaxError",
                format!("Identifier '{name}' has already been declared"),
                line,
                column,
            ));
        }
        self.scopes[scope]
            .bindings
            .insert(name.to_owned(), Binding { value, mutable });
        Ok(())
    }

    fn get_binding(&self, mut scope: ScopeId, name: &str) -> Option<Value> {
        loop {
            if let Some(binding) = self.scopes[scope].bindings.get(name) {
                return Some(binding.value.clone());
            }
            scope = self.scopes[scope].parent?;
        }
    }

    fn assign_binding(
        &mut self,
        mut scope: ScopeId,
        name: &str,
        value: Value,
        line: usize,
        column: usize,
    ) -> Result<Value, EngineError> {
        loop {
            if let Some(binding) = self.scopes[scope].bindings.get_mut(name) {
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
            let Some(parent) = self.scopes[scope].parent else {
                return Err(EngineError::new(
                    "ReferenceError",
                    format!("{name} is not defined"),
                    line,
                    column,
                ));
            };
            scope = parent;
        }
    }

    pub fn eval(&mut self, source: &str) -> Result<Value, EngineError> {
        let tokens = Lexer::new(source).tokenize()?;
        let program = parser::Parser::new(tokens).parse_program()?;
        match self.execute_statements(&program.statements, self.global)? {
            Completion::Normal(value) => Ok(value),
            Completion::Return(_) => Err(EngineError::new(
                "SyntaxError",
                "Return statement outside function",
                1,
                1,
            )),
        }
    }

    pub fn eval_to_string(&mut self, source: &str) -> Result<String, EngineError> {
        let value = self.eval(source)?;
        Ok(self.to_string(&value))
    }

    fn to_string(&self, value: &Value) -> String {
        match value {
            Value::Object(id) => {
                if let Some(function) = &self.objects[*id].function {
                    match function {
                        Function::User { .. } | Function::Native(_) => {
                            "function () { [native code] }".to_owned()
                        }
                    }
                } else {
                    "[object Object]".to_owned()
                }
            }
            _ => value.to_js_string(),
        }
    }

    fn execute_statements(
        &mut self,
        statements: &[Stmt],
        scope: ScopeId,
    ) -> Result<Completion, EngineError> {
        let mut result = Value::Undefined;
        for statement in statements {
            match self.execute_statement(statement, scope)? {
                Completion::Normal(value) => result = value,
                completion @ Completion::Return(_) => return Ok(completion),
            }
        }
        Ok(Completion::Normal(result))
    }

    fn execute_statement(
        &mut self,
        statement: &Stmt,
        scope: ScopeId,
    ) -> Result<Completion, EngineError> {
        match statement {
            Stmt::Empty => Ok(Completion::Normal(Value::Undefined)),
            Stmt::Expression(expression) => {
                self.evaluate(expression, scope).map(Completion::Normal)
            }
            Stmt::Declaration {
                name,
                initializer,
                mutable,
                line,
                column,
            } => {
                let value = if let Some(initializer) = initializer {
                    self.evaluate(initializer, scope)?
                } else {
                    Value::Undefined
                };
                self.declare(scope, name, value, *mutable, *line, *column)?;
                Ok(Completion::Normal(Value::Undefined))
            }
            Stmt::FunctionDeclaration {
                name,
                parameters,
                body,
            } => {
                let function = self.alloc_user_function(parameters.clone(), body.clone(), scope);
                self.declare(scope, name, Value::Object(function), true, 1, 1)?;
                Ok(Completion::Normal(Value::Undefined))
            }
            Stmt::Block(statements) => {
                let block_scope = self.new_scope(scope);
                self.execute_statements(statements, block_scope)
            }
            Stmt::If {
                condition,
                consequent,
                alternate,
            } => {
                if self.evaluate(condition, scope)?.is_truthy() {
                    self.execute_statement(consequent, scope)
                } else if let Some(alternate) = alternate {
                    self.execute_statement(alternate, scope)
                } else {
                    Ok(Completion::Normal(Value::Undefined))
                }
            }
            Stmt::While { condition, body } => {
                let mut result = Value::Undefined;
                while self.evaluate(condition, scope)?.is_truthy() {
                    match self.execute_statement(body, scope)? {
                        Completion::Normal(value) => result = value,
                        completion @ Completion::Return(_) => return Ok(completion),
                    }
                }
                Ok(Completion::Normal(result))
            }
            Stmt::Return(value) => {
                let value = if let Some(value) = value {
                    self.evaluate(value, scope)?
                } else {
                    Value::Undefined
                };
                Ok(Completion::Return(value))
            }
            Stmt::Throw(expression) => {
                let value = self.evaluate(expression, scope)?;
                Err(self.thrown_error(value, 1, 1))
            }
            Stmt::Try {
                body,
                catch_name,
                catch_body,
                finally_body,
            } => {
                let result = match self.execute_statement(body, scope) {
                    Ok(result) => Ok(result),
                    Err(error) => {
                        if let (Some(name), Some(catch_body)) = (catch_name, catch_body) {
                            let catch_scope = self.new_scope(scope);
                            let exception = if let Some(value) = error.thrown.clone() {
                                value
                            } else {
                                self.error_object(&error)
                            };
                            self.declare(catch_scope, name, exception, true, 1, 1)?;
                            self.execute_statement(catch_body, catch_scope)
                        } else {
                            Err(error)
                        }
                    }
                };
                if let Some(finally_body) = finally_body {
                    match self.execute_statement(finally_body, scope)? {
                        Completion::Normal(_) => result,
                        abrupt => Ok(abrupt),
                    }
                } else {
                    result
                }
            }
        }
    }

    fn evaluate(&mut self, expression: &Expr, scope: ScopeId) -> Result<Value, EngineError> {
        match expression {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::This => Ok(self.get_binding(scope, "this").unwrap_or(Value::Undefined)),
            Expr::Identifier { name, line, column } => {
                self.get_binding(scope, name).ok_or_else(|| {
                    EngineError::new(
                        "ReferenceError",
                        format!("{name} is not defined"),
                        *line,
                        *column,
                    )
                })
            }
            Expr::Assignment {
                target,
                value,
                line,
                column,
            } => {
                let value = self.evaluate(value, scope)?;
                match target.as_ref() {
                    Expr::Identifier { name, .. } => {
                        self.assign_binding(scope, name, value, *line, *column)
                    }
                    Expr::Member(object, property) => {
                        let object = self.evaluate(object, scope)?;
                        self.set_property_value(object, property, value.clone(), *line, *column)?;
                        Ok(value)
                    }
                    Expr::Index(object, property) => {
                        let object = self.evaluate(object, scope)?;
                        let property = self.evaluate(property, scope)?.to_js_string();
                        self.set_property_value(object, &property, value.clone(), *line, *column)?;
                        Ok(value)
                    }
                    _ => unreachable!("parser validates assignment targets"),
                }
            }
            Expr::Unary {
                operator,
                operand,
                line,
                column,
            } => {
                if *operator == UnaryOp::TypeOf
                    && matches!(operand.as_ref(), Expr::Identifier { name, .. } if self.get_binding(scope, name).is_none())
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
                            Value::Object(id) if self.objects[id].function.is_some() => "function",
                            Value::Object(_) => "object",
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
                let left = self.evaluate(left, scope)?;
                if *operator == BinaryOp::And && !left.is_truthy()
                    || *operator == BinaryOp::Or && left.is_truthy()
                    || *operator == BinaryOp::Nullish
                        && !matches!(left, Value::Null | Value::Undefined)
                {
                    return Ok(left);
                }
                let right = self.evaluate(right, scope)?;
                self.evaluate_binary(*operator, left, right)
            }
            Expr::Function {
                name,
                parameters,
                body,
            } => {
                let function = self.alloc_user_function(parameters.clone(), body.clone(), scope);
                if let Some(name) = name {
                    let name_scope = self.new_scope(scope);
                    self.declare(name_scope, name, Value::Object(function), false, 1, 1)?;
                    self.objects[function].function = Some(Function::User {
                        parameters: parameters.clone(),
                        body: body.clone(),
                        closure: name_scope,
                    });
                }
                Ok(Value::Object(function))
            }
            Expr::Object(properties) => {
                let object = self.alloc_object(Some(self.object_prototype));
                for (name, expression) in properties {
                    let value = self.evaluate(expression, scope)?;
                    self.set_property(object, name, value);
                }
                Ok(Value::Object(object))
            }
            Expr::Member(object, property) => {
                let object = self.evaluate(object, scope)?;
                self.get_property_value(object, property, 1, 1)
            }
            Expr::Index(object, property) => {
                let object = self.evaluate(object, scope)?;
                let property = self.evaluate(property, scope)?.to_js_string();
                self.get_property_value(object, &property, 1, 1)
            }
            Expr::Call(callee, arguments) => {
                let (function, this_value) = match callee.as_ref() {
                    Expr::Member(object, property) => {
                        let this_value = self.evaluate(object, scope)?;
                        let function =
                            self.get_property_value(this_value.clone(), property, 1, 1)?;
                        (function, this_value)
                    }
                    Expr::Index(object, property) => {
                        let this_value = self.evaluate(object, scope)?;
                        let property = self.evaluate(property, scope)?.to_js_string();
                        let function =
                            self.get_property_value(this_value.clone(), &property, 1, 1)?;
                        (function, this_value)
                    }
                    _ => (self.evaluate(callee, scope)?, Value::Undefined),
                };
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(self.evaluate(argument, scope)?);
                }
                self.call(function, this_value, values)
            }
            Expr::Construct(callee, arguments) => {
                let callee = self.evaluate(callee, scope)?;
                let Value::Object(function_id) = callee.clone() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Value is not a constructor",
                        1,
                        1,
                    ));
                };
                if self.objects[function_id].function.is_none() {
                    return Err(EngineError::new(
                        "TypeError",
                        "Value is not a constructor",
                        1,
                        1,
                    ));
                }
                let prototype = match self.get_property(function_id, "prototype") {
                    Some(Value::Object(prototype)) => Some(prototype),
                    _ => Some(self.object_prototype),
                };
                let instance = self.alloc_object(prototype);
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(self.evaluate(argument, scope)?);
                }
                match self.call(callee, Value::Object(instance), values)? {
                    object @ Value::Object(_) => Ok(object),
                    _ => Ok(Value::Object(instance)),
                }
            }
        }
    }

    fn evaluate_binary(
        &self,
        operator: BinaryOp,
        left: Value,
        right: Value,
    ) -> Result<Value, EngineError> {
        let number = |value| Ok(Value::Number(value));
        let boolean = |value| Ok(Value::Boolean(value));
        match operator {
            BinaryOp::Add => {
                if matches!(left, Value::String(_) | Value::Object(_))
                    || matches!(right, Value::String(_) | Value::Object(_))
                {
                    Ok(Value::String(format!(
                        "{}{}",
                        self.to_string(&left),
                        self.to_string(&right)
                    )))
                } else {
                    number(left.to_number() + right.to_number())
                }
            }
            BinaryOp::Subtract => number(left.to_number() - right.to_number()),
            BinaryOp::Multiply => number(left.to_number() * right.to_number()),
            BinaryOp::Divide => number(left.to_number() / right.to_number()),
            BinaryOp::Remainder => number(left.to_number() % right.to_number()),
            BinaryOp::StrictEqual => boolean(self.strict_equal(&left, &right)),
            BinaryOp::StrictNotEqual => boolean(!self.strict_equal(&left, &right)),
            BinaryOp::Equal | BinaryOp::NotEqual => {
                let equal = self.loose_equal(&left, &right);
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
                boolean(match (operator, ordering) {
                    (BinaryOp::LessThan, Some(ordering)) => ordering.is_lt(),
                    (BinaryOp::LessThanOrEqual, Some(ordering)) => !ordering.is_gt(),
                    (BinaryOp::GreaterThan, Some(ordering)) => ordering.is_gt(),
                    (BinaryOp::GreaterThanOrEqual, Some(ordering)) => !ordering.is_lt(),
                    _ => false,
                })
            }
            BinaryOp::And | BinaryOp::Or | BinaryOp::Nullish => Ok(right),
        }
    }

    fn strict_equal(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Number(left), Value::Number(right)) => {
                !left.is_nan() && !right.is_nan() && left == right
            }
            (Value::Object(left), Value::Object(right)) => left == right,
            _ => left == right,
        }
    }

    fn loose_equal(&self, left: &Value, right: &Value) -> bool {
        if std::mem::discriminant(left) == std::mem::discriminant(right) {
            return self.strict_equal(left, right);
        }
        match (left, right) {
            (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
            (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_)) => {
                left.to_number() == right.to_number()
            }
            (Value::Boolean(_), _) => self.loose_equal(&Value::Number(left.to_number()), right),
            (_, Value::Boolean(_)) => self.loose_equal(left, &Value::Number(right.to_number())),
            (Value::Object(id), _) => {
                self.loose_equal(&Value::String(self.to_string(&Value::Object(*id))), right)
            }
            (_, Value::Object(id)) => {
                self.loose_equal(left, &Value::String(self.to_string(&Value::Object(*id))))
            }
            _ => false,
        }
    }

    fn set_property(&mut self, object: ObjectId, name: &str, value: Value) {
        self.objects[object]
            .properties
            .insert(name.to_owned(), value);
    }

    fn get_own_property(&self, object: ObjectId, name: &str) -> Option<Value> {
        self.objects[object].properties.get(name).cloned()
    }

    fn get_property(&self, object: ObjectId, name: &str) -> Option<Value> {
        let mut current = Some(object);
        let mut visited = HashSet::new();
        while let Some(id) = current {
            if !visited.insert(id) {
                return None;
            }
            if let Some(value) = self.get_own_property(id, name) {
                return Some(value);
            }
            current = self.objects[id].prototype;
        }
        None
    }

    fn get_property_value(
        &self,
        value: Value,
        name: &str,
        line: usize,
        column: usize,
    ) -> Result<Value, EngineError> {
        match value {
            Value::Object(id) => Ok(self.get_property(id, name).unwrap_or(Value::Undefined)),
            Value::Null | Value::Undefined => Err(EngineError::new(
                "TypeError",
                format!("Cannot read properties of {}", value.to_js_string()),
                line,
                column,
            )),
            _ => Ok(Value::Undefined),
        }
    }

    fn set_property_value(
        &mut self,
        value: Value,
        name: &str,
        property: Value,
        line: usize,
        column: usize,
    ) -> Result<(), EngineError> {
        if let Value::Object(id) = value {
            self.set_property(id, name, property);
            Ok(())
        } else {
            Err(EngineError::new(
                "TypeError",
                "Cannot set a property on a non-object value",
                line,
                column,
            ))
        }
    }

    fn call(
        &mut self,
        callee: Value,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Value::Object(id) = callee else {
            return Err(EngineError::new("TypeError", "Value is not callable", 1, 1));
        };
        let Some(function) = self.objects[id].function.clone() else {
            return Err(EngineError::new("TypeError", "Value is not callable", 1, 1));
        };
        match function {
            Function::User {
                parameters,
                body,
                closure,
            } => {
                let function_scope = self.new_scope(closure);
                self.declare(function_scope, "this", this_value, true, 1, 1)?;
                for (index, parameter) in parameters.iter().enumerate() {
                    let value = arguments.get(index).cloned().unwrap_or(Value::Undefined);
                    self.declare(function_scope, parameter, value, true, 1, 1)?;
                }
                match self.execute_statements(&body, function_scope)? {
                    Completion::Normal(_) => Ok(Value::Undefined),
                    Completion::Return(value) => Ok(value),
                }
            }
            Function::Native(native) => self.call_native(native, arguments),
        }
    }

    fn call_native(
        &mut self,
        native: NativeFunction,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        match native {
            NativeFunction::Object => match arguments.first() {
                None | Some(Value::Undefined | Value::Null) => Ok(Value::Object(
                    self.alloc_object(Some(self.object_prototype)),
                )),
                Some(value @ Value::Object(_)) => Ok(value.clone()),
                Some(_) => Ok(Value::Object(
                    self.alloc_object(Some(self.object_prototype)),
                )),
            },
            NativeFunction::ObjectCreate => {
                let prototype = match arguments.first().unwrap_or(&Value::Undefined) {
                    Value::Null => None,
                    Value::Object(id) => Some(*id),
                    _ => {
                        return Err(EngineError::new(
                            "TypeError",
                            "Object prototype must be an object or null",
                            1,
                            1,
                        ));
                    }
                };
                Ok(Value::Object(self.alloc_object(prototype)))
            }
            NativeFunction::ObjectGetPrototypeOf => match arguments.first() {
                Some(Value::Object(id)) => Ok(self.objects[*id]
                    .prototype
                    .map(Value::Object)
                    .unwrap_or(Value::Null)),
                _ => Err(EngineError::new(
                    "TypeError",
                    "Object.getPrototypeOf requires an object",
                    1,
                    1,
                )),
            },
            NativeFunction::ObjectSetPrototypeOf => {
                let Some(Value::Object(id)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.setPrototypeOf requires an object",
                        1,
                        1,
                    ));
                };
                let prototype = match arguments.get(1).unwrap_or(&Value::Undefined) {
                    Value::Null => None,
                    Value::Object(prototype) => Some(*prototype),
                    _ => {
                        return Err(EngineError::new(
                            "TypeError",
                            "Object prototype must be an object or null",
                            1,
                            1,
                        ));
                    }
                };
                let mut current = prototype;
                let mut visited = HashSet::new();
                while let Some(prototype_id) = current {
                    if prototype_id == *id {
                        return Err(EngineError::new(
                            "TypeError",
                            "Cyclic prototype value",
                            1,
                            1,
                        ));
                    }
                    if !visited.insert(prototype_id) {
                        return Err(EngineError::new(
                            "TypeError",
                            "Prototype chain is already cyclic",
                            1,
                            1,
                        ));
                    }
                    current = self.objects[prototype_id].prototype;
                }
                self.objects[*id].prototype = prototype;
                Ok(Value::Object(*id))
            }
        }
    }

    fn error_object(&mut self, error: &EngineError) -> Value {
        let object = self.alloc_object(Some(self.object_prototype));
        self.set_property(object, "name", Value::String(error.name.to_owned()));
        self.set_property(object, "message", Value::String(error.message.clone()));
        Value::Object(object)
    }

    fn thrown_error(&self, value: Value, line: usize, column: usize) -> EngineError {
        let message = self.to_string(&value);
        EngineError {
            name: "Error",
            message: format!("Uncaught {message}"),
            line,
            column,
            thrown: Some(value),
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
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
            engine.eval("typeof notDeclared").unwrap(),
            Value::String("undefined".into())
        );
        assert_eq!(engine.eval("false == null").unwrap(), Value::Boolean(false));
        assert_eq!(engine.eval("false == 0").unwrap(), Value::Boolean(true));
    }

    #[test]
    fn functions_capture_and_mutate_lexical_bindings() {
        let mut engine = Engine::new();
        let value = engine.eval_to_string(
            "function makeCounter() { let n = 0; return function() { n = n + 1; return n; }; } let counter = makeCounter(); counter(); counter()",
        ).unwrap();
        assert_eq!(value, "2");
    }

    #[test]
    fn objects_support_properties_prototypes_and_method_this() {
        let mut engine = Engine::new();
        let value = engine.eval_to_string(
            "let prototype = { inherited: 7 }; let object = Object.create(prototype); object.own = 2; object.total = function() { return this.inherited + this.own; }; object.total()",
        ).unwrap();
        assert_eq!(value, "9");
        assert_eq!(
            engine
                .eval("Object.getPrototypeOf(object) === prototype")
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine
                .eval("Object.setPrototypeOf(prototype, object)")
                .unwrap_err()
                .name,
            "TypeError"
        );
        assert_eq!(
            engine
                .eval("Object.getPrototypeOf(Object) === Object.prototype")
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "function Box(value) { this.value = value; } let box = new Box(11); box.value",
                )
                .unwrap(),
            "11"
        );
        assert_eq!(
            engine
                .eval("Object.getPrototypeOf(box) === Box.prototype")
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn catches_runtime_errors_as_error_objects() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let name = ''; try { missingName; } catch (error) { name = error.name; } name",
                )
                .unwrap(),
            "ReferenceError"
        );
    }

    #[test]
    fn exceptions_are_caught_and_finally_runs() {
        let mut engine = Engine::new();
        let value = engine
            .eval_to_string("let state = 0; try { throw 'failure'; } catch (error) { state = error; } finally { state = state + '!'; } state")
            .unwrap();
        assert_eq!(value, "failure!");
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
