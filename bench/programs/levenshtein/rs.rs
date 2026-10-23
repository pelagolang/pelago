fn dist(a: &[i64], b: &[i64]) -> i64 {
    let mut prev: Vec<i64> = (0..=b.len() as i64).collect();
    let mut cur = vec![0i64; b.len() + 1];
    for i in 0..a.len() {
        cur[0] = i as i64 + 1;
        for j in 0..b.len() {
            let cost = if a[i] == b[j] { 0 } else { 1 };
            let mut best = prev[j] + cost;
            if prev[j + 1] + 1 < best {
                best = prev[j + 1] + 1;
            }
            if cur[j] + 1 < best {
                best = cur[j] + 1;
            }
            cur[j + 1] = best;
        }
        prev.copy_from_slice(&cur);
    }
    prev[b.len()]
}

fn main() {
    let n = 600;
    let mut words: Vec<Vec<i64>> = Vec::new();
    let mut s: i64 = 7;
    for _ in 0..n {
        s = (s * 1103515245 + 12345) & 2147483647;
        let len = 8 + (s >> 8) % 40;
        let mut w = Vec::new();
        for _ in 0..len {
            s = (s * 1103515245 + 12345) & 2147483647;
            w.push(97 + (s >> 8) % 26);
        }
        words.push(w);
    }
    let mut total = 0;
    for i in 0..n {
        for j in i + 1..n {
            total += dist(&words[i], &words[j]);
        }
    }
    println!("{}", total);
}
