fn main() {
    let n = 1000000usize;
    let mut xs: Vec<i64> = Vec::new();
    let mut s = 12345i64;
    for _ in 0..n {
        s = (s * 1103515245 + 12345) & 2147483647;
        xs.push(s);
    }
    xs.sort();
    let mut t = 0i64;
    for i in 0..xs.len() { t += (xs[i] & 255) * (i as i64 & 15); }
    println!("{} {} {}", xs[0], xs[xs.len() - 1], t);
}
