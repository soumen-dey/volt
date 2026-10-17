use std::{
    hint::black_box,
    process::Command,
    time::{Duration, Instant},
};

use volt_engine::Engine;

const EVAL_ITERATIONS: usize = 10_000;
const FUNCTION_ITERATIONS: usize = 5_000;
const GC_SAMPLES: usize = 40;
const TIMING_SAMPLES: usize = 9;
const WARMUP_ITERATIONS: usize = 1_000;

fn report(label: &str, iterations: usize, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    println!(
        "{label},{iterations},{},{:.1},{:.1}",
        samples.len(),
        median.as_nanos() as f64 / iterations as f64,
        p95.as_nanos() as f64 / iterations as f64
    );
}

fn measure<F>(label: &str, iterations: usize, mut operation: F)
where
    F: FnMut(),
{
    for _ in 0..WARMUP_ITERATIONS {
        operation();
    }
    let mut samples = Vec::with_capacity(TIMING_SAMPLES);
    for _ in 0..TIMING_SAMPLES {
        let start = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        samples.push(start.elapsed());
    }
    report(label, iterations, samples);
}

fn print_toolchain_version() {
    match Command::new("rustc").arg("--version").output() {
        Ok(output) if output.status.success() => {
            println!("# rustc={}", String::from_utf8_lossy(&output.stdout).trim());
        }
        Ok(output) => eprintln!(
            "failed to obtain rustc version: process exited with {}",
            output.status
        ),
        Err(error) => eprintln!("failed to run rustc --version: {error}"),
    }
}

fn main() {
    print_toolchain_version();
    println!(
        "# target={},{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if let Ok(cpu) = std::env::var("PROCESSOR_IDENTIFIER") {
        println!("# cpu={cpu}");
    }
    println!("benchmark,iterations,samples,median_ns_per_operation,p95_ns_per_operation");

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
    report("full_gc_pause", 1, gc_samples);
}
