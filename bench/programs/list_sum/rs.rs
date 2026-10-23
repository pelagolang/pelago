fn main() {
    let mut xs: Vec<i64> = Vec::new();
    for i in 0..5000000i64 { xs.push(i); }
    let mut t = 0i64;
    for i in 0..xs.len() { t += xs[i]; }
    println!("{}", t);
}
