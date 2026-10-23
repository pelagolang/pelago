fn main() {
    let mut table = [0i64; 256];
    for n in 0..256i64 { let mut c = n; for _ in 0..8 { c = if c & 1 == 1 { 3988292384 ^ (c >> 1) } else { c >> 1 }; } table[n as usize] = c; }
    let (mut crc, mut s) = (4294967295i64, 1i64);
    for _ in 0..20000000 { s = (s * 1103515245 + 12345) & 2147483647; crc = table[((crc ^ (s >> 8)) & 255) as usize] ^ (crc >> 8); }
    println!("{}", crc ^ 4294967295);
}
