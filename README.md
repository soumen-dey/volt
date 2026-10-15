# js-engine

A from-scratch JavaScript interpreter written in Rust. The project owns its
tokenizer, parser, runtime values, scopes, and evaluator; it does not embed
Boa, V8, or another JavaScript engine.

This is an early implementation milestone, **not a complete JavaScript
engine and not ES2020-conformant**. The longer-term target is headless ES2020,
measured against a pinned Test262 revision. Browser and server APIs are not
part of the runtime target.

## Current implementation

The initial interpreter supports:

- `undefined`, `null`, booleans, decimal numbers, and single-/double-quoted strings
- `let`, `const`, and `var` declarations (currently block-scoped)
- arithmetic, comparison, equality, logical, and nullish-coalescing operators
- assignment to declared identifiers
- unary `!`, `+`, `-`, `typeof`, and `void`
- blocks, `if`/`else`, and `while`
- line and block comments
- a persistent global environment across evaluations
- statement termination with semicolons, line breaks, closing braces, or EOF

Functions, objects, arrays, classes, exceptions, modules, promises, the
ECMAScript standard library, garbage collection, and most other language
features are not implemented. Unsupported syntax reports a `SyntaxError`.
This implementation must not be used to execute untrusted or unbounded scripts;
it does not yet enforce time, memory, or recursion limits.

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

1. Expand lexer/parser coverage and diagnostics.
2. Implement ECMAScript execution semantics, functions, closures, objects,
   prototypes, and exceptions.
3. Add standard built-ins, promise jobs, and modules.
4. Establish a useful Test262 harness and measure progress against a pinned
   suite revision; only then expand toward ES2020.
5. Design object-heap memory management and safe embedding/resource controls.

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
