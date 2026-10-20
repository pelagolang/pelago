fn main() {
    let mut s = String::new();
    let mut t = 0usize;
    for _ in 0..20000 {
        s = s + "ab";
        t += s.len() & 7;
    }
    println!("{} {}", s.len(), t);
}
