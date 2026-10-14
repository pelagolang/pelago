//! Compile-time code (`meta`, card A-05). Stage M1: `meta if` and the `target` record; stage M2:
//! values as generic parameters (`meta N: int`), whose arithmetic is evaluated here too.
//!
//! A `meta if` condition is evaluated while resolving names, from literals, the fields of `target`,
//! bare words that stand for a name (`linux`, `little`), `==`, `!=`, `&&`, `||` and `!`. Only the
//! chosen branch is resolved, checked and lowered; the other is parsed but otherwise left alone.

use crate::ast::*;
use crate::span::Span;
use crate::token::Punct;

/// The machine the program is built for. Defaults to the machine the compiler runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub os: String,
    pub arch: String,
    pub pointer_bits: i64,
    pub endian: String,
    pub debug: bool,
}

impl Target {
    pub fn host() -> Target {
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else if cfg!(target_arch = "x86") {
            "x86"
        } else {
            "x86_64"
        };
        Target {
            os: os.to_string(),
            arch: arch.to_string(),
            pointer_bits: usize::BITS as i64,
            endian: if cfg!(target_endian = "big") { "big" } else { "little" }.to_string(),
            debug: cfg!(debug_assertions),
        }
    }
}

impl Default for Target {
    fn default() -> Target {
        Target::host()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum MVal {
    Bool(bool),
    Int(i128),
    Word(String),
}

/// Evaluates the condition of a `meta if`. The error holds the place and the message.
pub fn eval_cond(cond: &Expr, src: &str, target: &Target) -> Result<bool, (Span, String)> {
    match eval(cond, src, target)? {
        MVal::Bool(b) => Ok(b),
        _ => Err((cond.span, "a `meta if` condition must be true or false".to_string())),
    }
}

/// Which branch a `meta if` takes. A bad condition takes the `then` branch (the error is reported
/// once, by the resolver).
pub fn chooses_then(i: &IfExpr, src: &str, target: &Target) -> bool {
    eval_cond(&i.cond, src, target).unwrap_or(true)
}

fn eval(e: &Expr, src: &str, t: &Target) -> Result<MVal, (Span, String)> {
    let bad = |what: &str| {
        (
            e.span,
            format!(
                "{what} is not known at compile time; a `meta if` condition uses literals, `target.os`, `target.arch`, `target.pointer_bits`, `target.endian`, `target.debug`, `==`, `!=`, `&&`, `||` and `!`"
            ),
        )
    };
    match &e.kind {
        ExprKind::Paren(i) => eval(i, src, t),
        ExprKind::Bool(b) => Ok(MVal::Bool(*b)),
        ExprKind::Int => src[e.span.start as usize..e.span.end as usize]
            .replace('_', "")
            .parse::<i128>()
            .map(MVal::Int)
            .map_err(|_| bad("this number")),
        ExprKind::Name(_) => Err(bad("this name")),
        ExprKind::Field { base, name, .. } => match &base.kind {
            ExprKind::Name(b) if b.text(src) == "target" => match name.text(src) {
                "os" => Ok(MVal::Word(t.os.clone())),
                "arch" => Ok(MVal::Word(t.arch.clone())),
                "endian" => Ok(MVal::Word(t.endian.clone())),
                "pointer_bits" => Ok(MVal::Int(t.pointer_bits as i128)),
                "debug" => Ok(MVal::Bool(t.debug)),
                other => Err((e.span, format!("`target` has no field `{other}`"))),
            },
            _ => Err(bad("this value")),
        },
        ExprKind::Unary {
            op: Punct::Bang,
            operand,
        } => match eval(operand, src, t)? {
            MVal::Bool(b) => Ok(MVal::Bool(!b)),
            _ => Err(bad("this value")),
        },
        ExprKind::Binary { op, lhs, rhs } => {
            // A bare word next to a `target` field names a value: `target.os == linux`.
            let word = |x: &Expr| match &x.kind {
                ExprKind::Name(id) => Some(MVal::Word(id.text(src).to_string())),
                _ => None,
            };
            let is_target = |x: &Expr| matches!(&x.kind, ExprKind::Field { base, .. } if matches!(&base.kind, ExprKind::Name(b) if b.text(src) == "target"));
            let side = |x: &Expr, other: &Expr| match word(x) {
                Some(w) if is_target(other) => Ok(w),
                _ => eval(x, src, t),
            };
            let (a, b) = (side(lhs, rhs)?, side(rhs, lhs)?);
            match (op, a, b) {
                (Punct::EqEq, a, b) => Ok(MVal::Bool(a == b)),
                (Punct::NotEq, a, b) => Ok(MVal::Bool(a != b)),
                (Punct::AndAnd, MVal::Bool(a), MVal::Bool(b)) => Ok(MVal::Bool(a && b)),
                (Punct::OrOr, MVal::Bool(a), MVal::Bool(b)) => Ok(MVal::Bool(a || b)),
                (Punct::Lt, MVal::Int(a), MVal::Int(b)) => Ok(MVal::Bool(a < b)),
                (Punct::LtEq, MVal::Int(a), MVal::Int(b)) => Ok(MVal::Bool(a <= b)),
                (Punct::Gt, MVal::Int(a), MVal::Int(b)) => Ok(MVal::Bool(a > b)),
                (Punct::GtEq, MVal::Int(a), MVal::Int(b)) => Ok(MVal::Bool(a >= b)),
                _ => Err(bad("this operation")),
            }
        }
        _ => Err(bad("this expression")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Program;
    use crate::resolve::resolve;

    fn diags(src: &str, target: Target) -> Vec<String> {
        let mut p = Program::with_core();
        p.target = target;
        p.add_package("app", &[("main", src)]);
        let res = resolve(&p);
        let typed = crate::typecheck::check(&p, &res);
        typed.diagnostics.iter().map(|d| d.message.clone()).collect()
    }

    fn linux() -> Target {
        Target {
            os: "linux".into(),
            ..Target::host()
        }
    }

    fn windows() -> Target {
        Target {
            os: "windows".into(),
            ..Target::host()
        }
    }

    const PLATFORM: &str = "meta if target.os == linux {\n fn platform() -> string = \"linux\"\n} else {\n fn platform() -> string = \"other\"\n}\nfn t() -> string = platform()\n";

    #[test]
    fn the_chosen_declarations_are_the_only_ones() {
        assert!(diags(PLATFORM, linux()).is_empty());
        assert!(diags(PLATFORM, windows()).is_empty());
    }

    #[test]
    fn the_other_branch_is_not_checked() {
        let src = "meta if target.os == windows {\n fn only_windows() -> int = no_such_function()\n}\nfn t() -> int = 1\n";
        assert!(diags(src, linux()).is_empty());
        assert!(!diags(src, windows()).is_empty());
    }

    #[test]
    fn inside_a_function_only_the_chosen_branch_counts() {
        let src = "fn t() -> int { meta if target.pointer_bits == 64 { 8 } else { missing_name } }\n";
        let t64 = Target {
            pointer_bits: 64,
            ..Target::host()
        };
        let t32 = Target {
            pointer_bits: 32,
            ..Target::host()
        };
        assert!(diags(src, t64).is_empty());
        assert!(!diags(src, t32).is_empty());
    }

    #[test]
    fn combined_conditions() {
        let src = "meta if target.os == linux && !target.debug || target.arch == riscv {\n fn f() -> int = 1\n} else {\n fn f() -> int = 2\n}\nfn t() -> int = f()\n";
        assert!(diags(src, linux()).is_empty());
    }

    #[test]
    fn a_condition_that_is_not_constant_is_an_error() {
        let src = "fn t(n: int) -> int { meta if n == 1 { 1 } else { 2 } }\n";
        let e = diags(src, linux());
        assert!(e.iter().any(|m| m.contains("not known at compile time")), "{e:?}");
    }
}
