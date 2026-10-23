use std::collections::HashMap;
fn main() {
    let mut m: HashMap<i64, i64> = HashMap::new();
    for i in 0..200000i64 { m.insert(i, i * 3); }
    let mut t = 0i64;
    for i in 0..200000i64 { t += m.get(&i).copied().unwrap_or(0); }
    println!("{} {}", m.len(), t);
}
