mod jit;
mod lexer;
mod parser;
mod vm;

use lexer::Lexer;
use parser::{BinaryOp, DeclarationKind, Expr, Stmt, UnaryOp};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

type ObjectId = usize;
type ScopeId = usize;

const NURSERY_ALLOCATION_LIMIT: usize = 256;
const MAJOR_COLLECTION_INTERVAL: usize = 8;

fn mark_object(object: ObjectId, marked: &mut [bool], queue: &mut VecDeque<ObjectId>) {
    if let Some(is_marked) = marked.get_mut(object)
        && !*is_marked
    {
        *is_marked = true;
        queue.push_back(object);
    }
}

fn mark_scope(scope: ScopeId, marked: &mut [bool], queue: &mut VecDeque<ScopeId>) {
    if let Some(is_marked) = marked.get_mut(scope)
        && !*is_marked
    {
        *is_marked = true;
        queue.push_back(scope);
    }
}

fn mark_value(value: &Value, marked: &mut [bool], queue: &mut VecDeque<ObjectId>) {
    if let Value::Object(object) = value {
        mark_object(*object, marked, queue);
    }
}

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
    ObjectDefineProperty,
    ObjectGetOwnPropertyDescriptor,
    ObjectIntegrity(&'static str),
    Reflect(&'static str),
    ObjectMethod(&'static str),
    Error(&'static str),
    Array,
    ArrayIsArray,
    ArrayMethod(&'static str),
    ArrayIteratorMethod(&'static str),
    IteratorNext,
    MapConstructor,
    MapMethod(&'static str),
    MapIteratorMethod(&'static str),
    SetConstructor,
    SetMethod(&'static str),
    SetIteratorMethod(&'static str),
    WeakMapConstructor,
    WeakMapMethod(&'static str),
    WeakSetConstructor,
    WeakSetMethod(&'static str),
    Date,
    DateMethod(&'static str),
    DateStatic(&'static str),
    RegExp,
    RegExpMethod(&'static str),
    String,
    StringMethod(&'static str),
    Number,
    NumberMethod(&'static str),
    Boolean,
    PrimitiveMethod(&'static str),
    Math(&'static str),
    Json(&'static str),
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
    property_order: Vec<String>,
    non_enumerable: HashSet<String>,
    property_attributes: HashMap<String, PropertyAttributes>,
    parameter_map: HashMap<String, (ScopeId, String)>,
    prototype: Option<ObjectId>,
    function: Option<Function>,
    is_array: bool,
    extensible: bool,
    array_iterator: Option<ArrayIterator>,
    collection_iterator: Option<CollectionIterator>,
    map_data: Option<Vec<Option<(Value, Value)>>>,
    set_data: Option<Vec<Option<Value>>>,
    weak_map_data: Option<Vec<(ObjectId, Value)>>,
    weak_set_data: Option<Vec<ObjectId>>,
    date_value: Option<f64>,
    regexp_data: Option<RegExpData>,
}

#[derive(Clone)]
struct RegExpData {
    matcher: regress::Regex,
    source: String,
    flags: String,
    global: bool,
    has_indices: bool,
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
    unicode: bool,
    unicode_sets: bool,
    sticky: bool,
}

#[derive(Clone)]
struct ArrayIterator {
    array: ObjectId,
    index: usize,
    kind: &'static str,
}

#[derive(Clone)]
struct CollectionIterator {
    collection: ObjectId,
    index: usize,
    kind: &'static str,
    is_map: bool,
    done: bool,
}

#[derive(Clone)]
struct PropertyAttributes {
    writable: bool,
    configurable: bool,
    kind: PropertyKind,
}

#[derive(Clone)]
enum PropertyKind {
    Data,
    Accessor {
        getter: Option<Value>,
        setter: Option<Value>,
    },
}

enum Completion {
    Normal(Value),
    Return(Value),
    Break,
    Continue,
}

enum Reference {
    Binding {
        scope: ScopeId,
        name: String,
        line: usize,
        column: usize,
    },
    Property {
        base: Value,
        name: String,
        line: usize,
        column: usize,
    },
}

/// A from-scratch, headless JavaScript interpreter for the subset listed in
/// the README. Objects and lexical environments are held in engine-owned
/// arenas. Unreachable objects are reclaimed at allocation safepoints or by
/// calling [`Engine::collect_garbage`].
pub struct Engine {
    global: ScopeId,
    scopes: Vec<Scope>,
    objects: Vec<Object>,
    object_ages: Vec<u8>,
    object_live: Vec<bool>,
    free_objects: Vec<ObjectId>,
    free_scopes: Vec<ScopeId>,
    external_object_roots: HashSet<ObjectId>,
    nursery_allocations: usize,
    minor_collections: usize,
    object_prototype: ObjectId,
    global_object: ObjectId,
    array_prototype: ObjectId,
    array_iterator_prototype: ObjectId,
    collection_iterator_prototype: ObjectId,
    map_prototype: ObjectId,
    set_prototype: ObjectId,
    weak_map_prototype: ObjectId,
    weak_set_prototype: ObjectId,
    date_prototype: ObjectId,
    regexp_prototype: ObjectId,
    string_prototype: ObjectId,
    number_prototype: ObjectId,
    boolean_prototype: ObjectId,
    error_prototypes: HashMap<&'static str, ObjectId>,
    strict: bool,
    random_state: u64,
    bytecode_cache: Vec<HashMap<usize, vm::Bytecode>>,
    jit_cache: Vec<jit::JitCache>,
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
            object_ages: Vec::new(),
            object_live: Vec::new(),
            free_objects: Vec::new(),
            free_scopes: Vec::new(),
            external_object_roots: HashSet::new(),
            nursery_allocations: 0,
            minor_collections: 0,
            object_prototype: 0,
            global_object: 0,
            array_prototype: 0,
            array_iterator_prototype: 0,
            collection_iterator_prototype: 0,
            map_prototype: 0,
            set_prototype: 0,
            weak_map_prototype: 0,
            weak_set_prototype: 0,
            date_prototype: 0,
            regexp_prototype: 0,
            string_prototype: 0,
            number_prototype: 0,
            boolean_prototype: 0,
            error_prototypes: HashMap::new(),
            strict: false,
            bytecode_cache: Vec::new(),
            jit_cache: Vec::new(),
            random_state: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .unwrap_or(0x9e3779b97f4a7c15)
                .max(1),
        };
        engine.object_prototype = engine.alloc_object(None);
        engine.global_object = engine.alloc_object(Some(engine.object_prototype));
        engine.array_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.array_iterator_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.collection_iterator_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.map_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.set_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.weak_map_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.weak_set_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.date_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.regexp_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.string_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.number_prototype = engine.alloc_object(Some(engine.object_prototype));
        engine.boolean_prototype = engine.alloc_object(Some(engine.object_prototype));
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
        let define_property = engine.alloc_native(
            NativeFunction::ObjectDefineProperty,
            Some(engine.object_prototype),
        );
        let get_own_property_descriptor = engine.alloc_native(
            NativeFunction::ObjectGetOwnPropertyDescriptor,
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
            "defineProperty",
            Value::Object(define_property),
        );
        engine.set_property(
            object_constructor,
            "getOwnPropertyDescriptor",
            Value::Object(get_own_property_descriptor),
        );
        for (function, name, length) in [
            (define_property, "defineProperty", 3.0),
            (get_own_property_descriptor, "getOwnPropertyDescriptor", 2.0),
        ] {
            engine.set_property(function, "name", Value::String(name.to_owned()));
            engine.set_property(function, "length", Value::Number(length));
        }
        for method in [
            "preventExtensions",
            "seal",
            "freeze",
            "isExtensible",
            "isSealed",
            "isFrozen",
        ] {
            let function = engine.alloc_native(
                NativeFunction::ObjectIntegrity(method),
                Some(engine.object_prototype),
            );
            engine.set_property(function, "name", Value::String(method.to_owned()));
            engine.set_property(function, "length", Value::Number(1.0));
            engine.set_property(object_constructor, method, Value::Object(function));
        }
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
        engine.objects[object_prototype]
            .non_enumerable
            .insert("constructor".to_owned());
        engine.declare_global("Object", Value::Object(object_constructor), false);
        engine.install_object_builtins(object_constructor);
        engine.install_reflect_builtins();
        engine.install_array_builtins();
        engine.install_primitive_builtins();
        engine.install_math_builtins();
        engine.install_collection_builtins();
        engine.install_date_builtins();
        engine.install_regexp_builtins();
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
        for name in [
            "Error",
            "EvalError",
            "RangeError",
            "ReferenceError",
            "SyntaxError",
            "TypeError",
            "URIError",
        ] {
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
            self.set_builtin_property(prototype, "constructor", Value::Object(constructor));
            self.set_property(constructor, "prototype", Value::Object(prototype));
            self.set_property(constructor, "name", Value::String(name.to_owned()));
            self.set_property(constructor, "length", Value::Number(1.0));
            self.declare_global(name, Value::Object(constructor), false);
        }
    }

    fn install_object_builtins(&mut self, constructor: ObjectId) {
        for (name, native, length) in [
            ("keys", "keys", 1.0),
            ("getOwnPropertyNames", "getOwnPropertyNames", 1.0),
            ("values", "values", 1.0),
            ("entries", "entries", 1.0),
            ("assign", "assign", 2.0),
            ("fromEntries", "fromEntries", 1.0),
            ("hasOwn", "hasOwn", 2.0),
            ("defineProperties", "defineProperties", 2.0),
            (
                "getOwnPropertyDescriptors",
                "getOwnPropertyDescriptors",
                1.0,
            ),
        ] {
            let function = self.alloc_native(
                NativeFunction::ObjectMethod(native),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(length));
            self.set_property(constructor, name, Value::Object(function));
        }
        let method = self.alloc_native(
            NativeFunction::ObjectMethod("toString"),
            Some(self.object_prototype),
        );
        self.set_property(method, "name", Value::String("toString".to_owned()));
        self.set_property(method, "length", Value::Number(0.0));
        self.set_builtin_property(self.object_prototype, "toString", Value::Object(method));
        for (name, arity) in [
            ("hasOwnProperty", 1.0),
            ("propertyIsEnumerable", 1.0),
            ("isPrototypeOf", 1.0),
        ] {
            let method = self.alloc_native(
                NativeFunction::ObjectMethod(name),
                Some(self.object_prototype),
            );
            self.set_property(method, "name", Value::String(name.to_owned()));
            self.set_property(method, "length", Value::Number(arity));
            self.set_builtin_property(self.object_prototype, name, Value::Object(method));
        }
        let is = self.alloc_native(
            NativeFunction::ObjectMethod("is"),
            Some(self.object_prototype),
        );
        self.set_property(is, "name", Value::String("is".to_owned()));
        self.set_property(is, "length", Value::Number(2.0));
        self.set_property(constructor, "is", Value::Object(is));
    }

    fn install_reflect_builtins(&mut self) {
        let reflect = self.alloc_object(Some(self.object_prototype));
        for (name, arity) in [
            ("apply", 3.0),
            ("construct", 2.0),
            ("get", 2.0),
            ("set", 3.0),
            ("has", 2.0),
            ("deleteProperty", 2.0),
            ("ownKeys", 1.0),
            ("getPrototypeOf", 1.0),
            ("setPrototypeOf", 2.0),
            ("isExtensible", 1.0),
            ("preventExtensions", 1.0),
            ("getOwnPropertyDescriptor", 2.0),
            ("defineProperty", 3.0),
        ] {
            let function =
                self.alloc_native(NativeFunction::Reflect(name), Some(self.object_prototype));
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(arity));
            self.set_property(reflect, name, Value::Object(function));
        }
        self.declare_global("Reflect", Value::Object(reflect), false);
    }

    fn install_collection_builtins(&mut self) {
        let iterator_next =
            self.alloc_native(NativeFunction::IteratorNext, Some(self.object_prototype));
        self.set_property(iterator_next, "name", Value::String("next".to_owned()));
        self.set_property(iterator_next, "length", Value::Number(0.0));
        self.set_builtin_property(
            self.collection_iterator_prototype,
            "next",
            Value::Object(iterator_next),
        );

        let map = self.alloc_native(NativeFunction::MapConstructor, Some(self.object_prototype));
        self.install_collection_prototype(
            map,
            "Map",
            self.map_prototype,
            &[
                ("set", 2.0),
                ("get", 1.0),
                ("has", 1.0),
                ("delete", 1.0),
                ("clear", 0.0),
                ("forEach", 1.0),
                ("keys", 0.0),
                ("values", 0.0),
                ("entries", 0.0),
            ],
            true,
        );
        self.declare_global("Map", Value::Object(map), false);

        let set = self.alloc_native(NativeFunction::SetConstructor, Some(self.object_prototype));
        self.install_collection_prototype(
            set,
            "Set",
            self.set_prototype,
            &[
                ("add", 1.0),
                ("has", 1.0),
                ("delete", 1.0),
                ("clear", 0.0),
                ("forEach", 1.0),
                ("keys", 0.0),
                ("values", 0.0),
                ("entries", 0.0),
            ],
            false,
        );
        self.declare_global("Set", Value::Object(set), false);

        let weak_map = self.alloc_native(
            NativeFunction::WeakMapConstructor,
            Some(self.object_prototype),
        );
        self.install_weak_collection_prototype(
            weak_map,
            "WeakMap",
            self.weak_map_prototype,
            &[("set", 2.0), ("get", 1.0), ("has", 1.0), ("delete", 1.0)],
            true,
        );
        self.declare_global("WeakMap", Value::Object(weak_map), false);

        let weak_set = self.alloc_native(
            NativeFunction::WeakSetConstructor,
            Some(self.object_prototype),
        );
        self.install_weak_collection_prototype(
            weak_set,
            "WeakSet",
            self.weak_set_prototype,
            &[("add", 1.0), ("has", 1.0), ("delete", 1.0)],
            false,
        );
        self.declare_global("WeakSet", Value::Object(weak_set), false);
    }

    fn install_weak_collection_prototype(
        &mut self,
        constructor: ObjectId,
        name: &str,
        prototype: ObjectId,
        methods: &[(&'static str, f64)],
        is_map: bool,
    ) {
        self.set_property(constructor, "name", Value::String(name.to_owned()));
        self.set_property(constructor, "length", Value::Number(0.0));
        self.set_property(constructor, "prototype", Value::Object(prototype));
        self.set_builtin_property(prototype, "constructor", Value::Object(constructor));
        for (method, length) in methods {
            let native = if is_map {
                NativeFunction::WeakMapMethod(method)
            } else {
                NativeFunction::WeakSetMethod(method)
            };
            let function = self.alloc_native(native, Some(self.object_prototype));
            self.set_property(function, "name", Value::String((*method).to_owned()));
            self.set_property(function, "length", Value::Number(*length));
            self.set_builtin_property(prototype, method, Value::Object(function));
        }
    }

    fn install_date_builtins(&mut self) {
        let constructor = self.alloc_native(NativeFunction::Date, Some(self.object_prototype));
        self.set_property(constructor, "name", Value::String("Date".to_owned()));
        self.set_property(constructor, "length", Value::Number(7.0));
        self.set_property(constructor, "prototype", Value::Object(self.date_prototype));
        self.set_builtin_property(
            self.date_prototype,
            "constructor",
            Value::Object(constructor),
        );
        for (name, length) in [
            ("getTime", 0.0),
            ("valueOf", 0.0),
            ("getFullYear", 0.0),
            ("getYear", 0.0),
            ("getMonth", 0.0),
            ("getDate", 0.0),
            ("getDay", 0.0),
            ("getHours", 0.0),
            ("getMinutes", 0.0),
            ("getSeconds", 0.0),
            ("getMilliseconds", 0.0),
            ("getTimezoneOffset", 0.0),
            ("getUTCFullYear", 0.0),
            ("getUTCMonth", 0.0),
            ("getUTCDate", 0.0),
            ("getUTCDay", 0.0),
            ("getUTCHours", 0.0),
            ("getUTCMinutes", 0.0),
            ("getUTCSeconds", 0.0),
            ("getUTCMilliseconds", 0.0),
            ("setFullYear", 1.0),
            ("setUTCFullYear", 1.0),
            ("setMonth", 1.0),
            ("setUTCMonth", 1.0),
            ("setDate", 1.0),
            ("setUTCDate", 1.0),
            ("setHours", 1.0),
            ("setUTCHours", 1.0),
            ("setMinutes", 1.0),
            ("setUTCMinutes", 1.0),
            ("setSeconds", 1.0),
            ("setUTCSeconds", 1.0),
            ("setMilliseconds", 1.0),
            ("setUTCMilliseconds", 1.0),
            ("setYear", 1.0),
            ("setTime", 1.0),
            ("toString", 0.0),
            ("toDateString", 0.0),
            ("toTimeString", 0.0),
            ("toLocaleString", 0.0),
            ("toLocaleDateString", 0.0),
            ("toLocaleTimeString", 0.0),
            ("toUTCString", 0.0),
            ("toGMTString", 0.0),
            ("toISOString", 0.0),
            ("toJSON", 0.0),
        ] {
            let function = self.alloc_native(
                NativeFunction::DateMethod(name),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(length));
            self.set_builtin_property(self.date_prototype, name, Value::Object(function));
        }
        for (name, length) in [("now", 0.0), ("parse", 1.0), ("UTC", 7.0)] {
            let function = self.alloc_native(
                NativeFunction::DateStatic(name),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(length));
            self.set_property(constructor, name, Value::Object(function));
        }
        self.declare_global("Date", Value::Object(constructor), false);
    }

    fn install_regexp_builtins(&mut self) {
        let constructor = self.alloc_native(NativeFunction::RegExp, Some(self.object_prototype));
        self.set_property(constructor, "name", Value::String("RegExp".to_owned()));
        self.set_property(constructor, "length", Value::Number(2.0));
        self.set_property(
            constructor,
            "prototype",
            Value::Object(self.regexp_prototype),
        );
        self.set_builtin_property(
            self.regexp_prototype,
            "constructor",
            Value::Object(constructor),
        );
        for (name, length) in [("exec", 1.0), ("test", 1.0), ("toString", 0.0)] {
            let function = self.alloc_native(
                NativeFunction::RegExpMethod(name),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(length));
            self.set_builtin_property(self.regexp_prototype, name, Value::Object(function));
        }
        self.declare_global("RegExp", Value::Object(constructor), false);
    }

    fn install_collection_prototype(
        &mut self,
        constructor: ObjectId,
        name: &str,
        prototype: ObjectId,
        methods: &[(&'static str, f64)],
        is_map: bool,
    ) {
        self.set_property(constructor, "name", Value::String(name.to_owned()));
        self.set_property(constructor, "length", Value::Number(0.0));
        self.set_property(constructor, "prototype", Value::Object(prototype));
        self.set_builtin_property(prototype, "constructor", Value::Object(constructor));
        for (method, length) in methods {
            let native = match (is_map, *method) {
                (true, "keys" | "values" | "entries") => NativeFunction::MapIteratorMethod(method),
                (false, "keys" | "values" | "entries") => NativeFunction::SetIteratorMethod(method),
                (true, _) => NativeFunction::MapMethod(method),
                (false, _) => NativeFunction::SetMethod(method),
            };
            let function = self.alloc_native(native, Some(self.object_prototype));
            self.set_property(function, "name", Value::String((*method).to_owned()));
            self.set_property(function, "length", Value::Number(*length));
            self.set_builtin_property(prototype, method, Value::Object(function));
        }
    }

    fn install_array_builtins(&mut self) {
        let constructor = self.alloc_native(NativeFunction::Array, Some(self.object_prototype));
        self.set_property(constructor, "name", Value::String("Array".to_owned()));
        self.set_property(constructor, "length", Value::Number(1.0));
        self.set_property(
            constructor,
            "prototype",
            Value::Object(self.array_prototype),
        );
        self.set_builtin_property(
            self.array_prototype,
            "constructor",
            Value::Object(constructor),
        );
        let is_array = self.alloc_native(NativeFunction::ArrayIsArray, Some(self.object_prototype));
        self.set_property(is_array, "name", Value::String("isArray".to_owned()));
        self.set_property(is_array, "length", Value::Number(1.0));
        self.set_property(constructor, "isArray", Value::Object(is_array));
        for (name, arity) in [("from", 1.0), ("of", 0.0)] {
            let function = self.alloc_native(
                NativeFunction::ArrayMethod(name),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(arity));
            self.set_property(constructor, name, Value::Object(function));
        }
        for (name, arity) in [
            ("push", 1.0),
            ("pop", 0.0),
            ("shift", 0.0),
            ("unshift", 1.0),
            ("splice", 2.0),
            ("join", 1.0),
            ("toString", 0.0),
            ("slice", 2.0),
            ("indexOf", 1.0),
            ("lastIndexOf", 1.0),
            ("includes", 1.0),
            ("reverse", 0.0),
            ("concat", 1.0),
            ("map", 1.0),
            ("filter", 1.0),
            ("forEach", 1.0),
            ("some", 1.0),
            ("every", 1.0),
            ("find", 1.0),
            ("findIndex", 1.0),
            ("flat", 0.0),
            ("flatMap", 1.0),
            ("reduce", 1.1),
            ("reduceRight", 1.1),
            ("at", 1.0),
            ("fill", 1.0),
            ("copyWithin", 2.0),
            ("sort", 1.0),
            ("keys", 0.0),
            ("values", 0.0),
            ("entries", 0.0),
        ] {
            let native = match name {
                "keys" | "values" | "entries" => NativeFunction::ArrayIteratorMethod(name),
                _ => NativeFunction::ArrayMethod(name),
            };
            let function = self.alloc_native(native, Some(self.object_prototype));
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(arity));
            self.set_builtin_property(self.array_prototype, name, Value::Object(function));
        }
        self.declare_global("Array", Value::Object(constructor), false);
        let iterator_next =
            self.alloc_native(NativeFunction::IteratorNext, Some(self.object_prototype));
        self.set_property(iterator_next, "name", Value::String("next".to_owned()));
        self.set_property(iterator_next, "length", Value::Number(0.0));
        self.set_builtin_property(
            self.array_iterator_prototype,
            "next",
            Value::Object(iterator_next),
        );
    }

    fn install_primitive_builtins(&mut self) {
        for (name, native, prototype) in [
            ("String", NativeFunction::String, self.string_prototype),
            ("Number", NativeFunction::Number, self.number_prototype),
            ("Boolean", NativeFunction::Boolean, self.boolean_prototype),
        ] {
            let constructor = self.alloc_native(native, Some(self.object_prototype));
            self.set_property(constructor, "name", Value::String(name.to_owned()));
            self.set_property(constructor, "length", Value::Number(1.0));
            self.set_property(constructor, "prototype", Value::Object(prototype));
            self.set_property(prototype, "constructor", Value::Object(constructor));
            self.declare_global(name, Value::Object(constructor), false);
        }
        for (name, length) in [
            ("toString", 0.0),
            ("valueOf", 0.0),
            ("charAt", 1.0),
            ("charCodeAt", 1.0),
            ("includes", 1.0),
            ("startsWith", 1.0),
            ("endsWith", 1.0),
            ("indexOf", 1.0),
            ("slice", 2.0),
            ("substring", 2.0),
            ("toLowerCase", 0.0),
            ("toUpperCase", 0.0),
            ("trim", 0.0),
            ("split", 2.0),
            ("concat", 1.0),
            ("repeat", 1.0),
            ("padStart", 1.0),
            ("padEnd", 1.0),
        ] {
            let function = self.alloc_native(
                NativeFunction::StringMethod(name),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(length));
            self.set_builtin_property(self.string_prototype, name, Value::Object(function));
        }
        for (name, arity) in [("fromCharCode", 1.0), ("fromCodePoint", 1.0)] {
            let function = self.alloc_native(
                NativeFunction::StringMethod(name),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(arity));
            let constructor = self.get_global_constructor("String");
            self.set_property(constructor, name, Value::Object(function));
        }
        for (name, value) in [
            ("MAX_VALUE", f64::MAX),
            ("MIN_VALUE", f64::MIN_POSITIVE),
            ("NaN", f64::NAN),
            ("POSITIVE_INFINITY", f64::INFINITY),
            ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("MAX_SAFE_INTEGER", 9_007_199_254_740_991.0),
            ("MIN_SAFE_INTEGER", -9_007_199_254_740_991.0),
        ] {
            self.set_property(
                self.get_global_constructor("Number"),
                name,
                Value::Number(value),
            );
        }
        for (prototype, method_name, native_name) in [
            (self.number_prototype, "toString", "numberToString"),
            (self.number_prototype, "valueOf", "numberValueOf"),
            (self.boolean_prototype, "toString", "booleanToString"),
            (self.boolean_prototype, "valueOf", "booleanValueOf"),
        ] {
            let function = self.alloc_native(
                NativeFunction::PrimitiveMethod(native_name),
                Some(self.object_prototype),
            );
            self.set_property(function, "name", Value::String(method_name.to_owned()));
            self.set_property(function, "length", Value::Number(0.0));
            self.set_builtin_property(prototype, method_name, Value::Object(function));
        }
        for (name, arity) in [
            ("isFinite", 1.0),
            ("isInteger", 1.0),
            ("isNaN", 1.0),
            ("isSafeInteger", 1.0),
            ("parseFloat", 1.0),
            ("parseInt", 2.0),
        ] {
            let native = match name {
                "parseFloat" | "parseInt" => NativeFunction::Math(name),
                _ => NativeFunction::NumberMethod(name),
            };
            let function = self.alloc_native(native, Some(self.object_prototype));
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(arity));
            let number = self.get_global_constructor("Number");
            self.set_property(number, name, Value::Object(function));
        }
    }

    fn install_math_builtins(&mut self) {
        let math = self.alloc_object(Some(self.object_prototype));
        for (name, value) in [
            ("E", std::f64::consts::E),
            ("LN2", std::f64::consts::LN_2),
            ("LN10", std::f64::consts::LN_10),
            ("LOG2E", std::f64::consts::LOG2_E),
            ("LOG10E", std::f64::consts::LOG10_E),
            ("PI", std::f64::consts::PI),
            ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
            ("SQRT2", std::f64::consts::SQRT_2),
        ] {
            self.set_property(math, name, Value::Number(value));
        }
        for (name, arity) in [
            ("abs", 1.0),
            ("acos", 1.0),
            ("acosh", 1.0),
            ("ceil", 1.0),
            ("cosh", 1.0),
            ("floor", 1.0),
            ("round", 1.0),
            ("trunc", 1.0),
            ("sqrt", 1.0),
            ("cbrt", 1.0),
            ("sign", 1.0),
            ("exp", 1.0),
            ("expm1", 1.0),
            ("log", 1.0),
            ("log1p", 1.0),
            ("log2", 1.0),
            ("log10", 1.0),
            ("sin", 1.0),
            ("sinh", 1.0),
            ("cos", 1.0),
            ("tan", 1.0),
            ("tanh", 1.0),
            ("asin", 1.0),
            ("asinh", 1.0),
            ("atanh", 1.0),
            ("atan", 1.0),
            ("atan2", 2.0),
            ("pow", 2.0),
            ("min", 2.0),
            ("max", 2.0),
            ("imul", 2.0),
            ("clz32", 1.0),
            ("fround", 1.0),
            ("hypot", 2.0),
            ("random", 0.0),
        ] {
            let function =
                self.alloc_native(NativeFunction::Math(name), Some(self.object_prototype));
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(arity));
            self.set_property(math, name, Value::Object(function));
        }
        self.declare_global("Math", Value::Object(math), false);
        let is_nan = self.make_global_native("isNaN");
        let is_finite = self.make_global_native("isFinite");
        let parse_int = self.make_global_native("parseInt");
        let parse_float = self.make_global_native("parseFloat");
        self.declare_global("isNaN", is_nan, true);
        self.declare_global("isFinite", is_finite, true);
        self.declare_global("parseInt", parse_int, true);
        self.declare_global("parseFloat", parse_float, true);
        let json = self.alloc_object(Some(self.object_prototype));
        for (name, arity) in [("stringify", 3.0), ("parse", 2.0)] {
            let function =
                self.alloc_native(NativeFunction::Json(name), Some(self.object_prototype));
            self.set_property(function, "name", Value::String(name.to_owned()));
            self.set_property(function, "length", Value::Number(arity));
            self.set_property(json, name, Value::Object(function));
        }
        self.declare_global("JSON", Value::Object(json), false);
    }

    fn get_global_constructor(&self, name: &str) -> ObjectId {
        match self.scopes[self.global]
            .bindings
            .get(name)
            .map(|binding| &binding.value)
        {
            Some(Value::Object(id)) => *id,
            _ => unreachable!("built-in constructor installed before its constants"),
        }
    }

    fn make_global_native(&mut self, name: &'static str) -> Value {
        let function = self.alloc_native(NativeFunction::Math(name), Some(self.object_prototype));
        self.set_property(function, "name", Value::String(name.to_owned()));
        Value::Object(function)
    }

    fn alloc_object(&mut self, prototype: Option<ObjectId>) -> ObjectId {
        let object = Object {
            prototype,
            extensible: true,
            ..Object::default()
        };
        let id = if let Some(id) = self.free_objects.pop() {
            self.objects[id] = object;
            self.object_ages[id] = 0;
            self.object_live[id] = true;
            id
        } else {
            let id = self.objects.len();
            self.objects.push(object);
            self.object_ages.push(0);
            self.object_live.push(true);
            id
        };
        self.nursery_allocations = self.nursery_allocations.saturating_add(1);
        id
    }

    /// Performs a full tracing collection and returns the number of reclaimed
    /// object slots.
    pub fn collect_garbage(&mut self) -> usize {
        self.collect_objects(true)
    }

    fn collect_objects(&mut self, full: bool) -> usize {
        let mut marked = vec![false; self.objects.len()];
        let mut marked_scopes = vec![false; self.scopes.len()];
        let mut object_queue = VecDeque::new();
        let mut scope_queue = VecDeque::new();

        for object in [
            self.object_prototype,
            self.global_object,
            self.array_prototype,
            self.array_iterator_prototype,
            self.collection_iterator_prototype,
            self.map_prototype,
            self.set_prototype,
            self.weak_map_prototype,
            self.weak_set_prototype,
            self.date_prototype,
            self.regexp_prototype,
            self.string_prototype,
            self.number_prototype,
            self.boolean_prototype,
        ]
        .into_iter()
        .chain(self.error_prototypes.values().copied())
        .chain(self.external_object_roots.iter().copied())
        {
            mark_object(object, &mut marked, &mut object_queue);
        }
        mark_scope(self.global, &mut marked_scopes, &mut scope_queue);
        self.trace_gc_references(
            &mut marked,
            &mut marked_scopes,
            &mut object_queue,
            &mut scope_queue,
        );

        loop {
            let previously_marked = marked.iter().filter(|is_marked| **is_marked).count();
            for (id, object) in self.objects.iter().enumerate() {
                if !marked[id] {
                    continue;
                }
                if let Some(entries) = &object.weak_map_data {
                    for (key, value) in entries {
                        if marked.get(*key).copied().unwrap_or(false) {
                            mark_value(value, &mut marked, &mut object_queue);
                        }
                    }
                }
            }
            self.trace_gc_references(
                &mut marked,
                &mut marked_scopes,
                &mut object_queue,
                &mut scope_queue,
            );
            if marked.iter().filter(|is_marked| **is_marked).count() == previously_marked {
                break;
            }
        }

        for (id, object) in self.objects.iter_mut().enumerate() {
            if !self.object_live[id] || !marked[id] {
                continue;
            }
            if let Some(entries) = &mut object.weak_map_data {
                entries.retain(|(key, _)| marked.get(*key).copied().unwrap_or(false));
            }
            if let Some(entries) = &mut object.weak_set_data {
                entries.retain(|key| marked.get(*key).copied().unwrap_or(false));
            }
        }

        let mut reclaimed = 0;
        for (id, &is_marked) in marked.iter().enumerate() {
            if self.object_live[id] && !is_marked && (full || self.object_ages[id] == 0) {
                self.objects[id] = Object::default();
                self.object_ages[id] = 0;
                self.object_live[id] = false;
                self.free_objects.push(id);
                reclaimed += 1;
            } else if self.object_live[id] && is_marked && self.object_ages[id] == 0 {
                self.object_ages[id] = 1;
            }
        }
        for (id, &is_marked) in marked_scopes.iter().enumerate() {
            if id != self.global
                && !is_marked
                && (self.scopes[id].parent.is_some()
                    || !self.scopes[id].bindings.is_empty()
                    || self.scopes[id].var_scope)
            {
                self.scopes[id] = Scope::default();
                self.free_scopes.push(id);
            }
        }
        self.nursery_allocations = 0;
        if full {
            self.minor_collections = 0;
        } else {
            self.minor_collections = self.minor_collections.saturating_add(1);
        }
        reclaimed
    }

    fn trace_gc_references(
        &self,
        marked: &mut [bool],
        marked_scopes: &mut [bool],
        object_queue: &mut VecDeque<ObjectId>,
        scope_queue: &mut VecDeque<ScopeId>,
    ) {
        while !object_queue.is_empty() || !scope_queue.is_empty() {
            while let Some(scope_id) = scope_queue.pop_front() {
                let Some(scope) = self.scopes.get(scope_id) else {
                    continue;
                };
                if let Some(parent) = scope.parent {
                    mark_scope(parent, marked_scopes, scope_queue);
                }
                for binding in scope.bindings.values() {
                    mark_value(&binding.value, marked, object_queue);
                }
            }
            while let Some(object_id) = object_queue.pop_front() {
                let Some(object) = self.objects.get(object_id) else {
                    continue;
                };
                if let Some(prototype) = object.prototype {
                    mark_object(prototype, marked, object_queue);
                }
                for value in object.properties.values() {
                    mark_value(value, marked, object_queue);
                }
                for attributes in object.property_attributes.values() {
                    if let PropertyKind::Accessor { getter, setter } = &attributes.kind {
                        if let Some(value) = getter {
                            mark_value(value, marked, object_queue);
                        }
                        if let Some(value) = setter {
                            mark_value(value, marked, object_queue);
                        }
                    }
                }
                for (scope, _) in object.parameter_map.values() {
                    mark_scope(*scope, marked_scopes, scope_queue);
                }
                if let Some(Function::User { closure, .. }) = &object.function {
                    mark_scope(*closure, marked_scopes, scope_queue);
                }
                if let Some(iterator) = &object.array_iterator {
                    mark_object(iterator.array, marked, object_queue);
                }
                if let Some(iterator) = &object.collection_iterator {
                    mark_object(iterator.collection, marked, object_queue);
                }
                if let Some(entries) = &object.map_data {
                    for entry in entries.iter().flatten() {
                        mark_value(&entry.0, marked, object_queue);
                        mark_value(&entry.1, marked, object_queue);
                    }
                }
                if let Some(values) = &object.set_data {
                    for value in values.iter().flatten() {
                        mark_value(value, marked, object_queue);
                    }
                }
            }
        }
    }

    fn alloc_array(&mut self) -> ObjectId {
        let array = self.alloc_object(Some(self.array_prototype));
        self.objects[array].is_array = true;
        self.objects[array]
            .properties
            .insert("length".to_owned(), Value::Number(0.0));
        self.objects[array]
            .non_enumerable
            .insert("length".to_owned());
        self.objects[array].property_attributes.insert(
            "length".to_owned(),
            PropertyAttributes {
                writable: true,
                configurable: false,
                kind: PropertyKind::Data,
            },
        );
        array
    }

    fn alloc_array_iterator(&mut self, array: ObjectId, kind: &'static str) -> ObjectId {
        let iterator = self.alloc_object(Some(self.array_iterator_prototype));
        self.objects[iterator].array_iterator = Some(ArrayIterator {
            array,
            index: 0,
            kind,
        });
        iterator
    }

    fn alloc_collection_iterator(
        &mut self,
        collection: ObjectId,
        kind: &'static str,
        is_map: bool,
    ) -> ObjectId {
        let iterator = self.alloc_object(Some(self.collection_iterator_prototype));
        self.objects[iterator].collection_iterator = Some(CollectionIterator {
            collection,
            index: 0,
            kind,
            is_map,
            done: false,
        });
        iterator
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
        let scope = Scope {
            bindings: HashMap::new(),
            parent: Some(parent),
            var_scope: false,
        };
        if let Some(id) = self.free_scopes.pop() {
            self.scopes[id] = scope;
            id
        } else {
            let id = self.scopes.len();
            self.scopes.push(scope);
            id
        }
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
        self.bytecode_cache.push(HashMap::new());
        self.jit_cache.push(jit::JitCache::default());
        let previous_strict = self.strict;
        self.strict = program.strict;
        let execution = self.execute_statements(&program.statements, self.global);
        self.jit_cache.pop();
        self.bytecode_cache.pop();
        self.strict = previous_strict;
        let result = match execution {
            Err(error) => Err(error),
            Ok(Completion::Normal(value)) => Ok(value),
            Ok(Completion::Return(_)) => Err(EngineError::new(
                "SyntaxError",
                "Return statement outside function",
                1,
                1,
            )),
            Ok(Completion::Break | Completion::Continue) => Err(EngineError::new(
                "SyntaxError",
                "Invalid loop control statement",
                1,
                1,
            )),
        };
        if let Ok(Value::Object(object)) = &result {
            self.external_object_roots.insert(*object);
        }
        if let Err(error) = &result
            && let Some(Value::Object(object)) = &error.thrown
        {
            self.external_object_roots.insert(*object);
        }
        if self.nursery_allocations >= NURSERY_ALLOCATION_LIMIT {
            let major = self.minor_collections + 1 >= MAJOR_COLLECTION_INTERVAL;
            self.collect_objects(major);
        }
        result
    }

    pub fn eval_to_string(&mut self, source: &str) -> Result<String, EngineError> {
        let value = self.eval(source)?;
        Ok(self.to_string(&value))
    }

    fn to_string(&self, value: &Value) -> String {
        self.to_string_inner(value, &mut HashSet::new())
    }

    fn to_string_inner(&self, value: &Value, ancestors: &mut HashSet<ObjectId>) -> String {
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
                } else if self.objects[*id].is_array {
                    if !ancestors.insert(*id) {
                        return String::new();
                    }
                    let result = self.array_join_string(*id, ",", ancestors);
                    ancestors.remove(id);
                    result
                } else {
                    "[object Object]".to_owned()
                }
            }
            _ => value.to_js_string(),
        }
    }

    fn array_join_string(
        &self,
        array: ObjectId,
        separator: &str,
        ancestors: &mut HashSet<ObjectId>,
    ) -> String {
        (0..self.array_length(array))
            .map(|index| match self.get_property(array, &index.to_string()) {
                None | Some(Value::Undefined | Value::Null) => String::new(),
                Some(value) => self.to_string_inner(&value, ancestors),
            })
            .collect::<Vec<_>>()
            .join(separator)
    }

    fn execute_statements(
        &mut self,
        statements: &[Stmt],
        scope: ScopeId,
    ) -> Result<Completion, EngineError> {
        self.instantiate_declarations(statements, scope)?;
        if let Some(mut bytecode) = vm::Bytecode::compile_statements(statements) {
            return bytecode.run(self, scope).map(Completion::Normal);
        }
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
            Stmt::ForEach {
                name,
                kind,
                iterable,
                body,
                is_of,
            } => {
                let iterable = self.evaluate(iterable, scope)?;
                let values = self.for_each_values(iterable, *is_of)?;
                let mut result = Value::Undefined;
                for value in values {
                    let iteration_scope = if *kind == DeclarationKind::Var {
                        let target = self.var_scope(scope);
                        self.assign_binding(target, name, value, 1, 1)?;
                        scope
                    } else {
                        let iteration_scope = self.new_scope(scope);
                        self.declare(
                            iteration_scope,
                            name,
                            value,
                            *kind == DeclarationKind::Let,
                            1,
                            1,
                        )?;
                        iteration_scope
                    };
                    match self.execute_statement(body, iteration_scope)? {
                        Completion::Normal(value) => result = value,
                        completion @ Completion::Return(_) => return Ok(completion),
                        Completion::Break => break,
                        Completion::Continue => {}
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
        let expression_key = expression as *const Expr as usize;
        let jit_variables = self
            .jit_cache
            .last_mut()
            .and_then(|cache| cache.prepare(expression_key, expression));
        if let Some(variables) = jit_variables {
            let mut inputs = Vec::with_capacity(variables.len());
            let mut is_numeric = true;
            for name in variables {
                match self.get_binding(scope, &name, 1, 1) {
                    Ok(Some(Value::Number(value))) => inputs.push(value),
                    Ok(_) | Err(_) => {
                        is_numeric = false;
                        break;
                    }
                }
            }
            if is_numeric
                && let Some(value) = self
                    .jit_cache
                    .last_mut()
                    .and_then(|cache| cache.run(expression_key, expression, &inputs))
            {
                return Ok(value);
            }
        }
        let bytecode = self.bytecode_cache.last_mut().and_then(|cache| {
            cache
                .remove(&expression_key)
                .or_else(|| vm::Bytecode::compile(expression))
        });
        if let Some(mut bytecode) = bytecode {
            let result = bytecode.run(self, scope);
            if let Some(cache) = self.bytecode_cache.last_mut() {
                cache.insert(expression_key, bytecode);
            }
            return result;
        }

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
                operator,
                ..
            } => {
                let reference = self.evaluate_reference(target, scope)?;
                let current = if operator.is_some() {
                    Some(self.read_reference(&reference)?)
                } else {
                    None
                };
                if let (Some(operator), Some(current)) = (operator, current.as_ref())
                    && ((*operator == BinaryOp::And && !current.is_truthy())
                        || (*operator == BinaryOp::Or && current.is_truthy())
                        || (*operator == BinaryOp::Nullish
                            && !matches!(current, Value::Null | Value::Undefined)))
                {
                    return Ok(current.clone());
                }
                let mut value = self.evaluate(value, scope)?;
                if let (Some(operator), Some(current)) = (operator, current) {
                    value = self.evaluate_binary(*operator, current, value)?;
                }
                self.write_reference(reference, value)
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                if self.evaluate(test, scope)?.is_truthy() {
                    self.evaluate(consequent, scope)
                } else {
                    self.evaluate(alternate, scope)
                }
            }
            Expr::Update {
                target,
                increment,
                prefix,
            } => {
                let reference = self.evaluate_reference(target, scope)?;
                let previous = self.read_reference(&reference)?;
                let next =
                    Value::Number(previous.to_number() + if *increment { 1.0 } else { -1.0 });
                self.write_reference(reference, next.clone())?;
                Ok(if *prefix { next } else { previous })
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
                    UnaryOp::BitNot => Ok(Value::Number(
                        (!to_uint32(operand.to_number()) as i32) as f64,
                    )),
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
            Expr::Array(elements) => {
                let array = self.alloc_array();
                for (index, element) in elements.iter().enumerate() {
                    if let Some(element) = element {
                        let value = self.evaluate(element, scope)?;
                        self.set_property(array, &index.to_string(), value);
                    }
                }
                self.set_property(array, "length", Value::Number(elements.len() as f64));
                Ok(Value::Object(array))
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
                let constructible = self.is_constructor(function_id);
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

    fn evaluate_reference(
        &mut self,
        expression: &Expr,
        scope: ScopeId,
    ) -> Result<Reference, EngineError> {
        match expression {
            Expr::Identifier { name, line, column } => Ok(Reference::Binding {
                scope,
                name: name.clone(),
                line: *line,
                column: *column,
            }),
            Expr::Member(object, property) => Ok(Reference::Property {
                base: self.evaluate(object, scope)?,
                name: property.clone(),
                line: 1,
                column: 1,
            }),
            Expr::Index(object, property) => {
                let base = self.evaluate(object, scope)?;
                let name = self.evaluate(property, scope)?.to_js_string();
                Ok(Reference::Property {
                    base,
                    name,
                    line: 1,
                    column: 1,
                })
            }
            _ => unreachable!("parser validates reference targets"),
        }
    }

    fn for_each_values(&mut self, iterable: Value, is_of: bool) -> Result<Vec<Value>, EngineError> {
        if is_of {
            return match iterable {
                Value::Object(id) if self.objects[id].is_array => Ok((0..self.array_length(id))
                    .map(|index| {
                        self.get_property(id, &index.to_string())
                            .unwrap_or(Value::Undefined)
                    })
                    .collect()),
                Value::Object(id) if self.objects[id].map_data.is_some() => {
                    let entries = self.objects[id]
                        .map_data
                        .as_ref()
                        .expect("Map internal data is initialized")
                        .clone();
                    let mut values = Vec::with_capacity(entries.len());
                    for (key, value) in entries.into_iter().flatten() {
                        let pair = self.alloc_array();
                        self.set_property(pair, "0", key);
                        self.set_property(pair, "1", value);
                        values.push(Value::Object(pair));
                    }
                    Ok(values)
                }
                Value::Object(id) if self.objects[id].set_data.is_some() => Ok(self.objects[id]
                    .set_data
                    .as_ref()
                    .expect("Set internal data is initialized")
                    .iter()
                    .flatten()
                    .cloned()
                    .collect()),
                Value::String(value) => Ok(value
                    .chars()
                    .map(|character| Value::String(character.to_string()))
                    .collect()),
                _ => Err(EngineError::new(
                    "TypeError",
                    "Value is not iterable by this runtime",
                    1,
                    1,
                )),
            };
        }
        match iterable {
            Value::Object(id) => {
                let mut keys = Vec::new();
                let mut seen = HashSet::new();
                let mut current = Some(id);
                while let Some(object) = current {
                    for name in self.enumerable_property_names(object) {
                        if seen.insert(name.clone()) {
                            keys.push(Value::String(name));
                        }
                    }
                    current = self.objects[object].prototype;
                }
                Ok(keys)
            }
            Value::String(value) => Ok((0..value.encode_utf16().count())
                .map(|index| Value::String(index.to_string()))
                .collect()),
            Value::Null | Value::Undefined => Err(EngineError::new(
                "TypeError",
                "Cannot enumerate null or undefined",
                1,
                1,
            )),
            _ => Ok(Vec::new()),
        }
    }

    fn read_reference(&mut self, reference: &Reference) -> Result<Value, EngineError> {
        match reference {
            Reference::Binding {
                scope,
                name,
                line,
                column,
            } => self
                .get_binding(*scope, name, *line, *column)?
                .ok_or_else(|| {
                    EngineError::new(
                        "ReferenceError",
                        format!("{name} is not defined"),
                        *line,
                        *column,
                    )
                }),
            Reference::Property {
                base,
                name,
                line,
                column,
            } => self.get_property_value(base.clone(), name, *line, *column),
        }
    }

    fn write_reference(
        &mut self,
        reference: Reference,
        value: Value,
    ) -> Result<Value, EngineError> {
        match reference {
            Reference::Binding {
                scope,
                name,
                line,
                column,
            } => self.assign_binding(scope, &name, value, line, column),
            Reference::Property {
                base,
                name,
                line,
                column,
            } => {
                self.set_property_value(base, &name, value.clone(), line, column)?;
                Ok(value)
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
            BinaryOp::Exponent => number(left.to_number().powf(right.to_number())),
            BinaryOp::BitAnd => {
                number((to_uint32(left.to_number()) & to_uint32(right.to_number())) as i32 as f64)
            }
            BinaryOp::BitOr => {
                number((to_uint32(left.to_number()) | to_uint32(right.to_number())) as i32 as f64)
            }
            BinaryOp::BitXor => {
                number((to_uint32(left.to_number()) ^ to_uint32(right.to_number())) as i32 as f64)
            }
            BinaryOp::ShiftLeft => number(
                ((to_uint32(left.to_number()) as i32) << (to_uint32(right.to_number()) & 31))
                    as f64,
            ),
            BinaryOp::ShiftRight => number(
                ((to_uint32(left.to_number()) as i32) >> (to_uint32(right.to_number()) & 31))
                    as f64,
            ),
            BinaryOp::UnsignedShiftRight => {
                number((to_uint32(left.to_number()) >> (to_uint32(right.to_number()) & 31)) as f64)
            }
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
                        if self.objects[id]
                            .property_attributes
                            .get(property)
                            .is_some_and(|attributes| !attributes.configurable)
                        {
                            return Ok(Value::Boolean(false));
                        }
                        self.objects[id].parameter_map.remove(property);
                        if self.objects[id].is_array && property == "length" {
                            Ok(Value::Boolean(false))
                        } else {
                            self.remove_property(id, property);
                            Ok(Value::Boolean(true))
                        }
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
                        if self.objects[id]
                            .property_attributes
                            .get(&property)
                            .is_some_and(|attributes| !attributes.configurable)
                        {
                            return Ok(Value::Boolean(false));
                        }
                        self.objects[id].parameter_map.remove(&property);
                        if self.objects[id].is_array && property == "length" {
                            Ok(Value::Boolean(false))
                        } else {
                            self.remove_property(id, &property);
                            Ok(Value::Boolean(true))
                        }
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
        if !self.objects[object]
            .property_order
            .iter()
            .any(|property| property == name)
        {
            self.objects[object].property_order.push(name.to_owned());
        }
        self.objects[object]
            .properties
            .insert(name.to_owned(), value);
        self.objects[object]
            .property_attributes
            .entry(name.to_owned())
            .or_insert(PropertyAttributes {
                writable: true,
                configurable: true,
                kind: PropertyKind::Data,
            });
        if self.objects[object].is_array
            && let Some(index) = array_index(name)
        {
            let length = self.objects[object]
                .properties
                .get("length")
                .map(Value::to_number)
                .unwrap_or(0.0);
            if f64::from(index) >= length {
                self.objects[object]
                    .properties
                    .insert("length".to_owned(), Value::Number(f64::from(index) + 1.0));
            }
        }
    }

    fn set_builtin_property(&mut self, object: ObjectId, name: &str, value: Value) {
        self.set_property(object, name, value);
        self.objects[object].non_enumerable.insert(name.to_owned());
    }

    fn remove_property(&mut self, object: ObjectId, name: &str) {
        self.objects[object].properties.remove(name);
        self.objects[object].property_attributes.remove(name);
        self.objects[object].non_enumerable.remove(name);
        self.objects[object]
            .property_order
            .retain(|property| property != name);
    }

    fn define_data_property(
        &mut self,
        object: ObjectId,
        name: &str,
        descriptor: ObjectId,
    ) -> Result<(), EngineError> {
        let has_get = self.objects[descriptor].properties.contains_key("get");
        let has_set = self.objects[descriptor].properties.contains_key("set");
        let has_value = self.objects[descriptor].properties.contains_key("value");
        let has_writable = self.objects[descriptor].properties.contains_key("writable");
        if (has_get || has_set) && (has_value || has_writable) {
            return Err(EngineError::new(
                "TypeError",
                "Invalid mixed property descriptor",
                1,
                1,
            ));
        }
        let exists = self.objects[object].properties.contains_key(name);
        if !exists && !self.objects[object].extensible {
            return Err(EngineError::new(
                "TypeError",
                "Cannot define a property on a non-extensible object",
                1,
                1,
            ));
        }
        let old_attributes = self.objects[object]
            .property_attributes
            .get(name)
            .cloned()
            .unwrap_or(PropertyAttributes {
                writable: true,
                configurable: true,
                kind: PropertyKind::Data,
            });
        let accessor_descriptor = has_get || has_set;
        let configurable = self
            .get_own_property(descriptor, "configurable")
            .map(|value| value.is_truthy())
            .unwrap_or(if exists {
                old_attributes.configurable
            } else {
                false
            });
        let writable = self
            .get_own_property(descriptor, "writable")
            .map(|value| value.is_truthy())
            .unwrap_or_else(|| match old_attributes.kind {
                PropertyKind::Data if exists && !accessor_descriptor => old_attributes.writable,
                _ => false,
            });
        let enumerable = self
            .get_own_property(descriptor, "enumerable")
            .map(|value| value.is_truthy())
            .unwrap_or(if exists {
                !self.objects[object].non_enumerable.contains(name)
            } else {
                false
            });
        let old_enumerable = !self.objects[object].non_enumerable.contains(name);
        let new_kind = if accessor_descriptor {
            let (old_getter, old_setter) = match &old_attributes.kind {
                PropertyKind::Accessor { getter, setter } => (getter.clone(), setter.clone()),
                PropertyKind::Data => (None, None),
            };
            let getter = if has_get {
                self.get_own_property(descriptor, "get")
                    .unwrap_or(Value::Undefined)
            } else {
                old_getter.unwrap_or(Value::Undefined)
            };
            let setter = if has_set {
                self.get_own_property(descriptor, "set")
                    .unwrap_or(Value::Undefined)
            } else {
                old_setter.unwrap_or(Value::Undefined)
            };
            for accessor in [&getter, &setter] {
                if !matches!(accessor, Value::Undefined)
                    && !matches!(accessor, Value::Object(id) if self.objects[*id].function.is_some())
                {
                    return Err(EngineError::new(
                        "TypeError",
                        "Getter and setter must be callable or undefined",
                        1,
                        1,
                    ));
                }
            }
            PropertyKind::Accessor {
                getter: (!matches!(getter, Value::Undefined)).then_some(getter),
                setter: (!matches!(setter, Value::Undefined)).then_some(setter),
            }
        } else if has_value || has_writable || !exists {
            PropertyKind::Data
        } else {
            old_attributes.kind.clone()
        };

        if exists && !old_attributes.configurable {
            if configurable
                || old_enumerable != enumerable
                || std::mem::discriminant(&old_attributes.kind) != std::mem::discriminant(&new_kind)
            {
                return Err(EngineError::new(
                    "TypeError",
                    "Cannot redefine a non-configurable property",
                    1,
                    1,
                ));
            }
            match (&old_attributes.kind, &new_kind) {
                (PropertyKind::Data, PropertyKind::Data) => {
                    if !old_attributes.writable
                        && (writable
                            || self.get_own_property(descriptor, "value").is_some_and(
                                |new_value| {
                                    self.get_own_property(object, name)
                                        .is_some_and(|old_value| {
                                            !same_value(&old_value, &new_value)
                                        })
                                },
                            ))
                    {
                        return Err(EngineError::new(
                            "TypeError",
                            "Cannot change a non-writable property",
                            1,
                            1,
                        ));
                    }
                }
                (
                    PropertyKind::Accessor {
                        getter: old_getter,
                        setter: old_setter,
                    },
                    PropertyKind::Accessor {
                        getter: new_getter,
                        setter: new_setter,
                    },
                ) if old_getter != new_getter || old_setter != new_setter => {
                    return Err(EngineError::new(
                        "TypeError",
                        "Cannot change a non-configurable accessor",
                        1,
                        1,
                    ));
                }
                _ => {}
            }
        }

        match &new_kind {
            PropertyKind::Data => {
                if let Some(value) = self.get_own_property(descriptor, "value") {
                    self.set_property(object, name, value);
                } else if !exists || matches!(old_attributes.kind, PropertyKind::Accessor { .. }) {
                    self.set_property(object, name, Value::Undefined);
                }
            }
            PropertyKind::Accessor { .. } => {
                if !exists || matches!(old_attributes.kind, PropertyKind::Data) {
                    self.set_property(object, name, Value::Undefined);
                }
            }
        }
        if enumerable {
            self.objects[object].non_enumerable.remove(name);
        } else {
            self.objects[object].non_enumerable.insert(name.to_owned());
        }
        self.objects[object].property_attributes.insert(
            name.to_owned(),
            PropertyAttributes {
                writable: matches!(new_kind, PropertyKind::Data) && writable,
                configurable,
                kind: new_kind,
            },
        );
        Ok(())
    }

    fn property_descriptor(&mut self, object: ObjectId, name: &str) -> Option<ObjectId> {
        self.get_own_property(object, name)?;
        let attributes = self.objects[object]
            .property_attributes
            .get(name)
            .cloned()
            .unwrap_or(PropertyAttributes {
                writable: true,
                configurable: true,
                kind: PropertyKind::Data,
            });
        let descriptor = self.alloc_object(Some(self.object_prototype));
        match attributes.kind {
            PropertyKind::Data => {
                self.set_property(
                    descriptor,
                    "value",
                    self.get_own_property(object, name)
                        .unwrap_or(Value::Undefined),
                );
                self.set_property(descriptor, "writable", Value::Boolean(attributes.writable));
            }
            PropertyKind::Accessor { getter, setter } => {
                self.set_property(descriptor, "get", getter.unwrap_or(Value::Undefined));
                self.set_property(descriptor, "set", setter.unwrap_or(Value::Undefined));
            }
        }
        self.set_property(
            descriptor,
            "enumerable",
            Value::Boolean(!self.objects[object].non_enumerable.contains(name)),
        );
        self.set_property(
            descriptor,
            "configurable",
            Value::Boolean(attributes.configurable),
        );
        Some(descriptor)
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
                    NativeFunction::Object
                        | NativeFunction::Error(_)
                        | NativeFunction::Array
                        | NativeFunction::String
                        | NativeFunction::Number
                        | NativeFunction::Boolean
                        | NativeFunction::MapConstructor
                        | NativeFunction::SetConstructor
                        | NativeFunction::WeakMapConstructor
                        | NativeFunction::WeakSetConstructor
                        | NativeFunction::Date
                        | NativeFunction::RegExp
                ))
        )
    }

    fn get_property_value(
        &mut self,
        value: Value,
        name: &str,
        line: usize,
        column: usize,
    ) -> Result<Value, EngineError> {
        match value {
            Value::Object(id) => self.get_property_with_receiver(id, Value::Object(id), name),
            Value::Null | Value::Undefined => Err(EngineError::new(
                "TypeError",
                format!("Cannot read properties of {}", value.to_js_string()),
                line,
                column,
            )),
            Value::String(value) if name == "length" => {
                Ok(Value::Number(value.encode_utf16().count() as f64))
            }
            Value::String(value) => {
                if let Some(index) = array_index(name)
                    && let Some(character) = value.chars().nth(index as usize)
                {
                    return Ok(Value::String(character.to_string()));
                }
                self.get_property_with_receiver(self.string_prototype, Value::String(value), name)
            }
            Value::Number(value) => {
                self.get_property_with_receiver(self.number_prototype, Value::Number(value), name)
            }
            Value::Boolean(value) => {
                self.get_property_with_receiver(self.boolean_prototype, Value::Boolean(value), name)
            }
        }
    }

    fn get_property_with_receiver(
        &mut self,
        object: ObjectId,
        receiver: Value,
        name: &str,
    ) -> Result<Value, EngineError> {
        let mut current = Some(object);
        let mut visited = HashSet::new();
        while let Some(id) = current {
            if !visited.insert(id) {
                return Ok(Value::Undefined);
            }
            if let Some(attributes) = self.objects[id].property_attributes.get(name).cloned()
                && let PropertyKind::Accessor { getter, .. } = attributes.kind
            {
                return match getter {
                    Some(getter) => self.call(getter, receiver, Vec::new()),
                    None => Ok(Value::Undefined),
                };
            }
            if let Some(value) = self.get_own_property(id, name) {
                return Ok(value);
            }
            current = self.objects[id].prototype;
        }
        if let Value::Object(id) = receiver {
            if let Some(entries) = &self.objects[id].map_data
                && name == "size"
            {
                return Ok(Value::Number(
                    entries.iter().filter(|entry| entry.is_some()).count() as f64,
                ));
            }
            if let Some(values) = &self.objects[id].set_data
                && name == "size"
            {
                return Ok(Value::Number(
                    values.iter().filter(|value| value.is_some()).count() as f64,
                ));
            }
        }
        Ok(Value::Undefined)
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
            let mut current = Some(id);
            let mut visited = HashSet::new();
            let mut found = None;
            while let Some(prototype) = current {
                if !visited.insert(prototype) {
                    break;
                }
                if let Some(attributes) = self.objects[prototype]
                    .property_attributes
                    .get(name)
                    .cloned()
                {
                    found = Some((prototype, attributes));
                    break;
                }
                current = self.objects[prototype].prototype;
            }
            if let Some((
                _,
                PropertyAttributes {
                    kind: PropertyKind::Accessor { setter, .. },
                    ..
                },
            )) = &found
            {
                if let Some(setter) = setter.clone() {
                    self.call(setter, Value::Object(id), vec![property])?;
                    return Ok(());
                }
                if self.strict {
                    return Err(EngineError::new(
                        "TypeError",
                        format!("Property '{name}' has no setter"),
                        line,
                        column,
                    ));
                }
                return Ok(());
            }
            if let Some((_owner, attributes)) = found
                && !attributes.writable
            {
                if self.strict {
                    return Err(EngineError::new(
                        "TypeError",
                        format!("Cannot assign to read only property '{name}'"),
                        line,
                        column,
                    ));
                }
                return Ok(());
            }
            let own_property = self.objects[id].properties.contains_key(name);
            if !own_property && !self.objects[id].extensible {
                if self.strict {
                    return Err(EngineError::new(
                        "TypeError",
                        format!("Cannot add property '{name}' to a non-extensible object"),
                        line,
                        column,
                    ));
                }
                return Ok(());
            }
            if self.objects[id].is_array && name == "length" {
                let length = property.to_number();
                if !length.is_finite()
                    || length < 0.0
                    || length.fract() != 0.0
                    || length > u32::MAX as f64
                {
                    return Err(EngineError::new(
                        "RangeError",
                        "Invalid array length",
                        line,
                        column,
                    ));
                }
                let length = length as u32;
                let keys: Vec<String> = self.objects[id]
                    .properties
                    .keys()
                    .filter(|key| array_index(key).is_some_and(|index| index >= length))
                    .cloned()
                    .collect();
                for key in keys {
                    self.objects[id].properties.remove(&key);
                }
                self.objects[id]
                    .properties
                    .insert(name.to_owned(), Value::Number(length as f64));
            } else {
                self.set_property(id, name, property);
            }
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
                self.bytecode_cache.push(HashMap::new());
                self.jit_cache.push(jit::JitCache::default());
                let result = if let Some(mut bytecode) = vm::Bytecode::compile_function(&body) {
                    match self.instantiate_declarations(&body, function_scope) {
                        Err(error) => Err(error),
                        Ok(()) => {
                            bytecode
                                .run_function(self, function_scope)
                                .map(|flow| match flow {
                                    vm::Flow::Complete(value) => Completion::Normal(value),
                                    vm::Flow::Return(value) => Completion::Return(value),
                                })
                        }
                    }
                } else {
                    self.execute_statements(&body, function_scope)
                };
                self.jit_cache.pop();
                self.bytecode_cache.pop();
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
            Function::Native(native) => self.call_native(native, this_value, arguments),
        }
    }

    fn call_native(
        &mut self,
        native: NativeFunction,
        this_value: Value,
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
            NativeFunction::Array => {
                let array = self.alloc_array();
                if let [Value::Number(length)] = arguments.as_slice() {
                    if !length.is_finite()
                        || *length < 0.0
                        || length.fract() != 0.0
                        || *length > u32::MAX as f64
                    {
                        return Err(EngineError::new("RangeError", "Invalid array length", 1, 1));
                    }
                    self.set_property(array, "length", Value::Number(*length));
                } else {
                    for (index, value) in arguments.into_iter().enumerate() {
                        self.set_property(array, &index.to_string(), value);
                    }
                }
                Ok(Value::Object(array))
            }
            NativeFunction::ArrayIsArray => Ok(Value::Boolean(matches!(
                arguments.first(),
                Some(Value::Object(id)) if self.objects[*id].is_array
            ))),
            NativeFunction::ArrayMethod(method) => {
                self.call_array_method(method, this_value, arguments)
            }
            NativeFunction::ArrayIteratorMethod(kind) => {
                let Value::Object(array) = this_value else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Array iterator method called on a non-object",
                        1,
                        1,
                    ));
                };
                if !self.objects[array].is_array {
                    return Err(EngineError::new(
                        "TypeError",
                        "Array iterator method called on a non-array",
                        1,
                        1,
                    ));
                }
                Ok(Value::Object(self.alloc_array_iterator(array, kind)))
            }
            NativeFunction::IteratorNext => {
                let Value::Object(iterator) = this_value else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Iterator next called on an incompatible receiver",
                        1,
                        1,
                    ));
                };
                let result = self.alloc_object(Some(self.object_prototype));
                if let Some(state) = self.objects[iterator].array_iterator.clone() {
                    if state.index >= self.array_length(state.array) {
                        self.set_property(result, "value", Value::Undefined);
                        self.set_property(result, "done", Value::Boolean(true));
                        return Ok(Value::Object(result));
                    }
                    let index = state.index;
                    let value = match state.kind {
                        "keys" => Value::Number(index as f64),
                        "values" => self
                            .get_property(state.array, &index.to_string())
                            .unwrap_or(Value::Undefined),
                        "entries" => {
                            let pair = self.alloc_array();
                            self.set_property(pair, "0", Value::Number(index as f64));
                            self.set_property(
                                pair,
                                "1",
                                self.get_property(state.array, &index.to_string())
                                    .unwrap_or(Value::Undefined),
                            );
                            Value::Object(pair)
                        }
                        _ => unreachable!("known array iterator kind"),
                    };
                    self.objects[iterator]
                        .array_iterator
                        .as_mut()
                        .expect("iterator state exists")
                        .index += 1;
                    self.set_property(result, "value", value);
                    self.set_property(result, "done", Value::Boolean(false));
                    return Ok(Value::Object(result));
                }
                let Some(state) = self.objects[iterator].collection_iterator.clone() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Iterator next called on an incompatible receiver",
                        1,
                        1,
                    ));
                };
                let item = self.next_collection_iterator_value(iterator, &state);
                if let Some(value) = item {
                    self.set_property(result, "value", value);
                    self.set_property(result, "done", Value::Boolean(false));
                } else {
                    self.set_property(result, "value", Value::Undefined);
                    self.set_property(result, "done", Value::Boolean(true));
                }
                Ok(Value::Object(result))
            }
            NativeFunction::MapConstructor => {
                let Value::Object(map) = this_value else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Map constructor must be called with new",
                        1,
                        1,
                    ));
                };
                self.objects[map].map_data = Some(Vec::new());
                if let Some(Value::Object(iterable)) = arguments.first()
                    && self.is_iterator_object(*iterable)
                {
                    for entry in self.iterator_to_list(*iterable)? {
                        self.map_insert_entry(map, entry)?;
                    }
                } else if let Some(Value::Object(iterable)) = arguments.first()
                    && let Some(entries) = self.objects[*iterable].map_data.clone()
                {
                    self.objects[map].map_data =
                        Some(entries.into_iter().flatten().map(Some).collect());
                } else if let Some(Value::Object(iterable)) = arguments.first()
                    && self.objects[*iterable].is_array
                {
                    for index in 0..self.array_length(*iterable) {
                        let entry = self
                            .get_property(*iterable, &index.to_string())
                            .unwrap_or(Value::Undefined);
                        let Value::Object(pair) = entry else {
                            return Err(EngineError::new(
                                "TypeError",
                                "Map entries must be objects",
                                1,
                                1,
                            ));
                        };
                        let key = self.get_property(pair, "0").unwrap_or(Value::Undefined);
                        let value = self.get_property(pair, "1").unwrap_or(Value::Undefined);
                        self.map_set(map, key, value);
                    }
                } else if arguments
                    .first()
                    .is_some_and(|value| !matches!(value, Value::Undefined | Value::Null))
                {
                    return Err(EngineError::new(
                        "TypeError",
                        "Map constructor argument is not iterable",
                        1,
                        1,
                    ));
                }
                Ok(Value::Object(map))
            }
            NativeFunction::MapMethod(method) => {
                self.call_map_method(method, this_value, arguments)
            }
            NativeFunction::MapIteratorMethod(kind) => {
                self.call_collection_iterator_method(kind, this_value, true)
            }
            NativeFunction::SetConstructor => {
                let Value::Object(set) = this_value else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Set constructor must be called with new",
                        1,
                        1,
                    ));
                };
                self.objects[set].set_data = Some(Vec::new());
                if let Some(Value::Object(iterable)) = arguments.first()
                    && self.is_iterator_object(*iterable)
                {
                    for value in self.iterator_to_list(*iterable)? {
                        self.set_add(set, value);
                    }
                } else if let Some(Value::Object(iterable)) = arguments.first()
                    && let Some(values) = self.objects[*iterable].set_data.clone()
                {
                    self.objects[set].set_data =
                        Some(values.into_iter().flatten().map(Some).collect());
                } else if let Some(Value::Object(iterable)) = arguments.first()
                    && let Some(entries) = self.objects[*iterable].map_data.clone()
                {
                    for (key, value) in entries.into_iter().flatten() {
                        let pair = self.alloc_array();
                        self.set_property(pair, "0", key);
                        self.set_property(pair, "1", value);
                        self.set_add(set, Value::Object(pair));
                    }
                } else if let Some(Value::Object(iterable)) = arguments.first()
                    && self.objects[*iterable].is_array
                {
                    for index in 0..self.array_length(*iterable) {
                        let value = self
                            .get_property(*iterable, &index.to_string())
                            .unwrap_or(Value::Undefined);
                        self.set_add(set, value);
                    }
                } else if arguments
                    .first()
                    .is_some_and(|value| !matches!(value, Value::Undefined | Value::Null))
                {
                    return Err(EngineError::new(
                        "TypeError",
                        "Set constructor argument is not iterable",
                        1,
                        1,
                    ));
                }
                Ok(Value::Object(set))
            }
            NativeFunction::SetMethod(method) => {
                self.call_set_method(method, this_value, arguments)
            }
            NativeFunction::SetIteratorMethod(kind) => {
                self.call_collection_iterator_method(kind, this_value, false)
            }
            NativeFunction::WeakMapConstructor => {
                let Value::Object(weak_map) = this_value else {
                    return Err(EngineError::new(
                        "TypeError",
                        "WeakMap constructor must be called with new",
                        1,
                        1,
                    ));
                };
                self.objects[weak_map].weak_map_data = Some(Vec::new());
                if let Some(Value::Object(iterable)) = arguments.first()
                    && self.objects[*iterable].is_array
                {
                    for index in 0..self.array_length(*iterable) {
                        let entry = self
                            .get_property(*iterable, &index.to_string())
                            .unwrap_or(Value::Undefined);
                        let Value::Object(pair) = entry else {
                            return Err(EngineError::new(
                                "TypeError",
                                "WeakMap entries must be objects",
                                1,
                                1,
                            ));
                        };
                        let Value::Object(key) =
                            self.get_property(pair, "0").unwrap_or(Value::Undefined)
                        else {
                            return Err(EngineError::new(
                                "TypeError",
                                "WeakMap keys must be objects",
                                1,
                                1,
                            ));
                        };
                        let value = self.get_property(pair, "1").unwrap_or(Value::Undefined);
                        self.weak_map_set(weak_map, key, value);
                    }
                } else if arguments
                    .first()
                    .is_some_and(|value| !matches!(value, Value::Undefined | Value::Null))
                {
                    return Err(EngineError::new(
                        "TypeError",
                        "WeakMap constructor argument is not iterable",
                        1,
                        1,
                    ));
                }
                Ok(Value::Object(weak_map))
            }
            NativeFunction::WeakMapMethod(method) => {
                self.call_weak_map_method(method, this_value, arguments)
            }
            NativeFunction::WeakSetConstructor => {
                let Value::Object(weak_set) = this_value else {
                    return Err(EngineError::new(
                        "TypeError",
                        "WeakSet constructor must be called with new",
                        1,
                        1,
                    ));
                };
                self.objects[weak_set].weak_set_data = Some(Vec::new());
                if let Some(Value::Object(iterable)) = arguments.first()
                    && self.objects[*iterable].is_array
                {
                    for index in 0..self.array_length(*iterable) {
                        let value = self
                            .get_property(*iterable, &index.to_string())
                            .unwrap_or(Value::Undefined);
                        let Value::Object(value) = value else {
                            return Err(EngineError::new(
                                "TypeError",
                                "WeakSet values must be objects",
                                1,
                                1,
                            ));
                        };
                        self.weak_set_add(weak_set, value);
                    }
                } else if arguments
                    .first()
                    .is_some_and(|value| !matches!(value, Value::Undefined | Value::Null))
                {
                    return Err(EngineError::new(
                        "TypeError",
                        "WeakSet constructor argument is not iterable",
                        1,
                        1,
                    ));
                }
                Ok(Value::Object(weak_set))
            }
            NativeFunction::WeakSetMethod(method) => {
                self.call_weak_set_method(method, this_value, arguments)
            }
            NativeFunction::Date => {
                if let Value::Object(date) = this_value {
                    let time = match arguments.first() {
                        None => current_time_value(),
                        Some(Value::String(input)) if arguments.len() == 1 => {
                            parse_date_string(input)
                                .map(|time| time as f64)
                                .unwrap_or(f64::NAN)
                        }
                        Some(value) if arguments.len() == 1 => time_clip(value.to_number()),
                        Some(_) => date_from_components(&arguments),
                    };
                    self.objects[date].date_value = Some(time);
                    Ok(Value::Object(date))
                } else {
                    Ok(Value::String(format_date_string(current_time_value())))
                }
            }
            NativeFunction::DateMethod(method) => {
                self.call_date_method(method, this_value, arguments)
            }
            NativeFunction::DateStatic(method) => self.call_date_static(method, arguments),
            NativeFunction::RegExp => {
                let regexp = if let Value::Object(regexp) = this_value {
                    regexp
                } else if let Some(Value::Object(existing)) = arguments.first()
                    && self.objects[*existing].regexp_data.is_some()
                    && arguments
                        .get(1)
                        .is_none_or(|value| matches!(value, Value::Undefined))
                {
                    return Ok(Value::Object(*existing));
                } else {
                    self.alloc_object(Some(self.regexp_prototype))
                };
                let (pattern, flags) = match arguments.first() {
                    Some(Value::Object(pattern_id))
                        if let Some(data) = self.objects[*pattern_id].regexp_data.as_ref() =>
                    {
                        (
                            data.source.clone(),
                            arguments
                                .get(1)
                                .filter(|value| !matches!(value, Value::Undefined))
                                .map(|value| self.to_string(value))
                                .unwrap_or_else(|| data.flags.clone()),
                        )
                    }
                    Some(Value::Undefined) | None => (String::new(), String::new()),
                    Some(value) => (
                        self.to_string(value),
                        arguments
                            .get(1)
                            .filter(|flags| !matches!(flags, Value::Undefined))
                            .map(|flags| self.to_string(flags))
                            .unwrap_or_default(),
                    ),
                };
                let data = build_regexp(pattern, flags)?;
                self.objects[regexp].regexp_data = Some(data.clone());
                self.set_property(regexp, "source", Value::String(regexp_source(&data.source)));
                self.set_property(regexp, "flags", Value::String(data.flags.clone()));
                self.set_property(regexp, "global", Value::Boolean(data.global));
                self.set_property(regexp, "hasIndices", Value::Boolean(data.has_indices));
                self.set_property(regexp, "ignoreCase", Value::Boolean(data.ignore_case));
                self.set_property(regexp, "multiline", Value::Boolean(data.multiline));
                self.set_property(regexp, "dotAll", Value::Boolean(data.dot_all));
                self.set_property(regexp, "unicode", Value::Boolean(data.unicode));
                self.set_property(regexp, "unicodeSets", Value::Boolean(data.unicode_sets));
                self.set_property(regexp, "sticky", Value::Boolean(data.sticky));
                self.set_property(regexp, "lastIndex", Value::Number(0.0));
                Ok(Value::Object(regexp))
            }
            NativeFunction::RegExpMethod(method) => {
                self.call_regexp_method(method, this_value, arguments)
            }
            NativeFunction::String => Ok(Value::String(
                arguments
                    .first()
                    .map(|value| self.to_string(value))
                    .unwrap_or_default(),
            )),
            NativeFunction::Number => Ok(Value::Number(
                arguments.first().map(Value::to_number).unwrap_or(0.0),
            )),
            NativeFunction::NumberMethod(method) => {
                let value = arguments.first();
                let result = match method {
                    "isNaN" => matches!(value, Some(Value::Number(value)) if value.is_nan()),
                    "isFinite" => matches!(value, Some(Value::Number(value)) if value.is_finite()),
                    "isInteger" => {
                        matches!(value, Some(Value::Number(value)) if value.is_finite() && value.fract() == 0.0)
                    }
                    "isSafeInteger" => matches!(
                        value,
                        Some(Value::Number(value))
                            if value.is_finite()
                                && value.fract() == 0.0
                                && value.abs() <= 9_007_199_254_740_991.0
                    ),
                    "parseFloat" | "parseInt" => {
                        return self.call_math(method, arguments);
                    }
                    _ => false,
                };
                Ok(Value::Boolean(result))
            }
            NativeFunction::Boolean => Ok(Value::Boolean(
                arguments.first().is_some_and(Value::is_truthy),
            )),
            NativeFunction::PrimitiveMethod(method) => match (method, this_value) {
                ("numberToString", Value::Number(value)) => {
                    if let Some(Value::Number(radix)) = arguments.first() {
                        let radix = *radix as u32;
                        if !(2..=36).contains(&radix) {
                            return Err(EngineError::new(
                                "RangeError",
                                "Radix must be between 2 and 36",
                                1,
                                1,
                            ));
                        }
                        Ok(Value::String(format_radix(value, radix)))
                    } else {
                        Ok(Value::String(Value::Number(value).to_js_string()))
                    }
                }
                ("numberValueOf", value @ Value::Number(_)) => Ok(value),
                ("booleanToString", Value::Boolean(value)) => Ok(Value::String(value.to_string())),
                ("booleanValueOf", value @ Value::Boolean(_)) => Ok(value),
                _ => Err(EngineError::new(
                    "TypeError",
                    "Incompatible primitive receiver",
                    1,
                    1,
                )),
            },
            NativeFunction::StringMethod(method) => {
                self.call_string_method(method, this_value, arguments)
            }
            NativeFunction::ObjectMethod(method) => {
                self.call_object_method(method, this_value, arguments)
            }
            NativeFunction::Math(method) => self.call_math(method, arguments),
            NativeFunction::Json(method) => match method {
                "stringify" => match arguments.first() {
                    None | Some(Value::Undefined) => Ok(Value::Undefined),
                    Some(value) => self.json_stringify_with_options(value.clone(), &arguments),
                },
                "parse" => {
                    let input = arguments
                        .first()
                        .map(|value| self.to_string(value))
                        .unwrap_or_else(|| "undefined".to_owned());
                    let mut parser = JsonParser::new(&input);
                    let value = parser.parse_value(self)?;
                    let reviver = arguments.get(1).cloned().filter(
                        |value| matches!(value, Value::Object(id) if self.objects[*id].function.is_some()),
                    );
                    if let Some(reviver) = reviver {
                        let root = self.alloc_object(Some(self.object_prototype));
                        self.set_property(root, "", value);
                        self.json_internalize(root, "", &reviver)
                    } else {
                        Ok(value)
                    }
                }
                _ => Ok(Value::Undefined),
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
            NativeFunction::ObjectDefineProperty => {
                let Some(Value::Object(object)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.defineProperty requires an object",
                        1,
                        1,
                    ));
                };
                let key = arguments
                    .get(1)
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| "undefined".to_owned());
                let Some(Value::Object(descriptor)) = arguments.get(2) else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Property description must be an object",
                        1,
                        1,
                    ));
                };
                self.define_data_property(*object, &key, *descriptor)?;
                Ok(Value::Object(*object))
            }
            NativeFunction::ObjectGetOwnPropertyDescriptor => {
                let Some(Value::Object(object)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.getOwnPropertyDescriptor requires an object",
                        1,
                        1,
                    ));
                };
                let key = arguments
                    .get(1)
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| "undefined".to_owned());
                Ok(self
                    .property_descriptor(*object, &key)
                    .map(Value::Object)
                    .unwrap_or(Value::Undefined))
            }
            NativeFunction::ObjectIntegrity(method) => {
                let object = match arguments.first() {
                    Some(Value::Object(object)) => Some(*object),
                    _ => None,
                };
                match method {
                    "preventExtensions" => {
                        if let Some(object) = object {
                            self.objects[object].extensible = false;
                        }
                        Ok(arguments.first().cloned().unwrap_or(Value::Undefined))
                    }
                    "seal" | "freeze" => {
                        if let Some(object) = object {
                            let names = self.own_property_names(object);
                            for name in names {
                                let attributes = self.objects[object]
                                    .property_attributes
                                    .entry(name)
                                    .or_insert(PropertyAttributes {
                                        writable: true,
                                        configurable: true,
                                        kind: PropertyKind::Data,
                                    });
                                attributes.configurable = false;
                                if method == "freeze" {
                                    attributes.writable = false;
                                }
                            }
                            self.objects[object].extensible = false;
                        }
                        Ok(arguments.first().cloned().unwrap_or(Value::Undefined))
                    }
                    "isExtensible" => Ok(Value::Boolean(
                        object.is_some_and(|object| self.objects[object].extensible),
                    )),
                    "isSealed" | "isFrozen" => Ok(Value::Boolean(object.is_none_or(|object| {
                        !self.objects[object].extensible
                            && self.objects[object].properties.keys().all(|name| {
                                self.objects[object]
                                    .property_attributes
                                    .get(name)
                                    .is_some_and(|attributes| {
                                        !attributes.configurable
                                            && (method != "isFrozen" || !attributes.writable)
                                    })
                            })
                    }))),
                    _ => unreachable!("known object integrity method"),
                }
            }
            NativeFunction::Reflect(method) => self.call_reflect(method, arguments),
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

    fn call_reflect(&mut self, method: &str, arguments: Vec<Value>) -> Result<Value, EngineError> {
        let Some(Value::Object(target)) = arguments.first() else {
            return Err(EngineError::new(
                "TypeError",
                format!("Reflect.{method} requires an object target"),
                1,
                1,
            ));
        };
        let target = *target;
        let key = || {
            arguments
                .get(1)
                .map(Value::to_js_string)
                .unwrap_or_else(|| "undefined".to_owned())
        };
        match method {
            "apply" => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let this_arg = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let args_list = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                let values = self.argument_list_values(args_list)?;
                self.call(target, this_arg, values)
            }
            "construct" => {
                let target = arguments.first().cloned().unwrap_or(Value::Undefined);
                let Value::Object(target_id) = target.clone() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Reflect.construct target must be a constructor",
                        1,
                        1,
                    ));
                };
                if !self.is_constructor(target_id) {
                    return Err(EngineError::new(
                        "TypeError",
                        "Reflect.construct target must be a constructor",
                        1,
                        1,
                    ));
                }
                let new_target = arguments.get(2).cloned().unwrap_or_else(|| target.clone());
                let Value::Object(new_target_id) = new_target else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Reflect.construct newTarget must be a constructor",
                        1,
                        1,
                    ));
                };
                if !self.is_constructor(new_target_id) {
                    return Err(EngineError::new(
                        "TypeError",
                        "Reflect.construct newTarget must be a constructor",
                        1,
                        1,
                    ));
                }
                let args_list = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let values = self.argument_list_values(args_list)?;
                let prototype = match self.get_property(new_target_id, "prototype") {
                    Some(Value::Object(prototype)) => Some(prototype),
                    _ => Some(self.object_prototype),
                };
                let instance = self.alloc_object(prototype);
                match self.call(target, Value::Object(instance), values)? {
                    object @ Value::Object(_) => Ok(object),
                    _ => Ok(Value::Object(instance)),
                }
            }
            "get" => self.get_property_with_receiver(
                target,
                arguments.get(2).cloned().unwrap_or(Value::Object(target)),
                &key(),
            ),
            "set" => {
                let key = key();
                let value = arguments.get(2).cloned().unwrap_or(Value::Undefined);
                let receiver = arguments.get(3).cloned().unwrap_or(Value::Object(target));
                let Value::Object(receiver_id) = receiver else {
                    return Ok(Value::Boolean(false));
                };
                let mut current = Some(target);
                let mut visited = HashSet::new();
                while let Some(owner) = current {
                    if !visited.insert(owner) {
                        break;
                    }
                    if let Some(attributes) =
                        self.objects[owner].property_attributes.get(&key).cloned()
                    {
                        match attributes.kind {
                            PropertyKind::Accessor {
                                setter: Some(setter),
                                ..
                            } => {
                                self.call(setter, Value::Object(receiver_id), vec![value])?;
                                return Ok(Value::Boolean(true));
                            }
                            PropertyKind::Accessor { setter: None, .. } => {
                                return Ok(Value::Boolean(false));
                            }
                            PropertyKind::Data if !attributes.writable => {
                                return Ok(Value::Boolean(false));
                            }
                            PropertyKind::Data => break,
                        }
                    }
                    current = self.objects[owner].prototype;
                }
                if !self.objects[receiver_id].properties.contains_key(&key)
                    && !self.objects[receiver_id].extensible
                {
                    return Ok(Value::Boolean(false));
                }
                if self.objects[receiver_id]
                    .property_attributes
                    .get(&key)
                    .is_some_and(|attributes| !attributes.writable)
                {
                    return Ok(Value::Boolean(false));
                }
                self.set_property(receiver_id, &key, value);
                Ok(Value::Boolean(true))
            }
            "has" => Ok(Value::Boolean(self.get_property(target, &key()).is_some())),
            "deleteProperty" => {
                let name = key();
                if self.objects[target]
                    .property_attributes
                    .get(&name)
                    .is_some_and(|attributes| !attributes.configurable)
                {
                    return Ok(Value::Boolean(false));
                }
                self.remove_property(target, &name);
                Ok(Value::Boolean(true))
            }
            "ownKeys" => {
                let keys = self.alloc_array();
                for (index, name) in self.own_property_names(target).into_iter().enumerate() {
                    self.set_property(keys, &index.to_string(), Value::String(name));
                }
                Ok(Value::Object(keys))
            }
            "getPrototypeOf" => Ok(self.objects[target]
                .prototype
                .map(Value::Object)
                .unwrap_or(Value::Null)),
            "setPrototypeOf" => {
                let prototype = match arguments.get(1).unwrap_or(&Value::Undefined) {
                    Value::Null => None,
                    Value::Object(id) => Some(*id),
                    _ => {
                        return Err(EngineError::new(
                            "TypeError",
                            "Prototype must be an object or null",
                            1,
                            1,
                        ));
                    }
                };
                if !self.objects[target].extensible && self.objects[target].prototype != prototype {
                    return Ok(Value::Boolean(false));
                }
                self.objects[target].prototype = prototype;
                Ok(Value::Boolean(true))
            }
            "isExtensible" => Ok(Value::Boolean(self.objects[target].extensible)),
            "preventExtensions" => {
                self.objects[target].extensible = false;
                Ok(Value::Boolean(true))
            }
            "getOwnPropertyDescriptor" => Ok(self
                .property_descriptor(target, &key())
                .map(Value::Object)
                .unwrap_or(Value::Undefined)),
            "defineProperty" => {
                let Some(Value::Object(descriptor)) = arguments.get(2) else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Property descriptor must be an object",
                        1,
                        1,
                    ));
                };
                match self.define_data_property(target, &key(), *descriptor) {
                    Ok(()) => Ok(Value::Boolean(true)),
                    Err(error) if error.name == "TypeError" => Ok(Value::Boolean(false)),
                    Err(error) => Err(error),
                }
            }
            _ => unreachable!("known Reflect method"),
        }
    }

    fn map_set(&mut self, map: ObjectId, key: Value, value: Value) {
        let entries = self.objects[map]
            .map_data
            .as_mut()
            .expect("Map internal data is initialized");
        if let Some((_, stored_value)) = entries
            .iter_mut()
            .filter_map(Option::as_mut)
            .find(|(stored_key, _)| same_value_zero(stored_key, &key))
        {
            *stored_value = value;
        } else {
            entries.push(Some((key, value)));
        }
    }

    fn is_iterator_object(&self, object: ObjectId) -> bool {
        self.objects[object].array_iterator.is_some()
            || self.objects[object].collection_iterator.is_some()
    }

    fn iterator_to_list(&mut self, iterator: ObjectId) -> Result<Vec<Value>, EngineError> {
        let next = self
            .get_property(iterator, "next")
            .unwrap_or(Value::Undefined);
        let mut values = Vec::new();
        loop {
            let Value::Object(result) =
                self.call(next.clone(), Value::Object(iterator), Vec::new())?
            else {
                return Err(EngineError::new(
                    "TypeError",
                    "Iterator next() did not return an object",
                    1,
                    1,
                ));
            };
            if self
                .get_property(result, "done")
                .is_some_and(|done| done.is_truthy())
            {
                return Ok(values);
            }
            values.push(
                self.get_property(result, "value")
                    .unwrap_or(Value::Undefined),
            );
            if values.len() >= 1_000_000 {
                return Err(EngineError::new(
                    "RangeError",
                    "Iterator produced too many values",
                    1,
                    1,
                ));
            }
        }
    }

    fn map_insert_entry(&mut self, map: ObjectId, entry: Value) -> Result<(), EngineError> {
        let Value::Object(entry) = entry else {
            return Err(EngineError::new(
                "TypeError",
                "Map entries must be objects",
                1,
                1,
            ));
        };
        let key = self.get_property(entry, "0").unwrap_or(Value::Undefined);
        let value = self.get_property(entry, "1").unwrap_or(Value::Undefined);
        self.map_set(map, key, value);
        Ok(())
    }

    fn set_add(&mut self, set: ObjectId, value: Value) {
        let values = self.objects[set]
            .set_data
            .as_mut()
            .expect("Set internal data is initialized");
        if !values
            .iter()
            .flatten()
            .any(|stored_value| same_value_zero(stored_value, &value))
        {
            values.push(Some(value));
        }
    }

    fn call_collection_iterator_method(
        &mut self,
        kind: &'static str,
        this_value: Value,
        is_map: bool,
    ) -> Result<Value, EngineError> {
        let Value::Object(collection) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "Collection iterator method called on an incompatible receiver",
                1,
                1,
            ));
        };
        let initialized = if is_map {
            self.objects[collection].map_data.is_some()
        } else {
            self.objects[collection].set_data.is_some()
        };
        if !initialized {
            return Err(EngineError::new(
                "TypeError",
                "Collection iterator method called on an incompatible receiver",
                1,
                1,
            ));
        }
        Ok(Value::Object(
            self.alloc_collection_iterator(collection, kind, is_map),
        ))
    }

    fn next_collection_iterator_value(
        &mut self,
        iterator: ObjectId,
        state: &CollectionIterator,
    ) -> Option<Value> {
        if state.done {
            return None;
        }
        loop {
            let index = self.objects[iterator]
                .collection_iterator
                .as_ref()
                .expect("collection iterator state exists")
                .index;
            let entry = if state.is_map {
                self.objects[state.collection]
                    .map_data
                    .as_ref()
                    .and_then(|entries| entries.get(index))
                    .and_then(Option::as_ref)
                    .map(|(key, value)| (key.clone(), value.clone()))
            } else {
                self.objects[state.collection]
                    .set_data
                    .as_ref()
                    .and_then(|values| values.get(index))
                    .and_then(Option::as_ref)
                    .cloned()
                    .map(|value| (value.clone(), value))
            };
            let length = if state.is_map {
                self.objects[state.collection]
                    .map_data
                    .as_ref()
                    .expect("Map internal data is initialized")
                    .len()
            } else {
                self.objects[state.collection]
                    .set_data
                    .as_ref()
                    .expect("Set internal data is initialized")
                    .len()
            };
            let iterator_state = self.objects[iterator]
                .collection_iterator
                .as_mut()
                .expect("collection iterator state exists");
            if index >= length {
                iterator_state.done = true;
                return None;
            }
            iterator_state.index += 1;
            let Some((key, value)) = entry else {
                continue;
            };
            return Some(match state.kind {
                "keys" if state.is_map => key,
                "values" => value,
                "keys" => value.clone(),
                "entries" => {
                    let pair = self.alloc_array();
                    self.set_property(pair, "0", key);
                    self.set_property(pair, "1", value);
                    Value::Object(pair)
                }
                _ => unreachable!("known collection iterator kind"),
            });
        }
    }

    fn call_collection_for_each(
        &mut self,
        collection: ObjectId,
        is_map: bool,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Some(callback @ Value::Object(callback_id)) = arguments.first().cloned() else {
            return Err(EngineError::new(
                "TypeError",
                "Collection forEach callback must be callable",
                1,
                1,
            ));
        };
        if self.objects[callback_id].function.is_none() {
            return Err(EngineError::new(
                "TypeError",
                "Collection forEach callback must be callable",
                1,
                1,
            ));
        }
        let this_arg = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let mut index = 0;
        loop {
            let item = if is_map {
                let entries = self.objects[collection]
                    .map_data
                    .as_ref()
                    .expect("Map internal data is initialized");
                if index >= entries.len() {
                    break;
                }
                entries[index].clone()
            } else {
                let values = self.objects[collection]
                    .set_data
                    .as_ref()
                    .expect("Set internal data is initialized");
                if index >= values.len() {
                    break;
                }
                values[index].clone().map(|value| (value.clone(), value))
            };
            index += 1;
            if let Some((key, value)) = item {
                let callback_args = if is_map {
                    vec![value, key, Value::Object(collection)]
                } else {
                    vec![value.clone(), value, Value::Object(collection)]
                };
                self.call(callback.clone(), this_arg.clone(), callback_args)?;
            }
        }
        Ok(Value::Undefined)
    }

    fn call_map_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Value::Object(map) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "Map method called on an incompatible receiver",
                1,
                1,
            ));
        };
        if self.objects[map].map_data.is_none() {
            return Err(EngineError::new(
                "TypeError",
                "Map method called on an incompatible receiver",
                1,
                1,
            ));
        }
        if method == "forEach" {
            return self.call_collection_for_each(map, true, arguments);
        }
        let key = arguments.first().cloned().unwrap_or(Value::Undefined);
        match method {
            "set" => {
                self.map_set(
                    map,
                    key,
                    arguments.get(1).cloned().unwrap_or(Value::Undefined),
                );
                return Ok(Value::Object(map));
            }
            "clear" => {
                self.objects[map]
                    .map_data
                    .as_mut()
                    .expect("Map internal data is initialized")
                    .fill(None);
                return Ok(Value::Undefined);
            }
            _ => {}
        }
        let entries = self.objects[map]
            .map_data
            .as_ref()
            .expect("Map internal data is initialized");
        match method {
            "get" => Ok(entries
                .iter()
                .flatten()
                .find(|(stored_key, _)| same_value_zero(stored_key, &key))
                .map(|(_, value)| value.clone())
                .unwrap_or(Value::Undefined)),
            "has" => Ok(Value::Boolean(
                entries
                    .iter()
                    .flatten()
                    .any(|(stored_key, _)| same_value_zero(stored_key, &key)),
            )),
            "delete" => {
                let entry = self.objects[map]
                    .map_data
                    .as_mut()
                    .expect("Map internal data is initialized")
                    .iter_mut()
                    .find(|entry| {
                        entry
                            .as_ref()
                            .is_some_and(|(stored_key, _)| same_value_zero(stored_key, &key))
                    });
                Ok(Value::Boolean(
                    entry.is_some_and(|entry| entry.take().is_some()),
                ))
            }
            _ => unreachable!("known Map method"),
        }
    }

    fn call_set_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Value::Object(set) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "Set method called on an incompatible receiver",
                1,
                1,
            ));
        };
        if self.objects[set].set_data.is_none() {
            return Err(EngineError::new(
                "TypeError",
                "Set method called on an incompatible receiver",
                1,
                1,
            ));
        }
        if method == "forEach" {
            return self.call_collection_for_each(set, false, arguments);
        }
        let value = arguments.first().cloned().unwrap_or(Value::Undefined);
        match method {
            "add" => {
                self.set_add(set, value);
                Ok(Value::Object(set))
            }
            "clear" => {
                self.objects[set]
                    .set_data
                    .as_mut()
                    .expect("Set internal data is initialized")
                    .fill(None);
                Ok(Value::Undefined)
            }
            "has" => Ok(Value::Boolean(
                self.objects[set]
                    .set_data
                    .as_ref()
                    .expect("Set internal data is initialized")
                    .iter()
                    .flatten()
                    .any(|stored_value| same_value_zero(stored_value, &value)),
            )),
            "delete" => {
                let values = self.objects[set]
                    .set_data
                    .as_mut()
                    .expect("Set internal data is initialized");
                let item = values.iter_mut().find(|item| {
                    item.as_ref()
                        .is_some_and(|stored_value| same_value_zero(stored_value, &value))
                });
                Ok(Value::Boolean(
                    item.is_some_and(|item| item.take().is_some()),
                ))
            }
            _ => unreachable!("known Set method"),
        }
    }

    fn weak_map_set(&mut self, weak_map: ObjectId, key: ObjectId, value: Value) {
        let entries = self.objects[weak_map]
            .weak_map_data
            .as_mut()
            .expect("WeakMap internal data is initialized");
        if let Some((_, stored_value)) = entries
            .iter_mut()
            .find(|(stored_key, _)| *stored_key == key)
        {
            *stored_value = value;
        } else {
            entries.push((key, value));
        }
    }

    fn weak_set_add(&mut self, weak_set: ObjectId, value: ObjectId) {
        let values = self.objects[weak_set]
            .weak_set_data
            .as_mut()
            .expect("WeakSet internal data is initialized");
        if !values.contains(&value) {
            values.push(value);
        }
    }

    fn call_weak_map_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Value::Object(weak_map) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "WeakMap method called on an incompatible receiver",
                1,
                1,
            ));
        };
        if self.objects[weak_map].weak_map_data.is_none() {
            return Err(EngineError::new(
                "TypeError",
                "WeakMap method called on an incompatible receiver",
                1,
                1,
            ));
        }
        let key = arguments.first().cloned().unwrap_or(Value::Undefined);
        match method {
            "set" => {
                let Value::Object(key) = key else {
                    return Err(EngineError::new(
                        "TypeError",
                        "WeakMap keys must be objects",
                        1,
                        1,
                    ));
                };
                self.weak_map_set(
                    weak_map,
                    key,
                    arguments.get(1).cloned().unwrap_or(Value::Undefined),
                );
                Ok(Value::Object(weak_map))
            }
            "get" => Ok(match key {
                Value::Object(key) => self.objects[weak_map]
                    .weak_map_data
                    .as_ref()
                    .expect("WeakMap internal data is initialized")
                    .iter()
                    .find(|(stored_key, _)| *stored_key == key)
                    .map(|(_, value)| value.clone())
                    .unwrap_or(Value::Undefined),
                _ => Value::Undefined,
            }),
            "has" => Ok(Value::Boolean(match key {
                Value::Object(key) => self.objects[weak_map]
                    .weak_map_data
                    .as_ref()
                    .expect("WeakMap internal data is initialized")
                    .iter()
                    .any(|(stored_key, _)| *stored_key == key),
                _ => false,
            })),
            "delete" => {
                let Value::Object(key) = key else {
                    return Ok(Value::Boolean(false));
                };
                let entries = self.objects[weak_map]
                    .weak_map_data
                    .as_mut()
                    .expect("WeakMap internal data is initialized");
                let old_length = entries.len();
                entries.retain(|(stored_key, _)| *stored_key != key);
                Ok(Value::Boolean(entries.len() != old_length))
            }
            _ => unreachable!("known WeakMap method"),
        }
    }

    fn call_weak_set_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Value::Object(weak_set) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "WeakSet method called on an incompatible receiver",
                1,
                1,
            ));
        };
        if self.objects[weak_set].weak_set_data.is_none() {
            return Err(EngineError::new(
                "TypeError",
                "WeakSet method called on an incompatible receiver",
                1,
                1,
            ));
        }
        let value = arguments.first().cloned().unwrap_or(Value::Undefined);
        match method {
            "add" => {
                let Value::Object(value) = value else {
                    return Err(EngineError::new(
                        "TypeError",
                        "WeakSet values must be objects",
                        1,
                        1,
                    ));
                };
                self.weak_set_add(weak_set, value);
                Ok(Value::Object(weak_set))
            }
            "has" => Ok(Value::Boolean(match value {
                Value::Object(value) => self.objects[weak_set]
                    .weak_set_data
                    .as_ref()
                    .expect("WeakSet internal data is initialized")
                    .contains(&value),
                _ => false,
            })),
            "delete" => {
                let Value::Object(value) = value else {
                    return Ok(Value::Boolean(false));
                };
                let values = self.objects[weak_set]
                    .weak_set_data
                    .as_mut()
                    .expect("WeakSet internal data is initialized");
                let old_length = values.len();
                values.retain(|stored_value| *stored_value != value);
                Ok(Value::Boolean(values.len() != old_length))
            }
            _ => unreachable!("known WeakSet method"),
        }
    }

    fn call_date_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Value::Object(date) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "Date method called on an incompatible receiver",
                1,
                1,
            ));
        };
        let Some(time) = self.objects[date].date_value else {
            return Err(EngineError::new(
                "TypeError",
                "Date method called on an incompatible receiver",
                1,
                1,
            ));
        };
        match method {
            "getTime" | "valueOf" => Ok(Value::Number(time)),
            "toString" | "toLocaleString" => Ok(Value::String(format_date_string(time))),
            "toDateString" | "toLocaleDateString" => Ok(Value::String(format_date_only(time))),
            "toTimeString" | "toLocaleTimeString" => Ok(Value::String(format_time_only(time))),
            "toUTCString" | "toGMTString" => Ok(Value::String(format_utc_string(time))),
            "setTime" => {
                let time = time_clip(arguments.first().map(Value::to_number).unwrap_or(f64::NAN));
                self.objects[date].date_value = Some(time);
                Ok(Value::Number(time))
            }
            "setFullYear" | "setUTCFullYear" | "setYear" | "setMonth" | "setUTCMonth"
            | "setDate" | "setUTCDate" | "setHours" | "setUTCHours" | "setMinutes"
            | "setUTCMinutes" | "setSeconds" | "setUTCSeconds" | "setMilliseconds"
            | "setUTCMilliseconds" => {
                if arguments.is_empty() {
                    self.objects[date].date_value = Some(f64::NAN);
                    return Ok(Value::Number(f64::NAN));
                }
                let recovers_invalid =
                    matches!(method, "setFullYear" | "setUTCFullYear" | "setYear");
                let parts = date_parts(if time.is_finite() { time } else { 0.0 });
                if !time.is_finite() && !recovers_invalid {
                    self.objects[date].date_value = Some(f64::NAN);
                    return Ok(Value::Number(f64::NAN));
                }
                let mut components = [
                    parts.year,
                    parts.month as i64 - 1,
                    parts.day as i64,
                    parts.hour as i64,
                    parts.minute as i64,
                    parts.second as i64,
                    parts.millisecond as i64,
                ];
                let component_indexes: &[usize] = match method {
                    "setFullYear" | "setUTCFullYear" | "setYear" => &[0, 1, 2],
                    "setMonth" | "setUTCMonth" => &[1, 2],
                    "setDate" | "setUTCDate" => &[2],
                    "setHours" | "setUTCHours" => &[3, 4, 5, 6],
                    "setMinutes" | "setUTCMinutes" => &[4, 5, 6],
                    "setSeconds" | "setUTCSeconds" => &[5, 6],
                    "setMilliseconds" | "setUTCMilliseconds" => &[6],
                    _ => unreachable!("known Date setter"),
                };
                for (argument_index, component_index) in component_indexes.iter().enumerate() {
                    let Some(value) = arguments.get(argument_index) else {
                        break;
                    };
                    let number = value.to_number();
                    if !number.is_finite() {
                        self.objects[date].date_value = Some(f64::NAN);
                        return Ok(Value::Number(f64::NAN));
                    }
                    components[*component_index] = number.trunc() as i64;
                }
                if method == "setYear" {
                    let year = components[0];
                    components[0] = if (0..=99).contains(&year) {
                        year + 1900
                    } else {
                        year
                    };
                }
                let updated = make_date_value(components);
                self.objects[date].date_value = Some(updated);
                Ok(Value::Number(updated))
            }
            "toISOString" => {
                if !time.is_finite() {
                    return Err(EngineError::new("RangeError", "Invalid time value", 1, 1));
                }
                Ok(Value::String(format_iso_date(time)))
            }
            "toJSON" => {
                if !time.is_finite() {
                    Ok(Value::Null)
                } else {
                    Ok(Value::String(format_iso_date(time)))
                }
            }
            "getTimezoneOffset" => Ok(Value::Number(if time.is_finite() { 0.0 } else { f64::NAN })),
            "getFullYear" | "getYear" | "getMonth" | "getDate" | "getDay" | "getHours"
            | "getMinutes" | "getSeconds" | "getMilliseconds" | "getUTCFullYear"
            | "getUTCMonth" | "getUTCDate" | "getUTCDay" | "getUTCHours" | "getUTCMinutes"
            | "getUTCSeconds" | "getUTCMilliseconds" => {
                if !time.is_finite() {
                    return Ok(Value::Number(f64::NAN));
                }
                let parts = date_parts(time);
                let value = match method {
                    "getFullYear" | "getUTCFullYear" => parts.year as f64,
                    "getYear" => parts.year as f64 - 1900.0,
                    "getMonth" | "getUTCMonth" => parts.month as f64 - 1.0,
                    "getDate" | "getUTCDate" => parts.day as f64,
                    "getDay" | "getUTCDay" => parts.weekday as f64,
                    "getHours" | "getUTCHours" => parts.hour as f64,
                    "getMinutes" | "getUTCMinutes" => parts.minute as f64,
                    "getSeconds" | "getUTCSeconds" => parts.second as f64,
                    "getMilliseconds" | "getUTCMilliseconds" => parts.millisecond as f64,
                    _ => unreachable!("known Date getter"),
                };
                Ok(Value::Number(value))
            }
            _ => unreachable!("known Date method"),
        }
    }

    fn call_date_static(&self, method: &str, arguments: Vec<Value>) -> Result<Value, EngineError> {
        match method {
            "now" => Ok(Value::Number(current_time_value())),
            "parse" => Ok(Value::Number(
                arguments
                    .first()
                    .map(Value::to_js_string)
                    .and_then(|input| parse_date_string(&input))
                    .map(|time| time as f64)
                    .unwrap_or(f64::NAN),
            )),
            "UTC" => {
                if arguments.is_empty() {
                    return Ok(Value::Number(f64::NAN));
                }
                let mut components = [0i64, 0, 1, 0, 0, 0, 0];
                for (index, value) in arguments.iter().take(7).enumerate() {
                    let number = value.to_number();
                    if !number.is_finite() {
                        return Ok(Value::Number(f64::NAN));
                    }
                    components[index] = number.trunc() as i64;
                }
                if arguments.len() < 2 {
                    components[1] = 0;
                }
                let year = components[0];
                components[0] = if (0..=99).contains(&year) {
                    year + 1900
                } else {
                    year
                };
                let time = make_date_value(components);
                Ok(Value::Number(time))
            }
            _ => unreachable!("known Date static method"),
        }
    }

    fn call_regexp_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Value::Object(regexp) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "RegExp method called on an incompatible receiver",
                1,
                1,
            ));
        };
        let Some(data) = self.objects[regexp].regexp_data.clone() else {
            return Err(EngineError::new(
                "TypeError",
                "RegExp method called on an incompatible receiver",
                1,
                1,
            ));
        };
        if method == "toString" {
            return Ok(Value::String(format!(
                "/{}/{}",
                regexp_source(&data.source),
                data.flags
            )));
        }
        let input = arguments
            .first()
            .map(|value| self.to_string(value))
            .unwrap_or_else(|| "undefined".to_owned());
        let last_index = if data.global || data.sticky {
            self.get_own_property(regexp, "lastIndex")
                .unwrap_or(Value::Number(0.0))
                .to_number()
        } else {
            0.0
        };
        let start = if last_index.is_nan() || last_index <= 0.0 {
            0
        } else if last_index.is_finite() {
            last_index.trunc() as usize
        } else {
            usize::MAX
        };
        let input_utf16: Vec<u16> = input.encode_utf16().collect();
        let matched: Option<regress::Match> = if start <= input_utf16.len() {
            let found = if data.unicode || data.unicode_sets {
                data.matcher.find_from_utf16(&input_utf16, start).next()
            } else {
                data.matcher.find_from_ucs2(&input_utf16, start).next()
            };
            found.filter(|found| !data.sticky || found.start() == start)
        } else {
            None
        };
        let Some(found) = matched else {
            if data.global || data.sticky {
                self.set_property(regexp, "lastIndex", Value::Number(0.0));
            }
            return Ok(if method == "test" {
                Value::Boolean(false)
            } else {
                Value::Null
            });
        };
        let start = found.start();
        let end = found.end();
        if data.global || data.sticky {
            self.set_property(regexp, "lastIndex", Value::Number(end as f64));
        }
        if method == "test" {
            return Ok(Value::Boolean(true));
        }
        let result = self.alloc_array();
        let capture_count = found.captures.len() + 1;
        for index in 0..capture_count {
            let value = found
                .group(index)
                .map(|range| Value::String(String::from_utf16_lossy(&input_utf16[range])))
                .unwrap_or(Value::Undefined);
            self.set_property(result, &index.to_string(), value);
        }
        self.set_property(result, "index", Value::Number(start as f64));
        self.set_property(result, "input", Value::String(input));
        let named_groups = found
            .named_groups()
            .map(|(name, range)| (name.to_owned(), range))
            .collect::<Vec<_>>();
        if named_groups.is_empty() {
            self.set_property(result, "groups", Value::Undefined);
        } else {
            let groups = self.alloc_object(Some(self.object_prototype));
            for (name, range) in named_groups {
                let value = range
                    .map(|range| Value::String(String::from_utf16_lossy(&input_utf16[range])))
                    .unwrap_or(Value::Undefined);
                self.set_property(groups, &name, value);
            }
            self.set_property(result, "groups", Value::Object(groups));
        }
        if data.has_indices {
            let indices = self.alloc_array();
            for index in 0..capture_count {
                let pair = found.group(index).map(|range| {
                    let pair = self.alloc_array();
                    self.set_property(pair, "0", Value::Number(range.start as f64));
                    self.set_property(pair, "1", Value::Number(range.end as f64));
                    Value::Object(pair)
                });
                self.set_property(
                    indices,
                    &index.to_string(),
                    pair.unwrap_or(Value::Undefined),
                );
            }
            let groups = self.alloc_object(Some(self.object_prototype));
            let mut has_named_groups = false;
            for (name, range) in found.named_groups() {
                has_named_groups = true;
                let value = range
                    .map(|range| {
                        let pair = self.alloc_array();
                        self.set_property(pair, "0", Value::Number(range.start as f64));
                        self.set_property(pair, "1", Value::Number(range.end as f64));
                        Value::Object(pair)
                    })
                    .unwrap_or(Value::Undefined);
                self.set_property(groups, name, value);
            }
            let groups = if has_named_groups {
                Value::Object(groups)
            } else {
                Value::Undefined
            };
            self.set_property(indices, "groups", groups);
            self.set_property(result, "indices", Value::Object(indices));
        } else {
            self.set_property(result, "indices", Value::Undefined);
        }
        Ok(Value::Object(result))
    }

    fn argument_list_values(&self, list: Value) -> Result<Vec<Value>, EngineError> {
        match list {
            Value::Undefined | Value::Null => Ok(Vec::new()),
            Value::Object(object) => {
                let length = self
                    .get_property(object, "length")
                    .unwrap_or(Value::Undefined)
                    .to_number();
                let length = if length.is_nan() || length <= 0.0 {
                    0
                } else if !length.is_finite() || length > 1_000_000.0 {
                    return Err(EngineError::new(
                        "RangeError",
                        "Argument list is too large",
                        1,
                        1,
                    ));
                } else {
                    length.trunc() as usize
                };
                Ok((0..length)
                    .map(|index| {
                        self.get_property(object, &index.to_string())
                            .unwrap_or(Value::Undefined)
                    })
                    .collect())
            }
            _ => Err(EngineError::new(
                "TypeError",
                "Argument list must be an object",
                1,
                1,
            )),
        }
    }

    fn call_array_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        if method == "of" {
            let array = self.alloc_array();
            for (index, value) in arguments.into_iter().enumerate() {
                self.set_property(array, &index.to_string(), value);
            }
            return Ok(Value::Object(array));
        }
        if method == "from" {
            let mut values = match arguments.first() {
                Some(Value::Object(id)) if self.is_iterator_object(*id) => {
                    self.iterator_to_list(*id)?.into_iter().map(Some).collect()
                }
                Some(Value::Object(id)) if self.objects[*id].is_array => self
                    .array_iterator_values(*id)?
                    .into_iter()
                    .map(Some)
                    .collect(),
                Some(Value::Object(id)) if self.objects[*id].map_data.is_some() => {
                    let entries = self.objects[*id]
                        .map_data
                        .as_ref()
                        .expect("Map internal data is initialized")
                        .iter()
                        .flatten()
                        .cloned()
                        .collect::<Vec<_>>();
                    entries
                        .into_iter()
                        .map(|(key, value)| {
                            let pair = self.alloc_array();
                            self.set_property(pair, "0", key);
                            self.set_property(pair, "1", value);
                            Some(Value::Object(pair))
                        })
                        .collect()
                }
                Some(Value::Object(id)) if self.objects[*id].set_data.is_some() => self.objects
                    [*id]
                    .set_data
                    .as_ref()
                    .expect("Set internal data is initialized")
                    .iter()
                    .flatten()
                    .cloned()
                    .map(Some)
                    .collect(),
                Some(Value::String(value)) => value
                    .chars()
                    .map(|character| Some(Value::String(character.to_string())))
                    .collect(),
                Some(Value::Null | Value::Undefined) | None => {
                    return Err(EngineError::new(
                        "TypeError",
                        "Array.from requires an array-like value",
                        1,
                        1,
                    ));
                }
                Some(Value::Object(id)) => {
                    let length = self
                        .get_property(*id, "length")
                        .map(|value| value.to_number())
                        .unwrap_or(0.0);
                    let length = if !length.is_finite() || length <= 0.0 {
                        0
                    } else {
                        length.ceil().min(u32::MAX as f64) as usize
                    };
                    (0..length)
                        .map(|index| self.get_property(*id, &index.to_string()))
                        .collect()
                }
                Some(_) => Vec::new(),
            };
            if let Some(mapper) = arguments.get(1) {
                for (index, value) in values.iter_mut().enumerate() {
                    let input = value.clone().unwrap_or(Value::Undefined);
                    *value = Some(self.call(
                        mapper.clone(),
                        arguments.get(2).cloned().unwrap_or(Value::Undefined),
                        vec![input, Value::Number(index as f64)],
                    )?);
                }
            }
            let source_length = values.len();
            let array = self.alloc_array();
            for (index, value) in values.into_iter().enumerate() {
                if let Some(value) = value {
                    self.set_property(array, &index.to_string(), value);
                }
            }
            self.objects[array]
                .properties
                .insert("length".to_owned(), Value::Number(source_length as f64));
            return Ok(Value::Object(array));
        }

        let Value::Object(array) = this_value else {
            return Err(EngineError::new(
                "TypeError",
                "Array method called on a non-object",
                1,
                1,
            ));
        };
        if !self.objects[array].is_array {
            return Err(EngineError::new(
                "TypeError",
                "Array method called on a non-array",
                1,
                1,
            ));
        }
        let length = self.array_length(array);
        let values: Vec<Option<Value>> = (0..length)
            .map(|index| self.get_property(array, &index.to_string()))
            .collect();
        if method == "flat" {
            let depth = match arguments.first() {
                None | Some(Value::Undefined) => 1.0,
                Some(value) => value.to_number(),
            };
            let depth = if depth.is_nan() || depth <= 0.0 {
                0
            } else if depth.is_infinite() {
                1000
            } else {
                depth.trunc().min(1000.0) as usize
            };
            let result = self.alloc_array();
            let mut flattened = Vec::new();
            for value in values.iter().flatten() {
                self.flatten_array_value(value, depth, &mut flattened);
            }
            for (index, value) in flattened.iter().enumerate() {
                self.set_property(result, &index.to_string(), value.clone());
            }
            return Ok(Value::Object(result));
        }
        if method == "flatMap" {
            let Some(callback) = arguments.first().cloned() else {
                return Err(EngineError::new(
                    "TypeError",
                    "Array callback method requires a callback",
                    1,
                    1,
                ));
            };
            let this_arg = arguments.get(1).cloned().unwrap_or(Value::Undefined);
            let result = self.alloc_array();
            let mut target = 0;
            for (index, value) in values.iter().enumerate() {
                if let Some(value) = value {
                    let mapped = self.call(
                        callback.clone(),
                        this_arg.clone(),
                        vec![
                            value.clone(),
                            Value::Number(index as f64),
                            Value::Object(array),
                        ],
                    )?;
                    let mut flattened = Vec::new();
                    self.flatten_array_value(&mapped, 1, &mut flattened);
                    for item in flattened {
                        self.set_property(result, &target.to_string(), item);
                        target += 1;
                    }
                }
            }
            return Ok(Value::Object(result));
        }
        match method {
            "push" => {
                for (offset, value) in arguments.into_iter().enumerate() {
                    self.set_property(array, &(length + offset).to_string(), value);
                }
                Ok(Value::Number(self.array_length(array) as f64))
            }
            "pop" => {
                if length == 0 {
                    return Ok(Value::Undefined);
                }
                let last = (length - 1).to_string();
                let value = self.get_property(array, &last).unwrap_or(Value::Undefined);
                self.objects[array].properties.remove(&last);
                self.objects[array]
                    .properties
                    .insert("length".to_owned(), Value::Number((length - 1) as f64));
                Ok(value)
            }
            "shift" => {
                if length == 0 {
                    return Ok(Value::Undefined);
                }
                let first = values[0].clone().unwrap_or(Value::Undefined);
                for (index, value) in values.iter().enumerate().skip(1) {
                    let from = index.to_string();
                    let to = (index - 1).to_string();
                    match value.clone() {
                        Some(value) => self.objects[array].properties.insert(to, value),
                        None => self.objects[array].properties.remove(&to),
                    };
                    self.objects[array].properties.remove(&from);
                }
                self.objects[array]
                    .properties
                    .insert("length".to_owned(), Value::Number((length - 1) as f64));
                Ok(first)
            }
            "unshift" => {
                let count = arguments.len();
                for index in (0..length).rev() {
                    let to = (index + count).to_string();
                    match values[index].clone() {
                        Some(value) => self.objects[array].properties.insert(to, value),
                        None => self.objects[array].properties.remove(&to),
                    };
                }
                for (index, value) in arguments.into_iter().enumerate() {
                    self.objects[array]
                        .properties
                        .insert(index.to_string(), value);
                }
                let new_length = length + count;
                self.objects[array]
                    .properties
                    .insert("length".to_owned(), Value::Number(new_length as f64));
                Ok(Value::Number(new_length as f64))
            }
            "splice" => {
                let start = normalize_slice_index(
                    arguments.first().map(Value::to_number).unwrap_or(0.0),
                    length,
                );
                let delete_count = arguments
                    .get(1)
                    .map(Value::to_number)
                    .map(|value| {
                        if value.is_nan() || value <= 0.0 {
                            0
                        } else {
                            (value.trunc() as usize).min(length - start)
                        }
                    })
                    .unwrap_or(length - start);
                let removed = self.alloc_array();
                for index in 0..delete_count {
                    if let Some(value) = values[start + index].clone() {
                        self.set_property(removed, &index.to_string(), value);
                    }
                }
                self.objects[removed]
                    .properties
                    .insert("length".to_owned(), Value::Number(delete_count as f64));
                let mut updated =
                    Vec::with_capacity(length - delete_count + arguments.len().saturating_sub(2));
                updated.extend(values[..start].iter().cloned());
                updated.extend(arguments.iter().skip(2).cloned().map(Some));
                updated.extend(values[start + delete_count..].iter().cloned());
                let old_indices: Vec<String> = self.objects[array]
                    .properties
                    .keys()
                    .filter(|name| array_index(name).is_some())
                    .cloned()
                    .collect();
                for name in old_indices {
                    self.objects[array].properties.remove(&name);
                }
                for (index, value) in updated.iter().enumerate() {
                    if let Some(value) = value {
                        self.set_property(array, &index.to_string(), value.clone());
                    }
                }
                self.objects[array]
                    .properties
                    .insert("length".to_owned(), Value::Number(updated.len() as f64));
                Ok(Value::Object(removed))
            }
            "join" | "toString" => {
                let separator = arguments
                    .first()
                    .filter(|value| !matches!(value, Value::Undefined))
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| ",".to_owned());
                let mut ancestors = HashSet::new();
                ancestors.insert(array);
                Ok(Value::String(self.array_join_string(
                    array,
                    &separator,
                    &mut ancestors,
                )))
            }
            "slice" => {
                let start = normalize_slice_index(
                    arguments.first().map(Value::to_number).unwrap_or(0.0),
                    length,
                );
                let end = normalize_slice_index(
                    arguments
                        .get(1)
                        .map(Value::to_number)
                        .unwrap_or(length as f64),
                    length,
                )
                .max(start);
                let result = self.alloc_array();
                for (target, source) in (start..end).enumerate() {
                    if let Some(value) = values[source].clone() {
                        self.set_property(result, &target.to_string(), value);
                    }
                }
                self.objects[result]
                    .properties
                    .insert("length".to_owned(), Value::Number((end - start) as f64));
                Ok(Value::Object(result))
            }
            "indexOf" | "lastIndexOf" | "includes" => {
                let search = arguments.first().cloned().unwrap_or(Value::Undefined);
                let start = if method == "lastIndexOf" {
                    match arguments.get(1).map(Value::to_number) {
                        None => length.saturating_sub(1),
                        Some(value) if value.is_nan() => length.saturating_sub(1),
                        Some(value) if value < 0.0 => {
                            let index = length as isize + value.trunc() as isize;
                            if index < 0 {
                                return Ok(Value::Number(-1.0));
                            }
                            index as usize
                        }
                        Some(value) => (value.trunc() as usize).min(length.saturating_sub(1)),
                    }
                } else {
                    normalize_relative_start(
                        arguments.get(1).map(Value::to_number).unwrap_or(0.0),
                        length,
                    )
                };
                if length == 0 {
                    return Ok(if method == "includes" {
                        Value::Boolean(false)
                    } else {
                        Value::Number(-1.0)
                    });
                }
                let mut indices: Box<dyn Iterator<Item = usize>> = if method == "lastIndexOf" {
                    Box::new((0..=start.min(length.saturating_sub(1))).rev())
                } else {
                    Box::new(start..length)
                };
                let found = indices.find(|index| {
                    let current = values[*index].as_ref().unwrap_or(&Value::Undefined);
                    if method == "includes" {
                        same_value_zero(current, &search)
                    } else {
                        values[*index]
                            .as_ref()
                            .is_some_and(|current| self.strict_equal(current, &search))
                    }
                });
                Ok(if method == "includes" {
                    Value::Boolean(found.is_some())
                } else {
                    Value::Number(found.map(|index| index as f64).unwrap_or(-1.0))
                })
            }
            "reverse" => {
                for index in 0..length / 2 {
                    let opposite = length - 1 - index;
                    for (to, from) in [(index, opposite), (opposite, index)] {
                        let key = to.to_string();
                        match values[from].clone() {
                            Some(value) => {
                                self.objects[array].properties.insert(key, value);
                            }
                            None => {
                                self.objects[array].properties.remove(&key);
                            }
                        }
                    }
                }
                Ok(Value::Object(array))
            }
            "concat" => {
                let result = self.alloc_array();
                let mut target = 0;
                for value in std::iter::once(Value::Object(array)).chain(arguments) {
                    if let Value::Object(id) = value
                        && self.objects[id].is_array
                    {
                        let count = self.array_length(id);
                        for index in 0..count {
                            if let Some(value) = self.get_property(id, &index.to_string()) {
                                self.set_property(result, &target.to_string(), value);
                            }
                            target += 1;
                        }
                    } else {
                        self.set_property(result, &target.to_string(), value);
                        target += 1;
                    }
                }
                self.objects[result]
                    .properties
                    .insert("length".to_owned(), Value::Number(target as f64));
                Ok(Value::Object(result))
            }
            "at" => {
                let index = normalize_relative_index(
                    arguments.first().map(Value::to_number).unwrap_or(f64::NAN),
                    length,
                );
                Ok(index
                    .and_then(|index| values[index].clone())
                    .unwrap_or(Value::Undefined))
            }
            "fill" => {
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                let start = normalize_index(
                    arguments.get(1).map(Value::to_number).unwrap_or(0.0),
                    length,
                );
                let end = normalize_index(
                    arguments
                        .get(2)
                        .map(Value::to_number)
                        .unwrap_or(length as f64),
                    length,
                );
                for index in start..end {
                    self.set_property(array, &index.to_string(), value.clone());
                }
                Ok(Value::Object(array))
            }
            "copyWithin" => {
                let target = normalize_relative_start(
                    arguments.first().map(Value::to_number).unwrap_or(0.0),
                    length,
                );
                let start = normalize_relative_start(
                    arguments.get(1).map(Value::to_number).unwrap_or(0.0),
                    length,
                );
                let end = normalize_relative_start(
                    arguments
                        .get(2)
                        .map(Value::to_number)
                        .unwrap_or(length as f64),
                    length,
                );
                let count = end.saturating_sub(start).min(length - target);
                let copied = values[start..start + count].to_vec();
                for (offset, value) in copied.into_iter().enumerate() {
                    let key = (target + offset).to_string();
                    match value {
                        Some(value) => self.set_property(array, &key, value),
                        None => {
                            self.objects[array].properties.remove(&key);
                        }
                    }
                }
                Ok(Value::Object(array))
            }
            "sort" => {
                let comparator = arguments.first().cloned();
                let mut defined = Vec::new();
                let mut undefined_count = 0;
                for value in values.iter().flatten() {
                    if matches!(value, Value::Undefined) {
                        undefined_count += 1;
                    } else {
                        defined.push(value.clone());
                    }
                }
                for index in 1..defined.len() {
                    let mut position = index;
                    while position > 0 {
                        let ordering = if let Some(comparator) = &comparator {
                            let compared = self.call(
                                comparator.clone(),
                                Value::Undefined,
                                vec![defined[position - 1].clone(), defined[position].clone()],
                            )?;
                            let number = compared.to_number();
                            if number.is_nan() || number == 0.0 {
                                std::cmp::Ordering::Equal
                            } else if number < 0.0 {
                                std::cmp::Ordering::Less
                            } else {
                                std::cmp::Ordering::Greater
                            }
                        } else {
                            self.to_string(&defined[position - 1])
                                .cmp(&self.to_string(&defined[position]))
                        };
                        if ordering != std::cmp::Ordering::Greater {
                            break;
                        }
                        defined.swap(position - 1, position);
                        position -= 1;
                    }
                }
                let mut sorted = defined;
                sorted.extend(std::iter::repeat_n(Value::Undefined, undefined_count));
                for index in 0..length {
                    let key = index.to_string();
                    if let Some(value) = sorted.get(index) {
                        self.set_property(array, &key, value.clone());
                    } else {
                        self.objects[array].properties.remove(&key);
                    }
                }
                Ok(Value::Object(array))
            }
            "map" | "filter" | "forEach" | "some" | "every" | "find" | "findIndex" | "reduce"
            | "reduceRight" => self.array_callback_method(method, array, values, arguments),
            _ => Ok(Value::Undefined),
        }
    }

    fn flatten_array_value(&self, value: &Value, depth: usize, output: &mut Vec<Value>) {
        if let Value::Object(id) = value
            && self.objects[*id].is_array
            && depth > 0
        {
            let length = self.array_length(*id);
            for index in 0..length {
                if let Some(item) = self.get_property(*id, &index.to_string()) {
                    self.flatten_array_value(&item, depth - 1, output);
                }
            }
        } else {
            output.push(value.clone());
        }
    }

    fn array_iterator_values(&mut self, array: ObjectId) -> Result<Vec<Value>, EngineError> {
        let iterator = self.alloc_array_iterator(array, "values");
        self.iterator_to_list(iterator)
    }

    fn array_callback_method(
        &mut self,
        method: &str,
        array: ObjectId,
        values: Vec<Option<Value>>,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let Some(callback) = arguments.first().cloned() else {
            return Err(EngineError::new(
                "TypeError",
                "Array callback method requires a callback",
                1,
                1,
            ));
        };
        let this_arg = arguments.get(1).cloned().unwrap_or(Value::Undefined);
        let result = if method == "map" || method == "filter" {
            Some(self.alloc_array())
        } else {
            None
        };
        let mut output_index = 0;
        let is_reduce = method == "reduce" || method == "reduceRight";
        let mut accumulator = arguments.get(1).cloned();
        let mut started = !is_reduce || accumulator.is_some();
        let indices: Box<dyn Iterator<Item = (usize, &Option<Value>)>> = if method == "reduceRight"
        {
            Box::new(values.iter().enumerate().rev())
        } else {
            Box::new(values.iter().enumerate())
        };
        for (index, entry) in indices {
            let value = match entry {
                Some(value) => value.clone(),
                None if method == "find" || method == "findIndex" => Value::Undefined,
                None => continue,
            };
            if is_reduce && !started {
                accumulator = Some(value);
                started = true;
                continue;
            }
            let callback_this = if is_reduce {
                Value::Undefined
            } else {
                this_arg.clone()
            };
            let callback_args = if is_reduce {
                vec![
                    accumulator.clone().unwrap_or(Value::Undefined),
                    value.clone(),
                    Value::Number(index as f64),
                    Value::Object(array),
                ]
            } else {
                vec![
                    value.clone(),
                    Value::Number(index as f64),
                    Value::Object(array),
                ]
            };
            let callback_result = self.call(callback.clone(), callback_this, callback_args)?;
            match method {
                "map" => {
                    self.set_property(
                        result.expect("map result exists"),
                        &index.to_string(),
                        callback_result,
                    );
                }
                "filter" if callback_result.is_truthy() => {
                    self.set_property(
                        result.expect("filter result exists"),
                        &output_index.to_string(),
                        value,
                    );
                    output_index += 1;
                }
                "some" if callback_result.is_truthy() => {
                    return Ok(Value::Boolean(true));
                }
                "every" if !callback_result.is_truthy() => {
                    return Ok(Value::Boolean(false));
                }
                "find" if callback_result.is_truthy() => return Ok(value),
                "findIndex" if callback_result.is_truthy() => {
                    return Ok(Value::Number(index as f64));
                }
                "reduce" | "reduceRight" => accumulator = Some(callback_result),
                _ => {}
            }
        }
        match method {
            "map" => {
                let result = result.expect("map result exists");
                self.objects[result]
                    .properties
                    .insert("length".to_owned(), Value::Number(values.len() as f64));
                Ok(Value::Object(result))
            }
            "filter" => Ok(Value::Object(result.expect("filter result exists"))),
            "forEach" => Ok(Value::Undefined),
            "some" => Ok(Value::Boolean(false)),
            "every" => Ok(Value::Boolean(true)),
            "find" => Ok(Value::Undefined),
            "findIndex" => Ok(Value::Number(-1.0)),
            "reduce" | "reduceRight" if !started => Err(EngineError::new(
                "TypeError",
                "Reduce of empty array with no initial value",
                1,
                1,
            )),
            "reduce" | "reduceRight" => Ok(accumulator.unwrap_or(Value::Undefined)),
            _ => unreachable!("only callback array methods reach this function"),
        }
    }

    fn array_length(&self, array: ObjectId) -> usize {
        self.objects[array]
            .properties
            .get("length")
            .map(Value::to_number)
            .unwrap_or(0.0) as usize
    }

    fn call_string_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        if method == "fromCharCode" {
            let units: Vec<u16> = arguments
                .iter()
                .map(|value| value.to_number() as u16)
                .collect();
            return Ok(Value::String(
                std::char::decode_utf16(units)
                    .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect(),
            ));
        }
        if method == "fromCodePoint" {
            let mut output = String::new();
            for value in arguments {
                let number = value.to_number();
                if !number.is_finite()
                    || number.fract() != 0.0
                    || !(0.0..=0x10ffff as f64).contains(&number)
                    || (0xd800..=0xdfff).contains(&(number as u32))
                {
                    return Err(EngineError::new("RangeError", "Invalid code point", 1, 1));
                }
                output.push(char::from_u32(number as u32).expect("validated Unicode scalar"));
            }
            return Ok(Value::String(output));
        }
        let string = match this_value {
            Value::Null | Value::Undefined => {
                return Err(EngineError::new(
                    "TypeError",
                    "String method called on null or undefined",
                    1,
                    1,
                ));
            }
            value => self.to_string(&value),
        };
        let chars: Vec<char> = string.chars().collect();
        let result = match method {
            "toString" | "valueOf" => return Ok(Value::String(string)),
            "toLowerCase" => string.to_lowercase(),
            "toUpperCase" => string.to_uppercase(),
            "trim" => string.trim().to_owned(),
            "concat" => {
                let mut result = string;
                for value in arguments {
                    result.push_str(&self.to_string(&value));
                }
                return Ok(Value::String(result));
            }
            "repeat" => {
                let count = arguments.first().map(Value::to_number).unwrap_or(0.0);
                if !count.is_finite() || count < 0.0 {
                    return Err(EngineError::new("RangeError", "Invalid repeat count", 1, 1));
                }
                return Ok(Value::String(
                    string.repeat((count as usize).min(1_000_000)),
                ));
            }
            "padStart" | "padEnd" => {
                let target = arguments.first().map(Value::to_number).unwrap_or(0.0);
                let target = if target.is_finite() && target > 0.0 {
                    (target as usize).min(1_000_000)
                } else {
                    0
                };
                let current_length = string.encode_utf16().count();
                if target <= current_length {
                    return Ok(Value::String(string));
                }
                let filler = arguments
                    .get(1)
                    .filter(|value| !matches!(value, Value::Undefined))
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| " ".to_owned());
                if filler.is_empty() {
                    return Ok(Value::String(string));
                }
                let padding_length = target - current_length;
                let padding: String = filler.chars().cycle().take(padding_length).collect();
                return Ok(Value::String(if method == "padStart" {
                    format!("{padding}{string}")
                } else {
                    format!("{string}{padding}")
                }));
            }
            "charAt" => chars
                .get(normalize_index(
                    arguments.first().map(Value::to_number).unwrap_or(0.0),
                    chars.len(),
                ))
                .map(char::to_string)
                .unwrap_or_default(),
            "charCodeAt" => {
                let index = normalize_index(
                    arguments.first().map(Value::to_number).unwrap_or(0.0),
                    chars.len(),
                );
                return Ok(chars
                    .get(index)
                    .map(|character| Value::Number(*character as u32 as f64))
                    .unwrap_or(Value::Number(f64::NAN)));
            }
            "includes" | "startsWith" | "endsWith" => {
                let search = arguments
                    .first()
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| "undefined".to_owned());
                return Ok(Value::Boolean(match method {
                    "includes" => string.contains(&search),
                    "startsWith" => string.starts_with(&search),
                    _ => string.ends_with(&search),
                }));
            }
            "indexOf" => {
                let search = arguments
                    .first()
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| "undefined".to_owned());
                let start = normalize_index(
                    arguments.get(1).map(Value::to_number).unwrap_or(0.0),
                    chars.len(),
                );
                let tail: String = chars[start..].iter().collect();
                return Ok(Value::Number(
                    tail.find(&search)
                        .map(|byte_offset| (start + tail[..byte_offset].chars().count()) as f64)
                        .unwrap_or(-1.0),
                ));
            }
            "slice" | "substring" => {
                let mut start = if method == "slice" {
                    normalize_slice_index(
                        arguments.first().map(Value::to_number).unwrap_or(0.0),
                        chars.len(),
                    )
                } else {
                    normalize_index(
                        arguments.first().map(Value::to_number).unwrap_or(0.0),
                        chars.len(),
                    )
                };
                let mut end = if method == "slice" {
                    normalize_slice_index(
                        arguments
                            .get(1)
                            .map(Value::to_number)
                            .unwrap_or(chars.len() as f64),
                        chars.len(),
                    )
                } else {
                    normalize_index(
                        arguments
                            .get(1)
                            .map(Value::to_number)
                            .unwrap_or(chars.len() as f64),
                        chars.len(),
                    )
                };
                if method == "substring" && start > end {
                    std::mem::swap(&mut start, &mut end);
                }
                chars[start..end].iter().collect()
            }
            "split" => {
                let limit = arguments
                    .get(1)
                    .map(Value::to_number)
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| (value as usize).min(u32::MAX as usize))
                    .unwrap_or(u32::MAX as usize);
                let pieces = match arguments.first() {
                    None | Some(Value::Undefined) => vec![string],
                    Some(separator) => {
                        let separator = self.to_string(separator);
                        if separator.is_empty() {
                            string
                                .chars()
                                .map(|character| character.to_string())
                                .collect()
                        } else {
                            string.split(&separator).map(str::to_owned).collect()
                        }
                    }
                };
                let array = self.alloc_array();
                for (index, piece) in pieces.into_iter().take(limit).enumerate() {
                    self.set_property(array, &index.to_string(), Value::String(piece));
                }
                return Ok(Value::Object(array));
            }
            _ => string,
        };
        Ok(Value::String(result))
    }

    fn call_object_method(
        &mut self,
        method: &str,
        this_value: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, EngineError> {
        match method {
            "is" => Ok(Value::Boolean(match arguments.as_slice() {
                [left, right, ..] => same_value_zero(left, right),
                _ => false,
            })),
            "hasOwnProperty" | "propertyIsEnumerable" => {
                let Value::Object(object) = this_value else {
                    return Ok(Value::Boolean(false));
                };
                let name = arguments
                    .first()
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| "undefined".to_owned());
                Ok(Value::Boolean(
                    self.get_own_property(object, &name).is_some()
                        && (method != "propertyIsEnumerable"
                            || !self.objects[object].non_enumerable.contains(&name)),
                ))
            }
            "isPrototypeOf" => {
                let Value::Object(prototype) = this_value else {
                    return Ok(Value::Boolean(false));
                };
                let Some(Value::Object(object)) = arguments.first() else {
                    return Ok(Value::Boolean(false));
                };
                let mut current = self.objects[*object].prototype;
                let mut visited = HashSet::new();
                while let Some(id) = current {
                    if id == prototype {
                        return Ok(Value::Boolean(true));
                    }
                    if !visited.insert(id) {
                        break;
                    }
                    current = self.objects[id].prototype;
                }
                Ok(Value::Boolean(false))
            }
            "toString" => match this_value {
                Value::Null => Ok(Value::String("[object Null]".to_owned())),
                Value::Undefined => Ok(Value::String("[object Undefined]".to_owned())),
                Value::Object(id) if self.is_error_object(id) => {
                    let name = self
                        .get_property(id, "name")
                        .map(|value| self.to_string(&value))
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| "Error".to_owned());
                    let message = self
                        .get_property(id, "message")
                        .map(|value| self.to_string(&value))
                        .unwrap_or_default();
                    Ok(Value::String(if message.is_empty() {
                        name
                    } else {
                        format!("{name}: {message}")
                    }))
                }
                Value::Object(id) if self.objects[id].is_array => {
                    Ok(Value::String("[object Array]".to_owned()))
                }
                Value::Object(id) if self.objects[id].function.is_some() => {
                    Ok(Value::String("[object Function]".to_owned()))
                }
                Value::Object(_) => Ok(Value::String("[object Object]".to_owned())),
                _ => Ok(Value::String(format!(
                    "[object {}]",
                    self.to_string(&this_value)
                ))),
            },
            "hasOwn" => {
                let Some(Value::Object(id)) = arguments.first() else {
                    return Ok(Value::Boolean(false));
                };
                let name = arguments
                    .get(1)
                    .map(|value| self.to_string(value))
                    .unwrap_or_else(|| "undefined".to_owned());
                Ok(Value::Boolean(self.get_own_property(*id, &name).is_some()))
            }
            "defineProperties" => {
                let Some(Value::Object(target)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.defineProperties requires an object",
                        1,
                        1,
                    ));
                };
                let Some(Value::Object(descriptors)) = arguments.get(1) else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Property descriptions must be an object",
                        1,
                        1,
                    ));
                };
                let entries: Vec<(String, ObjectId)> = self
                    .enumerable_property_names(*descriptors)
                    .into_iter()
                    .filter_map(|key| match self.get_own_property(*descriptors, &key) {
                        Some(Value::Object(descriptor)) => Some(Ok((key, descriptor))),
                        Some(_) => Some(Err(EngineError::new(
                            "TypeError",
                            "Property description must be an object",
                            1,
                            1,
                        ))),
                        None => None,
                    })
                    .collect::<Result<_, _>>()?;
                for (key, descriptor) in entries {
                    self.define_data_property(*target, &key, descriptor)?;
                }
                Ok(Value::Object(*target))
            }
            "getOwnPropertyDescriptors" => {
                let Some(Value::Object(object)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.getOwnPropertyDescriptors requires an object",
                        1,
                        1,
                    ));
                };
                let result = self.alloc_object(Some(self.object_prototype));
                for name in self.own_property_names(*object) {
                    if let Some(descriptor) = self.property_descriptor(*object, &name) {
                        self.set_property(result, &name, Value::Object(descriptor));
                    }
                }
                Ok(Value::Object(result))
            }
            "keys" | "getOwnPropertyNames" | "values" | "entries" => {
                let Some(Value::Object(id)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        format!("Object.{method} requires an object"),
                        1,
                        1,
                    ));
                };
                let names = if method == "getOwnPropertyNames" {
                    self.own_property_names(*id)
                } else {
                    self.enumerable_property_names(*id)
                };
                let result = self.alloc_array();
                for (index, name) in names.into_iter().enumerate() {
                    let value = self
                        .get_own_property(*id, &name)
                        .unwrap_or(Value::Undefined);
                    let item = match method {
                        "keys" | "getOwnPropertyNames" => Value::String(name),
                        "values" => value,
                        _ => {
                            let pair = self.alloc_array();
                            self.set_property(pair, "0", Value::String(name));
                            self.set_property(pair, "1", value);
                            Value::Object(pair)
                        }
                    };
                    self.set_property(result, &index.to_string(), item);
                }
                Ok(Value::Object(result))
            }
            "assign" => {
                let Some(Value::Object(target)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.assign target must be an object",
                        1,
                        1,
                    ));
                };
                for source in arguments.iter().skip(1) {
                    if let Value::Object(source) = source {
                        for name in self.enumerable_property_names(*source) {
                            if let Some(value) = self.get_own_property(*source, &name) {
                                self.set_property(*target, &name, value);
                            }
                        }
                    }
                }
                Ok(Value::Object(*target))
            }
            "fromEntries" => {
                let Some(Value::Object(entries)) = arguments.first() else {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.fromEntries requires an iterable",
                        1,
                        1,
                    ));
                };
                if !self.objects[*entries].is_array {
                    return Err(EngineError::new(
                        "TypeError",
                        "Object.fromEntries requires an array in this runtime",
                        1,
                        1,
                    ));
                }
                let result = self.alloc_object(Some(self.object_prototype));
                for index in 0..self.array_length(*entries) {
                    let entry = self
                        .get_property(*entries, &index.to_string())
                        .unwrap_or(Value::Undefined);
                    let Value::Object(pair) = entry else {
                        return Err(EngineError::new(
                            "TypeError",
                            "Object.fromEntries entry must be an object",
                            1,
                            1,
                        ));
                    };
                    let key = self
                        .get_property(pair, "0")
                        .unwrap_or(Value::Undefined)
                        .to_js_string();
                    let value = self.get_property(pair, "1").unwrap_or(Value::Undefined);
                    self.set_property(result, &key, value);
                }
                Ok(Value::Object(result))
            }
            _ => Ok(Value::Undefined),
        }
    }

    fn own_property_names(&self, object: ObjectId) -> Vec<String> {
        let properties = &self.objects[object];
        let mut ordered: Vec<String> = properties
            .property_order
            .iter()
            .filter(|name| properties.properties.contains_key(*name))
            .cloned()
            .collect();
        for name in properties.properties.keys() {
            if !ordered.contains(name) {
                ordered.push(name.clone());
            }
        }
        let mut indices = Vec::new();
        let mut strings = Vec::new();
        for name in ordered {
            if let Some(index) = array_index(&name) {
                indices.push((index, name));
            } else {
                strings.push(name);
            }
        }
        indices.sort_by_key(|(index, _)| *index);
        indices
            .into_iter()
            .map(|(_, name)| name)
            .chain(strings)
            .collect()
    }

    fn enumerable_property_names(&self, object: ObjectId) -> Vec<String> {
        self.own_property_names(object)
            .into_iter()
            .filter(|name| !self.objects[object].non_enumerable.contains(name))
            .collect()
    }

    fn json_stringify_with_options(
        &mut self,
        value: Value,
        arguments: &[Value],
    ) -> Result<Value, EngineError> {
        let replacer = arguments.get(1).cloned();
        let replacer_function = replacer.as_ref().filter(
            |value| matches!(value, Value::Object(id) if self.objects[*id].function.is_some()),
        );
        let property_list = if let Some(Value::Object(id)) = &replacer
            && self.objects[*id].is_array
        {
            let length = self.array_length(*id);
            let mut names = Vec::new();
            for index in 0..length {
                let Some(value) = self.get_property(*id, &index.to_string()) else {
                    continue;
                };
                let name = match value {
                    Value::String(value) => Some(value),
                    Value::Number(value) => Some(Value::Number(value).to_js_string()),
                    _ => None,
                };
                if let Some(name) = name
                    && !names.contains(&name)
                {
                    names.push(name);
                }
            }
            Some(names)
        } else {
            None
        };
        let gap = match arguments.get(2) {
            Some(Value::Number(value)) if value.is_finite() && *value > 0.0 => {
                " ".repeat((*value as usize).min(10))
            }
            Some(Value::String(value)) => value.chars().take(10).collect(),
            _ => String::new(),
        };
        let root = self.alloc_object(Some(self.object_prototype));
        self.set_property(root, "", value.clone());
        let serialized = self.json_stringify_property(
            Value::Object(root),
            "",
            value,
            replacer_function.cloned(),
            property_list.as_deref(),
            &gap,
            "",
            &mut HashSet::new(),
        )?;
        Ok(serialized.map(Value::String).unwrap_or(Value::Undefined))
    }

    fn json_internalize(
        &mut self,
        holder: ObjectId,
        key: &str,
        reviver: &Value,
    ) -> Result<Value, EngineError> {
        let value = self.get_property_with_receiver(holder, Value::Object(holder), key)?;
        if let Value::Object(object) = value.clone() {
            let keys = if self.objects[object].is_array {
                (0..self.array_length(object))
                    .map(|index| index.to_string())
                    .collect()
            } else {
                self.own_property_names(object)
            };
            for child_key in keys {
                let revived = self.json_internalize(object, &child_key, reviver)?;
                if matches!(revived, Value::Undefined) {
                    self.remove_property(object, &child_key);
                } else {
                    self.set_property(object, &child_key, revived);
                }
            }
        }
        self.call(
            reviver.clone(),
            Value::Object(holder),
            vec![Value::String(key.to_owned()), value],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn json_stringify_property(
        &mut self,
        holder: Value,
        key: &str,
        mut value: Value,
        replacer: Option<Value>,
        property_list: Option<&[String]>,
        gap: &str,
        indent: &str,
        ancestors: &mut HashSet<ObjectId>,
    ) -> Result<Option<String>, EngineError> {
        if let Value::Object(id) = value.clone()
            && let Some(to_json) = self.get_property(id, "toJSON")
            && matches!(to_json, Value::Object(function) if self.objects[function].function.is_some())
        {
            value = self.call(
                to_json,
                Value::Object(id),
                vec![Value::String(key.to_owned())],
            )?;
        }
        if let Some(replacer) = replacer.clone() {
            value = self.call(replacer, holder, vec![Value::String(key.to_owned()), value])?;
        }
        match value {
            Value::Undefined => Ok(None),
            Value::Null => Ok(Some("null".to_owned())),
            Value::Boolean(value) => Ok(Some(value.to_string())),
            Value::Number(value) if value.is_finite() && value == 0.0 => Ok(Some("0".to_owned())),
            Value::Number(value) if value.is_finite() => Ok(Some(value.to_string())),
            Value::Number(_) => Ok(Some("null".to_owned())),
            Value::String(value) => Ok(Some(json_quote(&value))),
            Value::Object(id) if self.objects[id].function.is_some() => Ok(None),
            Value::Object(id) => {
                if !ancestors.insert(id) {
                    return Err(EngineError::new(
                        "TypeError",
                        "Converting circular structure to JSON",
                        1,
                        1,
                    ));
                }
                let next_indent = format!("{indent}{gap}");
                if self.objects[id].is_array {
                    let length = self.array_length(id);
                    let mut parts = Vec::with_capacity(length);
                    for index in 0..length {
                        let item_key = index.to_string();
                        let item =
                            self.get_property_with_receiver(id, Value::Object(id), &item_key)?;
                        parts.push(
                            self.json_stringify_property(
                                Value::Object(id),
                                &item_key,
                                item,
                                replacer.clone(),
                                property_list,
                                gap,
                                &next_indent,
                                ancestors,
                            )?
                            .unwrap_or_else(|| "null".to_owned()),
                        );
                    }
                    let serialized = if gap.is_empty() || parts.is_empty() {
                        format!("[{}]", parts.join(","))
                    } else {
                        format!(
                            "[\n{}{}\n{}]",
                            next_indent,
                            parts.join(&format!(",\n{next_indent}")),
                            indent
                        )
                    };
                    ancestors.remove(&id);
                    Ok(Some(serialized))
                } else {
                    let names = property_list
                        .map(<[String]>::to_vec)
                        .unwrap_or_else(|| self.enumerable_property_names(id));
                    let mut parts = Vec::new();
                    for key in names {
                        if !self.objects[id].properties.contains_key(&key) {
                            continue;
                        }
                        let item = self.get_property_with_receiver(id, Value::Object(id), &key)?;
                        if let Some(serialized) = self.json_stringify_property(
                            Value::Object(id),
                            &key,
                            item,
                            replacer.clone(),
                            property_list,
                            gap,
                            &next_indent,
                            ancestors,
                        )? {
                            let separator = if gap.is_empty() { ":" } else { ": " };
                            parts.push(format!("{}{separator}{serialized}", json_quote(&key)));
                        }
                    }
                    let serialized = if gap.is_empty() || parts.is_empty() {
                        format!("{{{}}}", parts.join(","))
                    } else {
                        format!(
                            "{{\n{}{}\n{}}}",
                            next_indent,
                            parts.join(&format!(",\n{next_indent}")),
                            indent
                        )
                    };
                    ancestors.remove(&id);
                    Ok(Some(serialized))
                }
            }
        }
    }

    fn call_math(&mut self, method: &str, arguments: Vec<Value>) -> Result<Value, EngineError> {
        let argument = |index: usize| arguments.get(index);
        let number = |index: usize| argument(index).map(Value::to_number).unwrap_or(f64::NAN);
        let a = number(0);
        let b = number(1);
        let value = match method {
            "abs" => a.abs(),
            "acos" => a.acos(),
            "acosh" => a.acosh(),
            "asin" => a.asin(),
            "asinh" => a.asinh(),
            "atan" => a.atan(),
            "atanh" => a.atanh(),
            "atan2" => a.atan2(b),
            "ceil" => a.ceil(),
            "cos" => a.cos(),
            "cosh" => a.cosh(),
            "floor" => a.floor(),
            "round" => {
                if (-0.5..0.0).contains(&a) {
                    -0.0
                } else {
                    (a + 0.5).floor()
                }
            }
            "trunc" => a.trunc(),
            "sqrt" => a.sqrt(),
            "cbrt" => a.cbrt(),
            "sign" => {
                if a.is_nan() || a == 0.0 {
                    a
                } else {
                    a.signum()
                }
            }
            "exp" => a.exp(),
            "expm1" => a.exp_m1(),
            "log" => a.ln(),
            "log1p" => a.ln_1p(),
            "log2" => a.log2(),
            "log10" => a.log10(),
            "sin" => a.sin(),
            "sinh" => a.sinh(),
            "tan" => a.tan(),
            "tanh" => a.tanh(),
            "pow" => a.powf(b),
            "min" => {
                let numbers: Vec<f64> = arguments.iter().map(Value::to_number).collect();
                if numbers.iter().any(|number| number.is_nan()) {
                    f64::NAN
                } else {
                    numbers.into_iter().fold(f64::INFINITY, f64::min)
                }
            }
            "max" => {
                let numbers: Vec<f64> = arguments.iter().map(Value::to_number).collect();
                if numbers.iter().any(|number| number.is_nan()) {
                    f64::NAN
                } else {
                    numbers.into_iter().fold(f64::NEG_INFINITY, f64::max)
                }
            }
            "imul" => (to_uint32(a) as i32).wrapping_mul(to_uint32(b) as i32) as f64,
            "clz32" => to_uint32(a).leading_zeros() as f64,
            "fround" => (a as f32) as f64,
            "hypot" => {
                let numbers: Vec<f64> = arguments.iter().map(Value::to_number).collect();
                if numbers.iter().any(|number| number.is_infinite()) {
                    f64::INFINITY
                } else if numbers.iter().any(|number| number.is_nan()) {
                    f64::NAN
                } else {
                    numbers.into_iter().fold(0.0_f64, f64::hypot)
                }
            }
            "random" => {
                let mut state = self.random_state;
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                self.random_state = state;
                ((state >> 11) as f64) / ((1_u64 << 53) as f64)
            }
            "isNaN" => return Ok(Value::Boolean(a.is_nan())),
            "isFinite" => return Ok(Value::Boolean(a.is_finite())),
            "isInteger" => {
                return Ok(Value::Boolean(a.is_finite() && a.fract() == 0.0));
            }
            "isSafeInteger" => {
                return Ok(Value::Boolean(
                    a.is_finite() && a.fract() == 0.0 && a.abs() <= 9_007_199_254_740_991.0,
                ));
            }
            "parseInt" => {
                let input = arguments
                    .first()
                    .map(Value::to_js_string)
                    .unwrap_or_else(|| "undefined".to_owned());
                let input = input.trim();
                let requested_radix = arguments.get(1).map(Value::to_number).unwrap_or(0.0);
                if !requested_radix.is_finite() || requested_radix.fract() != 0.0 {
                    return Ok(Value::Number(f64::NAN));
                }
                let mut radix = requested_radix as u32;
                if requested_radix != 0.0 && !(2..=36).contains(&radix) {
                    return Ok(Value::Number(f64::NAN));
                }
                let sign = if input.starts_with('-') { -1.0 } else { 1.0 };
                let unsigned = input.trim_start_matches(['-', '+']);
                if radix == 0 {
                    radix = if unsigned.starts_with("0x") || unsigned.starts_with("0X") {
                        16
                    } else {
                        10
                    };
                }
                let digits =
                    if radix == 16 && (unsigned.starts_with("0x") || unsigned.starts_with("0X")) {
                        &unsigned[2..]
                    } else {
                        unsigned
                    };
                let valid: String = digits
                    .chars()
                    .take_while(|character| character.is_digit(radix))
                    .collect();
                return Ok(Value::Number(
                    i64::from_str_radix(&valid, radix)
                        .map(|value| sign * value as f64)
                        .unwrap_or(f64::NAN),
                ));
            }
            "parseFloat" => {
                let input = arguments
                    .first()
                    .map(Value::to_js_string)
                    .unwrap_or_else(|| "undefined".to_owned());
                let trimmed = input.trim_start();
                for end in (1..=trimmed.len()).rev() {
                    if trimmed.is_char_boundary(end)
                        && let Ok(parsed) = trimmed[..end].parse::<f64>()
                    {
                        return Ok(Value::Number(parsed));
                    }
                }
                return Ok(Value::Number(f64::NAN));
            }
            _ => return Ok(Value::Undefined),
        };
        Ok(Value::Number(value))
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
                        Value::String(name) if name == "RangeError" => "RangeError",
                        Value::String(name) if name == "EvalError" => "EvalError",
                        Value::String(name) if name == "URIError" => "URIError",
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

fn array_index(name: &str) -> Option<u32> {
    let index = name.parse::<u32>().ok()?;
    (index != u32::MAX && index.to_string() == name).then_some(index)
}

fn normalize_index(value: f64, length: usize) -> usize {
    if value.is_nan() || value <= 0.0 {
        return 0;
    }
    if value.is_infinite() || value >= length as f64 {
        return length;
    }
    value.trunc() as usize
}

fn normalize_slice_index(value: f64, length: usize) -> usize {
    if value.is_nan() {
        return 0;
    }
    if value == f64::NEG_INFINITY {
        return 0;
    }
    if value == f64::INFINITY {
        return length;
    }
    let index = value.trunc() as isize;
    if index < 0 {
        (length as isize + index).max(0) as usize
    } else {
        (index as usize).min(length)
    }
}

fn normalize_relative_start(value: f64, length: usize) -> usize {
    if value.is_nan() || value == f64::NEG_INFINITY {
        return 0;
    }
    if value == f64::INFINITY {
        return length;
    }
    let index = value.trunc() as isize;
    if index < 0 {
        (length as isize + index).max(0) as usize
    } else {
        (index as usize).min(length)
    }
}

fn normalize_relative_index(value: f64, length: usize) -> Option<usize> {
    if value.is_nan() || value == f64::INFINITY || value == f64::NEG_INFINITY {
        return None;
    }
    let index = value.trunc() as isize;
    let length = length as isize;
    let index = if index < 0 { length + index } else { index };
    (index >= 0 && index < length).then_some(index as usize)
}

fn same_value_zero(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            left == right || (left.is_nan() && right.is_nan())
        }
        _ => left == right,
    }
}

fn same_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            (left.is_nan() && right.is_nan()) || left.to_bits() == right.to_bits()
        }
        _ => left == right,
    }
}

#[derive(Clone, Copy)]
struct DateParts {
    year: i64,
    month: u32,
    day: u32,
    weekday: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millisecond: u32,
}

fn current_time_value() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as f64)
        .unwrap_or(0.0)
}

fn build_regexp(pattern: String, flags: String) -> Result<RegExpData, EngineError> {
    let mut seen = HashSet::new();
    for flag in flags.chars() {
        if !matches!(flag, 'd' | 'g' | 'i' | 'm' | 's' | 'u' | 'v' | 'y') || !seen.insert(flag) {
            return Err(EngineError::new(
                "SyntaxError",
                format!("Invalid regular expression flag '{flag}'"),
                1,
                1,
            ));
        }
    }
    if seen.contains(&'u') && seen.contains(&'v') {
        return Err(EngineError::new(
            "SyntaxError",
            "The 'u' and 'v' flags cannot be used together",
            1,
            1,
        ));
    }
    let canonical_flags: String = "dgimsuvy"
        .chars()
        .filter(|flag| seen.contains(flag))
        .collect();
    let source = if pattern.is_empty() {
        "(?:)".to_owned()
    } else {
        pattern
    };
    let matcher_flags = regress::Flags {
        icase: seen.contains(&'i'),
        multiline: seen.contains(&'m'),
        dot_all: seen.contains(&'s'),
        unicode: seen.contains(&'u'),
        unicode_sets: seen.contains(&'v'),
        ..regress::Flags::default()
    };
    let matcher = regress::Regex::with_flags(&source, matcher_flags).map_err(|error| {
        EngineError::new(
            "SyntaxError",
            format!("Invalid regular expression: {error}"),
            1,
            1,
        )
    })?;
    Ok(RegExpData {
        matcher,
        source,
        flags: canonical_flags,
        global: seen.contains(&'g'),
        has_indices: seen.contains(&'d'),
        ignore_case: seen.contains(&'i'),
        multiline: seen.contains(&'m'),
        dot_all: seen.contains(&'s'),
        unicode: seen.contains(&'u'),
        unicode_sets: seen.contains(&'v'),
        sticky: seen.contains(&'y'),
    })
}

fn regexp_source(pattern: &str) -> String {
    let mut source = String::with_capacity(pattern.len());
    let mut backslashes = 0;
    for character in pattern.chars() {
        match character {
            '/' if backslashes % 2 == 0 => source.push_str("\\/"),
            '\n' => source.push_str("\\n"),
            '\r' => source.push_str("\\r"),
            '\u{2028}' => source.push_str("\\u2028"),
            '\u{2029}' => source.push_str("\\u2029"),
            _ => source.push(character),
        }
        if character == '\\' {
            backslashes += 1;
        } else {
            backslashes = 0;
        }
    }
    source
}

fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8.64e15 {
        f64::NAN
    } else {
        time.trunc()
    }
}

fn days_from_civil(year: i128, month: i128, day: i128) -> i128 {
    let year = year - i128::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn date_parts(time: f64) -> DateParts {
    let milliseconds = time as i64;
    let days = milliseconds.div_euclid(86_400_000);
    let time_of_day = milliseconds.rem_euclid(86_400_000) as u32;
    let (year, month, day) = civil_from_days(days);
    DateParts {
        year,
        month,
        day,
        weekday: (days + 4).rem_euclid(7) as u32,
        hour: time_of_day / 3_600_000,
        minute: (time_of_day / 60_000) % 60,
        second: (time_of_day / 1_000) % 60,
        millisecond: time_of_day % 1_000,
    }
}

fn make_date_value(components: [i64; 7]) -> f64 {
    let mut components = components.map(i128::from);
    let month_adjustment = components[1].div_euclid(12);
    components[0] += month_adjustment;
    let month = components[1].rem_euclid(12) + 1;
    let days = days_from_civil(components[0], month, components[2]);
    let time = days as f64 * 86_400_000.0
        + components[3] as f64 * 3_600_000.0
        + components[4] as f64 * 60_000.0
        + components[5] as f64 * 1_000.0
        + components[6] as f64;
    time_clip(time)
}

fn date_from_components(arguments: &[Value]) -> f64 {
    let mut components = [0i64, 0, 1, 0, 0, 0, 0];
    for (index, value) in arguments.iter().take(7).enumerate() {
        let number = value.to_number();
        if !number.is_finite() {
            return f64::NAN;
        }
        components[index] = number.trunc() as i64;
    }
    let year = components[0];
    components[0] = if (0..=99).contains(&year) {
        year + 1900
    } else {
        year
    };
    make_date_value(components)
}

fn parse_fixed_digits(input: &str, start: usize, length: usize) -> Option<i64> {
    let text = input.get(start..start + length)?;
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_date_string(input: &str) -> Option<i64> {
    let bytes = input.as_bytes();
    if bytes.len() < 10 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let year = parse_fixed_digits(input, 0, 4)?;
    let month = parse_fixed_digits(input, 5, 2)?;
    let day = parse_fixed_digits(input, 8, 2)?;
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    let mut components = [year, month - 1, day, 0, 0, 0, 0];
    if bytes.len() > 10 {
        if !matches!(bytes.get(10), Some(b'T' | b't' | b' ')) {
            return None;
        }
        components[3] = parse_fixed_digits(input, 11, 2)?;
        if bytes.get(13) != Some(&b':') {
            return None;
        }
        components[4] = parse_fixed_digits(input, 14, 2)?;
        if bytes.get(16) != Some(&b':') {
            return None;
        }
        components[5] = parse_fixed_digits(input, 17, 2)?;
        if components[3] > 24
            || components[4] > 59
            || components[5] > 59
            || (components[3] == 24 && (components[4] != 0 || components[5] != 0))
        {
            return None;
        }
        let mut offset = 19;
        if bytes.get(offset) == Some(&b'.') {
            offset += 1;
            let start = offset;
            while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
                offset += 1;
            }
            if start == offset {
                return None;
            }
            let fraction = input.get(start..offset)?;
            let digits = fraction.chars().take(3).collect::<String>();
            let millis = digits.parse::<i64>().ok()?;
            components[6] = millis * 10_i64.pow(3_u32.saturating_sub(digits.len() as u32));
        }
        let timezone_minutes = match bytes.get(offset) {
            Some(b'Z' | b'z') if offset + 1 == bytes.len() => 0,
            Some(sign @ (b'+' | b'-')) => {
                let hours = parse_fixed_digits(input, offset + 1, 2)?;
                if bytes.get(offset + 3) != Some(&b':') {
                    return None;
                }
                let minutes = parse_fixed_digits(input, offset + 4, 2)?;
                if hours > 23 || minutes > 59 || offset + 6 != bytes.len() {
                    return None;
                }
                let amount = hours * 60 + minutes;
                if *sign == b'+' { amount } else { -amount }
            }
            None if offset == bytes.len() => 0,
            _ => return None,
        };
        let time = time_clip(make_date_value(components) - timezone_minutes as f64 * 60_000.0);
        return time.is_finite().then_some(time as i64);
    }
    let time = make_date_value(components);
    time.is_finite().then_some(time as i64)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year.rem_euclid(4) == 0
            && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0) =>
        {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn format_iso_date(time: f64) -> String {
    let parts = date_parts(time);
    let year = if (0..=9999).contains(&parts.year) {
        format!("{:04}", parts.year)
    } else {
        format!("{:+07}", parts.year)
    };
    format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        parts.month, parts.day, parts.hour, parts.minute, parts.second, parts.millisecond
    )
}

fn format_date_string(time: f64) -> String {
    if time.is_finite() {
        format!("{} {}", format_date_only(time), format_time_only(time))
    } else {
        "Invalid Date".to_owned()
    }
}

fn format_date_only(time: f64) -> String {
    if !time.is_finite() {
        return "Invalid Date".to_owned();
    }
    let parts = date_parts(time);
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {} {:02} {}",
        WEEKDAYS[parts.weekday as usize],
        MONTHS[parts.month as usize - 1],
        parts.day,
        parts.year
    )
}

fn format_time_only(time: f64) -> String {
    if !time.is_finite() {
        return "Invalid Date".to_owned();
    }
    let parts = date_parts(time);
    format!(
        "{:02}:{:02}:{:02} GMT+0000 (UTC)",
        parts.hour, parts.minute, parts.second
    )
}

fn format_utc_string(time: f64) -> String {
    if !time.is_finite() {
        return "Invalid Date".to_owned();
    }
    let parts = date_parts(time);
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[parts.weekday as usize],
        parts.day,
        MONTHS[parts.month as usize - 1],
        parts.year,
        parts.hour,
        parts.minute,
        parts.second
    )
}

struct JsonParser<'a> {
    input: &'a str,
    offset: usize,
}

fn json_quote(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{001f}' => {
                use std::fmt::Write;
                write!(&mut output, "\\u{:04x}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn format_radix(value: f64, radix: u32) -> String {
    if value.is_nan() || value.is_infinite() || value == 0.0 {
        return Value::Number(value).to_js_string();
    }
    let negative = value.is_sign_negative();
    let absolute = value.abs();
    if absolute >= u64::MAX as f64 {
        return Value::Number(value).to_js_string();
    }
    let mut integer = absolute.trunc() as u64;
    let mut digits = Vec::new();
    while integer > 0 {
        let digit = (integer % u64::from(radix)) as u8;
        digits.push(if digit < 10 {
            (b'0' + digit) as char
        } else {
            (b'a' + digit - 10) as char
        });
        integer /= u64::from(radix);
    }
    digits.reverse();
    let mut result: String = digits.into_iter().collect();
    let mut fraction = absolute.fract();
    if fraction != 0.0 {
        result.push('.');
        for _ in 0..16 {
            fraction *= f64::from(radix);
            let digit = fraction.floor() as u8;
            result.push(if digit < 10 {
                (b'0' + digit) as char
            } else {
                (b'a' + digit - 10) as char
            });
            fraction -= f64::from(digit);
            if fraction == 0.0 {
                break;
            }
        }
    }
    if negative {
        result.insert(0, '-');
    }
    result
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }

    fn parse_value(&mut self, engine: &mut Engine) -> Result<Value, EngineError> {
        let value = self.value(engine)?;
        self.whitespace();
        if self.offset != self.input.len() {
            return Err(self.syntax_error());
        }
        Ok(value)
    }

    fn value(&mut self, engine: &mut Engine) -> Result<Value, EngineError> {
        self.whitespace();
        match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Value::Null)
            }
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Value::Boolean(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Value::Boolean(false))
            }
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => {
                self.offset += 1;
                let array = engine.alloc_array();
                self.whitespace();
                if self.consume(b']') {
                    return Ok(Value::Object(array));
                }
                let mut index = 0;
                loop {
                    let item = self.value(engine)?;
                    engine.set_property(array, &index.to_string(), item);
                    index += 1;
                    self.whitespace();
                    if self.consume(b']') {
                        break;
                    }
                    if !self.consume(b',') {
                        return Err(self.syntax_error());
                    }
                }
                Ok(Value::Object(array))
            }
            Some(b'{') => {
                self.offset += 1;
                let object = engine.alloc_object(Some(engine.object_prototype));
                self.whitespace();
                if self.consume(b'}') {
                    return Ok(Value::Object(object));
                }
                loop {
                    self.whitespace();
                    if self.peek() != Some(b'"') {
                        return Err(self.syntax_error());
                    }
                    let key = self.string()?;
                    self.whitespace();
                    if !self.consume(b':') {
                        return Err(self.syntax_error());
                    }
                    let value = self.value(engine)?;
                    engine.set_property(object, &key, value);
                    self.whitespace();
                    if self.consume(b'}') {
                        break;
                    }
                    if !self.consume(b',') {
                        return Err(self.syntax_error());
                    }
                }
                Ok(Value::Object(object))
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.syntax_error()),
        }
    }

    fn string(&mut self) -> Result<String, EngineError> {
        if !self.consume(b'"') {
            return Err(self.syntax_error());
        }
        let mut units = Vec::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.syntax_error());
            };
            match byte {
                b'"' => {
                    self.offset += 1;
                    return Ok(std::char::decode_utf16(units)
                        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
                        .collect());
                }
                b'\\' => {
                    self.offset += 1;
                    let Some(escape) = self.peek() else {
                        return Err(self.syntax_error());
                    };
                    self.offset += 1;
                    match escape {
                        b'"' => units.push(b'"' as u16),
                        b'\\' => units.push(b'\\' as u16),
                        b'/' => units.push(b'/' as u16),
                        b'b' => units.push(0x0008),
                        b'f' => units.push(0x000c),
                        b'n' => units.push(b'\n' as u16),
                        b'r' => units.push(b'\r' as u16),
                        b't' => units.push(b'\t' as u16),
                        b'u' => {
                            let end = self.offset + 4;
                            let digits = self
                                .input
                                .get(self.offset..end)
                                .ok_or_else(|| self.syntax_error())?;
                            let unit =
                                u16::from_str_radix(digits, 16).map_err(|_| self.syntax_error())?;
                            units.push(unit);
                            self.offset = end;
                        }
                        _ => return Err(self.syntax_error()),
                    }
                }
                0..=0x1f => return Err(self.syntax_error()),
                _ => {
                    let character = self.input[self.offset..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.syntax_error())?;
                    let mut encoded = [0; 2];
                    units.extend(character.encode_utf16(&mut encoded).iter().copied());
                    self.offset += character.len_utf8();
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, EngineError> {
        let start = self.offset;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.syntax_error());
                }
            }
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.offset += 1;
                }
            }
            _ => return Err(self.syntax_error()),
        }
        if self.consume(b'.') {
            let fraction_start = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.offset += 1;
            }
            if fraction_start == self.offset {
                return Err(self.syntax_error());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            let exponent_start = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.offset += 1;
            }
            if exponent_start == self.offset {
                return Err(self.syntax_error());
            }
        }
        self.input[start..self.offset]
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| self.syntax_error())
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), EngineError> {
        if self.input.as_bytes()[self.offset..].starts_with(literal) {
            self.offset += literal.len();
            Ok(())
        } else {
            Err(self.syntax_error())
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.offset += 1;
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.offset).copied()
    }

    fn syntax_error(&self) -> EngineError {
        EngineError::new("SyntaxError", "Invalid JSON text", 1, 1)
    }
}

fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    value.trunc().rem_euclid(4_294_967_296.0) as u32
}

impl vm::Host for Engine {
    fn load_binding(
        &mut self,
        scope: usize,
        name: &str,
        line: usize,
        column: usize,
        cache: &mut vm::InlineCache,
    ) -> Result<Value, EngineError> {
        if let Some(binding_scope) = cache.binding_for(scope) {
            if let Some(binding_scope) = binding_scope
                && let Some(binding) = self.scopes[binding_scope].bindings.get(name)
            {
                if !binding.initialized {
                    return Err(EngineError::new(
                        "ReferenceError",
                        format!("Cannot access '{name}' before initialization"),
                        line,
                        column,
                    ));
                }
                return Ok(binding.value.clone());
            }
            if binding_scope.is_none() {
                return self.get_binding(scope, name, line, column)?.ok_or_else(|| {
                    EngineError::new(
                        "ReferenceError",
                        format!("{name} is not defined"),
                        line,
                        column,
                    )
                });
            }
        }

        let binding_scope = self.find_binding_scope(scope, name);
        cache.cache_binding(scope, binding_scope);
        self.get_binding(scope, name, line, column)?.ok_or_else(|| {
            EngineError::new(
                "ReferenceError",
                format!("{name} is not defined"),
                line,
                column,
            )
        })
    }

    fn unary(&mut self, operator: UnaryOp, value: Value) -> Result<Value, EngineError> {
        match operator {
            UnaryOp::Not => Ok(Value::Boolean(!value.is_truthy())),
            UnaryOp::Plus => Ok(Value::Number(value.to_number())),
            UnaryOp::Minus => Ok(Value::Number(-value.to_number())),
            UnaryOp::BitNot => Ok(Value::Number((!to_uint32(value.to_number()) as i32) as f64)),
            UnaryOp::TypeOf => Ok(Value::String(
                match value {
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
                "InternalError",
                "Delete expressions are not supported by bytecode",
                1,
                1,
            )),
        }
    }

    fn binary(
        &mut self,
        operator: BinaryOp,
        left: Value,
        right: Value,
        cache: &mut vm::InlineCache,
    ) -> Result<Value, EngineError> {
        if cache.record_binary(&left, &right)
            && let (Value::Number(left), Value::Number(right)) = (&left, &right)
        {
            let result = match operator {
                BinaryOp::Add => Some(left + right),
                BinaryOp::Subtract => Some(left - right),
                BinaryOp::Multiply => Some(left * right),
                BinaryOp::Divide => Some(left / right),
                BinaryOp::Remainder => Some(left % right),
                BinaryOp::Exponent => Some(left.powf(*right)),
                _ => None,
            };
            if let Some(result) = result {
                return Ok(Value::Number(result));
            }
        }
        self.evaluate_binary(operator, left, right)
    }

    fn check_binding_assignment(
        &self,
        scope: usize,
        name: &str,
        line: usize,
        column: usize,
    ) -> Result<(), EngineError> {
        let Some(binding_scope) = self.find_binding_scope(scope, name) else {
            if self.strict {
                return Err(EngineError::new(
                    "ReferenceError",
                    format!("{name} is not defined"),
                    line,
                    column,
                ));
            }
            return Ok(());
        };
        let binding = &self.scopes[binding_scope].bindings[name];
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
        Ok(())
    }

    fn store_binding(
        &mut self,
        scope: usize,
        name: &str,
        value: Value,
        line: usize,
        column: usize,
    ) -> Result<Value, EngineError> {
        self.assign_binding(scope, name, value, line, column)
    }

    fn initialize_declaration(
        &mut self,
        scope: usize,
        name: &str,
        value: Value,
        kind: DeclarationKind,
    ) -> Result<(), EngineError> {
        let scope = if kind == DeclarationKind::Var {
            self.var_scope(scope)
        } else {
            scope
        };
        if !self.scopes[scope].bindings.contains_key(name) {
            return Err(EngineError::new(
                "InternalError",
                "Bytecode declaration was not instantiated",
                1,
                1,
            ));
        }
        self.initialize_binding(scope, name, value.clone());
        if kind == DeclarationKind::Var && scope == self.global {
            self.set_property(self.global_object, name, value);
        }
        Ok(())
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
            Stmt::ForEach {
                name,
                kind: DeclarationKind::Var,
                body,
                ..
            } => {
                names.push(name.clone());
                collect_var_names(std::slice::from_ref(body), names);
            }
            Stmt::ForEach { body, .. } => {
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
    fn garbage_collector_reclaims_unreachable_objects_and_preserves_closures() {
        let mut engine = Engine::new();
        engine.eval("0").unwrap();
        engine
            .eval("let gcGarbage = {}; gcGarbage = null; 0")
            .unwrap();
        assert!(engine.collect_garbage() > 0);

        engine.eval("let gcKept = { value: 41 }").unwrap();
        assert_eq!(engine.eval("gcKept.value").unwrap(), Value::Number(41.0));
        assert_eq!(engine.eval("gcKept.value").unwrap(), Value::Number(41.0));
        engine
            .eval("function gcRead() { return gcKept.value; }")
            .unwrap();
        assert_eq!(engine.eval("gcRead()").unwrap(), Value::Number(41.0));
        engine.collect_garbage();
        assert_eq!(engine.eval("gcRead()").unwrap(), Value::Number(41.0));
    }

    #[test]
    fn garbage_collector_clears_dead_weak_collection_entries() {
        let mut engine = Engine::new();
        engine
            .eval(
                "let gcWeakKey = {}; let gcWeak = new WeakMap(); gcWeak.set(gcWeakKey, 1); gcWeakKey = null;",
            )
            .unwrap();
        engine.collect_garbage();
        assert_eq!(
            engine.eval("gcWeak.has(gcWeakKey)").unwrap(),
            Value::Boolean(false)
        );
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

    #[test]
    fn arrays_support_sparse_elements_and_common_mutation_methods() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string("let items = [1,,3]; items.push(4); items.pop(); items.join('-')",)
                .unwrap(),
            "1--3"
        );
        assert_eq!(
            engine
                .eval_to_string("items.length + ':' + (1 in items) + ':' + Array.isArray(items)",)
                .unwrap(),
            "3:false:true"
        );
        assert_eq!(
            engine.eval_to_string("items.slice(-2).join(',')").unwrap(),
            ",3"
        );
        assert_eq!(
            engine
                .eval_to_string("Array.of(2, 4).reverse().join(',')")
                .unwrap(),
            "4,2"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let removed = items.splice(1, 1, 8); items.toString() + ':' + removed.toString()",
                )
                .unwrap(),
            "1,8,3:"
        );
        assert_eq!(engine.eval_to_string("Array(3).join('-')").unwrap(), "--");
        assert_eq!(
            engine
                .eval_to_string("let nested = []; nested.push(nested); nested.join(',') + ':' + nested.toString()")
                .unwrap(),
            ":"
        );
        assert_eq!(
            engine
                .eval_to_string("[3,1,2].sort().copyWithin(1, 0, 2).join(',')",)
                .unwrap(),
            "1,1,2"
        );
        assert_eq!(
            engine
                .eval("let invalidArray = []; invalidArray.length = -1")
                .unwrap_err()
                .name,
            "RangeError"
        );
    }

    #[test]
    fn array_callback_and_conversion_methods_work() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval_to_string(
                "[1,2,3,4].map(function(value) { return value * 2; }).filter(function(value) { return value > 4; }).reduce(function(total, value) { return total + value; }, 0)",
            ).unwrap(),
            "14"
        );
        assert_eq!(
            engine
                .eval_to_string("Array.from({ 0: 'a', 1: 'b', length: 2 }).join('-')",)
                .unwrap(),
            "a-b"
        );
        assert_eq!(
            engine.eval_to_string("[1,,3].includes(undefined)").unwrap(),
            "true"
        );
        assert_eq!(
            engine.eval_to_string("[1,,3].indexOf(undefined)").unwrap(),
            "-1"
        );
    }

    #[test]
    fn array_flatten_methods_handle_sparse_arrays() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string("[1,,[2,,[3]]].flat(2).join(',')")
                .unwrap(),
            "1,2,3"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "[1,2,3].flatMap(function(value) { return [value, value * 2]; }).join(',')"
                )
                .unwrap(),
            "1,2,2,4,3,6"
        );
    }

    #[test]
    fn array_key_value_and_entry_iterators_return_iterator_results() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let values = [4,,6]; let iterator = values.values(); let first = iterator.next(); values[1] = 5; let second = iterator.next(); first.value + second.value + ':' + second.done + ':' + iterator.next().value",
                )
                .unwrap(),
            "9:false:6"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let keys = [7,8].keys(); let entries = [7,8].entries(); keys.next().value + ':' + entries.next().value.join('-') + ':' + entries.next().done",
                )
                .unwrap(),
            "0:0-7:false"
        );
        assert_eq!(
            engine
                .eval_to_string("let doneIterator = [1].values(); doneIterator.next().done + ':' + doneIterator.next().done + ':' + doneIterator.next().value")
                .unwrap(),
            "false:true:undefined"
        );
        assert_eq!(
            engine
                .eval_to_string("Array.from([3,4]).join(',')")
                .unwrap(),
            "3,4"
        );
    }

    #[test]
    fn maps_and_sets_preserve_insertion_order_and_same_value_zero_keys() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let map = new Map([['first', 1], ['second', 2], ['first', 3]]); map.set(NaN, 4); map.set(NaN, 5); map.get('first') + ':' + map.get(NaN) + ':' + map.size + ':' + map.has('second')",
                )
                .unwrap(),
            "3:5:3:true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let identityMap = new Map(); let key = {}; identityMap.set(key, 7); identityMap.has({}) + ':' + identityMap.get(key) + ':' + identityMap.delete(key) + ':' + identityMap.size",
                )
                .unwrap(),
            "false:7:true:0"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let collectionSet = new Set([1, 1, NaN]); collectionSet.add(NaN).add(2); collectionSet.size + ':' + collectionSet.has(NaN) + ':' + collectionSet.delete(1) + ':' + collectionSet.size",
                )
                .unwrap(),
            "3:true:true:2"
        );
        assert_eq!(
            engine
                .eval_to_string("let clearedSet = new Set(); clearedSet.add(1); clearedSet.clear(); clearedSet.size")
                .unwrap(),
            "0"
        );
        assert_eq!(engine.eval("new Map([1])").unwrap_err().name, "TypeError");
        assert_eq!(engine.eval("new Map({})").unwrap_err().name, "TypeError");
        assert_eq!(engine.eval("new Set({})").unwrap_err().name, "TypeError");
    }

    #[test]
    fn weak_collections_enforce_object_keys_and_support_basic_operations() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let weakMapKey = {}; let weakMap = new WeakMap([[weakMapKey, 3]]); weakMap.set(weakMapKey, 4); weakMap.get(weakMapKey) + ':' + weakMap.has(weakMapKey) + ':' + weakMap.has(1) + ':' + weakMap.get(1) + ':' + weakMap.delete(weakMapKey) + ':' + weakMap.has(weakMapKey)",
                )
                .unwrap(),
            "4:true:false:undefined:true:false"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let weakSetKey = {}; let weakSet = new WeakSet([weakSetKey, weakSetKey]); weakSet.add(weakSetKey) === weakSet && weakSet.has(weakSetKey) && !weakSet.has(1) && weakSet.delete(weakSetKey) && !weakSet.has(weakSetKey)",
                )
                .unwrap(),
            "true"
        );
        assert_eq!(
            engine.eval("new WeakMap([[1, 2]])").unwrap_err().name,
            "TypeError"
        );
        assert_eq!(
            engine.eval("new WeakSet([1])").unwrap_err().name,
            "TypeError"
        );
        assert_eq!(
            engine.eval("new WeakMap().set(1, 2)").unwrap_err().name,
            "TypeError"
        );
        assert_eq!(
            engine.eval("new WeakSet().add(1)").unwrap_err().name,
            "TypeError"
        );
    }

    #[test]
    fn date_supports_time_values_iso_dates_and_utc_components() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let epochDate = new Date(0); epochDate.toISOString() + ':' + epochDate.getUTCFullYear() + ':' + epochDate.getUTCMonth() + ':' + epochDate.getUTCDay()",
                )
                .unwrap(),
            "1970-01-01T00:00:00.000Z:1970:0:4"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let parsedDate = new Date('2020-02-29T12:34:56.789Z'); parsedDate.toISOString() + ':' + parsedDate.getUTCDate() + ':' + parsedDate.getUTCHours() + ':' + parsedDate.getUTCMilliseconds()",
                )
                .unwrap(),
            "2020-02-29T12:34:56.789Z:29:12:789"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "Date.parse('1970-01-01T01:00:00+01:00') + ':' + Date.UTC(2000, 0, 1) + ':' + Date.UTC(99, 0, 1)",
                )
                .unwrap(),
            "0:946684800000:915148800000"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let mutableDate = new Date(1.9); mutableDate.setTime(42.9) + ':' + mutableDate.valueOf() + ':' + mutableDate.toJSON()",
                )
                .unwrap(),
            "42:42:1970-01-01T00:00:00.042Z"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let componentDate = new Date(2024, 1, 29, 12, 30, 45, 6); componentDate.toISOString() + ':' + componentDate.getFullYear() + ':' + componentDate.getMonth() + ':' + componentDate.getYear()",
                )
                .unwrap(),
            "2024-02-29T12:30:45.006Z:2024:1:124"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "componentDate.setUTCMonth(12, 1); componentDate.toISOString() + ':' + componentDate.setUTCHours(25)",
                )
                .unwrap(),
            "2025-01-01T12:30:45.006Z:1735781445006"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let invalidDate = new Date(NaN); invalidDate.setUTCFullYear(2000); invalidDate.toISOString()",
                )
                .unwrap(),
            "2000-01-01T00:00:00.000Z"
        );
        assert_eq!(
            engine
                .eval_to_string("new Date(2023, 0, 1, 0, 0, 0, 1000).toISOString()")
                .unwrap(),
            "2023-01-01T00:00:01.000Z"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let formatDate = new Date(0); formatDate.toString() + '|' + formatDate.toDateString() + '|' + formatDate.toTimeString() + '|' + formatDate.toUTCString() + '|' + formatDate.getTimezoneOffset()",
                )
                .unwrap(),
            "Thu Jan 01 1970 00:00:00 GMT+0000 (UTC)|Thu Jan 01 1970|00:00:00 GMT+0000 (UTC)|Thu, 01 Jan 1970 00:00:00 GMT|0"
        );
        assert_eq!(
            engine.eval("isNaN(Date.parse('2021-02-29'))").unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine.eval("new Date(NaN).toISOString()").unwrap_err().name,
            "RangeError"
        );
        assert_eq!(
            engine
                .eval("Date.prototype.getTime.call({})")
                .unwrap_err()
                .name,
            "TypeError"
        );
        assert!(engine.eval("Date.now() >= 0").unwrap().is_truthy());
    }

    #[test]
    fn regular_expressions_support_matching_captures_flags_and_global_state() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let matcher = new RegExp('(ab)(c)?', 'ig'); let capture = matcher.exec('xxAB'); capture[0] + ':' + capture[1] + ':' + capture[2] + ':' + capture.index + ':' + matcher.lastIndex + ':' + matcher.flags + ':' + matcher.toString()",
                )
                .unwrap(),
            "AB:AB:undefined:2:4:gi:/(ab)(c)?/gi"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "matcher.lastIndex = 0; matcher.test('xxab') + ':' + matcher.lastIndex + ':' + matcher.test('no') + ':' + matcher.lastIndex",
                )
                .unwrap(),
            "true:4:false:0"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let stickyMatcher = new RegExp('a+', 'y'); stickyMatcher.lastIndex = 1; stickyMatcher.exec('baa')[0] + ':' + stickyMatcher.lastIndex + ':' + stickyMatcher.test('baa')",
                )
                .unwrap(),
            "aa:3:false"
        );
        assert_eq!(
            engine
                .eval_to_string("new RegExp('x', 'mi').flags + ':' + new RegExp('').source")
                .unwrap(),
            "im:(?:)"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let originalPattern = new RegExp('a/b', 'i'); let clonedPattern = new RegExp(originalPattern, 'gi'); clonedPattern.source + ':' + clonedPattern.flags + ':' + clonedPattern.test('A/B')",
                )
                .unwrap(),
            "a\\/b:gi:true"
        );
        assert_eq!(
            engine.eval("new RegExp('x', 'gg')").unwrap_err().name,
            "SyntaxError"
        );
        assert_eq!(
            engine.eval("new RegExp('(')").unwrap_err().name,
            "SyntaxError"
        );
        assert_eq!(
            engine
                .eval("RegExp.prototype.exec.call({}, 'x')")
                .unwrap_err()
                .name,
            "TypeError"
        );
    }

    #[test]
    fn regular_expressions_cover_ecmascript_groups_lookarounds_unicode_and_indices() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let namedMatch = new RegExp('(?<letters>[A-Z]+)(?=!)', 'd').exec('xxABC!'); namedMatch.groups.letters + ':' + namedMatch.indices[0].join('-') + ':' + namedMatch.indices.groups.letters.join('-')",
                )
                .unwrap(),
            "ABC:2-5:2-5"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let optionalMatch = new RegExp('(?<left>a)?(b)', 'd').exec('b'); optionalMatch[1] + ':' + optionalMatch.groups.left + ':' + optionalMatch.indices[1] + ':' + optionalMatch.indices.groups.left",
                )
                .unwrap(),
            "undefined:undefined:undefined:undefined"
        );
        assert_eq!(
            engine
                .eval_to_string("new RegExp('(?<=#)([a-z]+)-\\\\1').exec('#cat-cat')[0]",)
                .unwrap(),
            "cat-cat"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "new RegExp('\\\\p{Script=Greek}+', 'u').test('αβ') + ':' + new RegExp('[[A-Z]&&[B-D]]+', 'v').test('BCDF')",
                )
                .unwrap(),
            "true:true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let unicodeSticky = new RegExp('.', 'uy'); let unicodeResult = unicodeSticky.exec('😀'); unicodeResult[0] + ':' + unicodeSticky.lastIndex",
                )
                .unwrap(),
            "😀:2"
        );
        assert_eq!(
            engine
                .eval_to_string("let legacySticky = new RegExp('.', 'y'); legacySticky.exec('😀'); legacySticky.lastIndex")
                .unwrap(),
            "1"
        );
        assert_eq!(
            engine.eval("new RegExp('a', 'uv')").unwrap_err().name,
            "SyntaxError"
        );
        assert_eq!(
            engine.eval("RegExp('x') === RegExp('x')").unwrap(),
            Value::Boolean(false)
        );
        assert_eq!(
            engine
                .eval("let reusableRegExp = new RegExp('x'); RegExp(reusableRegExp) === reusableRegExp")
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let negativeLastIndex = new RegExp('a', 'g'); negativeLastIndex.lastIndex = -1; negativeLastIndex.exec('a')[0] + ':' + negativeLastIndex.lastIndex",
                )
                .unwrap(),
            "a:1"
        );
    }

    #[test]
    fn map_and_set_iterators_work_with_array_from_and_for_of() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let iterMap = new Map([['a', 1], ['b', 2]]); let iterSet = new Set([3, 4]); let mapEntryIterator = iterMap.entries(); let setEntryIterator = iterSet.entries(); let firstMapEntry = mapEntryIterator.next().value; let firstSetEntry = setEntryIterator.next().value; firstMapEntry[0] + ':' + firstMapEntry[1] + ':' + firstSetEntry[0] + ':' + firstSetEntry[1]",
                )
                .unwrap(),
            "a:1:3:3"
        );
        assert_eq!(
            engine
                .eval_to_string("Array.from(iterMap).length + ':' + Array.from(iterSet).join(',')",)
                .unwrap(),
            "2:3,4"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let mapLoopResult = ''; for (let pair of iterMap) { mapLoopResult = mapLoopResult + pair[0] + pair[1]; } let setLoopResult = ''; for (let item of iterSet) { setLoopResult = setLoopResult + item; } mapLoopResult + ':' + setLoopResult",
                )
                .unwrap(),
            "a1b2:34"
        );
        assert_eq!(
            engine
                .eval_to_string("let exhaustedMapIterator = iterMap.values(); exhaustedMapIterator.next().done + ':' + exhaustedMapIterator.next().done + ':' + exhaustedMapIterator.next().done")
                .unwrap(),
            "false:false:true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let copiedMap = new Map(iterMap); let copiedSet = new Set(iterSet); let setFromMap = new Set(iterMap); copiedMap.get('b') + ':' + copiedSet.size + ':' + setFromMap.size",
                )
                .unwrap(),
            "2:2:2"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "Array.from(iterMap.values()).join(',') + ':' + Array.from(iterSet.entries()).join(',')",
                )
                .unwrap(),
            "1,2:3,3,4,4"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let mapFromArrayIterator = new Map([['x', 9], ['y', 8]].entries()); let setFromMapIterator = new Set(iterMap.values()); mapFromArrayIterator.get(1) + ':' + setFromMapIterator.size",
                )
                .unwrap(),
            "y,8:2"
        );
    }

    #[test]
    fn collection_iteration_tracks_mutations_and_for_each_visits_new_entries() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let changingMap = new Map([['a', 1], ['b', 2], ['c', 3]]); let changingIterator = changingMap.keys(); let firstChangingKey = changingIterator.next().value; changingMap.delete('b'); changingMap.set('d', 4); firstChangingKey + ':' + changingIterator.next().value + ':' + changingIterator.next().value + ':' + changingIterator.next().done",
                )
                .unwrap(),
            "a:c:d:true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let visitedMap = new Map([['start', 1]]); let visitedKeys = ''; visitedMap.forEach(function(value, key, map) { visitedKeys += key; if (key === 'start') map.set('added', 2); }); visitedKeys + ':' + visitedMap.size",
                )
                .unwrap(),
            "startadded:2"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let visitedSet = new Set([1]); let visitedValues = ''; visitedSet.forEach(function(value, key, set) { visitedValues += value; if (value === 1) { set.delete(1); set.add(2); } }); visitedValues",
                )
                .unwrap(),
            "12"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let clearDuringIteration = new Set([1]); let clearIterator = clearDuringIteration.values(); clearDuringIteration.clear(); clearDuringIteration.add(2); clearIterator.next().value",
                )
                .unwrap(),
            "2"
        );
        assert_eq!(
            engine.eval("new Map().forEach(1)").unwrap_err().name,
            "TypeError"
        );
    }

    #[test]
    fn primitive_math_object_and_json_builtins_are_available() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval_to_string(
                "String.fromCharCode(65, 66).toLowerCase() + ':' + Number.isInteger(4) + ':' + Math.max(2, 7) + ':' + parseInt('12px')",
            ).unwrap(),
            "ab:true:7:12"
        );
        assert_eq!(
            engine.eval_to_string(
                "let decoded = JSON.parse('{\"items\":[1,true,null],\"text\":\"ok\"}'); JSON.stringify(decoded)",
            ).unwrap(),
            "{\"items\":[1,true,null],\"text\":\"ok\"}"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let visited = ''; let revived = JSON.parse('{\"items\":[1,2],\"drop\":3}', function(key, value) { visited += key + ','; if (key === '0' || key === 'drop') return undefined; return value; }); visited + ':' + JSON.stringify(revived)",
                )
                .unwrap(),
            "0,1,items,drop,,:{\"items\":[null,2]}"
        );
        assert_eq!(
            engine
                .eval_to_string("JSON.stringify({ second: 2, first: 1 })")
                .unwrap(),
            "{\"second\":2,\"first\":1}"
        );
        assert_eq!(
            engine
                .eval_to_string("JSON.stringify({ first: 1, second: 2 }, ['second'], 2)",)
                .unwrap(),
            "{\n  \"second\": 2\n}"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "JSON.stringify({ value: 3 }, function(key, value) { return typeof value === 'number' ? value * 2 : value; })",
                )
                .unwrap(),
            "{\"value\":6}"
        );
        assert_eq!(
            engine
                .eval_to_string("JSON.stringify({ value: 3 }, null, '---')",)
                .unwrap(),
            "{\n---\"value\": 3\n}"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let serializable = { value: 2, toJSON: function(key) { return { key: key, value: this.value }; } }; JSON.stringify(serializable)",
                )
                .unwrap(),
            "{\"key\":\"\",\"value\":2}"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "Object.fromEntries([['answer', 42]]).answer + ':' + Object.getOwnPropertyNames([1]).join(',')",
                )
                .unwrap(),
            "42:0,length"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "Object.keys([1]).join(',') + ':' + Object.keys({ a: 1 }).join(',')"
                )
                .unwrap(),
            "0:a"
        );
        assert_eq!(
            engine
                .eval("let circular = []; circular.push(circular); JSON.stringify(circular)")
                .unwrap_err()
                .name,
            "TypeError"
        );
        assert_eq!(
            engine
                .eval_to_string("'hello'.toUpperCase() + ':' + 'a,b'.split(',')[1]")
                .unwrap(),
            "HELLO:b"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "Object.is(NaN, NaN) + ':' + Object.hasOwn({ value: 1 }, 'value') + ':' + (42).toString(2)",
                )
                .unwrap(),
            "true:true:101010"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "[1,2,3].reduceRight(function(total, item) { return total - item; })"
                )
                .unwrap(),
            "0"
        );
        assert_eq!(
            engine
                .eval("Math.random() >= 0 && Math.random() < 1")
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            engine.eval("JSON.parse('{invalid}')").unwrap_err().name,
            "SyntaxError"
        );
    }

    #[test]
    fn object_property_descriptors_and_integrity_operations_work() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let object = {}; Object.defineProperty(object, 'hidden', { value: 7, writable: false, enumerable: false, configurable: false }); Object.getOwnPropertyDescriptor(object, 'hidden').value + ':' + Object.keys(object).length",
                )
                .unwrap(),
            "7:0"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let item = { value: 1 }; Object.freeze(item); item.value = 2; item.value + ':' + Object.isFrozen(item) + ':' + Object.isSealed(item)",
                )
                .unwrap(),
            "1:true:true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let fixedObject = {}; Object.defineProperty(fixedObject, 'fixed', { value: 1, configurable: false }); delete fixedObject.fixed",
                )
                .unwrap(),
            "false"
        );
        assert_eq!(
            engine            .eval("let extensibleObject = {}; Object.preventExtensions(extensibleObject); extensibleObject.newValue = 1; Object.isExtensible(extensibleObject)").unwrap(),
            Value::Boolean(false)
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let calls = 0; let access = {}; Object.defineProperty(access, 'value', { get: function() { return this._value; }, set: function(value) { calls++; this._value = value; }, enumerable: true, configurable: true }); access.value = 9; access.value + ':' + calls + ':' + (typeof Object.getOwnPropertyDescriptor(access, 'value').set)",
                )
                .unwrap(),
            "9:1:function"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let parent = {}; Object.defineProperty(parent, 'value', { set: function(value) { this.saved = value; }, configurable: true }); let child = Object.create(parent); child.value = 4; child.saved",
                )
                .unwrap(),
            "4"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let fixedAccessor = {}; Object.defineProperty(fixedAccessor, 'value', { get: function() { return 3; }, configurable: false }); Object.defineProperty(fixedAccessor, 'value', { get: function() { return 4; } })",
                )
                .unwrap_err()
                .name,
            "TypeError"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let target = {}; Object.defineProperties(target, { first: { value: 2, enumerable: true }, second: { value: 3 } }); let descriptors = Object.getOwnPropertyDescriptors(target); target.first + target.second + ':' + Object.keys(descriptors).join(',') + ':' + Object.prototype.isPrototypeOf(target)",
                )
                .unwrap(),
            "5:first,second:true"
        );
    }

    #[test]
    fn reflect_core_operations_use_object_properties() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string(
                    "let object = { value: 2 }; Reflect.set(object, 'value', 5) + ':' + Reflect.get(object, 'value') + ':' + Reflect.has(object, 'value')",
                )
                .unwrap(),
            "true:5:true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let lockedTarget = {}; let defined = Reflect.defineProperty(lockedTarget, 'locked', { value: 3, writable: false, configurable: false }); defined + ':' + Reflect.set(lockedTarget, 'locked', 4) + ':' + Reflect.deleteProperty(lockedTarget, 'locked') + ':' + Reflect.ownKeys(lockedTarget).join(',')",
                )
                .unwrap(),
            "true:false:false:locked"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let proto = {}; let reflectTarget = Object.create(proto); Reflect.setPrototypeOf(reflectTarget, proto) + ':' + (Reflect.getPrototypeOf(reflectTarget) === proto) + ':' + Reflect.preventExtensions(reflectTarget) + ':' + Reflect.isExtensible(reflectTarget)",
                )
                .unwrap(),
            "true:true:true:false"
        );
        assert_eq!(
            engine.eval("Reflect.get(1, 'x')").unwrap_err().name,
            "TypeError"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "function sum(a, b) { return this.base + a + b; } Reflect.apply(sum, { base: 4 }, [2, 3])",
                )
                .unwrap(),
            "9"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "function Box(value) { this.value = value; } let instance = Reflect.construct(Box, [7]); instance.value + ':' + (instance instanceof Box)",
                )
                .unwrap(),
            "7:true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "function Box(value) { this.value = value; } function Other() {} Other.prototype.kind = 'other'; let otherInstance = Reflect.construct(Box, [8], Other); otherInstance.value + ':' + otherInstance.kind + ':' + (otherInstance instanceof Other)",
                )
                .unwrap(),
            "8:other:true"
        );
        assert_eq!(
            engine.eval("Reflect.construct(1, [])").unwrap_err().name,
            "TypeError"
        );
    }

    #[test]
    fn core_expression_operators_follow_precedence_and_short_circuiting() {
        let mut engine = Engine::new();
        assert_eq!(engine.eval_to_string("2 ** 3 ** 2").unwrap(), "512");
        assert_eq!(
            engine
                .eval_to_string("(5 & 3) + (8 >> 1) + (1 << 2) + (8 >>> 1)")
                .unwrap(),
            "13"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let count = 0; let chosen = true ? ++count : ++count; chosen + count"
                )
                .unwrap(),
            "2"
        );
        assert_eq!(
            engine
                .eval_to_string("let total = 4; total += 3; total *= 2; total **= 2; total",)
                .unwrap(),
            "196"
        );
        assert_eq!(
            engine
                .eval_to_string("let maybe = null; maybe ??= 'ready'; maybe")
                .unwrap(),
            "ready"
        );
        assert_eq!(
            engine
                .eval_to_string("let flag = false; flag ||= true; flag &&= true; flag")
                .unwrap(),
            "true"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let calls = 0; let values = [10]; values[++calls] = 4; calls + values.length",
                )
                .unwrap(),
            "3"
        );
        assert_eq!(engine.eval_to_string("let n = 1; n++ + ++n").unwrap(), "4");
    }

    #[test]
    fn for_in_and_for_of_iterate_supported_values_with_fresh_lexical_bindings() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string("let sum = 0; for (const value of [2, 3, 4]) sum += value; sum")
                .unwrap(),
            "9"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let names = ''; for (let key in { first: 1, second: 2 }) names += key + ','; names",
                )
                .unwrap(),
            "first,second,"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let chars = ''; for (let character of 'hi') chars += character; chars"
                )
                .unwrap(),
            "hi"
        );
        assert_eq!(
            engine
                .eval_to_string(
                    "let saved = []; for (let value of [4, 9]) { if (value === 4) saved[0] = function() { return value; }; else saved[1] = function() { return value; }; } saved[0]() + saved[1]()",
                )
                .unwrap(),
            "13"
        );
        assert_eq!(
            engine.eval("for (let value of 1) {}").unwrap_err().name,
            "TypeError"
        );
    }
}
