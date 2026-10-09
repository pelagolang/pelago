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

fn lex_file(path: &str) {
    let src = if path == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map(|_| text)
    } else {
        std::fs::read_to_string(path)
    };
    let src = match src {
        Ok(text) => text,
        Err(err) => {
            eprintln!("cannot read `{path}`: {err}");
            std::process::exit(2);
        }
    };
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
