fn main() {
    let cap: usize = 1024; let space: i64 = 8192;
    let mut key = vec![-1i64; cap]; let mut prev = vec![-1i64; cap]; let mut next = vec![-1i64; cap]; let mut slot_of = vec![-1i64; space as usize];
    let (mut head, mut tail, mut size, mut hits, mut s) = (-1i64, -1i64, 0i64, 0i64, 17i64);
    for _ in 0..5000000 {
        s = (s * 1103515245 + 12345) & 2147483647;
        let r = (s >> 8) % space; let k = if s & 3 == 0 { r % 512 } else { r };
        let mut slot = slot_of[k as usize];
        if slot >= 0 {
            hits += 1;
            if slot != head {
                let p = prev[slot as usize]; let n = next[slot as usize];
                next[p as usize] = n;
                if n >= 0 { prev[n as usize] = p; } else { tail = p; }
                prev[slot as usize] = -1; next[slot as usize] = head; prev[head as usize] = slot; head = slot;
            }
        } else {
            if size < cap as i64 { slot = size; size += 1; } else {
                slot = tail; slot_of[key[slot as usize] as usize] = -1; tail = prev[slot as usize];
                if tail >= 0 { next[tail as usize] = -1; } else { head = -1; }
            }
            key[slot as usize] = k; slot_of[k as usize] = slot; prev[slot as usize] = -1; next[slot as usize] = head;
            if head >= 0 { prev[head as usize] = slot; } else { tail = slot; }
            head = slot;
        }
    }
    println!("{} {}", hits, head);
}
