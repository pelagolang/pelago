// After kostya/benchmarks base64 (MIT, Copyright (c) 2014 Konstantin Makarchev): encode and decode a 128 KiB block into newly allocated buffers, with '=' padding.
fn encode(src: &[i64], table: &[i64]) -> Vec<i64> {
    let mut out = Vec::new();
    let full = src.len() - src.len() % 3;
    let mut i = 0;
    while i < full {
        let v = (src[i] << 16) | (src[i + 1] << 8) | src[i + 2];
        out.push(table[((v >> 18) & 63) as usize]);
        out.push(table[((v >> 12) & 63) as usize]);
        out.push(table[((v >> 6) & 63) as usize]);
        out.push(table[(v & 63) as usize]);
        i += 3;
    }
    let rest = src.len() % 3;
    if rest == 1 {
        let v = src[i] << 16;
        out.push(table[((v >> 18) & 63) as usize]);
        out.push(table[((v >> 12) & 63) as usize]);
        out.push(61);
        out.push(61);
    } else if rest == 2 {
        let v = (src[i] << 16) | (src[i + 1] << 8);
        out.push(table[((v >> 18) & 63) as usize]);
        out.push(table[((v >> 12) & 63) as usize]);
        out.push(table[((v >> 6) & 63) as usize]);
        out.push(61);
    }
    out
}

fn decode(src: &[i64], rev: &[i64]) -> Vec<i64> {
    let mut size = src.len();
    while size > 0 && (src[size - 1] == 61 || src[size - 1] == 10 || src[size - 1] == 13) {
        size -= 1;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= size {
        let v = (rev[src[i] as usize] << 18) | (rev[src[i + 1] as usize] << 12) | (rev[src[i + 2] as usize] << 6) | rev[src[i + 3] as usize];
        out.push((v >> 16) & 255);
        out.push((v >> 8) & 255);
        out.push(v & 255);
        i += 4;
    }
    let rest = size - i;
    if rest == 2 {
        let v = (rev[src[i] as usize] << 18) | (rev[src[i + 1] as usize] << 12);
        out.push((v >> 16) & 255);
    } else if rest == 3 {
        let v = (rev[src[i] as usize] << 18) | (rev[src[i + 1] as usize] << 12) | (rev[src[i + 2] as usize] << 6);
        out.push((v >> 16) & 255);
        out.push((v >> 8) & 255);
    }
    out
}

fn main() {
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
    let text = vec![97i64; 131072];
    let encoded = encode(&text, &table);
    let ok = if decode(&encoded, &rev).len() == text.len() { 1 } else { 0 };
    let (mut s_encoded, mut s_decoded) = (0usize, 0usize);
    for _ in 0..256 {
        s_encoded += encode(&text, &table).len();
    }
    for _ in 0..256 {
        s_decoded += decode(&encoded, &rev).len();
    }
    println!("{} {} {}", ok, s_encoded, s_decoded);
}
