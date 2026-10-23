use std::hint::black_box;
fn main() {
    let mut s: i64 = black_box(1);
    for _ in 0..1000 {
        s = (s * 1103515245 + 12345) & 2147483647;
    }
    let u = 20 + s % 30;
    let r = ((s >> 8) % 10000) as usize;
    let mut a = vec![0i64; 10000];
    for i in 0..10000 {
        for j in 0..10000i64 {
            a[i] = a[i] + j % u;
        }
        a[i] += r as i64;
    }
    println!("{}", a[r]);
}
