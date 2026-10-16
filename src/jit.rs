use std::{collections::HashMap, ffi::c_void, ptr::NonNull};

use crate::{
    Value,
    parser::{BinaryOp, DeclarationKind, Expr, Stmt},
};

const OPTIMIZE_AFTER: u32 = 8;

#[derive(Default)]
pub(crate) struct JitCache {
    entries: HashMap<usize, Entry>,
}

pub(crate) struct FunctionJit {
    variables: Vec<String>,
    baseline: NativeCode,
    tier: FunctionJitTier,
    result: FunctionJitResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FunctionJitTier {
    Baseline,
    Optimizing,
}

#[derive(Clone, Copy)]
enum FunctionJitResult {
    Number,
    Boolean,
}

impl FunctionJit {
    pub(crate) fn compile(parameters: &[String], body: &[Stmt]) -> Option<Self> {
        if let Some(limit) = counted_sum_loop(parameters, body) {
            let (variables, graph) = match limit {
                LoopLimit::Constant(limit) => (Vec::new(), SeaOfNodes::unrolled_sum(limit)?),
                LoopLimit::Parameter(name) => {
                    let slot = parameters
                        .iter()
                        .rposition(|parameter| parameter == &name)?;
                    (vec![name], SeaOfNodes::counted_sum(slot))
                }
            };
            let graph = graph.optimize()?;
            let baseline = NativeCode::compile(&graph.emit()?)?;
            return Some(Self {
                variables,
                baseline,
                tier: FunctionJitTier::Optimizing,
                result: FunctionJitResult::Number,
            });
        }
        let [Stmt::Return(Some(expression))] = body else {
            return None;
        };
        let result = expression_result_kind(expression)?;
        if let Some((code, variables)) = BaselineCompiler::compile(expression)
            && let Some(baseline) = NativeCode::compile(&code)
        {
            return Some(Self {
                variables,
                baseline,
                tier: FunctionJitTier::Baseline,
                result,
            });
        }
        let variables = collect_numeric_variables(expression)?;
        let graph = SeaOfNodes::compile(expression, &variables)?.optimize()?;
        let baseline = NativeCode::compile(&graph.emit()?)?;
        Some(Self {
            variables,
            baseline,
            tier: FunctionJitTier::Optimizing,
            result,
        })
    }

    pub(crate) fn variables(&self) -> &[String] {
        &self.variables
    }

    pub(crate) fn run(&self, inputs: &[f64]) -> Value {
        let result = self.baseline.call(inputs);
        match self.result {
            FunctionJitResult::Number => Value::Number(result),
            FunctionJitResult::Boolean => Value::Boolean(result != 0.0),
        }
    }

    pub(crate) fn tier(&self) -> FunctionJitTier {
        self.tier
    }
}

enum LoopLimit {
    Constant(f64),
    Parameter(String),
}

fn identifier_is(expression: &Expr, expected: &str) -> bool {
    matches!(expression, Expr::Identifier { name, .. } if name == expected)
}

fn counted_sum_loop(parameters: &[String], body: &[Stmt]) -> Option<LoopLimit> {
    let [
        Stmt::Declaration {
            name: accumulator,
            initializer: Some(Expr::Literal(Value::Number(initial_sum))),
            kind: accumulator_kind,
            ..
        },
        Stmt::For {
            initializer: Some(initializer),
            condition: Some(condition),
            update: Some(update),
            body: loop_body,
        },
        Stmt::Return(Some(return_value)),
    ] = body
    else {
        return None;
    };
    if *initial_sum != 0.0
        || !matches!(
            accumulator_kind,
            DeclarationKind::Let | DeclarationKind::Var
        )
        || !identifier_is(return_value, accumulator)
    {
        return None;
    }
    let Stmt::Declaration {
        name: index,
        initializer: Some(Expr::Literal(Value::Number(initial_index))),
        kind: index_kind,
        ..
    } = initializer.as_ref()
    else {
        return None;
    };
    if *initial_index != 0.0
        || !matches!(index_kind, DeclarationKind::Let | DeclarationKind::Var)
        || accumulator == index
        || parameters
            .iter()
            .any(|parameter| parameter == accumulator || parameter == index)
    {
        return None;
    }
    let Expr::Binary {
        operator: BinaryOp::LessThan,
        left,
        right,
    } = condition
    else {
        return None;
    };
    if !identifier_is(left, index) {
        return None;
    }
    let limit = match right.as_ref() {
        Expr::Literal(Value::Number(value)) => LoopLimit::Constant(*value),
        Expr::Identifier { name, .. } if parameters.iter().any(|parameter| parameter == name) => {
            LoopLimit::Parameter(name.clone())
        }
        _ => return None,
    };
    if !matches!(
        update,
        Expr::Update {
            target,
            increment: true,
            ..
        } if identifier_is(target, index)
    ) {
        return None;
    }
    let Stmt::Block(loop_statements) = loop_body.as_ref() else {
        return None;
    };
    let [
        Stmt::Expression(Expr::Assignment {
            target,
            value,
            operator: None,
        }),
    ] = loop_statements.as_slice()
    else {
        return None;
    };
    if !identifier_is(target, accumulator)
        || !matches!(
            value.as_ref(),
            Expr::Binary {
                operator: BinaryOp::Add,
                left,
                right,
            } if identifier_is(left, accumulator) && identifier_is(right, index)
        )
    {
        return None;
    }
    Some(limit)
}

struct Entry {
    executions: u32,
    variables: Vec<String>,
    baseline: NativeCode,
    optimized: Option<NativeCode>,
}

impl JitCache {
    pub(crate) fn prepare(&mut self, key: usize, expression: &Expr) -> Option<Vec<String>> {
        if let std::collections::hash_map::Entry::Vacant(slot) = self.entries.entry(key) {
            let (code, variables) = BaselineCompiler::compile(expression)?;
            let baseline = NativeCode::compile(&code)?;
            slot.insert(Entry {
                executions: 0,
                variables,
                baseline,
                optimized: None,
            });
        }
        self.entries.get(&key).map(|entry| entry.variables.clone())
    }

    pub(crate) fn run(&mut self, key: usize, expression: &Expr, inputs: &[f64]) -> Option<Value> {
        let entry = self.entries.get_mut(&key)?;
        entry.executions = entry.executions.saturating_add(1);
        if entry.executions >= OPTIMIZE_AFTER && entry.optimized.is_none() {
            entry.optimized = Turbovolt::compile(expression, &entry.variables);
        }

        let result = entry
            .optimized
            .as_ref()
            .unwrap_or(&entry.baseline)
            .call(inputs);
        Some(Value::Number(result))
    }
}

struct NativeCode {
    memory: ExecutableMemory,
}

impl NativeCode {
    fn compile(code: &[u8]) -> Option<Self> {
        Some(Self {
            memory: ExecutableMemory::new(code)?,
        })
    }

    fn call(&self, inputs: &[f64]) -> f64 {
        type NativeFunction = unsafe extern "C" fn(*const f64) -> f64;
        // SAFETY: the code buffer is generated by the verified x86_64 emitter
        // and ends with a return instruction matching this function signature.
        let function: NativeFunction = unsafe { std::mem::transmute(self.memory.pointer()) };
        // SAFETY: the function pointer targets executable memory owned by self.
        unsafe { function(inputs.as_ptr()) }
    }
}

struct ExecutableMemory {
    pointer: NonNull<u8>,
    #[cfg(unix)]
    allocation_len: usize,
}

impl ExecutableMemory {
    #[cfg(all(
        target_arch = "x86_64",
        any(
            windows,
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )
    ))]
    fn new(code: &[u8]) -> Option<Self> {
        let allocation_len = code.len().max(1);
        #[cfg(windows)]
        let pointer = {
            const MEM_COMMIT_RESERVE: u32 = 0x3000;
            const PAGE_READWRITE: u32 = 0x04;
            const PAGE_EXECUTE_READ: u32 = 0x20;

            unsafe extern "system" {
                fn VirtualAlloc(
                    address: *mut c_void,
                    size: usize,
                    allocation_type: u32,
                    protection: u32,
                ) -> *mut c_void;
                fn VirtualProtect(
                    address: *mut c_void,
                    size: usize,
                    new_protection: u32,
                    old_protection: *mut u32,
                ) -> i32;
                fn VirtualFree(address: *mut c_void, size: usize, free_type: u32) -> i32;
            }

            let pointer = NonNull::new(unsafe {
                VirtualAlloc(
                    std::ptr::null_mut(),
                    allocation_len,
                    MEM_COMMIT_RESERVE,
                    PAGE_READWRITE,
                )
                .cast::<u8>()
            })?;
            // SAFETY: VirtualAlloc returned a writable allocation of this size.
            unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), pointer.as_ptr(), code.len()) };
            let mut old_protection = 0;
            if unsafe {
                VirtualProtect(
                    pointer.as_ptr().cast(),
                    allocation_len,
                    PAGE_EXECUTE_READ,
                    &mut old_protection,
                )
            } == 0
            {
                unsafe { VirtualFree(pointer.as_ptr().cast(), 0, 0x8000) };
                return None;
            }
            pointer
        };

        #[cfg(unix)]
        let pointer = {
            const PROT_READ: i32 = 0x1;
            const PROT_WRITE: i32 = 0x2;
            const PROT_EXEC: i32 = 0x4;
            const MAP_PRIVATE: i32 = 0x2;
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            const MAP_ANONYMOUS: i32 = 0x1000;
            #[cfg(not(any(target_os = "macos", target_os = "ios")))]
            const MAP_ANONYMOUS: i32 = 0x20;

            unsafe extern "C" {
                fn mmap(
                    address: *mut c_void,
                    length: usize,
                    protection: i32,
                    flags: i32,
                    file: i32,
                    offset: isize,
                ) -> *mut c_void;
                fn mprotect(address: *mut c_void, length: usize, protection: i32) -> i32;
                fn munmap(address: *mut c_void, length: usize) -> i32;
            }

            let pointer = NonNull::new(unsafe {
                mmap(
                    std::ptr::null_mut(),
                    allocation_len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS,
                    -1,
                    0,
                )
                .cast::<u8>()
            })?;
            if pointer.as_ptr() as isize == -1 {
                return None;
            }
            // SAFETY: mmap returned a writable allocation of this size.
            unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), pointer.as_ptr(), code.len()) };
            if unsafe {
                mprotect(
                    pointer.as_ptr().cast(),
                    allocation_len,
                    PROT_READ | PROT_EXEC,
                )
            } != 0
            {
                unsafe { munmap(pointer.as_ptr().cast(), allocation_len) };
                return None;
            }
            pointer
        };

        Some(Self {
            pointer,
            #[cfg(unix)]
            allocation_len,
        })
    }

    #[cfg(not(all(
        target_arch = "x86_64",
        any(
            windows,
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )
    )))]
    fn new(_: &[u8]) -> Option<Self> {
        None
    }

    fn pointer(&self) -> *const u8 {
        self.pointer.as_ptr()
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn VirtualFree(address: *mut c_void, size: usize, free_type: u32) -> i32;
            }
            // SAFETY: this pointer was allocated by VirtualAlloc.
            unsafe { VirtualFree(self.pointer.as_ptr().cast(), 0, 0x8000) };
        }

        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn munmap(address: *mut c_void, length: usize) -> i32;
            }
            // SAFETY: this pointer and length were returned by mmap.
            unsafe { munmap(self.pointer.as_ptr().cast(), self.allocation_len) };
        }
    }
}

struct BaselineCompiler {
    code: Vec<u8>,
    stack_depth: usize,
    variables: Vec<String>,
    variable_slots: HashMap<String, usize>,
}

impl BaselineCompiler {
    fn compile(expression: &Expr) -> Option<(Vec<u8>, Vec<String>)> {
        let mut compiler = Self {
            code: Vec::new(),
            stack_depth: 0,
            variables: Vec::new(),
            variable_slots: HashMap::new(),
        };
        compiler.expression(expression)?;
        if compiler.stack_depth != 1 {
            return None;
        }
        compiler.code.extend_from_slice(&[
            0xF2, 0x0F, 0x10, 0x04, 0x24, // movsd xmm0, [rsp]
            0x48, 0x83, 0xC4, 0x08, // add rsp, 8
            0xC3, // ret
        ]);
        Some((compiler.code, compiler.variables))
    }

    fn expression(&mut self, expression: &Expr) -> Option<()> {
        match expression {
            Expr::Literal(Value::Number(value)) => self.push_number(*value),
            Expr::Identifier { name, .. } => {
                let slot = if let Some(&slot) = self.variable_slots.get(name) {
                    slot
                } else {
                    let slot = self.variables.len();
                    self.variables.push(name.clone());
                    self.variable_slots.insert(name.clone(), slot);
                    slot
                };
                self.push_variable(slot)
            }
            Expr::Unary {
                operator: crate::parser::UnaryOp::Minus,
                operand,
                ..
            } => {
                self.expression(operand)?;
                self.code.extend_from_slice(&[
                    0xF2, 0x0F, 0x10, 0x04, 0x24, // movsd xmm0, [rsp]
                    0x48, 0xB8, // mov rax, imm64
                ]);
                self.code
                    .extend_from_slice(&0x8000_0000_0000_0000_u64.to_le_bytes());
                self.code.extend_from_slice(&[
                    0x66, 0x48, 0x0F, 0x6E, 0xC8, // movq xmm1, rax
                    0x66, 0x0F, 0x57, 0xC1, // xorpd xmm0, xmm1
                    0xF2, 0x0F, 0x11, 0x04, 0x24, // movsd [rsp], xmm0
                ]);
                Some(())
            }
            Expr::Binary {
                operator,
                left,
                right,
            } if matches!(
                operator,
                crate::parser::BinaryOp::Add
                    | crate::parser::BinaryOp::Subtract
                    | crate::parser::BinaryOp::Multiply
                    | crate::parser::BinaryOp::Divide
            ) =>
            {
                self.expression(left)?;
                self.expression(right)?;
                self.code.extend_from_slice(&[
                    0xF2, 0x0F, 0x10, 0x44, 0x24, 0x08, // movsd xmm0, [rsp + 8]
                ]);
                match operator {
                    crate::parser::BinaryOp::Add => self.code.extend_from_slice(&[
                        0xF2, 0x0F, 0x58, 0x04, 0x24, // addsd xmm0, [rsp]
                    ]),
                    crate::parser::BinaryOp::Subtract => self.code.extend_from_slice(&[
                        0xF2, 0x0F, 0x5C, 0x04, 0x24, // subsd xmm0, [rsp]
                    ]),
                    crate::parser::BinaryOp::Multiply => self.code.extend_from_slice(&[
                        0xF2, 0x0F, 0x59, 0x04, 0x24, // mulsd xmm0, [rsp]
                    ]),
                    crate::parser::BinaryOp::Divide => self.code.extend_from_slice(&[
                        0xF2, 0x0F, 0x5E, 0x04, 0x24, // divsd xmm0, [rsp]
                    ]),
                    _ => unreachable!(),
                }
                self.code.extend_from_slice(&[
                    0x48, 0x83, 0xC4, 0x10, // add rsp, 16
                    0x48, 0x83, 0xEC, 0x08, // sub rsp, 8
                    0xF2, 0x0F, 0x11, 0x04, 0x24, // movsd [rsp], xmm0
                ]);
                self.stack_depth -= 1;
                Some(())
            }
            _ => None,
        }
    }

    fn push_number(&mut self, value: f64) -> Option<()> {
        self.code.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
        self.code.extend_from_slice(&value.to_bits().to_le_bytes());
        self.code.push(0x50); // push rax
        self.stack_depth += 1;
        Some(())
    }

    fn push_variable(&mut self, slot: usize) -> Option<()> {
        let offset = i32::try_from(slot.checked_mul(std::mem::size_of::<f64>())?).ok()?;
        #[cfg(windows)]
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x10, 0x81]); // movsd xmm0, [rcx + disp32]
        #[cfg(not(windows))]
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x10, 0x87]); // movsd xmm0, [rdi + disp32]
        self.code.extend_from_slice(&offset.to_le_bytes());
        self.code.extend_from_slice(&[
            0x48, 0x83, 0xEC, 0x08, // sub rsp, 8
            0xF2, 0x0F, 0x11, 0x04, 0x24, // movsd [rsp], xmm0
        ]);
        self.stack_depth += 1;
        Some(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Node {
    Constant(u64),
    Add(usize, usize),
    Subtract(usize, usize),
    Multiply(usize, usize),
    Divide(usize, usize),
    Negate(usize),
    Variable(usize),
    Conditional(usize, usize, usize),
    LogicalAnd(usize, usize),
    LogicalOr(usize, usize),
    LessThan(usize, usize),
    LessThanOrEqual(usize, usize),
    GreaterThan(usize, usize),
    GreaterThanOrEqual(usize, usize),
    StrictEqual(usize, usize),
    StrictNotEqual(usize, usize),
    CountedSum(usize),
}

struct SeaOfNodes {
    nodes: Vec<Node>,
    root: usize,
}

struct Turbovolt;

impl Turbovolt {
    fn compile(expression: &Expr, variables: &[String]) -> Option<NativeCode> {
        let graph = SeaOfNodes::compile(expression, variables)?.optimize()?;
        NativeCode::compile(&graph.emit()?)
    }
}

impl SeaOfNodes {
    fn counted_sum(limit_slot: usize) -> Self {
        Self {
            nodes: vec![Node::CountedSum(limit_slot)],
            root: 0,
        }
    }

    fn unrolled_sum(limit: f64) -> Option<Self> {
        if !limit.is_finite() || !(0.0..=16.0).contains(&limit) {
            return None;
        }
        let mut nodes = vec![Node::Constant(0.0_f64.to_bits())];
        let mut sum = 0;
        for index in 0..limit.ceil() as usize {
            let value = nodes.len();
            nodes.push(Node::Constant((index as f64).to_bits()));
            let next = nodes.len();
            nodes.push(Node::Add(sum, value));
            sum = next;
        }
        Some(Self { nodes, root: sum })
    }

    fn compile(expression: &Expr, variables: &[String]) -> Option<Self> {
        let mut graph = Self {
            nodes: Vec::new(),
            root: 0,
        };
        let variable_slots = variables
            .iter()
            .enumerate()
            .map(|(slot, name)| (name.as_str(), slot))
            .collect();
        graph.root = graph.expression(expression, &variable_slots)?;
        Some(graph)
    }

    fn expression(
        &mut self,
        expression: &Expr,
        variable_slots: &HashMap<&str, usize>,
    ) -> Option<usize> {
        let node = match expression {
            Expr::Literal(Value::Number(value)) => Node::Constant(value.to_bits()),
            Expr::Identifier { name, .. } => Node::Variable(*variable_slots.get(name.as_str())?),
            Expr::Unary {
                operator: crate::parser::UnaryOp::Minus,
                operand,
                ..
            } => {
                let operand = self.expression(operand, variable_slots)?;
                Node::Negate(operand)
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.expression(left, variable_slots)?;
                let right = self.expression(right, variable_slots)?;
                match operator {
                    crate::parser::BinaryOp::Add => Node::Add(left, right),
                    crate::parser::BinaryOp::Subtract => Node::Subtract(left, right),
                    crate::parser::BinaryOp::Multiply => Node::Multiply(left, right),
                    crate::parser::BinaryOp::Divide => Node::Divide(left, right),
                    crate::parser::BinaryOp::And => Node::LogicalAnd(left, right),
                    crate::parser::BinaryOp::Or => Node::LogicalOr(left, right),
                    crate::parser::BinaryOp::LessThan => Node::LessThan(left, right),
                    crate::parser::BinaryOp::LessThanOrEqual => Node::LessThanOrEqual(left, right),
                    crate::parser::BinaryOp::GreaterThan => Node::GreaterThan(left, right),
                    crate::parser::BinaryOp::GreaterThanOrEqual => {
                        Node::GreaterThanOrEqual(left, right)
                    }
                    crate::parser::BinaryOp::StrictEqual => Node::StrictEqual(left, right),
                    crate::parser::BinaryOp::StrictNotEqual => Node::StrictNotEqual(left, right),
                    _ => return None,
                }
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => Node::Conditional(
                self.expression(test, variable_slots)?,
                self.expression(consequent, variable_slots)?,
                self.expression(alternate, variable_slots)?,
            ),
            _ => return None,
        };
        let index = self.nodes.len();
        self.nodes.push(node);
        Some(index)
    }

    fn optimize(self) -> Option<Self> {
        let root = self.root;
        let mut replacements = Vec::with_capacity(self.nodes.len());
        let mut unique = HashMap::new();
        let mut optimized = Vec::new();

        for node in self.nodes {
            let node = match node {
                Node::Add(left, right) => {
                    fold_binary(&optimized, replacements[left], replacements[right], add_f64)
                        .unwrap_or(Node::Add(replacements[left], replacements[right]))
                }
                Node::Subtract(left, right) => fold_binary(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    subtract_f64,
                )
                .unwrap_or(Node::Subtract(replacements[left], replacements[right])),
                Node::Multiply(left, right) => fold_binary(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    multiply_f64,
                )
                .unwrap_or(Node::Multiply(replacements[left], replacements[right])),
                Node::Divide(left, right) => fold_binary(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    divide_f64,
                )
                .unwrap_or(Node::Divide(replacements[left], replacements[right])),
                Node::Negate(value) => match optimized[replacements[value]] {
                    Node::Constant(bits) => Node::Constant((-f64::from_bits(bits)).to_bits()),
                    _ => Node::Negate(replacements[value]),
                },
                Node::Conditional(test, consequent, alternate) => {
                    let test = replacements[test];
                    let consequent = replacements[consequent];
                    let alternate = replacements[alternate];
                    match optimized[test] {
                        Node::Constant(bits) if f64::from_bits(bits) == 0.0 => optimized[alternate],
                        Node::Constant(bits) if !f64::from_bits(bits).is_nan() => {
                            optimized[consequent]
                        }
                        _ => Node::Conditional(test, consequent, alternate),
                    }
                }
                Node::LogicalAnd(left, right) => match optimized[replacements[left]] {
                    Node::Constant(bits) if !number_is_truthy(f64::from_bits(bits)) => {
                        optimized[replacements[left]]
                    }
                    Node::Constant(_) => optimized[replacements[right]],
                    _ => Node::LogicalAnd(replacements[left], replacements[right]),
                },
                Node::LogicalOr(left, right) => match optimized[replacements[left]] {
                    Node::Constant(bits) if number_is_truthy(f64::from_bits(bits)) => {
                        optimized[replacements[left]]
                    }
                    Node::Constant(_) => optimized[replacements[right]],
                    _ => Node::LogicalOr(replacements[left], replacements[right]),
                },
                Node::LessThan(left, right) => fold_comparison(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    |left, right| left < right,
                )
                .unwrap_or(Node::LessThan(replacements[left], replacements[right])),
                Node::LessThanOrEqual(left, right) => fold_comparison(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    |left, right| left <= right,
                )
                .unwrap_or(Node::LessThanOrEqual(
                    replacements[left],
                    replacements[right],
                )),
                Node::GreaterThan(left, right) => fold_comparison(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    |left, right| left > right,
                )
                .unwrap_or(Node::GreaterThan(replacements[left], replacements[right])),
                Node::GreaterThanOrEqual(left, right) => fold_comparison(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    |left, right| left >= right,
                )
                .unwrap_or(Node::GreaterThanOrEqual(
                    replacements[left],
                    replacements[right],
                )),
                Node::StrictEqual(left, right) => fold_comparison(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    |left, right| left == right,
                )
                .unwrap_or(Node::StrictEqual(replacements[left], replacements[right])),
                Node::StrictNotEqual(left, right) => fold_comparison(
                    &optimized,
                    replacements[left],
                    replacements[right],
                    |left, right| left != right,
                )
                .unwrap_or(Node::StrictNotEqual(
                    replacements[left],
                    replacements[right],
                )),
                Node::CountedSum(limit) => Node::CountedSum(limit),
                constant => constant,
            };

            let index = if let Some(&existing) = unique.get(&node) {
                existing
            } else {
                let index = optimized.len();
                optimized.push(node);
                unique.insert(node, index);
                index
            };
            replacements.push(index);
        }

        let root = *replacements.get(root)?;
        let graph = Self {
            nodes: optimized,
            root,
        };
        graph.proves_no_object_allocations().then_some(graph)
    }

    fn proves_no_object_allocations(&self) -> bool {
        self.nodes.iter().all(|node| {
            matches!(
                node,
                Node::Constant(_)
                    | Node::Add(_, _)
                    | Node::Subtract(_, _)
                    | Node::Multiply(_, _)
                    | Node::Divide(_, _)
                    | Node::Negate(_)
                    | Node::Conditional(_, _, _)
                    | Node::LogicalAnd(_, _)
                    | Node::LogicalOr(_, _)
                    | Node::LessThan(_, _)
                    | Node::LessThanOrEqual(_, _)
                    | Node::GreaterThan(_, _)
                    | Node::GreaterThanOrEqual(_, _)
                    | Node::StrictEqual(_, _)
                    | Node::StrictNotEqual(_, _)
                    | Node::CountedSum(_)
                    | Node::Variable(_)
            )
        })
    }

    fn emit(&self) -> Option<Vec<u8>> {
        let mut compiler = BaselineCompiler {
            code: Vec::new(),
            stack_depth: 0,
            variables: Vec::new(),
            variable_slots: HashMap::new(),
        };
        self.emit_node(self.root, &mut compiler)?;
        if compiler.stack_depth != 1 {
            return None;
        }
        compiler
            .code
            .extend_from_slice(&[0xF2, 0x0F, 0x10, 0x04, 0x24, 0x48, 0x83, 0xC4, 0x08, 0xC3]);
        Some(compiler.code)
    }

    fn emit_node(&self, index: usize, compiler: &mut BaselineCompiler) -> Option<()> {
        match self.nodes[index] {
            Node::Constant(bits) => compiler.push_number(f64::from_bits(bits)),
            Node::Add(left, right)
            | Node::Subtract(left, right)
            | Node::Multiply(left, right)
            | Node::Divide(left, right) => {
                compiler.emit_operation(self, left, right, self.nodes[index])
            }
            Node::Negate(value) => {
                self.emit_node(value, compiler)?;
                compiler.emit_negate();
                Some(())
            }
            Node::Variable(slot) => compiler.push_variable(slot),
            Node::Conditional(test, consequent, alternate) => {
                compiler.emit_conditional(self, test, consequent, alternate)
            }
            Node::LogicalAnd(left, right) => compiler.emit_conditional(self, left, right, left),
            Node::LogicalOr(left, right) => compiler.emit_conditional(self, left, left, right),
            Node::LessThan(left, right)
            | Node::LessThanOrEqual(left, right)
            | Node::GreaterThan(left, right)
            | Node::GreaterThanOrEqual(left, right)
            | Node::StrictEqual(left, right)
            | Node::StrictNotEqual(left, right) => {
                compiler.emit_comparison(self, left, right, self.nodes[index])
            }
            Node::CountedSum(limit_slot) => compiler.emit_counted_sum(limit_slot),
        }
    }
}

fn number_is_truthy(value: f64) -> bool {
    value != 0.0 && !value.is_nan()
}

fn collect_numeric_variables(expression: &Expr) -> Option<Vec<String>> {
    fn collect(expression: &Expr, variables: &mut Vec<String>) -> Option<()> {
        match expression {
            Expr::Literal(Value::Number(_)) => Some(()),
            Expr::Identifier { name, .. } => {
                if !variables.contains(name) {
                    variables.push(name.clone());
                }
                Some(())
            }
            Expr::Unary {
                operator: crate::parser::UnaryOp::Minus,
                operand,
                ..
            } => collect(operand, variables),
            Expr::Binary {
                operator:
                    crate::parser::BinaryOp::Add
                    | crate::parser::BinaryOp::Subtract
                    | crate::parser::BinaryOp::Multiply
                    | crate::parser::BinaryOp::Divide
                    | crate::parser::BinaryOp::And
                    | crate::parser::BinaryOp::Or
                    | crate::parser::BinaryOp::LessThan
                    | crate::parser::BinaryOp::LessThanOrEqual
                    | crate::parser::BinaryOp::GreaterThan
                    | crate::parser::BinaryOp::GreaterThanOrEqual
                    | crate::parser::BinaryOp::StrictEqual
                    | crate::parser::BinaryOp::StrictNotEqual,
                left,
                right,
            } => {
                collect(left, variables)?;
                collect(right, variables)
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                collect(test, variables)?;
                collect(consequent, variables)?;
                collect(alternate, variables)
            }
            _ => None,
        }
    }

    let mut variables = Vec::new();
    collect(expression, &mut variables)?;
    Some(variables)
}

fn expression_result_kind(expression: &Expr) -> Option<FunctionJitResult> {
    match expression {
        Expr::Literal(Value::Number(_)) | Expr::Identifier { .. } => {
            Some(FunctionJitResult::Number)
        }
        Expr::Unary {
            operator: crate::parser::UnaryOp::Minus,
            operand,
            ..
        } if matches!(expression_result_kind(operand)?, FunctionJitResult::Number) => {
            Some(FunctionJitResult::Number)
        }
        Expr::Binary {
            operator:
                crate::parser::BinaryOp::Add
                | crate::parser::BinaryOp::Subtract
                | crate::parser::BinaryOp::Multiply
                | crate::parser::BinaryOp::Divide,
            left,
            right,
        } => {
            expression_result_kind(left)?;
            expression_result_kind(right)?;
            Some(FunctionJitResult::Number)
        }
        Expr::Binary {
            operator:
                crate::parser::BinaryOp::LessThan
                | crate::parser::BinaryOp::LessThanOrEqual
                | crate::parser::BinaryOp::GreaterThan
                | crate::parser::BinaryOp::GreaterThanOrEqual
                | crate::parser::BinaryOp::StrictEqual
                | crate::parser::BinaryOp::StrictNotEqual,
            left,
            right,
        } => {
            if !matches!(expression_result_kind(left)?, FunctionJitResult::Number)
                || !matches!(expression_result_kind(right)?, FunctionJitResult::Number)
            {
                return None;
            }
            Some(FunctionJitResult::Boolean)
        }
        Expr::Binary {
            operator: crate::parser::BinaryOp::And | crate::parser::BinaryOp::Or,
            left,
            right,
        } => {
            let left_kind = expression_result_kind(left)?;
            let right_kind = expression_result_kind(right)?;
            match (left_kind, right_kind) {
                (FunctionJitResult::Number, FunctionJitResult::Number) => {
                    Some(FunctionJitResult::Number)
                }
                (FunctionJitResult::Boolean, FunctionJitResult::Boolean) => {
                    Some(FunctionJitResult::Boolean)
                }
                _ => None,
            }
        }
        Expr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_result_kind(test)?;
            let consequent = expression_result_kind(consequent)?;
            let alternate = expression_result_kind(alternate)?;
            match (consequent, alternate) {
                (FunctionJitResult::Number, FunctionJitResult::Number) => {
                    Some(FunctionJitResult::Number)
                }
                (FunctionJitResult::Boolean, FunctionJitResult::Boolean) => {
                    Some(FunctionJitResult::Boolean)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

impl BaselineCompiler {
    fn emit_counted_sum(&mut self, limit_slot: usize) -> Option<()> {
        if self.stack_depth != 0 || limit_slot != 0 {
            return None;
        }
        self.code.extend_from_slice(&[
            0x0F, 0x57, 0xC0, // xorps xmm0, xmm0
            0x0F, 0x57, 0xC9, // xorps xmm1, xmm1
        ]);
        #[cfg(windows)]
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x10, 0x91]); // movsd xmm2, [rcx + disp32]
        #[cfg(not(windows))]
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x10, 0x97]); // movsd xmm2, [rdi + disp32]
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[
            0x48, 0xB8, // mov rax, 1.0 bits
        ]);
        self.code
            .extend_from_slice(&1.0_f64.to_bits().to_le_bytes());
        self.code.extend_from_slice(&[0x66, 0x48, 0x0F, 0x6E, 0xD8]); // movq xmm3, rax

        let loop_start = self.code.len();
        self.code.extend_from_slice(&[
            0x66, 0x0F, 0x2E, 0xCA, // ucomisd xmm1, xmm2
            0x0F, 0x8A, // jp exit (NaN limit)
        ]);
        let unordered_exit = self.emit_rel32_placeholder();
        self.code.extend_from_slice(&[0x0F, 0x83]); // jae exit
        let bound_exit = self.emit_rel32_placeholder();
        self.code.extend_from_slice(&[
            0xF2, 0x0F, 0x58, 0xC1, // addsd xmm0, xmm1
            0xF2, 0x0F, 0x58, 0xCB, // addsd xmm1, xmm3
            0xE9, // jmp loop
        ]);
        let backedge = self.emit_rel32_placeholder();
        let exit = self.code.len();
        self.patch_rel32(unordered_exit, exit)?;
        self.patch_rel32(bound_exit, exit)?;
        self.patch_rel32(backedge, loop_start)?;
        self.code.extend_from_slice(&[
            0x48, 0x83, 0xEC, 0x08, // sub rsp, 8
            0xF2, 0x0F, 0x11, 0x04, 0x24, // movsd [rsp], xmm0
        ]);
        self.stack_depth = 1;
        Some(())
    }

    fn emit_comparison(
        &mut self,
        graph: &SeaOfNodes,
        left: usize,
        right: usize,
        operation: Node,
    ) -> Option<()> {
        graph.emit_node(left, self)?;
        graph.emit_node(right, self)?;
        self.code.extend_from_slice(&[
            0xF2, 0x0F, 0x10, 0x44, 0x24, 0x08, // movsd xmm0, [rsp + 8]
            0x66, 0x0F, 0x2E, 0x04, 0x24, // ucomisd xmm0, [rsp]
        ]);
        match operation {
            Node::LessThan(_, _) | Node::LessThanOrEqual(_, _) => {
                let setcc = if matches!(operation, Node::LessThan(_, _)) {
                    0x92
                } else {
                    0x96
                };
                self.code.extend_from_slice(&[
                    0x0F, setcc, 0xC0, // setb/setbe al
                    0x0F, 0x9B, 0xC2, // setnp dl
                    0x20, 0xD0, // and al, dl
                ]);
            }
            Node::GreaterThan(_, _) | Node::GreaterThanOrEqual(_, _) => {
                let setcc = if matches!(operation, Node::GreaterThan(_, _)) {
                    0x97
                } else {
                    0x93
                };
                self.code.extend_from_slice(&[0x0F, setcc, 0xC0]); // seta/setae al
            }
            Node::StrictEqual(_, _) => self.code.extend_from_slice(&[
                0x0F, 0x94, 0xC0, // sete al
                0x0F, 0x9B, 0xC2, // setnp dl
                0x20, 0xD0, // and al, dl
            ]),
            Node::StrictNotEqual(_, _) => self.code.extend_from_slice(&[
                0x0F, 0x95, 0xC0, // setne al
                0x0F, 0x9A, 0xC2, // setp dl
                0x08, 0xD0, // or al, dl
            ]),
            _ => return None,
        }
        self.code.extend_from_slice(&[
            0x0F, 0xB6, 0xC0, // movzx eax, al
            0xF2, 0x0F, 0x2A, 0xC0, // cvtsi2sd xmm0, eax
            0x48, 0x83, 0xC4, 0x10, // add rsp, 16
            0x48, 0x83, 0xEC, 0x08, // sub rsp, 8
            0xF2, 0x0F, 0x11, 0x04, 0x24, // movsd [rsp], xmm0
        ]);
        self.stack_depth -= 1;
        Some(())
    }

    fn emit_conditional(
        &mut self,
        graph: &SeaOfNodes,
        test: usize,
        consequent: usize,
        alternate: usize,
    ) -> Option<()> {
        let base_depth = self.stack_depth;
        graph.emit_node(test, self)?;
        self.code.extend_from_slice(&[
            0xF2, 0x0F, 0x10, 0x04, 0x24, // movsd xmm0, [rsp]
            0x48, 0x83, 0xC4, 0x08, // add rsp, 8
            0x0F, 0x57, 0xC9, // xorps xmm1, xmm1
            0x66, 0x0F, 0x2E, 0xC1, // ucomisd xmm0, xmm1
            0x0F, 0x85, // jne consequent
        ]);
        self.stack_depth -= 1;
        let consequent_jump = self.emit_rel32_placeholder();

        graph.emit_node(alternate, self)?;
        if self.stack_depth != base_depth + 1 {
            return None;
        }
        self.code.push(0xE9); // jmp end
        let end_jump = self.emit_rel32_placeholder();

        self.patch_rel32(consequent_jump, self.code.len())?;
        self.stack_depth = base_depth;
        graph.emit_node(consequent, self)?;
        if self.stack_depth != base_depth + 1 {
            return None;
        }
        self.patch_rel32(end_jump, self.code.len())?;
        Some(())
    }

    fn emit_rel32_placeholder(&mut self) -> usize {
        let operand = self.code.len();
        self.code.extend_from_slice(&[0; 4]);
        operand
    }

    fn patch_rel32(&mut self, operand: usize, target: usize) -> Option<()> {
        let target = i64::try_from(target).ok()?;
        let next_instruction = i64::try_from(operand.checked_add(4)?).ok()?;
        let displacement = i32::try_from(target - next_instruction).ok()?;
        self.code[operand..operand + 4].copy_from_slice(&displacement.to_le_bytes());
        Some(())
    }

    fn emit_operation(
        &mut self,
        graph: &SeaOfNodes,
        left: usize,
        right: usize,
        operation: Node,
    ) -> Option<()> {
        graph.emit_node(left, self)?;
        graph.emit_node(right, self)?;
        self.emit_binary(operation);
        Some(())
    }

    fn emit_negate(&mut self) {
        self.code
            .extend_from_slice(&[0xF2, 0x0F, 0x10, 0x04, 0x24, 0x48, 0xB8]);
        self.code
            .extend_from_slice(&0x8000_0000_0000_0000_u64.to_le_bytes());
        self.code.extend_from_slice(&[
            0x66, 0x48, 0x0F, 0x6E, 0xC8, 0x66, 0x0F, 0x57, 0xC1, 0xF2, 0x0F, 0x11, 0x04, 0x24,
        ]);
    }

    fn emit_binary(&mut self, operation: Node) {
        self.code
            .extend_from_slice(&[0xF2, 0x0F, 0x10, 0x44, 0x24, 0x08]);
        match operation {
            Node::Add(_, _) => self.code.extend_from_slice(&[0xF2, 0x0F, 0x58, 0x04, 0x24]),
            Node::Subtract(_, _) => self.code.extend_from_slice(&[0xF2, 0x0F, 0x5C, 0x04, 0x24]),
            Node::Multiply(_, _) => self.code.extend_from_slice(&[0xF2, 0x0F, 0x59, 0x04, 0x24]),
            Node::Divide(_, _) => self.code.extend_from_slice(&[0xF2, 0x0F, 0x5E, 0x04, 0x24]),
            _ => unreachable!(),
        }
        self.code.extend_from_slice(&[
            0x48, 0x83, 0xC4, 0x10, 0x48, 0x83, 0xEC, 0x08, 0xF2, 0x0F, 0x11, 0x04, 0x24,
        ]);
        self.stack_depth -= 1;
    }
}

fn add_f64(left: f64, right: f64) -> f64 {
    left + right
}

fn subtract_f64(left: f64, right: f64) -> f64 {
    left - right
}

fn multiply_f64(left: f64, right: f64) -> f64 {
    left * right
}

fn divide_f64(left: f64, right: f64) -> f64 {
    left / right
}

fn fold_binary(
    nodes: &[Node],
    left: usize,
    right: usize,
    operation: fn(f64, f64) -> f64,
) -> Option<Node> {
    match (nodes.get(left)?, nodes.get(right)?) {
        (Node::Constant(left), Node::Constant(right)) => Some(Node::Constant(
            operation(f64::from_bits(*left), f64::from_bits(*right)).to_bits(),
        )),
        _ => None,
    }
}

fn fold_comparison(
    nodes: &[Node],
    left: usize,
    right: usize,
    operation: fn(f64, f64) -> bool,
) -> Option<Node> {
    match (nodes.get(left)?, nodes.get(right)?) {
        (Node::Constant(left), Node::Constant(right)) => {
            let result: f64 = if operation(f64::from_bits(*left), f64::from_bits(*right)) {
                1.0
            } else {
                0.0
            };
            Some(Node::Constant(result.to_bits()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Engine, Value};

    #[test]
    fn counted_sum_pattern_is_recognized() {
        let source = "function tierDynamicSum(limit) { let total = 0; for (let index = 0; index < limit; index++) { total = total + index; } return total; }";
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let program = crate::parser::Parser::new(tokens).parse_program().unwrap();
        let crate::parser::Stmt::FunctionDeclaration {
            parameters, body, ..
        } = &program.statements[0]
        else {
            panic!("expected function declaration");
        };
        assert!(super::counted_sum_loop(parameters, body).is_some());
    }

    #[test]
    fn native_jit_preserves_binary64_arithmetic() {
        let source = "((0.1 + 0.2) * 3.0) / 2.0";
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let program = crate::parser::Parser::new(tokens).parse_program().unwrap();
        let crate::parser::Stmt::Expression(expression) = &program.statements[0] else {
            panic!("expected an expression statement");
        };
        let key = expression as *const crate::parser::Expr as usize;
        let mut cache = super::JitCache::default();
        let variables = cache.prepare(key, expression).unwrap();
        assert!(variables.is_empty());
        let expected = Value::Number(((0.1_f64 + 0.2) * 3.0) / 2.0);
        for _ in 0..12 {
            assert_eq!(cache.run(key, expression, &[]), Some(expected.clone()));
        }
    }

    #[test]
    fn native_jit_loads_current_numeric_bindings_in_both_tiers() {
        let source = "jitX * 3 + jitX";
        let tokens = crate::lexer::Lexer::new(source).tokenize().unwrap();
        let program = crate::parser::Parser::new(tokens).parse_program().unwrap();
        let crate::parser::Stmt::Expression(expression) = &program.statements[0] else {
            panic!("expected an expression statement");
        };
        let key = expression as *const crate::parser::Expr as usize;
        let mut cache = super::JitCache::default();
        assert_eq!(
            cache.prepare(key, expression),
            Some(vec!["jitX".to_owned()])
        );
        for value in 1..=12 {
            let input = [f64::from(value)];
            assert_eq!(
                cache.run(key, expression, &input),
                Some(Value::Number(input[0] * 4.0))
            );
        }
    }

    #[test]
    fn non_numeric_and_object_expressions_fall_back_to_the_interpreter() {
        let mut engine = Engine::new();
        assert_eq!(
            engine.eval("let jitNumeric = 4; jitNumeric * 3").unwrap(),
            Value::Number(12.0)
        );
        assert_eq!(
            engine.eval("let jitString = '2'; jitString * 3").unwrap(),
            Value::Number(6.0)
        );
        assert_eq!(engine.eval("[1, 2].length").unwrap(), Value::Number(2.0));
    }
}
