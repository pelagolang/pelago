use std::collections::HashMap;
fn main() {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for i in 0..200000i64 { *counts.entry(format!("w{}", i * 7919 % 5003)).or_insert(0) += 1; }
    let mut t = 0i64;
    for k in 0..5003i64 { t += counts.get(&format!("w{}", k)).copied().unwrap_or(0) * (k & 15); }
    println!("{} {}", counts.len(), t);
}
