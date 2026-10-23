fn main() {
    let n = 256usize;
    let a: Vec<i64> = (0..n * n).map(|i| (i as i64 * 7) & 15).collect();
    let b: Vec<i64> = (0..n * n).map(|i| (i as i64 * 13 + 3) & 15).collect();
    let mut c = vec![0i64; n * n];
    for i in 0..n { for j in 0..n {
        let mut sum = 0i64;
        for k in 0..n { sum += a[i * n + k] * b[k * n + j]; }
        c[i * n + j] = sum;
    } }
    let mut t = 0i64; for i in 0..n * n { t += c[i] * (i as i64 & 7); }
    println!("{}", t);
}
