use std::collections::HashMap;

fn main() {
    let n = 500000usize;
    let mut seq: Vec<i64> = Vec::new();
    let mut s: i64 = 1;
    for _ in 0..n {
        s = (s * 1103515245 + 12345) & 2147483647;
        seq.push((s >> 16) & 3);
    }
    for k in [1usize, 2, 4, 6, 9] {
        let mut m: HashMap<i64, i64> = HashMap::new();
        let mask = (1i64 << (2 * k)) - 1;
        let mut w = 0i64;
        for i in 0..n {
            w = ((w << 2) | seq[i]) & mask;
            if i + 1 >= k {
                *m.entry(w).or_insert(0) += 1;
            }
        }
        let (mut most, mut total) = (0i64, 0i64);
        for (_, &c) in &m {
            total += c;
            if c > most {
                most = c;
            }
        }
        println!("{} {} {} {}", k, m.len(), most, total);
    }
}
