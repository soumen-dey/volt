use crate::{
    EngineError, Value,
    parser::{BinaryOp, Expr, UnaryOp},
};

#[derive(Clone)]
enum Constant {
    Value(Value),
    Binding {
        name: String,
        line: usize,
        column: usize,
    },
}

#[derive(Clone, Copy)]
#[repr(u16)]
enum Opcode {
    LoadConstant = 0,
    LoadBinding = 1,
    Unary = 2,
    Binary = 3,
    Return = 4,
}

pub(crate) struct Bytecode {
    words: Vec<u16>,
    constants: Vec<Constant>,
    caches: Vec<InlineCache>,
}

type DispatchHandler<H> = fn(&mut Frame<'_, H>, usize) -> Result<(), EngineError>;

impl Bytecode {
    pub(crate) fn compile(expression: &Expr) -> Option<Self> {
        let mut compiler = Compiler {
            words: Vec::new(),
            constants: Vec::new(),
        };
        compiler.expression(expression)?;
        compiler.emit(Opcode::Return);
        let cache_count = compiler.words.len();
        Some(Self {
            words: compiler.words,
            constants: compiler.constants,
            caches: vec![InlineCache::default(); cache_count],
        })
    }

    pub(crate) fn run<H: Host>(
        &mut self,
        host: &mut H,
        scope: usize,
    ) -> Result<Value, EngineError> {
        let words = &self.words;
        let constants = &self.constants;
        let caches = &mut self.caches;
        let mut frame = Frame {
            words,
            constants,
            pc: 0,
            stack: Vec::new(),
            caches,
            host,
            scope,
            result: None,
        };
        let dispatch: [DispatchHandler<H>; 5] = [
            dispatch_load_constant::<H>,
            dispatch_load_binding::<H>,
            dispatch_unary::<H>,
            dispatch_binary::<H>,
            dispatch_return::<H>,
        ];

        while frame.result.is_none() {
            let instruction = frame.read_word()?;
            let opcode = match instruction {
                value if value == Opcode::LoadConstant as u16 => Opcode::LoadConstant,
                value if value == Opcode::LoadBinding as u16 => Opcode::LoadBinding,
                value if value == Opcode::Unary as u16 => Opcode::Unary,
                value if value == Opcode::Binary as u16 => Opcode::Binary,
                value if value == Opcode::Return as u16 => Opcode::Return,
                _ => return Err(invalid_bytecode()),
            };
            let site = frame.pc - 1;
            dispatch[opcode as usize](&mut frame, site)?;
        }

        frame.result.ok_or_else(invalid_bytecode)
    }
}

struct Compiler {
    words: Vec<u16>,
    constants: Vec<Constant>,
}

impl Compiler {
    fn expression(&mut self, expression: &Expr) -> Option<()> {
        match expression {
            Expr::Literal(value) => {
                let index = self.constant(Constant::Value(value.clone()))?;
                self.emit(Opcode::LoadConstant);
                self.words.push(index);
            }
            Expr::Identifier { name, line, column } => {
                let index = self.constant(Constant::Binding {
                    name: name.clone(),
                    line: *line,
                    column: *column,
                })?;
                self.emit(Opcode::LoadBinding);
                self.words.push(index);
            }
            Expr::Unary {
                operator, operand, ..
            } if matches!(
                operator,
                UnaryOp::Not | UnaryOp::Plus | UnaryOp::Minus | UnaryOp::BitNot
            ) =>
            {
                self.expression(operand)?;
                self.emit(Opcode::Unary);
                self.words.push(*operator as u16);
            }
            Expr::Binary {
                operator: BinaryOp::And | BinaryOp::Or | BinaryOp::Nullish,
                ..
            }
            | Expr::Unary {
                operator: UnaryOp::Delete,
                ..
            } => return None,
            Expr::Unary {
                operator: UnaryOp::TypeOf,
                operand,
                ..
            } if matches!(operand.as_ref(), Expr::Identifier { .. }) => return None,
            Expr::Unary {
                operator: UnaryOp::TypeOf,
                operand,
                ..
            }
            | Expr::Unary {
                operator: UnaryOp::Void,
                operand,
                ..
            } => {
                self.expression(operand)?;
                self.emit(Opcode::Unary);
                let operator = match expression {
                    Expr::Unary { operator, .. } => *operator,
                    _ => unreachable!(),
                };
                self.words.push(operator as u16);
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => {
                self.expression(left)?;
                self.expression(right)?;
                self.emit(Opcode::Binary);
                self.words.push(*operator as u16);
            }
            _ => return None,
        }
        Some(())
    }

    fn constant(&mut self, constant: Constant) -> Option<u16> {
        let index = u16::try_from(self.constants.len()).ok()?;
        self.constants.push(constant);
        Some(index)
    }

    fn emit(&mut self, opcode: Opcode) {
        self.words.push(opcode as u16);
    }
}

#[derive(Default, Clone)]
pub(crate) struct InlineCache {
    lookup_scope: Option<usize>,
    binding_scope: Option<usize>,
    left_types: u8,
    right_types: u8,
    numeric_hits: u32,
    generic_hits: u32,
}

impl InlineCache {
    pub(crate) fn binding_for(&self, lookup_scope: usize) -> Option<Option<usize>> {
        (self.lookup_scope == Some(lookup_scope)).then_some(self.binding_scope)
    }

    pub(crate) fn cache_binding(&mut self, lookup_scope: usize, binding_scope: Option<usize>) {
        self.lookup_scope = Some(lookup_scope);
        self.binding_scope = binding_scope;
    }

    pub(crate) fn record_binary(&mut self, left: &Value, right: &Value) -> bool {
        self.left_types |= value_type_mask(left);
        self.right_types |= value_type_mask(right);
        if matches!(left, Value::Number(_)) && matches!(right, Value::Number(_)) {
            self.numeric_hits = self.numeric_hits.saturating_add(1);
            true
        } else {
            self.generic_hits = self.generic_hits.saturating_add(1);
            false
        }
    }
}

fn value_type_mask(value: &Value) -> u8 {
    match value {
        Value::Undefined => 1 << 0,
        Value::Null => 1 << 1,
        Value::Boolean(_) => 1 << 2,
        Value::Number(_) => 1 << 3,
        Value::String(_) => 1 << 4,
        Value::Object(_) => 1 << 5,
    }
}

pub(crate) trait Host {
    fn load_binding(
        &mut self,
        scope: usize,
        name: &str,
        line: usize,
        column: usize,
        cache: &mut InlineCache,
    ) -> Result<Value, EngineError>;

    fn unary(&mut self, operator: UnaryOp, value: Value) -> Result<Value, EngineError>;

    fn binary(
        &mut self,
        operator: BinaryOp,
        left: Value,
        right: Value,
        cache: &mut InlineCache,
    ) -> Result<Value, EngineError>;
}

struct Frame<'a, H> {
    words: &'a [u16],
    constants: &'a [Constant],
    pc: usize,
    stack: Vec<Value>,
    caches: &'a mut [InlineCache],
    host: &'a mut H,
    scope: usize,
    result: Option<Value>,
}

fn dispatch_load_constant<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.load_constant(site)
}

fn dispatch_load_binding<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.load_binding(site)
}

fn dispatch_unary<H: Host>(frame: &mut Frame<'_, H>, site: usize) -> Result<(), EngineError> {
    frame.unary(site)
}

fn dispatch_binary<H: Host>(frame: &mut Frame<'_, H>, site: usize) -> Result<(), EngineError> {
    frame.binary(site)
}

fn dispatch_return<H: Host>(frame: &mut Frame<'_, H>, site: usize) -> Result<(), EngineError> {
    frame.return_value(site)
}

impl<H: Host> Frame<'_, H> {
    fn read_word(&mut self) -> Result<u16, EngineError> {
        let word = self
            .words
            .get(self.pc)
            .copied()
            .ok_or_else(invalid_bytecode)?;
        self.pc += 1;
        Ok(word)
    }

    fn load_constant(&mut self, _: usize) -> Result<(), EngineError> {
        let index = usize::from(self.read_word()?);
        let Some(Constant::Value(value)) = self.constants.get(index) else {
            return Err(invalid_bytecode());
        };
        self.stack.push(value.clone());
        Ok(())
    }

    fn load_binding(&mut self, site: usize) -> Result<(), EngineError> {
        let index = usize::from(self.read_word()?);
        let Some(Constant::Binding { name, line, column }) = self.constants.get(index) else {
            return Err(invalid_bytecode());
        };
        let value =
            self.host
                .load_binding(self.scope, name, *line, *column, &mut self.caches[site])?;
        self.stack.push(value);
        Ok(())
    }

    fn unary(&mut self, _: usize) -> Result<(), EngineError> {
        let operator = decode_unary(self.read_word()?)?;
        let value = self.stack.pop().ok_or_else(invalid_bytecode)?;
        self.stack.push(self.host.unary(operator, value)?);
        Ok(())
    }

    fn binary(&mut self, site: usize) -> Result<(), EngineError> {
        let operator = decode_binary(self.read_word()?)?;
        let right = self.stack.pop().ok_or_else(invalid_bytecode)?;
        let left = self.stack.pop().ok_or_else(invalid_bytecode)?;
        self.stack.push(
            self.host
                .binary(operator, left, right, &mut self.caches[site])?,
        );
        Ok(())
    }

    fn return_value(&mut self, _: usize) -> Result<(), EngineError> {
        if self.stack.len() != 1 {
            return Err(invalid_bytecode());
        }
        self.result = self.stack.pop();
        Ok(())
    }
}

fn decode_unary(value: u16) -> Result<UnaryOp, EngineError> {
    match value {
        value if value == UnaryOp::Not as u16 => Ok(UnaryOp::Not),
        value if value == UnaryOp::Plus as u16 => Ok(UnaryOp::Plus),
        value if value == UnaryOp::Minus as u16 => Ok(UnaryOp::Minus),
        value if value == UnaryOp::BitNot as u16 => Ok(UnaryOp::BitNot),
        value if value == UnaryOp::TypeOf as u16 => Ok(UnaryOp::TypeOf),
        value if value == UnaryOp::Void as u16 => Ok(UnaryOp::Void),
        value if value == UnaryOp::Delete as u16 => Ok(UnaryOp::Delete),
        _ => Err(invalid_bytecode()),
    }
}

fn decode_binary(value: u16) -> Result<BinaryOp, EngineError> {
    match value {
        value if value == BinaryOp::Add as u16 => Ok(BinaryOp::Add),
        value if value == BinaryOp::Subtract as u16 => Ok(BinaryOp::Subtract),
        value if value == BinaryOp::Multiply as u16 => Ok(BinaryOp::Multiply),
        value if value == BinaryOp::Divide as u16 => Ok(BinaryOp::Divide),
        value if value == BinaryOp::Remainder as u16 => Ok(BinaryOp::Remainder),
        value if value == BinaryOp::Exponent as u16 => Ok(BinaryOp::Exponent),
        value if value == BinaryOp::BitAnd as u16 => Ok(BinaryOp::BitAnd),
        value if value == BinaryOp::BitOr as u16 => Ok(BinaryOp::BitOr),
        value if value == BinaryOp::BitXor as u16 => Ok(BinaryOp::BitXor),
        value if value == BinaryOp::ShiftLeft as u16 => Ok(BinaryOp::ShiftLeft),
        value if value == BinaryOp::ShiftRight as u16 => Ok(BinaryOp::ShiftRight),
        value if value == BinaryOp::UnsignedShiftRight as u16 => Ok(BinaryOp::UnsignedShiftRight),
        value if value == BinaryOp::StrictEqual as u16 => Ok(BinaryOp::StrictEqual),
        value if value == BinaryOp::StrictNotEqual as u16 => Ok(BinaryOp::StrictNotEqual),
        value if value == BinaryOp::Equal as u16 => Ok(BinaryOp::Equal),
        value if value == BinaryOp::NotEqual as u16 => Ok(BinaryOp::NotEqual),
        value if value == BinaryOp::LessThan as u16 => Ok(BinaryOp::LessThan),
        value if value == BinaryOp::LessThanOrEqual as u16 => Ok(BinaryOp::LessThanOrEqual),
        value if value == BinaryOp::GreaterThan as u16 => Ok(BinaryOp::GreaterThan),
        value if value == BinaryOp::GreaterThanOrEqual as u16 => Ok(BinaryOp::GreaterThanOrEqual),
        value if value == BinaryOp::In as u16 => Ok(BinaryOp::In),
        value if value == BinaryOp::InstanceOf as u16 => Ok(BinaryOp::InstanceOf),
        value if value == BinaryOp::And as u16 => Ok(BinaryOp::And),
        value if value == BinaryOp::Or as u16 => Ok(BinaryOp::Or),
        value if value == BinaryOp::Nullish as u16 => Ok(BinaryOp::Nullish),
        _ => Err(invalid_bytecode()),
    }
}

fn invalid_bytecode() -> EngineError {
    EngineError::new("InternalError", "Invalid bytecode", 1, 1)
}

#[cfg(test)]
mod tests {
    use crate::{Engine, Value};

    #[test]
    fn bytecode_runs_arithmetic_expressions_and_caches_bindings() {
        let mut engine = Engine::new();
        assert_eq!(engine.eval("1 + 2 * 3").unwrap(), Value::Number(7.0));
        assert_eq!(
            engine
                .eval("let vmCounter = 0; while (vmCounter < 20) { vmCounter = vmCounter + 1; } vmCounter")
                .unwrap(),
            Value::Number(20.0)
        );
    }

    #[test]
    fn bytecode_falls_back_for_short_circuit_and_special_typeof_semantics() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval("0 && missingVmBinding").unwrap(),
            Value::Number(0.0)
        );
        assert_eq!(
            engine.eval("typeof missingVmBinding").unwrap(),
            Value::String("undefined".to_owned())
        );
        assert_eq!(
            engine.eval("'volt' + 1").unwrap(),
            Value::String("volt1".to_owned())
        );
    }
}
