# Volt

A from-scratch JavaScript interpreter written in Rust. The project owns its
tokenizer, parser, runtime values, scopes, and evaluator; it does not embed
Boa, V8, or another JavaScript engine.

This is an early implementation milestone, **not a complete JavaScript
engine and not ES2020-conformant**. The longer-term target is headless ES2020,
measured against a pinned Test262 revision. Browser and server APIs are not
part of the runtime target.

## Current implementation

The interpreter currently supports:

- `undefined`, `null`, booleans, decimal numbers, and single-/double-quoted strings
- `let` and `const` block bindings, function/global-scoped `var`, declaration
  hoisting, lexical TDZ checks, and function declarations
- arithmetic, exponentiation, bitwise, shift, comparison, equality, logical,
  nullish-coalescing, and conditional operators
- simple and selected compound assignments, prefix/postfix increment and
  decrement, and assignment to identifiers and object properties
- unary `!`, `+`, `-`, `~`, `typeof`, `void`, and `delete`
- blocks, `if`/`else`, `while`, `do...while`, C-style `for`, `break`, and
  `continue`, plus array/string `for...of` and object `for...in`
- strict-mode directives and selected strict-mode early/runtime errors
- function declarations and expressions, parameters, return, closures,
  function names/length, `arguments`, and strict/sloppy direct-call `this`
- object literals, property reads/writes, constructor calls, prototype lookup,
  `in`, `instanceof`, and `Object.create`, `Object.getPrototypeOf`, and
  `Object.setPrototypeOf`, basic data-property descriptors and integrity
  operations, plus `Object.keys`, `getOwnPropertyNames`, `values`, `entries`,
  `assign`, `fromEntries`, `hasOwn`, and `is`
- a basic string-keyed `Reflect` subset for apply/construct, property access,
  definition, deletion, own keys, prototype operations, and extensibility
- sparse array literals and basic `Array` construction, indexed properties,
  mutable `length`, and common array methods (`push`, `pop`, `shift`,
  `unshift`, `splice`, `slice`, `concat`, `join`, `reverse`, `flat`, `flatMap`,
  searches, callback methods, and explicit `keys()`, `values()`, and `entries()`
  iterators
- basic insertion-ordered `Map` and `Set` storage, core lookup and mutation
  methods, `forEach`, mutation-aware `keys()`, `values()`, and `entries()`
  iterators, and iteration in `for...of` and `Array.from`
- basic `WeakMap` and `WeakSet` construction and object-keyed lookup/mutation
  methods
- core `Date` time values, numeric/component/supported ISO-string construction,
  UTC getters/setters, UTC-based local-time aliases, `setTime`, `Date.now`,
  `Date.parse`, `Date.UTC`, and ISO output
- ECMAScript-oriented `RegExp` construction, `test`, `exec` captures and named
  groups, `d` match indices, Unicode-aware matching, lookarounds, backreferences,
  supported flags (including `u` and `v`), and global/sticky `lastIndex`
  tracking, backed by the `regress` engine
- selected `String`, `Number`, and `Boolean` conversions and prototype methods,
  `Math`, global numeric conversion helpers, and `JSON.parse`/`JSON.stringify`
  with reviver, replacer, and indentation arguments
- `try`/`catch`/`finally`, explicit throws, and basic `Error`,
  `EvalError`, `RangeError`, `ReferenceError`, `SyntaxError`, `TypeError`,
  and `URIError` objects
- line and block comments
- AVX2-accelerated x86 and NEON-accelerated AArch64 scanning of ASCII
  identifier runs and horizontal whitespace, with a portable scalar fallback;
  identifier and operator token text borrows the source instead of allocating
  per token
- a compact 16-bit-word bytecode VM for literals, binding reads/writes, unary
  and binary expressions, short-circuit/conditional branches, and a subset of
  statement control flow (`if`, `while`, and classic `for`); it has function-
  table dispatch, guarded binding inline caches, and numeric operand type
  feedback; supported user functions tier from AST interpretation to bytecode
  after three calls or 32 interpreted loop iterations, then supported simple
  numeric return expressions can run in the baseline native tier; unsupported
  forms continue through the AST evaluator or VM
- an optimizing native-IR path for one precisely recognized numeric counted
  sum-loop shape; parameter-bounded loops emit native control flow, while
  constant bounds up to 16 are unrolled and folded at compile time
- a two-tier x86-64 native JIT for side-effect-free numeric expressions using
  numeric literals and numeric bindings: a baseline template emitter runs on
  first use, then the Turbovolt Sea-of-Nodes backend applies constant folding,
  common-subexpression elimination, numeric comparisons and conditional/logical
  control flow, and an allocation-free IR check after repeated execution
- a tracing generational object collector with arena-slot reuse, a young-object
  nursery, survivor promotion, closure/property/collection tracing, weak
  collection cleanup, automatic minor collections, and an explicit full
  collection API
- a persistent global environment across evaluations
- statement termination with semicolons, line breaks, closing braces, or EOF

The built-ins are a focused subset, not the full ECMAScript standard library.
Property descriptors include basic data and accessor behavior, but descriptor
invariants and descriptor-aware behavior across every internal mutation path
remain incomplete. The `Reflect` subset does not include symbols or every
specification operation. Array and collection iterators are available through
their methods, but the `Symbol.iterator` protocol and generic iterable
consumption are not implemented; `Array.from`, `Map`, and `Set` can consume
the engine's own array and collection iterator objects, but arbitrary
user-defined iterators are not supported. `for...of` recognizes only the
runtime's supported built-in collection types. Collection iteration handles
basic additions and deletions, but the full collection API is not complete.
The `Date` parser accepts a limited ISO date/time form; local-time methods are
UTC aliases in this headless runtime, and the complete ECMAScript date-string
grammar is not implemented.
`WeakMap` and `WeakSet` enforce object-key requirements and discard entries
whose keys are no longer reachable during collection; finalizers and
specification-complete weak-reference behavior are not implemented.
SIMD scanning currently accelerates ASCII identifier runs and spaces/tabs;
string decoding, token storage, and AST construction still allocate.
The bytecode VM is an initial execution path, not a replacement for the AST
evaluator. Its supported statements include `if`, `while`, and classic `for`,
with `break` and `continue`; lexical declarations in nested blocks, `do...while`,
`for...of`, and exceptions fall back to the AST evaluator. User functions
start in AST interpretation and supported bodies tier to VM frames after three
calls or 32 interpreted loop iterations. Per-function profiles persist for
the function object's lifetime and collect interpreted loop iterations,
bytecode loop backedges, and numeric/generic operand type feedback from VM
execution. Bytecode is still compiled per invocation; this tier is not native
code generation by itself. Hot functions with a single supported numeric
return expression can additionally run in the native tier. The optimizing
graph supports numeric comparisons, conditional branches, and logical
short-circuiting. Comparisons return JavaScript booleans and handle unordered
`NaN` comparisons. It also recognizes one narrowly defined local counted-sum
loop: zero-based induction, `total = total + index`, then returning `total`.
Parameter-bounded loops execute as native loops; constant bounds from 0
through 16 are unrolled and folded. Only numeric parameters/local bindings are
eligible, with runtime guards and VM/interpreter fallback on failure. Other
function and loop shapes remain interpreted or in VM bytecode. Dispatch is
function-table based, not direct-threaded, and no comparative speedup is
claimed.
The collector allocates object handles from a stable arena, reuses reclaimed
slots, and performs minor collections after 256 object allocations. Survivors
are promoted; a full collection runs every eight minor collections or when
`Engine::collect_garbage` is called. Collections occur at evaluation
boundaries, not in the middle of script execution. The collector is currently
stop-the-world and single-threaded; pause duration depends on the reachable
heap and is not guaranteed to stay below 150 μs. Values returned to the host
are conservatively retained as roots for the lifetime of the engine because
the API has no explicit host-root release handle.
The native JIT is available on x86-64 Linux, macOS, and Windows where
executable memory is permitted. Numeric guards send nonnumeric arguments
back through the VM/interpreter. Other expressions and platforms continue
through the VM/interpreter. This is a limited optimizing graph, not a general
SSA/Sea-of-Nodes compiler: it has no general deoptimization-state
reconstruction, heap allocation nodes, or general escape analysis. Its
allocation check only permits the explicitly supported allocation-free
numeric nodes.

SIMD auto-vectorization and speculative call devirtualization are not enabled.
The currently supported numeric reduction has observable left-to-right
binary64 rounding, and the VM has no call-site feedback/IR needed to safely
specialize general calls. No JIT speedup or GC pause guarantee is claimed.
Native expression-JIT data remains scoped to an evaluation or function
invocation; per-function VM tiering profiles and compiled function code
persist with their function objects.
Some array methods, complete string/number behavior, RegExp literal parsing
and the complete set of RegExp/String integration algorithms and edge cases,
`Symbol`, `BigInt`, `Proxy`, `Reflect`, modules, promises/jobs, and many other
specified APIs are not implemented. Classes, many constructor edge cases,
complete `this` binding rules, and complete ECMAScript error objects are also
missing. The parser and runtime semantics remain a limited subset; passing the
focused Rust tests does not establish general ECMAScript conformance.

## Benchmarks

Run the dependency-free microbenchmark harness in release mode:

```text
cargo bench --bench runtime
```

It prints CSV-style elapsed time per operation for end-to-end arithmetic
evaluation (including lexing/parsing), warmed native counted-sum calls,
constant-loop unrolling, and full-collection pauses (median and p95 over 40
samples). The harness reports target OS/architecture but does not control CPU
frequency, thermal state, background load, or allocator state. Record the
Rust compiler version and CPU model when comparing runs. Measurements are
diagnostic, not portable performance claims.
Unsupported syntax reports a `SyntaxError`. Object and scope storage is
engine-owned and reclaimed when the engine is dropped; long-running scripts
can still retain reachable objects, and execution time, memory, and recursion
limits are not enforced. This implementation must not be used to execute
untrusted or unbounded scripts.

## Run

```text
cargo run -- --eval "let answer = 6 * 7; answer"
cargo run -- path\to\first.js path\to\second.js
Get-Content path\to\script.js | cargo run
```

Files passed together execute in order in one persistent global environment.
The CLI prints the completion value of each file or evaluation. The `--module`
option is reserved but modules are not implemented.

## Install the CLI

On Linux or macOS with an x86_64 or arm64 CPU, install the latest release with:

```sh
curl -fsSL https://soumendey.com/volt/install | sh
```

The installer verifies the downloaded release archive against its published
SHA-256 checksum and installs the `volt` executable to `~/.local/bin`.
Add that directory to `PATH` if it is not already there. To install a specific
release, set `VOLT_VERSION` (for example, `VOLT_VERSION=v0.1.0`).
Set `VOLT_INSTALL_DIR` to choose another install directory. Windows and
other platform/architecture combinations are not supported by this installer.

## Embed

```rust
use volt_engine::Engine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new();
    let answer = engine.eval_to_string("let answer = 40 + 2; answer")?;
    println!("{answer}");
    Ok(())
}
```

`Engine::eval` returns the interpreter's `Value`; `Engine::eval_to_string`
formats that value for the CLI and simple embedding use. The `Value` type and
`EngineError` are public.

## Roadmap and conformance

The intended sequence is to grow the own implementation incrementally:

1. Continue covering and correcting ECMAScript function, object, prototype,
   constructor, and exception semantics.
2. Add arrays and the standard built-ins systematically.
3. Implement promise jobs and modules.
4. Add concurrent marking, bounded pause-time strategies, and safe
   embedding/resource controls to the initial stop-the-world collector.
5. Make the Test262 harness work with the owned runtime and measure progress
   against a pinned suite revision; only then expand toward ES2020.

The Test262 runner records scenario pass/fail/skip counts and is pinned to
`tc39/test262` revision
`d53ebc8b1931d9f1541e43d59650776e4b766248`. It requires a checkout at that
revision by default; `--allow-unpinned` is available for explicitly exploratory
runs and is recorded in the report. The runner is not yet a full Test262
conformance harness: modules, async host protocols, Test262 harness facilities,
and much of the language and standard library are unsupported. No conformance
rate should be claimed until the pinned suite is run and its results are
reviewed.

## Checks

```text
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```
