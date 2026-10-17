use std::{
    ffi::{CStr, CString, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};

use crate::{CNumberCallback, Engine, EngineLimits};

pub struct VoltEngine {
    engine: Engine,
    last_error: CString,
}

const STATUS_OK: i32 = 0;
const STATUS_INVALID_ARGUMENT: i32 = 1;
const STATUS_EVALUATION_ERROR: i32 = 2;
const STATUS_INVALID_OUTPUT: i32 = 3;
const STATUS_INTERNAL_ERROR: i32 = 4;

/// Creates an engine for the C ABI.
///
/// Returns null only if Rust allocation panics. The returned pointer must be
/// released with `volt_engine_free` and must not be used concurrently.
#[unsafe(no_mangle)]
pub extern "C" fn volt_engine_new() -> *mut VoltEngine {
    create_engine(Engine::new)
}

/// Creates an engine with source, interpreter-step, and call-depth limits.
/// This does not impose a hard memory or wall-clock limit.
#[unsafe(no_mangle)]
pub extern "C" fn volt_engine_new_with_limits(
    max_source_bytes: usize,
    max_execution_steps: usize,
    max_call_depth: usize,
) -> *mut VoltEngine {
    create_engine(|| {
        Engine::with_limits(EngineLimits {
            max_source_bytes,
            max_execution_steps,
            max_call_depth,
            max_evaluation_time: std::time::Duration::MAX,
        })
    })
}

/// Creates an engine with cooperative per-evaluation budgets.
///
/// The time budget is checked during runtime execution; it does not interrupt
/// parsing or an operation that is currently running.
#[unsafe(no_mangle)]
pub extern "C" fn volt_engine_new_with_budgets(
    max_source_bytes: usize,
    max_execution_steps: usize,
    max_call_depth: usize,
    max_evaluation_time_ms: u64,
) -> *mut VoltEngine {
    create_engine(|| {
        Engine::with_limits(EngineLimits {
            max_source_bytes,
            max_execution_steps,
            max_call_depth,
            max_evaluation_time: std::time::Duration::from_millis(max_evaluation_time_ms),
        })
    })
}

fn create_engine(create: impl FnOnce() -> Engine) -> *mut VoltEngine {
    catch_unwind(AssertUnwindSafe(|| {
        Box::into_raw(Box::new(VoltEngine {
            engine: create(),
            last_error: c"".to_owned(),
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// Frees an engine created by `volt_engine_new`. Null is accepted.
///
/// # Safety
/// A non-null pointer must have been returned by `volt_engine_new` and must
/// not have been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn volt_engine_free(engine: *mut VoltEngine) {
    if engine.is_null() {
        return;
    }
    // SAFETY: upheld by the caller's contract above.
    drop(unsafe { Box::from_raw(engine) });
}

/// Registers a numeric-only host callback exposed as a global JavaScript
/// function. Returns the ordinary `VoltStatus` code.
///
/// The callback receives a pointer to `argument_count` doubles and writes one
/// double to `out_result`. Return zero for success and nonzero for failure.
///
/// # Safety
/// `engine` must be live; `name` must point to a NUL-terminated UTF-8 string;
/// callback and user_data must remain valid until engine destruction. Callback
/// code must not unwind, re-enter the engine, or retain the argument/output
/// pointers. The registered callback must only be called on this engine's
/// thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn volt_engine_register_number_fn(
    engine: *mut VoltEngine,
    name: *const c_char,
    callback: Option<CNumberCallback>,
    user_data: *mut c_void,
) -> i32 {
    if engine.is_null() || name.is_null() || callback.is_none() {
        return STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: pointer validity is the caller's responsibility as documented.
    let engine = unsafe { &mut *engine };
    // SAFETY: name must be a readable NUL-terminated string.
    let name = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(name) => name,
        Err(_) => {
            engine.last_error = c"host function name is not valid UTF-8".to_owned();
            return STATUS_INVALID_ARGUMENT;
        }
    };
    // SAFETY: callback lifetime, user data, and reentrancy requirements are
    // documented on this exported C function and required of its caller.
    match unsafe {
        engine.engine.register_c_number_fn(
            name,
            callback.expect("callback was checked above"),
            user_data,
        )
    } {
        Ok(()) => {
            engine.last_error = c"".to_owned();
            STATUS_OK
        }
        Err(error) => {
            engine.last_error = CString::new(error.to_string())
                .unwrap_or_else(|_| c"host registration error".to_owned());
            STATUS_EVALUATION_ERROR
        }
    }
}

/// Evaluates UTF-8 source and returns its completion value as a newly
/// allocated UTF-8 C string through `out_result`.
///
/// Status codes: 0 success, 1 invalid argument/UTF-8, 2 JavaScript evaluation
/// error, 3 result contains an interior NUL byte, 4 internal panic.
///
/// # Safety
/// `engine` must be a live pointer from `volt_engine_new`; `source` must point
/// to a NUL-terminated string; and `out_result` must be writable. The engine
/// must not be accessed concurrently. On success, release `*out_result` with
/// `volt_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn volt_engine_eval(
    engine: *mut VoltEngine,
    source: *const c_char,
    out_result: *mut *mut c_char,
) -> i32 {
    if engine.is_null() || source.is_null() || out_result.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }

    // SAFETY: pointer validity is the caller's responsibility as documented.
    unsafe { out_result.write(ptr::null_mut()) };
    // SAFETY: pointer validity is the caller's responsibility as documented.
    let engine = unsafe { &mut *engine };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: source must be a readable NUL-terminated string.
        let source = unsafe { CStr::from_ptr(source) };
        let source = source.to_str().map_err(|_| {
            engine.last_error = c"source is not valid UTF-8".to_owned();
            STATUS_INVALID_ARGUMENT
        })?;
        let result = engine.engine.eval_to_string(source).map_err(|error| {
            let formatted = error.to_string();
            engine.last_error = CString::new(formatted)
                .unwrap_or_else(|_| c"evaluation error contained an interior NUL".to_owned());
            STATUS_EVALUATION_ERROR
        })?;
        let result = CString::new(result).map_err(|_| {
            engine.last_error = c"evaluation result contains an interior NUL".to_owned();
            STATUS_INVALID_OUTPUT
        })?;
        engine.last_error = c"".to_owned();
        // SAFETY: out_result was checked for null and documented writable.
        unsafe { out_result.write(result.into_raw()) };
        Ok::<(), i32>(())
    }));

    match outcome {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(status)) => status,
        Err(_) => {
            engine.last_error = c"internal panic during evaluation".to_owned();
            STATUS_INTERNAL_ERROR
        }
    }
}

/// Returns the most recent error string, or an empty string after success.
/// The pointer is borrowed and remains valid until the next call on this
/// engine.
///
/// # Safety
/// `engine` must be null or a live pointer returned by `volt_engine_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn volt_engine_last_error(engine: *const VoltEngine) -> *const c_char {
    if engine.is_null() {
        return ptr::null();
    }
    // SAFETY: pointer validity is the caller's responsibility as documented.
    unsafe { &*engine }.last_error.as_ptr()
}

/// Releases a string returned by `volt_engine_eval`. Null is accepted.
///
/// # Safety
/// A non-null pointer must have been returned by `volt_engine_eval` and not
/// freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn volt_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    // SAFETY: upheld by the caller's contract above.
    drop(unsafe { CString::from_raw(value) });
}

#[cfg(test)]
mod tests {
    use super::{
        STATUS_EVALUATION_ERROR, STATUS_OK, volt_engine_eval, volt_engine_free,
        volt_engine_last_error, volt_engine_new, volt_engine_new_with_budgets,
        volt_engine_new_with_limits, volt_engine_register_number_fn, volt_string_free,
    };
    use std::ffi::{CStr, CString, c_void};
    use std::ptr;

    #[test]
    fn c_abi_evaluates_and_reports_errors() {
        // SAFETY: pointers are created and released according to the C ABI.
        unsafe {
            let engine = volt_engine_new();
            assert!(!engine.is_null());

            let source = CString::new("40 + 2").unwrap();
            let mut result = ptr::null_mut();
            assert_eq!(
                volt_engine_eval(engine, source.as_ptr(), &mut result),
                STATUS_OK
            );
            assert_eq!(CStr::from_ptr(result).to_str().unwrap(), "42");
            volt_string_free(result);

            let invalid = CString::new("missingBinding").unwrap();
            assert_eq!(
                volt_engine_eval(engine, invalid.as_ptr(), &mut result),
                STATUS_EVALUATION_ERROR
            );
            assert!(result.is_null());
            let error = CStr::from_ptr(volt_engine_last_error(engine))
                .to_str()
                .unwrap();
            assert!(error.contains("ReferenceError"));

            volt_engine_free(engine);
        }
    }

    #[test]
    fn c_abi_constructor_can_configure_limits() {
        // SAFETY: pointers are created and released according to the C ABI.
        unsafe {
            let engine = volt_engine_new_with_limits(16, 32, 4);
            assert!(!engine.is_null());
            let source = CString::new(" ".repeat(17)).unwrap();
            let mut result = ptr::null_mut();
            assert_eq!(
                volt_engine_eval(engine, source.as_ptr(), &mut result),
                STATUS_EVALUATION_ERROR
            );
            let error = CStr::from_ptr(volt_engine_last_error(engine))
                .to_str()
                .unwrap();
            assert!(error.contains("source byte length"));
            volt_engine_free(engine);
        }
    }

    unsafe extern "C" fn c_double(
        _user_data: *mut c_void,
        arguments: *const f64,
        argument_count: usize,
        out_result: *mut f64,
    ) -> i32 {
        if argument_count != 1 || arguments.is_null() || out_result.is_null() {
            return 1;
        }
        // SAFETY: caller provides valid pointers for the callback duration.
        unsafe {
            out_result.write(arguments.read() * 2.0);
        }
        0
    }

    #[test]
    fn c_abi_registers_numeric_callbacks() {
        // SAFETY: callback has static lifetime and follows the C callback contract.
        unsafe {
            let engine = volt_engine_new();
            let name = CString::new("doubleFromC").unwrap();
            assert_eq!(
                volt_engine_register_number_fn(
                    engine,
                    name.as_ptr(),
                    Some(c_double),
                    ptr::null_mut(),
                ),
                STATUS_OK
            );

            let source = CString::new("doubleFromC(21)").unwrap();
            let mut result = ptr::null_mut();
            assert_eq!(
                volt_engine_eval(engine, source.as_ptr(), &mut result),
                STATUS_OK
            );
            assert_eq!(CStr::from_ptr(result).to_str().unwrap(), "42");
            volt_string_free(result);

            let invalid = CString::new("doubleFromC('21')").unwrap();
            assert_eq!(
                volt_engine_eval(engine, invalid.as_ptr(), &mut result),
                STATUS_EVALUATION_ERROR
            );
            assert!(
                CStr::from_ptr(volt_engine_last_error(engine))
                    .to_str()
                    .unwrap()
                    .contains("numeric arguments only")
            );
            volt_engine_free(engine);
        }
    }

    #[test]
    fn c_abi_time_budget_is_reported_as_evaluation_error() {
        // SAFETY: engine and source pointers follow the C ABI contract.
        unsafe {
            let engine = volt_engine_new_with_budgets(1024, 10_000, 32, 0);
            let source = CString::new("1 + 2").unwrap();
            let mut result = ptr::null_mut();
            assert_eq!(
                volt_engine_eval(engine, source.as_ptr(), &mut result),
                STATUS_EVALUATION_ERROR
            );
            let error = CStr::from_ptr(volt_engine_last_error(engine))
                .to_str()
                .unwrap();
            assert!(error.contains("Cooperative evaluation time budget"));
            volt_engine_free(engine);
        }
    }
}
