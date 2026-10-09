//! Stage 0 compiler for the language (name to be decided).
//!
//! This crate is deliberately dependency-free: a language that cares about
//! compile speed should start with a compiler that builds fast.

pub mod keyword;
pub mod span;
pub mod token;
