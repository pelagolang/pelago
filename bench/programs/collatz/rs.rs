fn steps(mut n: i64) -> i64 {
    let mut s = 0;
    while n != 1 {
        if n % 2 == 0 { n /= 2 } else { n = 3 * n + 1 }
        s += 1;
    }
    s
}
fn main() {
    let mut t = 0i64;
    for i in 1..300001 { t += steps(i); }
    println!("{}", t);
}
