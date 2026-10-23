fn main() {
    // String::push_str grows its capacity geometrically, so this is linear overall.
    let mut s = String::new();
    let mut t = 0usize;
    for _ in 0..2000000 {
        s.push_str("ab");
        t += s.len() & 7;
    }
    println!("{} {}", s.len(), t);
}
