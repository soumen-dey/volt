pub use boa_engine::{Context, JsResult, JsValue};
use boa_engine::{JsError, JsNativeError, Source, builtins::promise::PromiseState, module::Module};
use std::path::Path;

/// An embeddable, headless ECMAScript runtime backed by Boa.
///
/// The default context exposes ECMAScript built-ins without browser or
/// server-specific globals.
pub struct Engine {
    context: Context,
}

impl Engine {
    /// Creates a runtime with the standard Boa ECMAScript context.
    pub fn new() -> Self {
        Self {
            context: Context::default(),
        }
    }

    /// Evaluates JavaScript in this engine's persistent context.
    ///
    /// Global declarations and other script state remain available to later
    /// calls. Host exceptions are returned as JavaScript errors.
    pub fn eval(&mut self, source: &str) -> JsResult<JsValue> {
        self.context.eval(Source::from_bytes(source))
    }

    /// Evaluates JavaScript and converts its completion value with ECMAScript
    /// `ToString`.
    pub fn eval_to_string(&mut self, source: &str) -> JsResult<String> {
        self.eval(source)
            .and_then(|value| value.to_string(&mut self.context))
            .map(|value| value.to_std_string_escaped())
    }

    /// Drains JavaScript jobs currently queued in the context.
    ///
    /// Embedders can call this at their chosen microtask checkpoints.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        self.context.run_jobs()
    }

    /// Parses, links, and evaluates an ECMAScript module.
    ///
    /// Relative imports require a source path; use [`Engine::eval_module_file`]
    /// when the module imports other files.
    pub fn eval_module(&mut self, source: &str) -> JsResult<()> {
        let module = Module::parse(Source::from_bytes(source), None, &mut self.context)?;
        self.evaluate_module(module)
    }

    /// Parses, links, and evaluates an ECMAScript module from a file.
    ///
    /// Relative imports are resolved by Boa's configured module loader.
    pub fn eval_module_file(&mut self, path: impl AsRef<Path>) -> JsResult<()> {
        let source = Source::from_filepath(path.as_ref()).map_err(JsError::from_rust)?;
        let module = Module::parse(source, None, &mut self.context)?;
        self.evaluate_module(module)
    }

    fn evaluate_module(&mut self, module: Module) -> JsResult<()> {
        let promise = module.load_link_evaluate(&mut self.context);
        self.context.run_jobs()?;

        match promise.state() {
            PromiseState::Fulfilled(_) => Ok(()),
            PromiseState::Rejected(reason) => Err(JsError::from_opaque(reason)),
            PromiseState::Pending => Err(JsNativeError::typ()
                .with_message("module evaluation did not complete")
                .into()),
        }
    }

    /// Provides mutable access to Boa's context for specialized embeddings.
    ///
    /// Additional host APIs registered through this context are outside the
    /// default headless runtime surface.
    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Engine;

    #[test]
    fn evaluates_javascript_and_preserves_global_state() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string("globalThis.answer = 6 * 7; answer")
                .unwrap(),
            "42"
        );
        assert_eq!(engine.eval_to_string("answer + 1").unwrap(), "43");
    }

    #[test]
    fn supports_core_collection_and_typed_array_builtins() {
        let mut engine = Engine::new();
        let result = engine
            .eval_to_string(
                "const values = new Map([['answer', 42]]);\
                 const bytes = new Uint8Array([values.get('answer'), 257]);\
                 [Array.isArray([1]), values instanceof Map, bytes.join(','),\
                  new Set([1, 1, 2]).size].join('|')",
            )
            .expect("core ECMAScript built-ins should be available");
        assert_eq!(result, "true|true|42,1|2");
    }

    #[test]
    fn supports_promise_jobs() {
        let mut engine = Engine::new();
        engine
            .eval("globalThis.jobResult = 0; Promise.resolve().then(() => jobResult = 42)")
            .expect("script should schedule a promise job");
        engine.run_jobs().expect("queued jobs should run");
        assert_eq!(engine.eval_to_string("jobResult").unwrap(), "42");
    }

    #[test]
    fn does_not_install_browser_or_server_globals() {
        let mut engine = Engine::new();
        let result = engine
            .eval_to_string(
                "[typeof window, typeof self, typeof document, typeof fetch,\
                  typeof URL, typeof localStorage, typeof setTimeout,\
                  typeof process, typeof require].join('|')",
            )
            .expect("global names should be inspectable");
        assert_eq!(
            result,
            "undefined|undefined|undefined|undefined|undefined|undefined|undefined|undefined|undefined"
        );
    }

    #[test]
    fn initializes_intl_with_bundled_locale_data() {
        let mut engine = Engine::new();
        assert_eq!(
            engine
                .eval_to_string("new Intl.NumberFormat('de-DE').format(1234.5)")
                .expect("Intl should have a data provider"),
            "1.234,5"
        );
    }

    #[test]
    fn evaluates_modules_with_relative_imports() {
        let mut engine = Engine::new();
        engine
            .eval_module_file("tests/fixtures/module-main.mjs")
            .expect("module and relative import should evaluate");
        assert_eq!(engine.eval_to_string("moduleAnswer").unwrap(), "42");
    }

    #[test]
    fn returns_javascript_exceptions_as_errors() {
        let mut engine = Engine::new();
        assert!(engine.eval("throw new Error('failure')").is_err());
    }
}
