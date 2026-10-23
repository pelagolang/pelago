fn main() {
    let mut bins = [0i64; 1024];
    let mut s: i64 = 3;
    for _ in 0..30000000 { s = (s * 1103515245 + 12345) & 2147483647; let b = ((s >> 10) & 1023) as usize; bins[b] += 1; }
    let mut t = 0i64; for i in 0..1024usize { t += bins[i] * (i as i64 & 15); }
    println!("{}", t);
}
