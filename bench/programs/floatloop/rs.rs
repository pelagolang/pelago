fn main() {
    let (mut x, mut v, mut e) = (0.5f64, 0.0f64, 0.0f64);
    for _ in 0..20000000 {
        let a = -x * 0.001;
        v += a * 0.01;
        x += v * 0.01;
        e += x * x + v * v;
    }
    println!("{:.6}", e);
}
