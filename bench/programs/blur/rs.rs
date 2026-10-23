fn main() {
    let n = 1024usize;
    let mut a = Vec::with_capacity(n * n);
    let mut b = vec![0i64; n * n];
    let mut s: i64 = 5;
    for _ in 0..n * n { s = (s * 1103515245 + 12345) & 2147483647; a.push(s & 255); }
    for _ in 0..4 {
        for y in 1..n - 1 { for x in 1..n - 1 {
            let i = y * n + x;
            b[i] = (a[i - n - 1] + a[i - n] + a[i - n + 1] + a[i - 1] + a[i] + a[i + 1] + a[i + n - 1] + a[i + n] + a[i + n + 1]) / 9;
        } }
        for i in n..n * n - n { a[i] = b[i]; }
    }
    let mut t = 0i64; for i in 0..n * n { t += a[i] * (i as i64 & 7); }
    println!("{}", t);
}
