use std::fmt::Write;
fn sum_words(text: &str) -> Result<i64, std::num::ParseIntError> {
    let mut t = 0i64;
    for w in text.split(' ') {
        if w.is_empty() { continue; }
        t += w.parse::<i64>()? & 1023;
    }
    Ok(t)
}
fn main() {
    let mut s = String::new();
    for i in 0..300000 { write!(s, "{} ", i).unwrap(); }
    println!("{} {}", s.len(), sum_words(&s).unwrap_or(0));
}
