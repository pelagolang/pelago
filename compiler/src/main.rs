//! Command-line entry point. Only inspection commands exist so far.

use pelago::keyword::{Keyword, Strength};
use pelago::lexer::tokenize;
use pelago::span::LineIndex;
use pelago::token::{Punct, TokenKind};

/// The compiler runs on a thread with a big stack: the parser and the checker recurse, and the
/// main thread has only 1 MiB on Windows and 8 MiB on Linux.
fn main() {
    let worker = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(real_main);
    match worker {
        Ok(h) => {
            if h.join().is_err() {
                std::process::exit(101);
            }
        }
        Err(_) => real_main(),
    }
}

fn real_main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("keywords") => match args.next().as_deref() {
            Some("--json") => println!("{}", pelago::editors::keywords_json()),
            _ => print_keywords(),
        },
        Some("highlight") => {
            let rest: Vec<String> = args.collect();
            highlight_command(&rest);
        }
        Some("grammar") => print!("{}", pelago::editors::textmate_grammar()),
        Some("tokens") => print_punct(),
        Some("lex-bench") => match args.next() {
            Some(path) => lex_bench(&path),
            None => {
                eprintln!("usage: pelago lex-bench <file>");
                std::process::exit(2);
            }
        },
        Some("parse") => match args.next() {
            Some(flag) if flag == "--no-docs" => match args.next() {
                Some(path) => parse_file(&path, false),
                None => {
                    eprintln!("usage: pelago parse [--no-docs] <file>");
                    std::process::exit(2);
                }
            },
            Some(path) => parse_file(&path, true),
            None => {
                eprintln!("usage: pelago parse <file>");
                std::process::exit(2);
            }
        },
        Some("check") => match args.next() {
            Some(path) => check_path(&path),
            None => {
                eprintln!("usage: pelago check <file or directory>");
                std::process::exit(2);
            }
        },
        Some(cmd @ ("ir" | "run" | "build")) => {
            let mut rest: Vec<String> = args.collect();
            // `-o <file>` names the output of `build`.
            let mut out_name: Option<String> = None;
            if let Some(i) = rest.iter().position(|a| a == "-o") {
                if i + 1 < rest.len() {
                    out_name = Some(rest.remove(i + 1));
                }
                rest.remove(i);
            }
            let flags: Vec<&str> = rest
                .iter()
                .filter(|a| a.starts_with("--"))
                .map(|a| a.as_str())
                .collect();
            match rest.iter().find(|a| !a.starts_with("--")) {
                Some(path) => ir_or_run(path, cmd, &flags, out_name),
                None => {
                    eprintln!("usage: pelago {cmd} [--opt|--no-opt] [--stats] <file or directory>");
                    eprintln!("       pelago run --native <path>      build natively and run");
                    eprintln!("       pelago build [--no-opt] [--leak-check] [--target=linux|windows] [-o out] <path>");
                    std::process::exit(2);
                }
            }
        }
        Some("lsp") => std::process::exit(pelago::lsp::run_stdio()),
        Some("parse-bench") => match args.next() {
            Some(path) => parse_bench(&path),
            None => {
                eprintln!("usage: pelago parse-bench <file>");
                std::process::exit(2);
            }
        },
        Some("lex") => match args.next() {
            Some(path) => lex_file(&path),
            None => {
                eprintln!("usage: pelago lex <file>");
                std::process::exit(2);
            }
        },
        Some(other) => {
            eprintln!("unknown command `{other}`");
            usage();
            std::process::exit(2);
        }
        None => usage(),
    }
}

fn usage() {
    eprintln!("usage: pelago <command>");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  keywords   list every keyword, hard ones first");
    eprintln!("  tokens     list operators and punctuation");
    eprintln!("  lex-bench <file>  time the lexer alone on a file");
    eprintln!("  parse [--no-docs] <file>  print the syntax tree and any errors (`-` reads standard input)");
    eprintln!("  ir <path>  check, lower to the IR, verify and print it");
    eprintln!("  run <path>  lower to the IR and run `main` in the interpreter (`--native` builds and runs machine code)");
    eprintln!("  build [-o out] <path>  build a native x86-64 Linux executable");
    eprintln!("  check <path>  parse and resolve names of a file or a directory of `.pel` files");
    eprintln!("  parse-bench <file>  time lexing plus parsing on a file");
    eprintln!("  lex <file> print the tokens of a source file (`-` reads standard input)");
}

fn print_keywords() {
    for strength in [Strength::Hard, Strength::Soft] {
        let label = match strength {
            Strength::Hard => "hard (reserved everywhere)",
            Strength::Soft => "soft (special only in one position)",
        };
        let words: Vec<&str> = Keyword::ALL
            .iter()
            .filter(|k| k.strength() == strength)
            .map(|k| k.as_str())
            .collect();
        println!("{label}: {}", words.len());
        println!("  {}", words.join(" "));
        println!();
    }
}

/// `pelago highlight <file> [--ansi | --html | --html-page | --plain] [--theme dark|light]`
fn highlight_command(args: &[String]) {
    let mut mode = "ansi";
    let mut theme = pelago::highlight::Theme::Dark;
    let mut path: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ansi" => mode = "ansi",
            "--html" => mode = "html",
            "--html-page" => mode = "html-page",
            "--plain" => mode = "plain",
            "--theme" => {
                i += 1;
                theme = match args.get(i).map(|s| s.as_str()) {
                    Some("light") => pelago::highlight::Theme::Light,
                    Some("dark") => pelago::highlight::Theme::Dark,
                    _ => {
                        eprintln!("usage: pelago highlight <file> [--ansi | --html | --html-page | --plain] [--theme dark|light]");
                        std::process::exit(2);
                    }
                };
            }
            other if other.starts_with("--") => {
                eprintln!("unknown option `{other}`");
                std::process::exit(2);
            }
            other => path = Some(other),
        }
        i += 1;
    }
    let Some(path) = path else {
        eprintln!("usage: pelago highlight <file> [--ansi | --html | --html-page | --plain] [--theme dark|light]");
        std::process::exit(2);
    };
    let src = read_source(path);
    let pieces = pelago::highlight::classify(&src);
    // NO_COLOR asks for no escape codes.
    if mode == "ansi" && std::env::var_os("NO_COLOR").is_some() {
        mode = "plain";
    }
    let text = match mode {
        "html" => pelago::highlight::render_html(&src, &pieces, false),
        "html-page" => pelago::highlight::render_html(&src, &pieces, true),
        "plain" => pelago::highlight::render_plain(&src, &pieces),
        _ => pelago::highlight::render_ansi(&src, &pieces, theme),
    };
    print!("{text}");
}

fn print_punct() {
    let texts: Vec<&str> = Punct::ALL.iter().map(|p| p.as_str()).collect();
    println!("{} tokens", texts.len());
    println!("  {}", texts.join(" "));
}

fn check_path(path: &str) {
    let mut program = pelago::package::Program::with_core();
    if let Err(e) = program.add_package_from_path(std::path::Path::new(path)) {
        eprintln!("{e}");
        std::process::exit(2);
    }
    let resolved = pelago::resolve::resolve(&program);
    let typed = pelago::typecheck::check(&program, &resolved);
    let mut diags = typed.diagnostics.clone();
    diags.extend(pelago::values::check(&program, &resolved, &typed));
    diags.sort_by_key(|d| (d.module, d.span.start));
    diags.dedup();
    for d in &diags {
        let module = program.module(d.module);
        let index = LineIndex::new(&module.src);
        let (line, col) = index.line_col(&module.src, d.span.start);
        let kind = if d.warning { "warning" } else { "error" };
        eprintln!("{}:{line}:{col}: {kind}: {}", module.path, d.message);
    }
    let warnings = diags.iter().filter(|d| d.warning).count();
    let diags: Vec<_> = diags.into_iter().filter(|d| !d.warning).collect();
    let modules = program.modules.len();
    println!(
        "{} module{}, {} definitions, {} error{}",
        modules,
        if modules == 1 { "" } else { "s" },
        resolved.defs.len(),
        diags.len(),
        if diags.len() == 1 { "" } else { "s" }
    );
    if warnings > 0 {
        println!("{warnings} warning{}", if warnings == 1 { "" } else { "s" });
    }
    if !diags.is_empty() {
        std::process::exit(1);
    }
}

/// Checks the program, lowers it to the IR, verifies it and prints it.
fn ir_or_run(path: &str, cmd: &str, flags: &[&str], out_name: Option<String>) {
    let run = cmd == "run";
    let build = cmd == "build";
    let native = build || flags.contains(&"--native");
    let mut program = pelago::package::Program::with_core();
    if let Err(e) = program.add_package_from_path(std::path::Path::new(path)) {
        eprintln!("{e}");
        std::process::exit(2);
    }
    let resolved = pelago::resolve::resolve(&program);
    let typed = pelago::typecheck::check(&program, &resolved);
    let mut diags = typed.diagnostics.clone();
    diags.extend(pelago::values::check(&program, &resolved, &typed));
    let mut failed = false;
    for d in diags.iter().filter(|d| !d.warning) {
        let module = program.module(d.module);
        let index = LineIndex::new(&module.src);
        let (line, col) = index.line_col(&module.src, d.span.start);
        eprintln!("{}:{line}:{col}: error: {}", module.path, d.message);
        failed = true;
    }
    if failed {
        std::process::exit(1);
    }
    let lowered = pelago::lower::lower(&program, &resolved, &typed);
    for d in &lowered.diagnostics {
        let module = program.module(d.module);
        let index = LineIndex::new(&module.src);
        let (line, col) = index.line_col(&module.src, d.span.start);
        let kind = if d.warning { "note" } else { "error" };
        eprintln!("{}:{line}:{col}: {kind}: {}", module.path, d.message);
        if !d.warning {
            failed = true;
        }
    }
    for e in pelago::ir::verify_module(&lowered.module) {
        eprintln!("ir error: {e}");
        failed = true;
    }
    let mut module = lowered.module;
    let before = pelago::ir::opt::stats_of(&module);
    // Native builds optimise unless `--no-opt`; the interpreter and `ir` only with `--opt`.
    let optimise = if native {
        !flags.contains(&"--no-opt")
    } else {
        flags.contains(&"--opt")
    };
    if optimise {
        pelago::ir::opt::optimize(&mut module, pelago::ir::opt::Options::default());
        pelago::ir::opt::prune(&mut module, &["main"]);
    }
    if flags.contains(&"--stats") {
        let after = pelago::ir::opt::stats_of(&module);
        eprintln!("before: {before:?}");
        eprintln!("after:  {after:?}");
    }
    if native {
        if failed {
            std::process::exit(1);
        }
        native_build(&module, path, build, flags, out_name);
        return;
    }
    if run {
        if failed {
            std::process::exit(1);
        }
        let out = pelago::ir::run_full(&module, "main", &[]);
        print!("{}", out.output);
        if flags.contains(&"--stats") {
            eprintln!("steps: {}", out.steps);
        }
        if let Err(e) = out.result {
            eprintln!("stopped: {e}");
            std::process::exit(1);
        }
        if out.leaked > 0 {
            eprintln!("leaked {} allocation(s)", out.leaked);
        }
        return;
    }
    print!("{}", pelago::ir::print_module(&module));
    if failed {
        std::process::exit(1);
    }
}

/// Builds a native executable (`build`) or builds it into a temporary file and runs it (`run --native`).
fn native_build(
    module: &pelago::ir::Module,
    path: &str,
    build: bool,
    flags: &[&str],
    out_name: Option<String>,
) {
    let os = match flags.iter().find_map(|f| f.strip_prefix("--target=")) {
        Some("windows") => pelago::native::Os::Windows,
        Some("linux") => pelago::native::Os::Linux,
        Some(other) => {
            eprintln!("unknown target `{other}` (use --target=linux or --target=windows)");
            std::process::exit(2);
        }
        None => pelago::native::Os::host().unwrap_or(pelago::native::Os::Linux),
    };
    if !build && Some(os) != pelago::native::Os::host() {
        eprintln!("this program was built for another system; use `pelago build` and run it there");
        std::process::exit(2);
    }
    let opts = pelago::native::gen::Options {
        leak_check: flags.contains(&"--leak-check"),
        target: os,
        regalloc: !flags.contains(&"--no-regalloc"),
    };
    let image = match pelago::native::gen::compile(module, "main", &opts) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("native build failed: {e}");
            std::process::exit(1);
        }
    };
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let target = match (&out_name, build) {
        (Some(o), _) => std::path::PathBuf::from(o),
        (None, true) => {
            let stem = std::path::Path::new(path)
                .file_stem()
                .map_or("a".to_string(), |s| s.to_string_lossy().into_owned());
            std::path::PathBuf::from(if os == pelago::native::Os::Windows {
                format!("{stem}.exe")
            } else {
                stem
            })
        }
        (None, false) => std::env::temp_dir().join(format!(
            "pelago-run-{}{}",
            std::process::id(),
            if os == pelago::native::Os::Windows {
                ".exe"
            } else {
                ""
            }
        )),
    };
    if let Err(e) = std::fs::write(&target, &image) {
        eprintln!("cannot write `{}`: {e}", target.display());
        std::process::exit(2);
    }
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    if build {
        println!("wrote {} ({} bytes)", target.display(), image.len());
        return;
    }
    let status = std::process::Command::new(&target).status();
    let _ = std::fs::remove_file(&target);
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("cannot run the native program: {e} (native code runs on x86-64 Linux only for now)");
            std::process::exit(1);
        }
    }
}

fn read_source(path: &str) -> String {
    let src = if path == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map(|_| text)
    } else {
        std::fs::read_to_string(path)
    };
    match src {
        Ok(text) => text,
        Err(err) => {
            eprintln!("cannot read `{path}`: {err}");
            std::process::exit(2);
        }
    }
}

fn parse_file(path: &str, keep_docs: bool) {
    let src = read_source(path);
    let (out, lex_errors) =
        pelago::parser::parse_and_check_with(&src, pelago::parser::ParseOptions { keep_docs });
    println!("{}", pelago::ast_print::print_file(&out.file, &src));
    let index = LineIndex::new(&src);
    for error in &lex_errors {
        let (line, col) = index.line_col(&src, error.span.start);
        eprintln!("{path}:{line}:{col}: error: {}", error.kind.message());
    }
    for error in &out.errors {
        let (line, col) = index.line_col(&src, error.span.start);
        eprintln!("{path}:{line}:{col}: error: {}", error.message);
    }
    if !lex_errors.is_empty() || !out.errors.is_empty() {
        std::process::exit(1);
    }
}

/// Times lexing, parsing, checking and freeing the tree separately, so compile speed is measured
/// from the first compiler.
fn parse_bench(path: &str) {
    use std::time::{Duration, Instant};
    let src = read_source(path);
    let runs = 10;
    let (mut lex, mut parse, mut check, mut free) =
        (Duration::MAX, Duration::MAX, Duration::MAX, Duration::MAX);
    let mut tokens = 0;
    for _ in 0..runs {
        let t0 = Instant::now();
        let lexed = tokenize(&src);
        let t1 = Instant::now();
        let out = pelago::parser::parse(&src, &lexed.tokens);
        let t2 = Instant::now();
        let errors = pelago::check::check_file(&out.file, &src);
        let t3 = Instant::now();
        std::hint::black_box(&errors);
        drop(out);
        let t4 = Instant::now();
        lex = lex.min(t1 - t0);
        parse = parse.min(t2 - t1);
        check = check.min(t3 - t2);
        free = free.min(t4 - t3);
        tokens = lexed.tokens.len();
    }
    let mb = src.len() as f64 / 1e6;
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let rate = |d: Duration| mb / d.as_secs_f64();
    println!("{mb:.1} MB, {tokens} tokens, best of {runs}:");
    println!("  lex    {:7.1} ms  {:5.0} MB/s", ms(lex), rate(lex));
    println!("  parse  {:7.1} ms  {:5.0} MB/s", ms(parse), rate(parse));
    println!("  check  {:7.1} ms  {:5.0} MB/s", ms(check), rate(check));
    println!("  free   {:7.1} ms", ms(free));
    let all = lex + parse + check;
    println!(
        "  total  {:7.1} ms  {:5.0} MB/s (without freeing)",
        ms(all),
        rate(all)
    );
}

fn lex_file(path: &str) {
    let src = read_source(path);
    let out = tokenize(&src);
    let index = LineIndex::new(&src);
    for token in &out.tokens {
        if token.kind == TokenKind::Newline {
            continue;
        }
        let (line, col) = index.line_col(&src, token.span.start);
        println!("{line}:{col}\t{:?}\t{:?}", token.kind, token.text(&src));
    }
    for error in &out.errors {
        let (line, col) = index.line_col(&src, error.span.start);
        eprintln!("{path}:{line}:{col}: error: {}", error.kind.message());
    }
    if !out.errors.is_empty() {
        std::process::exit(1);
    }
}

/// Times lexing alone (no printing), so compile speed is measured from the first compiler.
fn lex_bench(path: &str) {
    let src = std::fs::read_to_string(path).unwrap_or_else(|err| {
        eprintln!("cannot read `{path}`: {err}");
        std::process::exit(2);
    });
    let runs = 10;
    let mut best = std::time::Duration::MAX;
    let mut tokens = 0;
    for _ in 0..runs {
        let start = std::time::Instant::now();
        let out = tokenize(&src);
        best = best.min(start.elapsed());
        tokens = out.tokens.len();
        std::hint::black_box(&out);
    }
    let mb = src.len() as f64 / 1e6;
    println!(
        "{mb:.1} MB, {tokens} tokens, best of {runs}: {:.1} ms ({:.0} MB/s, {:.0} M tokens/s)",
        best.as_secs_f64() * 1e3,
        mb / best.as_secs_f64(),
        tokens as f64 / best.as_secs_f64() / 1e6
    );
}
