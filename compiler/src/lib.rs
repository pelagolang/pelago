//! Stage 0 compiler for the language (name to be decided).
//!
//! This crate is deliberately dependency-free: a language that cares about
//! compile speed should start with a compiler that builds fast.

pub mod ast;
pub mod ast_print;
pub mod check;
pub mod ir;
pub mod keyword;
pub mod lexer;
pub mod lower;
pub mod package;
pub mod parser;
pub mod resolve;
pub mod span;
pub mod token;
pub mod typecheck;
pub mod types;
pub mod values;
pub mod visit;
