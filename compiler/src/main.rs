//! Command-line entry point. Only inspection commands exist so far.

use langc::keyword::{Keyword, Strength};
use langc::lexer::tokenize;
use langc::span::LineIndex;
use langc::token::{Punct, TokenKind};

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("keywords") => print_keywords(),
        Some("tokens") => print_punct(),
        Some("lex-bench") => match args.next() {
            Some(path) => lex_bench(&path),
            None => {
                eprintln!("usage: langc lex-bench <file>");
                std::process::exit(2);
            }
        },
        Some("parse") => match args.next() {
            Some(flag) if flag == "--no-docs" => match args.next() {
                Some(path) => parse_file(&path, false),
                None => {
                    eprintln!("usage: langc parse [--no-docs] <file>");
                    std::process::exit(2);
                }
            },
            Some(path) => parse_file(&path, true),
            None => {
                eprintln!("usage: langc parse <file>");
                std::process::exit(2);
            }
        },
        Some("check") => match args.next() {
            Some(path) => check_path(&path),
            None => {
                eprintln!("usage: langc check <file or directory>");
                std::process::exit(2);
            }
        },
        Some("parse-bench") => match args.next() {
            Some(path) => parse_bench(&path),
            None => {
                eprintln!("usage: langc parse-bench <file>");
                std::process::exit(2);
            }
        },
        Some("lex") => match args.next() {
            Some(path) => lex_file(&path),
            None => {
                eprintln!("usage: langc lex <file>");
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
    eprintln!("usage: langc <command>");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  keywords   list every keyword, hard ones first");
    eprintln!("  tokens     list operators and punctuation");
    eprintln!("  lex-bench <file>  time the lexer alone on a file");
    eprintln!("  parse [--no-docs] <file>  print the syntax tree and any errors (`-` reads standard input)");
    eprintln!("  check <path>  parse and resolve names of a file or a directory of `.lang` files");
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

fn print_punct() {
    let texts: Vec<&str> = Punct::ALL.iter().map(|p| p.as_str()).collect();
    println!("{} tokens", texts.len());
    println!("  {}", texts.join(" "));
}

fn check_path(path: &str) {
    let mut program = langc::package::Program::with_core();
    if let Err(e) = program.add_package_from_path(std::path::Path::new(path)) {
        eprintln!("{e}");
        std::process::exit(2);
    }
    let resolved = langc::resolve::resolve(&program);
    let typed = langc::typecheck::check(&program, &resolved);
    let mut diags = typed.diagnostics.clone();
    diags.sort_by_key(|d| (d.module, d.span.start));
    diags.dedup();
    for d in &diags {
        let module = program.module(d.module);
        let index = LineIndex::new(&module.src);
        let (line, col) = index.line_col(&module.src, d.span.start);
        eprintln!("{}:{line}:{col}: error: {}", module.path, d.message);
    }
    let modules = program.modules.len();
    println!(
        "{} module{}, {} definitions, {} error{}",
        modules,
        if modules == 1 { "" } else { "s" },
        resolved.defs.len(),
        diags.len(),
        if diags.len() == 1 { "" } else { "s" }
    );
    if !diags.is_empty() {
        std::process::exit(1);
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
        langc::parser::parse_and_check_with(&src, langc::parser::ParseOptions { keep_docs });
    println!("{}", langc::ast_print::print_file(&out.file, &src));
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
        let out = langc::parser::parse(&src, &lexed.tokens);
        let t2 = Instant::now();
        let errors = langc::check::check_file(&out.file, &src);
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
