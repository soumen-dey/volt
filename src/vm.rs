use crate::{
    EngineError, Value,
    parser::{BinaryOp, DeclarationKind, Expr, Stmt, UnaryOp},
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
    JumpIfFalse = 5,
    JumpIfFalseKeep = 6,
    JumpIfTrueKeep = 7,
    JumpIfNotNullishKeep = 8,
    Jump = 9,
    Pop = 10,
    StoreBinding = 11,
    InitializeBinding = 12,
    CheckBinding = 13,
    SetResult = 14,
    ClearResult = 15,
    FunctionReturn = 16,
}

pub(crate) struct Bytecode {
    words: Vec<u16>,
    constants: Vec<Constant>,
    caches: Vec<InlineCache>,
}

pub(crate) enum Flow {
    Complete(Value),
    Return(Value),
}

type DispatchHandler<H> = fn(&mut Frame<'_, H>, usize) -> Result<(), EngineError>;

impl Bytecode {
    pub(crate) fn compile(expression: &Expr) -> Option<Self> {
        let mut compiler = Compiler {
            words: Vec::new(),
            constants: Vec::new(),
            loops: Vec::new(),
            statement_depth: 0,
            allow_return: false,
        };
        compiler.expression(expression)?;
        compiler.emit(Opcode::Return);
        Some(compiler.finish())
    }

    pub(crate) fn compile_statements(statements: &[Stmt]) -> Option<Self> {
        let mut compiler = Compiler {
            words: Vec::new(),
            constants: Vec::new(),
            loops: Vec::new(),
            statement_depth: 0,
            allow_return: false,
        };
        compiler.statements(statements)?;
        compiler.emit(Opcode::Return);
        Some(compiler.finish())
    }

    pub(crate) fn compile_function(statements: &[Stmt]) -> Option<Self> {
        let mut compiler = Compiler {
            words: Vec::new(),
            constants: Vec::new(),
            loops: Vec::new(),
            statement_depth: 0,
            allow_return: true,
        };
        compiler.statements(statements)?;
        compiler.emit(Opcode::Return);
        Some(compiler.finish())
    }

    pub(crate) fn run<H: Host>(
        &mut self,
        host: &mut H,
        scope: usize,
    ) -> Result<Value, EngineError> {
        match self.run_flow(host, scope)? {
            Flow::Complete(value) | Flow::Return(value) => Ok(value),
        }
    }

    pub(crate) fn run_function<H: Host>(
        &mut self,
        host: &mut H,
        scope: usize,
    ) -> Result<Flow, EngineError> {
        self.run_flow(host, scope)
    }

    fn run_flow<H: Host>(&mut self, host: &mut H, scope: usize) -> Result<Flow, EngineError> {
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
            completion: Value::Undefined,
        };
        let dispatch: [DispatchHandler<H>; 17] = [
            dispatch_load_constant::<H>,
            dispatch_load_binding::<H>,
            dispatch_unary::<H>,
            dispatch_binary::<H>,
            dispatch_return::<H>,
            dispatch_jump_if_false::<H>,
            dispatch_jump_if_false_keep::<H>,
            dispatch_jump_if_true_keep::<H>,
            dispatch_jump_if_not_nullish_keep::<H>,
            dispatch_jump::<H>,
            dispatch_pop::<H>,
            dispatch_store_binding::<H>,
            dispatch_initialize_binding::<H>,
            dispatch_check_binding::<H>,
            dispatch_set_result::<H>,
            dispatch_clear_result::<H>,
            dispatch_function_return::<H>,
        ];

        while frame.result.is_none() {
            let instruction = frame.read_word()?;
            let opcode = match instruction {
                value if value == Opcode::LoadConstant as u16 => Opcode::LoadConstant,
                value if value == Opcode::LoadBinding as u16 => Opcode::LoadBinding,
                value if value == Opcode::Unary as u16 => Opcode::Unary,
                value if value == Opcode::Binary as u16 => Opcode::Binary,
                value if value == Opcode::Return as u16 => Opcode::Return,
                value if value == Opcode::JumpIfFalse as u16 => Opcode::JumpIfFalse,
                value if value == Opcode::JumpIfFalseKeep as u16 => Opcode::JumpIfFalseKeep,
                value if value == Opcode::JumpIfTrueKeep as u16 => Opcode::JumpIfTrueKeep,
                value if value == Opcode::JumpIfNotNullishKeep as u16 => {
                    Opcode::JumpIfNotNullishKeep
                }
                value if value == Opcode::Jump as u16 => Opcode::Jump,
                value if value == Opcode::Pop as u16 => Opcode::Pop,
                value if value == Opcode::StoreBinding as u16 => Opcode::StoreBinding,
                value if value == Opcode::InitializeBinding as u16 => Opcode::InitializeBinding,
                value if value == Opcode::CheckBinding as u16 => Opcode::CheckBinding,
                value if value == Opcode::SetResult as u16 => Opcode::SetResult,
                value if value == Opcode::ClearResult as u16 => Opcode::ClearResult,
                value if value == Opcode::FunctionReturn as u16 => Opcode::FunctionReturn,
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
    loops: Vec<LoopContext>,
    statement_depth: usize,
    allow_return: bool,
}

#[derive(Default)]
struct LoopContext {
    breaks: Vec<usize>,
    continues: Vec<usize>,
}

impl Compiler {
    fn finish(self) -> Bytecode {
        let cache_count = self.words.len();
        Bytecode {
            words: self.words,
            constants: self.constants,
            caches: vec![InlineCache::default(); cache_count],
        }
    }

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
                operator: operator @ (BinaryOp::And | BinaryOp::Or | BinaryOp::Nullish),
                left,
                right,
            } => {
                self.expression(left)?;
                let jump = match operator {
                    BinaryOp::And => Opcode::JumpIfFalseKeep,
                    BinaryOp::Or => Opcode::JumpIfTrueKeep,
                    BinaryOp::Nullish => Opcode::JumpIfNotNullishKeep,
                    _ => unreachable!(),
                };
                let jump_site = self.emit_jump(jump)?;
                self.emit(Opcode::Pop);
                self.expression(right)?;
                self.patch_jump(jump_site)?;
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.expression(test)?;
                let alternate_jump = self.emit_jump(Opcode::JumpIfFalse)?;
                self.expression(consequent)?;
                let end_jump = self.emit_jump(Opcode::Jump)?;
                self.patch_jump(alternate_jump)?;
                self.expression(alternate)?;
                self.patch_jump(end_jump)?;
            }
            Expr::Unary {
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
            Expr::Assignment {
                target,
                value,
                operator,
            } => {
                let Expr::Identifier { name, line, column } = target.as_ref() else {
                    return None;
                };
                if matches!(
                    operator,
                    Some(BinaryOp::And | BinaryOp::Or | BinaryOp::Nullish)
                ) {
                    return None;
                }
                let binding = self.constant(Constant::Binding {
                    name: name.clone(),
                    line: *line,
                    column: *column,
                })?;
                self.emit(Opcode::CheckBinding);
                self.words.push(binding);
                if let Some(operator) = operator {
                    self.emit(Opcode::LoadBinding);
                    self.words.push(binding);
                    self.expression(value)?;
                    self.emit(Opcode::Binary);
                    self.words.push(*operator as u16);
                } else {
                    self.expression(value)?;
                }
                self.emit(Opcode::StoreBinding);
                self.words.push(binding);
            }
            _ => return None,
        }
        Some(())
    }

    fn statements(&mut self, statements: &[Stmt]) -> Option<()> {
        for statement in statements {
            self.statement(statement)?;
        }
        Some(())
    }

    fn statement(&mut self, statement: &Stmt) -> Option<()> {
        match statement {
            Stmt::Empty => self.emit(Opcode::ClearResult),
            Stmt::Expression(expression) => {
                self.expression(expression)?;
                self.emit(Opcode::SetResult);
            }
            Stmt::Declaration {
                name,
                initializer,
                kind,
                ..
            } => {
                if self.statement_depth > 0
                    && matches!(kind, DeclarationKind::Let | DeclarationKind::Const)
                {
                    return None;
                }
                if *kind == DeclarationKind::Var && initializer.is_none() {
                    self.emit(Opcode::ClearResult);
                    return Some(());
                }
                self.expression(
                    initializer
                        .as_ref()
                        .unwrap_or(&Expr::Literal(Value::Undefined)),
                )?;
                let binding = self.constant(Constant::Binding {
                    name: name.clone(),
                    line: 1,
                    column: 1,
                })?;
                self.emit(Opcode::InitializeBinding);
                self.words.push(binding);
                self.words.push(*kind as u16);
                self.emit(Opcode::ClearResult);
            }
            Stmt::FunctionDeclaration { .. } => self.emit(Opcode::ClearResult),
            Stmt::Block(statements) => {
                if statements.iter().any(|statement| {
                    matches!(
                        statement,
                        Stmt::Declaration {
                            kind: DeclarationKind::Let | DeclarationKind::Const,
                            ..
                        } | Stmt::FunctionDeclaration { .. }
                    )
                }) {
                    return None;
                }
                self.emit(Opcode::ClearResult);
                self.statement_depth += 1;
                self.statements(statements)?;
                self.statement_depth -= 1;
            }
            Stmt::If {
                condition,
                consequent,
                alternate,
            } => {
                self.expression(condition)?;
                let alternate_jump = self.emit_jump(Opcode::JumpIfFalse)?;
                self.statement_depth += 1;
                self.statement(consequent)?;
                self.statement_depth -= 1;
                if let Some(alternate) = alternate {
                    let end_jump = self.emit_jump(Opcode::Jump)?;
                    self.patch_jump(alternate_jump)?;
                    self.statement_depth += 1;
                    self.statement(alternate)?;
                    self.statement_depth -= 1;
                    self.patch_jump(end_jump)?;
                } else {
                    let end_jump = self.emit_jump(Opcode::Jump)?;
                    self.patch_jump(alternate_jump)?;
                    self.emit(Opcode::ClearResult);
                    self.patch_jump(end_jump)?;
                }
            }
            Stmt::While { condition, body } => {
                self.emit(Opcode::ClearResult);
                let loop_start = self.words.len();
                self.expression(condition)?;
                let exit_jump = self.emit_jump(Opcode::JumpIfFalse)?;
                self.loops.push(LoopContext::default());
                self.statement_depth += 1;
                self.statement(body)?;
                self.statement_depth -= 1;
                let continue_target = self.words.len();
                self.emit_jump_to(Opcode::Jump, loop_start)?;
                let loop_end = self.words.len();
                self.patch_jump_to(exit_jump, loop_end)?;
                let loop_context = self.loops.pop()?;
                for jump in loop_context.breaks {
                    self.patch_jump_to(jump, loop_end)?;
                }
                for jump in loop_context.continues {
                    self.patch_jump_to(jump, continue_target)?;
                }
            }
            Stmt::For {
                initializer,
                condition,
                update,
                body,
            } => {
                if let Some(initializer) = initializer.as_deref() {
                    if matches!(
                        initializer,
                        Stmt::Declaration {
                            kind: DeclarationKind::Let | DeclarationKind::Const,
                            ..
                        }
                    ) {
                        return None;
                    }
                    self.statement(initializer)?;
                }
                self.emit(Opcode::ClearResult);
                let loop_start = self.words.len();
                let exit_jump = if let Some(condition) = condition {
                    self.expression(condition)?;
                    Some(self.emit_jump(Opcode::JumpIfFalse)?)
                } else {
                    None
                };
                self.loops.push(LoopContext::default());
                self.statement_depth += 1;
                self.statement(body)?;
                self.statement_depth -= 1;
                let continue_target = self.words.len();
                if let Some(update) = update {
                    self.expression(update)?;
                    self.emit(Opcode::SetResult);
                }
                self.emit_jump_to(Opcode::Jump, loop_start)?;
                let loop_end = self.words.len();
                if let Some(exit_jump) = exit_jump {
                    self.patch_jump_to(exit_jump, loop_end)?;
                }
                let loop_context = self.loops.pop()?;
                for jump in loop_context.breaks {
                    self.patch_jump_to(jump, loop_end)?;
                }
                for jump in loop_context.continues {
                    self.patch_jump_to(jump, continue_target)?;
                }
            }
            Stmt::Break => {
                let jump = self.emit_jump(Opcode::Jump)?;
                self.loops.last_mut()?.breaks.push(jump);
            }
            Stmt::Continue => {
                let jump = self.emit_jump(Opcode::Jump)?;
                self.loops.last_mut()?.continues.push(jump);
            }
            Stmt::Return(value) => {
                if !self.allow_return {
                    return None;
                }
                if let Some(value) = value {
                    self.expression(value)?;
                } else {
                    let undefined = self.constant(Constant::Value(Value::Undefined))?;
                    self.emit(Opcode::LoadConstant);
                    self.words.push(undefined);
                }
                self.emit(Opcode::FunctionReturn);
            }
            Stmt::Throw(_) | Stmt::Try { .. } | Stmt::ForEach { .. } | Stmt::DoWhile { .. } => {
                return None;
            }
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

    fn emit_jump(&mut self, opcode: Opcode) -> Option<usize> {
        self.emit(opcode);
        let operand = self.words.len();
        self.words.push(0);
        Some(operand)
    }

    fn patch_jump(&mut self, operand: usize) -> Option<()> {
        self.patch_jump_to(operand, self.words.len())
    }

    fn patch_jump_to(&mut self, operand: usize, target: usize) -> Option<()> {
        self.words[operand] = u16::try_from(target).ok()?;
        Some(())
    }

    fn emit_jump_to(&mut self, opcode: Opcode, target: usize) -> Option<()> {
        self.emit(opcode);
        self.words.push(u16::try_from(target).ok()?);
        Some(())
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

    fn check_binding_assignment(
        &self,
        scope: usize,
        name: &str,
        line: usize,
        column: usize,
    ) -> Result<(), EngineError>;

    fn store_binding(
        &mut self,
        scope: usize,
        name: &str,
        value: Value,
        line: usize,
        column: usize,
    ) -> Result<Value, EngineError>;

    fn initialize_declaration(
        &mut self,
        scope: usize,
        name: &str,
        value: Value,
        kind: DeclarationKind,
    ) -> Result<(), EngineError>;
}

struct Frame<'a, H> {
    words: &'a [u16],
    constants: &'a [Constant],
    pc: usize,
    stack: Vec<Value>,
    caches: &'a mut [InlineCache],
    host: &'a mut H,
    scope: usize,
    result: Option<Flow>,
    completion: Value,
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

fn dispatch_jump_if_false<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.jump_if_false(site)
}

fn dispatch_jump_if_false_keep<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.jump_if_false_keep(site)
}

fn dispatch_jump_if_true_keep<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.jump_if_true_keep(site)
}

fn dispatch_jump_if_not_nullish_keep<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.jump_if_not_nullish_keep(site)
}

fn dispatch_jump<H: Host>(frame: &mut Frame<'_, H>, site: usize) -> Result<(), EngineError> {
    frame.jump(site)
}

fn dispatch_pop<H: Host>(frame: &mut Frame<'_, H>, site: usize) -> Result<(), EngineError> {
    frame.pop(site)
}

fn dispatch_store_binding<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.store_binding(site)
}

fn dispatch_initialize_binding<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.initialize_binding(site)
}

fn dispatch_check_binding<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.check_binding(site)
}

fn dispatch_set_result<H: Host>(frame: &mut Frame<'_, H>, site: usize) -> Result<(), EngineError> {
    frame.set_result(site)
}

fn dispatch_clear_result<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.clear_result(site)
}

fn dispatch_function_return<H: Host>(
    frame: &mut Frame<'_, H>,
    site: usize,
) -> Result<(), EngineError> {
    frame.function_return(site)
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
        if self.stack.is_empty() {
            self.result = Some(Flow::Complete(self.completion.clone()));
            return Ok(());
        }
        if self.stack.len() != 1 {
            return Err(invalid_bytecode());
        }
        self.result = self.stack.pop().map(Flow::Complete);
        Ok(())
    }

    fn function_return(&mut self, _: usize) -> Result<(), EngineError> {
        if self.stack.len() != 1 {
            return Err(invalid_bytecode());
        }
        self.result = self.stack.pop().map(Flow::Return);
        Ok(())
    }

    fn jump_if_false(&mut self, _: usize) -> Result<(), EngineError> {
        let target = usize::from(self.read_word()?);
        let condition = self.stack.pop().ok_or_else(invalid_bytecode)?;
        if !condition.is_truthy() {
            self.pc = target;
        }
        Ok(())
    }

    fn jump_if_false_keep(&mut self, _: usize) -> Result<(), EngineError> {
        let target = usize::from(self.read_word()?);
        let value = self.stack.last().ok_or_else(invalid_bytecode)?;
        if !value.is_truthy() {
            self.pc = target;
        }
        Ok(())
    }

    fn jump_if_true_keep(&mut self, _: usize) -> Result<(), EngineError> {
        let target = usize::from(self.read_word()?);
        let value = self.stack.last().ok_or_else(invalid_bytecode)?;
        if value.is_truthy() {
            self.pc = target;
        }
        Ok(())
    }

    fn jump_if_not_nullish_keep(&mut self, _: usize) -> Result<(), EngineError> {
        let target = usize::from(self.read_word()?);
        let value = self.stack.last().ok_or_else(invalid_bytecode)?;
        if !matches!(value, Value::Null | Value::Undefined) {
            self.pc = target;
        }
        Ok(())
    }

    fn jump(&mut self, _: usize) -> Result<(), EngineError> {
        self.pc = usize::from(self.read_word()?);
        Ok(())
    }

    fn pop(&mut self, _: usize) -> Result<(), EngineError> {
        self.stack.pop().ok_or_else(invalid_bytecode)?;
        Ok(())
    }

    fn store_binding(&mut self, _: usize) -> Result<(), EngineError> {
        let index = usize::from(self.read_word()?);
        let Some(Constant::Binding { name, line, column }) = self.constants.get(index) else {
            return Err(invalid_bytecode());
        };
        let value = self.stack.pop().ok_or_else(invalid_bytecode)?;
        let value = self
            .host
            .store_binding(self.scope, name, value, *line, *column)?;
        self.stack.push(value);
        Ok(())
    }

    fn initialize_binding(&mut self, _: usize) -> Result<(), EngineError> {
        let index = usize::from(self.read_word()?);
        let kind = decode_declaration(self.read_word()?)?;
        let Some(Constant::Binding { name, .. }) = self.constants.get(index) else {
            return Err(invalid_bytecode());
        };
        let value = self.stack.pop().ok_or_else(invalid_bytecode)?;
        self.host
            .initialize_declaration(self.scope, name, value, kind)
    }

    fn check_binding(&mut self, _: usize) -> Result<(), EngineError> {
        let index = usize::from(self.read_word()?);
        let Some(Constant::Binding { name, line, column }) = self.constants.get(index) else {
            return Err(invalid_bytecode());
        };
        self.host
            .check_binding_assignment(self.scope, name, *line, *column)
    }

    fn set_result(&mut self, _: usize) -> Result<(), EngineError> {
        self.completion = self.stack.pop().ok_or_else(invalid_bytecode)?;
        Ok(())
    }

    fn clear_result(&mut self, _: usize) -> Result<(), EngineError> {
        self.completion = Value::Undefined;
        Ok(())
    }
}

fn decode_declaration(value: u16) -> Result<DeclarationKind, EngineError> {
    match value {
        0 => Ok(DeclarationKind::Let),
        1 => Ok(DeclarationKind::Const),
        2 => Ok(DeclarationKind::Var),
        _ => Err(invalid_bytecode()),
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
    fn bytecode_branches_for_short_circuit_and_conditionals() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval("0 && missingVmBinding").unwrap(),
            Value::Number(0.0)
        );
        assert_eq!(
            engine.eval("'kept' || missingVmBinding").unwrap(),
            Value::String("kept".to_owned())
        );
        assert_eq!(
            engine.eval("null ?? 'fallback'").unwrap(),
            Value::String("fallback".to_owned())
        );
        assert_eq!(
            engine.eval("9 ?? missingVmBinding").unwrap(),
            Value::Number(9.0)
        );
        assert_eq!(
            engine.eval("true ? 'yes' : missingVmBinding").unwrap(),
            Value::String("yes".to_owned())
        );
        assert_eq!(
            engine.eval("false ? missingVmBinding : 'no'").unwrap(),
            Value::String("no".to_owned())
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

    #[test]
    fn compiler_emits_branch_bytecode_for_logical_and_conditional_expressions() {
        for source in [
            "left && right",
            "left || right",
            "left ?? right",
            "test ? consequent : alternate",
        ] {
            let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
            let program = crate::parser::Parser::new(tokens).parse_program().unwrap();
            let crate::parser::Stmt::Expression(expression) = &program.statements[0] else {
                panic!("expected expression statement");
            };
            assert!(
                super::Bytecode::compile(expression).is_some(),
                "expected bytecode for {source}"
            );
        }
    }

    #[test]
    fn statement_bytecode_executes_declarations_branches_and_loop_backedges() {
        let mut engine = Engine::new();
        for source in [
            "let vmProbe = 0; while (vmProbe < 2) { vmProbe = vmProbe + 1; } vmProbe",
            "let vmForProbe = 0; for (var vmForStep = 0; vmForStep < 2; vmForStep = vmForStep + 1) { vmForProbe = vmForProbe + 1; } vmForProbe",
            "let vmBranchProbe = 0; if (true) vmBranchProbe = 1; else vmBranchProbe = 2; vmBranchProbe",
        ] {
            let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
            let program = crate::parser::Parser::new(tokens).parse_program().unwrap();
            assert!(
                super::Bytecode::compile_statements(&program.statements).is_some(),
                "expected statement bytecode for {source}"
            );
        }
        assert_eq!(
            engine
                .eval(
                    "let vmLoopCount = 0; let vmLoopTotal = 0; while (vmLoopCount < 6) { vmLoopCount = vmLoopCount + 1; if (vmLoopCount === 2) continue; if (vmLoopCount === 5) break; vmLoopTotal = vmLoopTotal + vmLoopCount; } vmLoopTotal"
                )
                .unwrap(),
            Value::Number(8.0)
        );
        assert_eq!(
            engine
                .eval(
                    "let vmForTotal = 0; for (var vmForIndex = 0; vmForIndex < 4; vmForIndex = vmForIndex + 1) { vmForTotal = vmForTotal + vmForIndex; } vmForTotal"
                )
                .unwrap(),
            Value::Number(6.0)
        );
        assert_eq!(
            engine
                .eval(
                    "let vmBranchValue = 0; if (false) vmBranchValue = 1; else vmBranchValue = 2; vmBranchValue"
                )
                .unwrap(),
            Value::Number(2.0)
        );
    }

    #[test]
    fn statement_compiler_rejects_lexical_declarations_in_uninstantiated_branches() {
        let source = "if (true) { let vmBranchLet = 1; }";
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let program = crate::parser::Parser::new(tokens).parse_program().unwrap();
        assert!(super::Bytecode::compile_statements(&program.statements).is_none());
    }

    #[test]
    fn user_function_frames_execute_vm_loops_parameters_closures_and_returns() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval(
                    "function vmFunction(a, b) { var total = a + b; while (total < 10) { total = total + 1; } if (total === 10) return total * 2; return 0; } vmFunction(3, 4)"
                )
                .unwrap(),
            Value::Number(20.0)
        );
        assert_eq!(
            engine
                .eval(
                    "let vmClosureValue = 6; function vmClosureRead() { return vmClosureValue + 1; } vmClosureRead()"
                )
                .unwrap(),
            Value::Number(7.0)
        );
        assert_eq!(
            engine
                .eval("function vmNoReturn() { var local = 1; } vmNoReturn()")
                .unwrap(),
            Value::Undefined
        );
        assert!(
            engine
                .eval("function vmBadReturn() { return missingVmFunctionBinding; } vmBadReturn()")
                .is_err()
        );
    }
}
