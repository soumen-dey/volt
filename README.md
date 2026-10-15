# js-engine

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
  `Object.setPrototypeOf`, plus basic `Object.keys`, `getOwnPropertyNames`,
  `values`, `entries`, `assign`, `fromEntries`, `hasOwn`, and `is`
- sparse array literals and basic `Array` construction, indexed properties,
  mutable `length`, and common array methods (`push`, `pop`, `shift`,
  `unshift`, `splice`, `slice`, `concat`, `join`, `reverse`, `flat`, `flatMap`,
  searches, and callback methods)
- selected `String`, `Number`, and `Boolean` conversions and prototype methods,
  `Math`, global numeric conversion helpers, and `JSON.parse`/`JSON.stringify`
- `try`/`catch`/`finally`, explicit throws, and basic `Error`,
  `EvalError`, `RangeError`, `ReferenceError`, `SyntaxError`, `TypeError`,
  and `URIError` objects
- line and block comments
- a persistent global environment across evaluations
- statement termination with semicolons, line breaks, closing braces, or EOF

The built-ins are a focused subset, not the full ECMAScript standard library.
Array iteration protocols and some methods, complete string/number behavior,
property descriptors, `Date`, `RegExp`, `Symbol`, `BigInt`, `Proxy`, `Reflect`,
modules, promises/jobs, and many other specified APIs are not implemented.
Classes, many constructor edge cases, complete `this` binding rules, and
complete ECMAScript error objects are also missing. The parser and runtime
semantics remain a limited subset; passing the focused Rust tests does not
establish general ECMAScript conformance. Unsupported syntax reports a
`SyntaxError`. Object and scope storage is engine-owned and reclaimed
when the engine is dropped; there is no garbage collector yet, so long-running
engines retain allocations made by evaluated scripts. This implementation must
not be used to execute untrusted or unbounded scripts; it does not yet enforce
time, memory, or recursion limits.

## Run

```text
cargo run -- --eval "let answer = 6 * 7; answer"
cargo run -- path\to\first.js path\to\second.js
Get-Content path\to\script.js | cargo run
```

Files passed together execute in order in one persistent global environment.
The CLI prints the completion value of each file or evaluation. The `--module`
option is reserved but modules are not implemented.

## Embed

```rust
use js_engine::Engine;

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
4. Replace engine-lifetime arena retention with a tracing garbage collector;
   add safe embedding/resource controls.
5. Make the Test262 harness work with the owned runtime and measure progress
   against a pinned suite revision; only then expand toward ES2020.

The repository's Test262 runner is an initial scaffold, not currently a valid
conformance harness for this implementation. It depends on features (including
the standard library and harness behavior) that the interpreter does not yet
provide. Do not interpret its results as an ECMAScript pass rate. Each stage
should add focused tests, then update the harness and publish explicit pass,
fail, and skip counts.

## Checks

```text
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```
