//! Command-line entry point. Only inspection commands exist so far.

use langc::keyword::{Keyword, Strength};
use langc::token::Punct;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("keywords") => print_keywords(),
        Some("tokens") => print_punct(),
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
