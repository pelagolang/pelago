fn main() {
    let size: i64 = 400;
    let mut total = 0i64;
    for py in 0..size { for px in 0..size {
        let cx = (px * 3500) / size - 2500; let cy = (py * 2000) / size - 1000;
        let (mut x, mut y, mut it) = (0i64, 0i64, 0i64);
        while it < 100 && (x * x + y * y) < 4000000 { let nx = (x * x - y * y) / 1000 + cx; y = (2 * x * y) / 1000 + cy; x = nx; it += 1; }
        total += it;
    } }
    println!("{}", total);
}
