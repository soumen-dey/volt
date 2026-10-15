# js-engine

A headless JavaScript runtime written in Rust and backed by
[Boa](https://github.com/boa-dev/boa). The project aims to target ECMAScript
2026 (ES2026) language and standard built-ins. It does not reimplement Boa, and
the target is not a claim of complete ES2026 conformance.

Boa 0.22.0 is an experimental ECMAScript engine with version-specific,
incomplete feature support. This project currently has no Test262 conformance
run or measured ES2026 pass rate. Its local tests are smoke tests only; passing
them does not certify any edition of ECMAScript.

## Scope

The default runtime is headless. It does not install browser or server globals:
there is no `window`, `document`, DOM, CSS/layout, `fetch`, `URL`, Web Storage,
timers, `process`, or `require`. Standard ECMAScript globals, including
collections, typed arrays, promises, and the internationalization
implementation enabled by Boa, come from Boa.

`Engine::context_mut()` is available to embedders who intentionally want to
register their own host APIs. Such additions are not part of this project's
default JavaScript environment or its ES2026 conformance target.

## Requirements

- Rust toolchain compatible with the selected Boa release

## Run JavaScript

```text
cargo run -- --eval "const square = x => x * x; square(9)"
cargo run -- path\to\first.js path\to\second.js
cargo run -- --module path\to\main.mjs
Get-Content path\to\script.js | cargo run
```

Script files passed together execute in order in one persistent JavaScript
context. The command-line runner prints each script's completion value. With
no file or option, it reads one script from standard input. Use `--module` to
parse and evaluate ECMAScript modules; imports are resolved by Boa's
file-based module loader.

## Embed the runtime

```rust
use js_engine::Engine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new();
    let result = engine.eval_to_string("globalThis.answer = 40 + 2; answer")?;
    println!("{result}");
    Ok(())
}
```

## Conformance status

The desired target is the current ECMAScript 2026 specification, but the
runtime has not yet been validated against the official
[Test262](https://github.com/tc39/test262) suite. In particular, this
repository does not yet have a Test262 runner, an edition-aware feature
baseline, or recorded pass/fail results. Do not treat the current built-in
smoke tests or Boa's version number as proof of ES2026 support.

The next conformance milestone is to integrate a reproducible Test262 run,
record the exact Test262 revision and runner configuration, and publish
pass/fail/skip counts. Implementations should then be driven by those failures
and the ES2026 specification. Until that work is complete, this is an
experimental headless runtime, not a complete or certified ES2026 engine.

The runtime has no per-script time, memory, or recursion limits. Do not use it
as a security boundary for untrusted scripts.

## Tests

```text
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```
