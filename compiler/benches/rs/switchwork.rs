#[derive(Clone, Copy)]
enum Op { Add(i64), Mul(i64), Xor(i64), Shl(i64), Neg, Skip }
fn apply(acc: i64, o: Op) -> i64 {
    match o {
        Op::Add(n) => (acc + n) & 1048575,
        Op::Mul(n) => (acc * n) & 1048575,
        Op::Xor(n) => acc ^ n,
        Op::Shl(n) => (acc << n) & 1048575,
        Op::Neg => 1048575 - acc,
        Op::Skip => acc,
    }
}
fn main() {
    let mut prog = Vec::new();
    let mut s = 7i64;
    for _ in 0..1024 {
        s = (s * 1103515245 + 12345) & 2147483647;
        let k = (s >> 8) & 255;
        prog.push(match (s >> 20) % 6 {
            0 => Op::Add(k), 1 => Op::Mul(k + 1), 2 => Op::Xor(k), 3 => Op::Shl(k & 7), 4 => Op::Neg, _ => Op::Skip,
        });
    }
    let mut acc = 1i64;
    for _ in 0..20000 { for i in 0..prog.len() { acc = apply(acc, prog[i]); } }
    println!("{}", acc);
}
