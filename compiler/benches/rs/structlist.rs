struct Body { x: i64, y: i64, vx: i64, vy: i64 }
fn main() {
    let mut bs: Vec<Body> = Vec::new();
    for i in 0..1000i64 { bs.push(Body { x: i * 7, y: i * 13, vx: (i & 7) - 3, vy: (i & 3) - 1 }); }
    for _ in 0..20000 {
        for i in 0..bs.len() {
            bs[i].x = (bs[i].x + bs[i].vx) & 65535;
            bs[i].y = (bs[i].y + bs[i].vy) & 65535;
        }
    }
    let mut t = 0i64;
    for i in 0..bs.len() { t += bs[i].x ^ bs[i].y; }
    println!("{}", t);
}
