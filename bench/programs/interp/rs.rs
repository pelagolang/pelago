enum Expr { Num(i64), X, Add(Box<Expr>, Box<Expr>), Mul(Box<Expr>, Box<Expr>), Neg(Box<Expr>) }
fn build(d: i64, s: i64) -> Expr {
    if d == 0 { return if s & 1 == 1 { Expr::X } else { Expr::Num(s & 15) }; }
    let l = (s * 1103515245 + 12345) & 2147483647;
    let r = (s * 69069 + 1) & 2147483647;
    match s % 3 {
        0 => Expr::Add(Box::new(build(d - 1, l)), Box::new(build(d - 1, r))),
        1 => Expr::Mul(Box::new(build(d - 1, l)), Box::new(build(d - 1, r))),
        _ => Expr::Neg(Box::new(build(d - 1, l))),
    }
}
fn eval(e: &Expr, v: i64) -> i64 {
    match e {
        Expr::Num(n) => *n,
        Expr::X => v & 1048575,
        Expr::Add(a, b) => (eval(a, v) + eval(b, v)) & 1048575,
        Expr::Mul(a, b) => (eval(a, v) * eval(b, v)) & 1048575,
        Expr::Neg(a) => 1048575 - eval(a, v),
    }
}
fn main() { let e = build(14, 7); let mut t = 0i64; for v in 0..3000i64 { t = (t + eval(&e, v)) & 1048575; } println!("{}", t); }
