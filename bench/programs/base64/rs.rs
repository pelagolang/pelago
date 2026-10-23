fn main() {
    let n = 3000000usize;
    let mut table = vec![0i64; 64];
    for i in 0..26 {
        table[i] = 65 + i as i64;
        table[26 + i] = 97 + i as i64;
    }
    for i in 0..10 {
        table[52 + i] = 48 + i as i64;
    }
    table[62] = 43;
    table[63] = 47;
    let mut rev = vec![0i64; 256];
    for i in 0..64 {
        rev[table[i] as usize] = i as i64;
    }
    let mut data: Vec<i64> = Vec::new();
    let mut s: i64 = 1;
    for _ in 0..n {
        s = (s * 1103515245 + 12345) & 2147483647;
        data.push((s >> 8) & 255);
    }
    let (mut check, mut bad) = (0i64, 0i64);
    for _ in 0..3 {
        let mut enc: Vec<i64> = Vec::new();
        let mut i = 0;
        while i < n {
            let v = (data[i] << 16) | (data[i + 1] << 8) | data[i + 2];
            enc.push(table[((v >> 18) & 63) as usize]);
            enc.push(table[((v >> 12) & 63) as usize]);
            enc.push(table[((v >> 6) & 63) as usize]);
            enc.push(table[(v & 63) as usize]);
            i += 3;
        }
        for &c in &enc {
            check = (check * 31 + c) & 2147483647;
        }
        let (mut j, mut i) = (0, 0);
        while j < enc.len() {
            let v = (rev[enc[j] as usize] << 18) | (rev[enc[j + 1] as usize] << 12) | (rev[enc[j + 2] as usize] << 6) | rev[enc[j + 3] as usize];
            if data[i] != (v >> 16) & 255 {
                bad += 1;
            }
            if data[i + 1] != (v >> 8) & 255 {
                bad += 1;
            }
            if data[i + 2] != v & 255 {
                bad += 1;
            }
            i += 3;
            j += 4;
        }
    }
    println!("{} {}", check, bad);
}
