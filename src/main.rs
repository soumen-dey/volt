use std::{
    env, fs,
    io::{self, Read},
    process,
};

fn main() {
    let mut args = env::args().skip(1);
    let first = args.next();

    match first.as_deref() {
        Some("--help") | Some("-h") => print_help(),
        Some("--version") | Some("-V") => println!("js-engine {}", env!("CARGO_PKG_VERSION")),
        Some("-e") | Some("--eval") => {
            let source = args.collect::<Vec<_>>().join(" ");
            if source.is_empty() {
                eprintln!("Usage error: --eval requires JavaScript source");
                process::exit(2);
            }
            evaluate_source(&source, "<eval>");
        }
        Some("-m") | Some("--module") => {
            let Some(path) = args.next() else {
                eprintln!("Usage error: --module requires a file path");
                process::exit(2);
            };
            let mut engine = js_engine::Engine::new();
            if let Err(error) = evaluate_module_file(&mut engine, &path) {
                eprintln!("{error}");
                process::exit(1);
            }
            for path in args {
                if let Err(error) = evaluate_module_file(&mut engine, &path) {
                    eprintln!("{error}");
                    process::exit(1);
                }
            }
        }
        Some(arg) if arg.starts_with('-') => {
            eprintln!("Unknown option: {arg}");
            eprintln!("Run 'js-engine --help' for usage.");
            process::exit(2);
        }
        Some(path) => {
            let mut engine = js_engine::Engine::new();
            if let Err(error) = evaluate_file(&mut engine, path) {
                eprintln!("{error}");
                process::exit(1);
            }
            for path in args {
                if let Err(error) = evaluate_file(&mut engine, &path) {
                    eprintln!("{error}");
                    process::exit(1);
                }
            }
        }
        None => evaluate_stdin(),
    }
}

fn evaluate_source(source: &str, name: &str) {
    let mut engine = js_engine::Engine::new();
    match engine.eval_to_string(source) {
        Ok(value) => {
            println!("{value}");
            if let Err(error) = engine.run_jobs() {
                eprintln!("{name}: {error}");
                process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{name}: {error}");
            process::exit(1);
        }
    }
}

fn evaluate_stdin() {
    let mut source = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut source) {
        eprintln!("Input error: {error}");
        process::exit(1);
    }
    evaluate_source(&source, "<stdin>");
}

fn evaluate_file(engine: &mut js_engine::Engine, path: &str) -> Result<(), String> {
    let source = fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    let value = engine
        .eval_to_string(&source)
        .map_err(|error| format!("{path}: {error}"))?;
    println!("{value}");
    engine
        .run_jobs()
        .map_err(|error| format!("{path}: {error}"))
}

fn evaluate_module_file(engine: &mut js_engine::Engine, path: &str) -> Result<(), String> {
    engine
        .eval_module_file(path)
        .map_err(|error| format!("{path}: {error}"))
}

fn print_help() {
    println!(
        "js-engine {}\n\
         Headless ECMAScript runtime CLI\n\n\
         Usage:\n\
           js-engine [OPTIONS] [FILE...]\n\
           js-engine --eval <SOURCE>\n\
           js-engine --module <FILE...>\n\
           js-engine < script.js\n\n\
         Options:\n\
           -e, --eval <SOURCE>  Evaluate JavaScript source\n\
           -m, --module <FILE>  Evaluate module file(s) with imports\n\
           -h, --help           Print this help\n\
           -V, --version        Print version\n\n\
         Files are evaluated in order in one persistent JavaScript context.",
        env!("CARGO_PKG_VERSION")
    );
}
