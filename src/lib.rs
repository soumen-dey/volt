mod lexer;
mod parser;

use lexer::Lexer;
use parser::{BinaryOp, DeclarationKind, Expr, Stmt, UnaryOp};
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
    initialized: bool,
    var_binding: bool,
}

#[derive(Default)]
struct Scope {
    bindings: HashMap<String, Binding>,
    parent: Option<ScopeId>,
    var_scope: bool,
}

#[derive(Clone)]
enum NativeFunction {
    Object,
    ObjectCreate,
    ObjectGetPrototypeOf,
    ObjectSetPrototypeOf,
    Error(&'static str),
}

#[derive(Clone)]
enum Function {
    User {
        parameters: Vec<String>,
        body: Vec<Stmt>,
        closure: ScopeId,
        strict: bool,
    },
    Native(NativeFunction),
}

#[derive(Default)]
struct Object {
    properties: HashMap<String, Value>,
    parameter_map: HashMap<String, (ScopeId, String)>,
    prototype: Option<ObjectId>,
    function: Option<Function>,
}

enum Completion {
    Normal(Value),
    Return(Value),
    Break,
    Continue,
}

/// A from-scratch, headless JavaScript interpreter for the subset listed in
/// the README. Objects and lexical environments are held in engine-owned
/// arenas and are reclaimed when the engine is dropped.
pub struct Engine {
    global: ScopeId,
    scopes: Vec<Scope>,
    objects: Vec<Object>,
    object_prototype: ObjectId,
    global_object: ObjectId,
    error_prototypes: HashMap<&'static str, ObjectId>,
    strict: bool,
}

impl Engine {
    pub fn new() -> Self {
        let mut engine = Self {
            global: 0,
            scopes: vec![Scope {
                var_scope: true,
                ..Scope::default()
            }],
            objects: Vec::new(),
            object_prototype: 0,
            global_object: 0,
            error_prototypes: HashMap::new(),
            strict: false,
        };
        engine.object_prototype = engine.alloc_object(None);
        engine.global_object = engine.alloc_object(Some(engine.object_prototype));
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
        engine.set_property(
            object_constructor,
            "name",
            Value::String("Object".to_owned()),
        );
        engine.set_property(object_constructor, "length", Value::Number(1.0));
        let object_prototype = engine.object_prototype;
        engine.set_property(
            object_prototype,
            "constructor",
            Value::Object(object_constructor),
        );
        engine.declare_global("Object", Value::Object(object_constructor), false);
        Self::declare_global(&mut engine, "undefined", Value::Undefined, false);
        Self::declare_global(&mut engine, "NaN", Value::Number(f64::NAN), false);
        Self::declare_global(&mut engine, "Infinity", Value::Number(f64::INFINITY), false);
        let global_object = engine.global_object;
        Self::declare_global(
            &mut engine,
            "globalThis",
            Value::Object(global_object),
            false,
        );
        engine.install_error_constructors();
        engine
    }

    fn declare_global(&mut self, name: &str, value: Value, mutable: bool) {
        self.scopes[self.global].bindings.insert(
            name.to_owned(),
            Binding {
                value: value.clone(),
                mutable,
                initialized: true,
                var_binding: false,
            },
        );
        self.set_property(self.global_object, name, value);
    }

    fn install_error_constructors(&mut self) {
        let error_base = self.alloc_object(Some(self.object_prototype));
        self.set_property(error_base, "name", Value::String("Error".to_owned()));
        self.set_property(error_base, "message", Value::String(String::new()));
        self.error_prototypes.insert("Error", error_base);
        for name in ["Error", "TypeError", "ReferenceError", "SyntaxError"] {
            let constructor =
                self.alloc_native(NativeFunction::Error(name), Some(self.object_prototype));
            let prototype = if name == "Error" {
                error_base
            } else {
                let prototype = self.alloc_object(Some(error_base));
                self.error_prototypes.insert(name, prototype);
                prototype
            };
            self.set_property(prototype, "name", Value::String(name.to_owned()));
            self.set_property(prototype, "message", Value::String(String::new()));
            self.set_property(prototype, "constructor", Value::Object(constructor));
            self.set_property(constructor, "prototype", Value::Object(prototype));
            self.set_property(constructor, "name", Value::String(name.to_owned()));
            self.set_property(constructor, "length", Value::Number(1.0));
            self.declare_global(name, Value::Object(constructor), false);
        }
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
        strict: bool,
    ) -> ObjectId {
        let id = self.alloc_object(Some(self.object_prototype));
        self.objects[id].function = Some(Function::User {
            parameters: parameters.clone(),
            body,
            closure,
            strict: strict || self.strict,
        });
        self.set_property(id, "name", Value::String(String::new()));
        self.set_property(id, "length", Value::Number(parameters.len() as f64));
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
            var_scope: false,
        });
        id
    }

    fn new_function_scope(&mut self, parent: ScopeId) -> ScopeId {
        let id = self.new_scope(parent);
        self.scopes[id].var_scope = true;
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

        self.scopes[scope].bindings.insert(
            name.to_owned(),
            Binding {
                value,
                mutable,
                initialized: true,
                var_binding: false,
            },
        );
        Ok(())
    }

    fn declare_var_if_absent(&mut self, scope: ScopeId, name: &str) -> Result<(), EngineError> {
        if let Some(binding) = self.scopes[scope].bindings.get(name) {
            if !self.scopes[scope].var_scope || !binding.var_binding {
                return Err(EngineError::new(
                    "SyntaxError",
                    format!("Identifier '{name}' has already been declared"),
                    1,
                    1,
                ));
            }
            return Ok(());
        }
        let initial_value = if scope == self.global {
            self.get_own_property(self.global_object, name)
                .unwrap_or(Value::Undefined)
        } else {
            Value::Undefined
        };
        self.declare(scope, name, initial_value.clone(), true, 1, 1)?;
        self.scopes[scope]
            .bindings
            .get_mut(name)
            .expect("new var binding was just inserted")
            .var_binding = true;
        if scope == self.global {
            self.set_property(self.global_object, name, initial_value);
        }
        Ok(())
    }

    fn declare_tdz(
        &mut self,
        scope: ScopeId,
        name: &str,
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
        self.scopes[scope].bindings.insert(
            name.to_owned(),
            Binding {
                value: Value::Undefined,
                mutable,
                initialized: false,
                var_binding: false,
            },
        );
        Ok(())
    }

    fn initialize_binding(&mut self, scope: ScopeId, name: &str, value: Value) {
        let binding = self.scopes[scope]
            .bindings
            .get_mut(name)
            .expect("binding was instantiated before initialization");
        binding.value = value;
        binding.initialized = true;
    }

    fn var_scope(&self, mut scope: ScopeId) -> ScopeId {
        loop {
            if self.scopes[scope].var_scope {
                return scope;
            }
            scope = self.scopes[scope]
                .parent
                .expect("every non-root scope has a parent");
        }
    }

    fn find_binding_scope(&self, mut scope: ScopeId, name: &str) -> Option<ScopeId> {
        loop {
            if self.scopes[scope].bindings.contains_key(name) {
                return Some(scope);
            }
            scope = self.scopes[scope].parent?;
        }
    }

    fn get_binding(
        &self,
        scope: ScopeId,
        name: &str,
        line: usize,
        column: usize,
    ) -> Result<Option<Value>, EngineError> {
        if let Some(binding_scope) = self.find_binding_scope(scope, name) {
            let binding = &self.scopes[binding_scope].bindings[name];
            if !binding.initialized {
                return Err(EngineError::new(
                    "ReferenceError",
                    format!("Cannot access '{name}' before initialization"),
                    line,
                    column,
                ));
            }
            return Ok(Some(binding.value.clone()));
        }
        Ok(self.get_own_property(self.global_object, name))
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
                if !binding.initialized {
                    return Err(EngineError::new(
                        "ReferenceError",
                        format!("Cannot access '{name}' before initialization"),
                        line,
                        column,
                    ));
                }
                if !binding.mutable {
                    return Err(EngineError::new(
                        "TypeError",
                        format!("Assignment to constant variable '{name}'"),
                        line,
                        column,
                    ));
                }
                binding.value = value.clone();
                let is_global = scope == self.global;
                let is_var_binding = binding.var_binding;
                let _ = binding;
                if is_global && is_var_binding {
                    self.set_property(self.global_object, name, value.clone());
                }
                return Ok(value);
            }
            let Some(parent) = self.scopes[scope].parent else {
                if !self.strict {
                    self.set_property(self.global_object, name, value.clone());
                    return Ok(value);
                }
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
        let previous_strict = self.strict;
        self.strict = program.strict;
        let execution = self.execute_statements(&program.statements, self.global);
        self.strict = previous_strict;
        match execution? {
            Completion::Normal(value) => Ok(value),
            Completion::Return(_) => Err(EngineError::new(
                "SyntaxError",
                "Return statement outside function",
                1,
                1,
            )),
            Completion::Break | Completion::Continue => Err(EngineError::new(
                "SyntaxError",
                "Invalid loop control statement",
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
                } else if self.is_error_object(*id) {
                    let name = self
                        .get_property(*id, "name")
                        .map(|value| value.to_js_string())
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| "Error".to_owned());
                    let message = self
                        .get_property(*id, "message")
                        .map(|value| value.to_js_string())
                        .unwrap_or_default();
                    if message.is_empty() {
                        name
                    } else {
                        format!("{name}: {message}")
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
        self.instantiate_declarations(statements, scope)?;
        let mut result = Value::Undefined;
        for statement in statements {
            match self.execute_statement(statement, scope)? {
                Completion::Normal(value) => result = value,
                completion @ (Completion::Return(_) | Completion::Break | Completion::Continue) => {
                    return Ok(completion);
                }
            }
        }
        Ok(Completion::Normal(result))
    }

    fn instantiate_declarations(
        &mut self,
        statements: &[Stmt],
        scope: ScopeId,
    ) -> Result<(), EngineError> {
        let target = self.var_scope(scope);
        let mut vars = Vec::new();
        collect_var_names(statements, &mut vars);
        for statement in statements {
            if let Stmt::Declaration {
                name,
                kind: DeclarationKind::Let | DeclarationKind::Const,
                line,
                column,
                ..
            } = statement
                && vars.iter().any(|var_name| var_name == name)
            {
                return Err(EngineError::new(
                    "SyntaxError",
                    format!("Identifier '{name}' is declared as both lexical and var"),
                    *line,
                    *column,
                ));
            }
        }
        for name in vars {
            self.declare_var_if_absent(target, &name)?;
        }
        for statement in statements {
            match statement {
                Stmt::Declaration {
                    name,
                    kind: DeclarationKind::Let | DeclarationKind::Const,
                    line,
                    column,
                    ..
                } => self.declare_tdz(
                    scope,
                    name,
                    matches!(
                        statement,
                        Stmt::Declaration {
                            kind: DeclarationKind::Let,
                            ..
                        }
                    ),
                    *line,
                    *column,
                )?,
                Stmt::FunctionDeclaration {
                    name,
                    parameters,
                    body,
                    strict,
                } => {
                    let function =
                        self.alloc_user_function(parameters.clone(), body.clone(), scope, *strict);
                    if let Some(binding) = self.scopes[scope].bindings.get_mut(name) {
                        if !binding.var_binding {
                            return Err(EngineError::new(
                                "SyntaxError",
                                format!("Identifier '{name}' has already been declared"),
                                1,
                                1,
                            ));
                        }
                        binding.value = Value::Object(function);
                        binding.initialized = true;
                    } else {
                        self.declare(scope, name, Value::Object(function), true, 1, 1)?;
                        self.scopes[scope]
                            .bindings
                            .get_mut(name)
                            .expect("function binding was just inserted")
                            .var_binding = self.scopes[scope].var_scope;
                    }
                    if scope == self.global {
                        self.set_property(self.global_object, name, Value::Object(function));
                    }
                    self.set_property(function, "name", Value::String(name.clone()));
                }
                _ => {}
            }
        }
        Ok(())
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
                kind,
                line: _,
                column: _,
            } => {
                let value = if let Some(initializer) = initializer {
                    self.evaluate(initializer, scope)?
                } else {
                    Value::Undefined
                };
                match kind {
                    DeclarationKind::Var => {
                        let target = self.var_scope(scope);
                        if initializer.is_some() {
                            self.initialize_binding(target, name, value.clone());
                            if target == self.global {
                                self.set_property(self.global_object, name, value.clone());
                            }
                        }
                    }
                    DeclarationKind::Let | DeclarationKind::Const => {
                        if !self.scopes[scope].bindings.contains_key(name) {
                            return Err(EngineError::new(
                                "SyntaxError",
                                "Lexical declaration is not valid in this statement position",
                                1,
                                1,
                            ));
                        }
                        self.initialize_binding(scope, name, value.clone());
                    }
                }
                if let Value::Object(function) = &value
                    && self.objects[*function].function.is_some()
                    && matches!(initializer, Some(Expr::Function { name: None, .. }))
                {
                    self.set_property(*function, "name", Value::String(name.clone()));
                }
                Ok(Completion::Normal(Value::Undefined))
            }
            Stmt::FunctionDeclaration { .. } => Ok(Completion::Normal(Value::Undefined)),
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
                        Completion::Break => break,
                        Completion::Continue => continue,
                    }
                }
                Ok(Completion::Normal(result))
            }
            Stmt::For {
                initializer,
                condition,
                update,
                body,
            } => {
                let mut loop_scope = self.new_scope(scope);
                let per_iteration = matches!(
                    initializer.as_deref(),
                    Some(Stmt::Declaration {
                        kind: DeclarationKind::Let | DeclarationKind::Const,
                        ..
                    })
                );
                if let Some(initializer) = initializer {
                    let completion =
                        self.execute_statements(std::slice::from_ref(initializer), loop_scope)?;
                    if !matches!(completion, Completion::Normal(_)) {
                        return Ok(completion);
                    }
                }
                let mut result = Value::Undefined;
                while match condition {
                    Some(condition) => self.evaluate(condition, loop_scope)?.is_truthy(),
                    None => true,
                } {
                    match self.execute_statement(body, loop_scope)? {
                        Completion::Normal(value) => result = value,
                        completion @ Completion::Return(_) => return Ok(completion),
                        Completion::Break => break,
                        Completion::Continue => {}
                    }
                    if per_iteration {
                        let next_scope = self.new_scope(scope);
                        self.scopes[next_scope].bindings = self.scopes[loop_scope].bindings.clone();
                        loop_scope = next_scope;
                    }
                    if let Some(update) = update {
                        result = self.evaluate(update, loop_scope)?;
                    }
                }
                Ok(Completion::Normal(result))
            }
            Stmt::DoWhile { body, condition } => {
                let mut result = Value::Undefined;
                loop {
                    match self.execute_statement(body, scope)? {
                        Completion::Normal(value) => result = value,
                        completion @ Completion::Return(_) => return Ok(completion),
                        Completion::Break => break,
                        Completion::Continue => {}
                    }
                    if !self.evaluate(condition, scope)?.is_truthy() {
                        break;
                    }
                }
                Ok(Completion::Normal(result))
            }
            Stmt::Break => Ok(Completion::Break),
            Stmt::Continue => Ok(Completion::Continue),
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
            Expr::This => match self.get_binding(scope, "this", 1, 1)? {
                Some(value) => Ok(value),
                None if self.strict => Ok(Value::Undefined),
                None => Ok(Value::Object(self.global_object)),
            },
            Expr::Identifier { name, line, column } => self
                .get_binding(scope, name, *line, *column)?
                .ok_or_else(|| {
                    EngineError::new(
                        "ReferenceError",
                        format!("{name} is not defined"),
                        *line,
                        *column,
                    )
                }),
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
                    && let Expr::Identifier { name, .. } = operand.as_ref()
                    && self.get_binding(scope, name, *line, *column)?.is_none()
                {
                    return Ok(Value::String("undefined".to_owned()));
                }
                if *operator == UnaryOp::Delete {
                    return self.delete_reference(operand, scope, *line, *column);
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
                    UnaryOp::Delete => unreachable!("delete handled before evaluation"),
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
                strict,
            } => {
                let function =
                    self.alloc_user_function(parameters.clone(), body.clone(), scope, *strict);
                if let Some(name) = name {
                    let name_scope = self.new_scope(scope);
                    self.declare(name_scope, name, Value::Object(function), false, 1, 1)?;
                    self.set_property(function, "name", Value::String(name.clone()));
                    self.objects[function].function = Some(Function::User {
                        parameters: parameters.clone(),
                        body: body.clone(),
                        closure: name_scope,
                        strict: *strict || self.strict,
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
                let constructible = matches!(
                    self.objects[function_id].function,
                    Some(Function::User { .. })
                        | Some(Function::Native(
                            NativeFunction::Object | NativeFunction::Error(_)
                        ))
                );
                if !constructible {
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
            BinaryOp::In => {
                let Value::Object(object) = right else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Right-hand side of 'in' is not an object",
                        1,
                        1,
                    ));
                };
                boolean(self.get_property(object, &left.to_js_string()).is_some())
            }
            BinaryOp::InstanceOf => {
                let Value::Object(constructor) = right else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Right-hand side of 'instanceof' is not callable",
                        1,
                        1,
                    ));
                };
                if !self.is_constructor(constructor) {
                    return Err(EngineError::new(
                        "TypeError",
                        "Right-hand side of 'instanceof' is not a constructor",
                        1,
                        1,
                    ));
                }
                let Some(Value::Object(prototype)) = self.get_property(constructor, "prototype")
                else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Function has non-object prototype in instanceof check",
                        1,
                        1,
                    ));
                };
                let Value::Object(object) = left else {
                    return boolean(false);
                };
                let mut current = self.objects[object].prototype;
                let mut visited = HashSet::new();
                while let Some(id) = current {
                    if id == prototype {
                        return boolean(true);
                    }
                    if !visited.insert(id) {
                        break;
                    }
                    current = self.objects[id].prototype;
                }
                boolean(false)
            }
            BinaryOp::And | BinaryOp::Or | BinaryOp::Nullish => Ok(right),
        }
    }

    fn delete_reference(
        &mut self,
        expression: &Expr,
        scope: ScopeId,
        line: usize,
        column: usize,
    ) -> Result<Value, EngineError> {
        match expression {
            Expr::Identifier { .. } => Ok(Value::Boolean(false)),
            Expr::Member(object, property) => {
                let object = self.evaluate(object, scope)?;
                match object {
                    Value::Object(id) => {
                        self.objects[id].parameter_map.remove(property);
                        self.objects[id].properties.remove(property);
                        Ok(Value::Boolean(true))
                    }
                    Value::Null | Value::Undefined => Err(EngineError::new(
                        "TypeError",
                        "Cannot convert null or undefined to object",
                        line,
                        column,
                    )),
                    _ => Ok(Value::Boolean(true)),
                }
            }
            Expr::Index(object, property) => {
                let object = self.evaluate(object, scope)?;
                let property = self.evaluate(property, scope)?.to_js_string();
                match object {
                    Value::Object(id) => {
                        self.objects[id].parameter_map.remove(&property);
                        self.objects[id].properties.remove(&property);
                        Ok(Value::Boolean(true))
                    }
                    Value::Null | Value::Undefined => Err(EngineError::new(
                        "TypeError",
                        "Cannot convert null or undefined to object",
                        line,
                        column,
                    )),
                    _ => Ok(Value::Boolean(true)),
                }
            }
            _ => {
                self.evaluate(expression, scope)?;
                Ok(Value::Boolean(true))
            }
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
        if let Some((scope, binding_name)) = self.objects[object].parameter_map.get(name).cloned()
            && let Some(binding) = self.scopes[scope].bindings.get_mut(&binding_name)
        {
            binding.value = value.clone();
        }
        self.objects[object]
            .properties
            .insert(name.to_owned(), value);
    }

    fn get_own_property(&self, object: ObjectId, name: &str) -> Option<Value> {
        if let Some((scope, binding_name)) = self.objects[object].parameter_map.get(name)
            && let Some(binding) = self.scopes[*scope].bindings.get(binding_name)
        {
            return Some(binding.value.clone());
        }
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

    fn is_error_object(&self, object: ObjectId) -> bool {
        let mut current = Some(object);
        let mut visited = HashSet::new();
        while let Some(id) = current {
            if !visited.insert(id) {
                return false;
            }
            if self
                .error_prototypes
                .values()
                .any(|prototype| *prototype == id)
            {
                return true;
            }
            current = self.objects[id].prototype;
        }
        false
    }

    fn is_constructor(&self, object: ObjectId) -> bool {
        matches!(
            &self.objects[object].function,
            Some(Function::User { .. })
                | Some(Function::Native(
                    NativeFunction::Object | NativeFunction::Error(_)
                ))
        )
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
                strict,
            } => {
                let function_scope = self.new_function_scope(closure);
                let this_value = if strict {
                    this_value
                } else {
                    match this_value {
                        Value::Undefined | Value::Null => Value::Object(self.global_object),
                        value => value,
                    }
                };
                self.declare(function_scope, "this", this_value, true, 1, 1)?;
                for (index, parameter) in parameters.iter().enumerate() {
                    let value = arguments.get(index).cloned().unwrap_or(Value::Undefined);
                    if let Some(binding) = self.scopes[function_scope].bindings.get_mut(parameter) {
                        binding.value = value;
                    } else {
                        self.declare(function_scope, parameter, value, true, 1, 1)?;
                    }
                    self.scopes[function_scope]
                        .bindings
                        .get_mut(parameter)
                        .expect("parameter was just declared")
                        .var_binding = true;
                }
                let arguments_object = self.alloc_object(Some(self.object_prototype));
                for (index, argument) in arguments.iter().enumerate() {
                    self.set_property(arguments_object, &index.to_string(), argument.clone());
                }
                self.set_property(
                    arguments_object,
                    "length",
                    Value::Number(arguments.len() as f64),
                );
                if !strict {
                    let mut mapped = HashSet::new();
                    for (index, parameter) in parameters.iter().enumerate().rev() {
                        if index < arguments.len() && mapped.insert(parameter.clone()) {
                            self.objects[arguments_object]
                                .parameter_map
                                .insert(index.to_string(), (function_scope, parameter.clone()));
                        }
                    }
                }
                if !self.scopes[function_scope]
                    .bindings
                    .contains_key("arguments")
                {
                    self.declare(
                        function_scope,
                        "arguments",
                        Value::Object(arguments_object),
                        true,
                        1,
                        1,
                    )?;
                    self.scopes[function_scope]
                        .bindings
                        .get_mut("arguments")
                        .expect("arguments binding was just declared")
                        .var_binding = true;
                }
                let previous_strict = self.strict;
                self.strict = strict;
                let result = self.execute_statements(&body, function_scope);
                self.strict = previous_strict;
                match result? {
                    Completion::Normal(_) => Ok(Value::Undefined),
                    Completion::Return(value) => Ok(value),
                    Completion::Break | Completion::Continue => Err(EngineError::new(
                        "SyntaxError",
                        "Invalid loop control statement",
                        1,
                        1,
                    )),
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
            NativeFunction::Error(name) => {
                let prototype = self.error_prototypes.get(name).copied();
                let object = self.alloc_object(prototype);
                let message = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined))
                    .map(|value| self.to_string(value))
                    .unwrap_or_default();
                self.set_property(object, "name", Value::String(name.to_owned()));
                self.set_property(object, "message", Value::String(message));
                Ok(Value::Object(object))
            }
        }
    }

    fn error_object(&mut self, error: &EngineError) -> Value {
        let prototype = self
            .error_prototypes
            .get(error.name)
            .copied()
            .or_else(|| self.error_prototypes.get("Error").copied());
        let object = self.alloc_object(prototype);
        self.set_property(object, "name", Value::String(error.name.to_owned()));
        self.set_property(object, "message", Value::String(error.message.clone()));
        Value::Object(object)
    }

    fn thrown_error(&self, value: Value, line: usize, column: usize) -> EngineError {
        let message = self.to_string(&value);
        let name = if let Value::Object(id) = &value {
            if self.is_error_object(*id) {
                self.get_property(*id, "name")
                    .map(|name| match name {
                        Value::String(name) if name == "TypeError" => "TypeError",
                        Value::String(name) if name == "ReferenceError" => "ReferenceError",
                        Value::String(name) if name == "SyntaxError" => "SyntaxError",
                        _ => "Error",
                    })
                    .unwrap_or("Error")
            } else {
                "Error"
            }
        } else {
            "Error"
        };
        EngineError {
            name,
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

fn collect_var_names(statements: &[Stmt], names: &mut Vec<String>) {
    for statement in statements {
        match statement {
            Stmt::Declaration {
                name,
                kind: DeclarationKind::Var,
                ..
            } => names.push(name.clone()),
            Stmt::Block(statements) => collect_var_names(statements, names),
            Stmt::If {
                consequent,
                alternate,
                ..
            } => {
                collect_var_names(std::slice::from_ref(consequent), names);
                if let Some(alternate) = alternate {
                    collect_var_names(std::slice::from_ref(alternate), names);
                }
            }
            Stmt::While { body, .. } => collect_var_names(std::slice::from_ref(body), names),
            Stmt::For {
                initializer, body, ..
            } => {
                if let Some(initializer) = initializer {
                    collect_var_names(std::slice::from_ref(initializer), names);
                }
                collect_var_names(std::slice::from_ref(body), names);
            }
            Stmt::DoWhile { body, .. } => collect_var_names(std::slice::from_ref(body), names),
            Stmt::Try {
                body,
                catch_body,
                finally_body,
                ..
            } => {
                collect_var_names(std::slice::from_ref(body), names);
                if let Some(body) = catch_body {
                    collect_var_names(std::slice::from_ref(body), names);
                }
                if let Some(body) = finally_body {
                    collect_var_names(std::slice::from_ref(body), names);
                }
            }
            Stmt::Empty
            | Stmt::Expression(_)
            | Stmt::FunctionDeclaration { .. }
            | Stmt::Declaration { .. }
            | Stmt::Return(_)
            | Stmt::Throw(_)
            | Stmt::Break
            | Stmt::Continue => {}
        }
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
        assert_eq!(
            engine.eval_to_string(
                "try { throw new TypeError('bad'); } catch (error) { error.name + ':' + error.message; }",
            ).unwrap(),
            "TypeError:bad"
        );
    }

    #[test]
    fn var_is_function_scoped_hoisted_and_redeclarable() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval_to_string(
                "function readBeforeInit() { let before = typeof later; if (false) { var later = 10; } return before; } readBeforeInit()",
            ).unwrap(),
            "undefined"
        );
        assert_eq!(
            engine.eval_to_string(
                "function readFromBlock() { { var nested = 4; } return nested; } readFromBlock()",
            ).unwrap(),
            "4"
        );
        assert_eq!(
            engine
                .eval("var repeated = 1; var repeated = 2; repeated")
                .unwrap(),
            Value::Number(2.0)
        );
        assert_eq!(
            engine
                .eval("function declaredLater() { return 9; } declaredLater()")
                .unwrap(),
            Value::Number(9.0)
        );
        engine.eval("implicitVarValue = 17").unwrap();
        engine.eval("var implicitVarValue").unwrap();
        assert_eq!(
            engine.eval("implicitVarValue").unwrap(),
            Value::Number(17.0)
        );
    }

    #[test]
    fn lexical_bindings_have_a_temporal_dead_zone() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval("value; let value = 1").unwrap_err().name,
            "ReferenceError"
        );
        assert_eq!(
            engine.eval("let same = 1; var same = 2").unwrap_err().name,
            "SyntaxError"
        );
        assert_eq!(
            engine.eval("if (true) let invalid = 1").unwrap_err().name,
            "SyntaxError"
        );
        assert_eq!(
            engine.eval("try {} catch (error) error;").unwrap_err().name,
            "SyntaxError"
        );
    }

    #[test]
    fn strict_mode_controls_this_assignment_and_early_errors() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval("function sloppyThis() { return this; } sloppyThis() === globalThis")
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine
                .eval("\"use strict\"; function strictThis() { return this; } strictThis()")
                .unwrap(),
            Value::Undefined
        );
        assert_eq!(
            engine
                .eval("implicitGlobal = 3; globalThis.implicitGlobal")
                .unwrap(),
            Value::Number(3.0)
        );
        assert_eq!(
            engine
                .eval("\"use strict\"; anotherImplicit = 3")
                .unwrap_err()
                .name,
            "ReferenceError"
        );
        assert_eq!(
            engine
                .eval("\"use strict\"; function duplicate(a, a) {}")
                .unwrap_err()
                .name,
            "SyntaxError"
        );
        assert_eq!(
            engine
                .eval("\"use strict\"; delete localName")
                .unwrap_err()
                .name,
            "SyntaxError"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let ownStrict = function() { 'use strict'; return this; }; typeof ownStrict()",
                )
                .unwrap(),
            "undefined"
        );
    }

    #[test]
    fn functions_expose_names_lengths_and_arguments() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "function combine(left, right) { return combine.name + ':' + combine.length + ':' + arguments.length + ':' + arguments[0] + ':' + left + right; } combine(2, 3, 4)",
                )
                .unwrap(),
            "combine:2:3:2:23"
        );
        assert_eq!(
            engine
                .eval_to_string("function f(a) { var a; return a; } f(8)")
                .unwrap(),
            "8"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let inferred = function() {}; inferred.name + ':' + inferred.length",
                )
                .unwrap(),
            "inferred:0"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "function mapped(value) { value = 9; return arguments[0]; } mapped(1)",
                )
                .unwrap(),
            "9"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "function mappedBack(value) { arguments[0] = 12; return value; } mappedBack(1)",
                )
                .unwrap(),
            "12"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "function unmapped(value) { 'use strict'; value = 9; return arguments[0]; } unmapped(1)",
                )
                .unwrap(),
            "1"
        );
    }

    #[test]
    fn supports_in_instanceof_and_property_delete() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval(
                "function Box() {} let box = new Box; 'constructor' in Box.prototype && box instanceof Box",
            ).unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine
                .eval("let subject = { value: 1 }; delete subject.value && !('value' in subject)",)
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine.eval("({}) instanceof Object").unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine.eval("delete subject.missing").unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn for_and_do_while_handle_loop_control_and_lexical_scope() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval_to_string(
                "let total = 0; for (let i = 0; i < 8; i = i + 1) { if (i === 2) continue; if (i === 6) break; total = total + i; } total",
            ).unwrap(),
            "13"
        );
        assert_eq!(
            engine.eval_to_string(
                "let count = 0; do { count = count + 1; } while (count < 3); count",
            ).unwrap(),
            "3"
        );
        assert_eq!(
            engine.eval("typeof i").unwrap(),
            Value::String("undefined".to_owned())
        );
        assert_eq!(
            engine.eval_to_string(
                "let callbacks = {}; for (let index = 0; index < 2; index = index + 1) { if (index === 0) callbacks.first = function() { return index; }; else callbacks.second = function() { return index; }; } callbacks.first() + callbacks.second()",
            ).unwrap(),
            "1"
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
