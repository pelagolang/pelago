#[derive(Clone, Copy)]
struct Vec2 { x: i64, y: i64 }
impl Vec2 {
    fn add(self, o: Vec2) -> Vec2 { Vec2 { x: self.x + o.x, y: self.y + o.y } }
    fn dot(self, o: Vec2) -> i64 { self.x * o.x + self.y * o.y }
}
fn main() {
    let mut p = Vec2 { x: 1, y: 2 };
    let d = Vec2 { x: 3, y: 1 };
    let mut t = 0i64;
    for _ in 0..10000000 {
        p = p.add(d);
        t += p.dot(d) & 1023;
        p = Vec2 { x: p.x & 65535, y: p.y & 65535 };
    }
    println!("{}", t);
}
