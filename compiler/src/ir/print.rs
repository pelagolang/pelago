//! The text form of the IR. It is also the format of the IR tests.

use super::*;

pub fn print_module(m: &Module) -> String {
    m.funcs
        .iter()
        .map(|f| print_func(f, Some(m)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prints one function. Calls show the callee's name when the module is given.
pub fn print_func(f: &Func, m: Option<&Module>) -> String {
    let mut out = String::new();
    let params: Vec<String> = f.blocks[0]
        .params
        .iter()
        .map(|v| format!("v{}: {}", v.0, f.ty(*v).name()))
        .collect();
    out.push_str(&format!("fn {}({})", f.name, params.join(", ")));
    if f.ret != IrTy::Void {
        out.push_str(&format!(" -> {}", f.ret.name()));
    }
    out.push_str(" {\n");
    for (i, b) in f.blocks.iter().enumerate() {
        if i == 0 {
            out.push_str("  b0:\n");
        } else {
            let ps: Vec<String> = b
                .params
                .iter()
                .map(|v| format!("v{}: {}", v.0, f.ty(*v).name()))
                .collect();
            if ps.is_empty() {
                out.push_str(&format!("  b{i}:\n"));
            } else {
                out.push_str(&format!("  b{i}({}):\n", ps.join(", ")));
            }
        }
        for &id in &b.insts {
            out.push_str(&format!("    {}\n", inst_text(f, &f.insts[id], m)));
        }
        match &b.term {
            Some(t) => out.push_str(&format!("    {}\n", term_text(t))),
            None => out.push_str("    <no terminator>\n"),
        }
    }
    out.push_str("}\n");
    out
}

fn v(x: Value) -> String {
    format!("v{}", x.0)
}

fn vs(xs: &[Value]) -> String {
    xs.iter().map(|x| v(*x)).collect::<Vec<_>>().join(", ")
}

fn target(t: &Target) -> String {
    if t.args.is_empty() {
        format!("b{}", t.block.0)
    } else {
        format!("b{}({})", t.block.0, vs(&t.args))
    }
}

fn term_text(t: &Term) -> String {
    match t {
        Term::Jump(t) => format!("jump {}", target(t)),
        Term::Branch(c, a, b) => format!("branch {}, {}, {}", v(*c), target(a), target(b)),
        Term::Return(Some(x)) => format!("return {}", v(*x)),
        Term::Return(None) => "return".to_string(),
        Term::Stop(msg) => format!("stop {msg:?}"),
        Term::Unreachable => "unreachable".to_string(),
    }
}

fn inst_text(f: &Func, i: &Inst, m: Option<&Module>) -> String {
    let body = match &i.op {
        Op::Const(ty, c) => {
            let c = match c {
                Const::Int(n) => n.to_string(),
                Const::Float(x) => format!("{x:?}"),
                Const::Bool(b) => b.to_string(),
            };
            format!("const {} {c}", ty.name())
        }
        Op::Bin(op, a, b) => format!("{} {}, {}", op.name(), v(*a), v(*b)),
        Op::Checked(op, signed, a, b) => {
            let u = if *signed { "" } else { "u" };
            format!("{u}{}_checked {}, {}", op.name(), v(*a), v(*b))
        }
        Op::Fma(a, b, c) => format!("fma {}, {}, {}", v(*a), v(*b), v(*c)),
        Op::Select(c, a, b) => format!("select {}, {}, {}", v(*c), v(*a), v(*b)),
        Op::CheckedIf(op, signed, test, a, b) => {
            let u = if *signed { "" } else { "u" };
            let test = match test {
                Test::Bool(c, true) => v(*c),
                Test::Bool(c, false) => format!("not {}", v(*c)),
                Test::Cmp(o, x, y) => format!("{} {}, {}", o.name(), v(*x), v(*y)),
            };
            format!("{u}{}_checked_if ({test}), {}, {}", op.name(), v(*a), v(*b))
        }
        Op::Cmp(op, a, b) => format!("{} {}, {}", op.name(), v(*a), v(*b)),
        Op::Neg(a) => format!("neg {}", v(*a)),
        Op::Not(a) => format!("not {}", v(*a)),
        Op::VUn(o, a) => match o {
            VUn::Splat => format!("vsplat {}", v(*a)),
            VUn::Shl(k) => format!("vshl {}, {k}", v(*a)),
            VUn::UShr(k) => format!("vushr {}, {k}", v(*a)),
            VUn::ReduceAdd => format!("vreduce_add {}", v(*a)),
        },
        Op::VBin(o, a, b) => format!("{} {}, {}", o.name(), v(*a), v(*b)),
        Op::Cast(k, a, ty) => format!("{} {} to {}", k.name(), v(*a), ty.name()),
        Op::Call(callee, args) => {
            let name = m
                .and_then(|m| m.funcs.get(callee.0 as usize))
                .map(|f| f.name.clone())
                .unwrap_or_else(|| format!("f{}", callee.0));
            format!("call @{name}({})", vs(args))
        }
        Op::Slot(size, align) => format!("slot {size}, {align}"),
        Op::PtrAdd(p, n) => format!("ptradd {}, {}", v(*p), v(*n)),
        Op::MemCopy(d, s, n) => format!("memcopy {}, {}, {}", v(*d), v(*s), v(*n)),
        Op::DataAddr(i) => format!("data d{i}"),
        Op::FuncAddr(f) => match m.and_then(|m| m.funcs.get(f.0 as usize)) {
            Some(g) => format!("funcaddr @{}", g.name),
            None => format!("funcaddr @f{}", f.0),
        },
        Op::CallPtr(ps, ret, f, args) => {
            let ps: Vec<String> = ps.iter().map(|t| t.name()).collect();
            format!(
                "callptr ({}) -> {}, {}({})",
                ps.join(", "),
                ret.name(),
                v(*f),
                vs(args)
            )
        }
        Op::Print(p, n) => format!("print {}, {}", v(*p), v(*n)),
        Op::Load(ty, p) => format!("load {}, {}", ty.name(), v(*p)),
        Op::Store(p, x) => format!("store {}, {}", v(*p), v(*x)),
        Op::Alloc(n) => format!("alloc {}", v(*n)),
        Op::Free(p) => format!("free {}", v(*p)),
        Op::Realloc(p, n) => format!("realloc {}, {}", v(*p), v(*n)),
        Op::CtxInit(b, n, c, e) => format!("ctxinit {}, {}, {}, {}", v(*b), v(*n), v(*c), v(*e)),
        Op::CtxSwitch(s, t) => format!("ctxswitch {}, {}", v(*s), v(*t)),
    };
    let _ = f;
    match i.result {
        Some(r) => format!("{} = {body}", v(r)),
        None => body,
    }
}
