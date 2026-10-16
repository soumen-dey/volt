use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use volt_engine::Engine;

const EVAL_ITERATIONS: usize = 10_000;
const FUNCTION_ITERATIONS: usize = 5_000;
const GC_SAMPLES: usize = 40;

fn report(label: &str, iterations: usize, elapsed: Duration) {
    let nanos_per_operation = elapsed.as_nanos() as f64 / iterations as f64;
    println!(
        "{label},{iterations},{},{nanos_per_operation:.1}",
        elapsed.as_nanos()
    );
}

fn measure<F>(label: &str, iterations: usize, mut operation: F)
where
    F: FnMut(),
{
    for _ in 0..(iterations / 10).max(1) {
        operation();
    }
    let start = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    report(label, iterations, start.elapsed());
}

fn main() {
    println!(
        "# target={},{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("benchmark,iterations,total_ns,ns_per_operation");

    let mut arithmetic = Engine::new();
    measure("eval_arithmetic_parse_and_execute", EVAL_ITERATIONS, || {
        black_box(arithmetic.eval("1 + 2 * 3").expect("arithmetic eval"));
    });

    let mut functions = Engine::new();
    functions
        .eval(
            "function dynamicSum(limit) { let total = 0; for (let index = 0; index < limit; index++) { total = total + index; } return total; } function constantSum() { let total = 0; for (let index = 0; index < 8; index++) { total = total + index; } return total; }",
        )
        .expect("function declarations");
    for _ in 0..3 {
        functions
            .eval("dynamicSum(100)")
            .expect("warm dynamic sum tier");
        functions
            .eval("constantSum()")
            .expect("warm constant sum tier");
    }
    measure(
        "eval_hot_native_dynamic_counted_sum",
        FUNCTION_ITERATIONS,
        || {
            black_box(functions.eval("dynamicSum(100)").expect("dynamic sum eval"));
        },
    );
    measure(
        "eval_hot_native_unrolled_constant_sum",
        FUNCTION_ITERATIONS,
        || {
            black_box(functions.eval("constantSum()").expect("constant sum eval"));
        },
    );

    let mut gc_samples = Vec::with_capacity(GC_SAMPLES);
    for _ in 0..GC_SAMPLES {
        let mut engine = Engine::new();
        engine
            .eval("let i = 0; while (i < 100) { let discarded = { value: i }; i = i + 1; } 0")
            .expect("allocate unreachable objects");
        let start = Instant::now();
        black_box(engine.collect_garbage());
        gc_samples.push(start.elapsed());
    }
    gc_samples.sort_unstable();
    report("full_gc_pause_median", 1, gc_samples[GC_SAMPLES / 2]);
    report(
        "full_gc_pause_p95",
        1,
        gc_samples[(GC_SAMPLES * 95 / 100).min(GC_SAMPLES - 1)],
    );
}
