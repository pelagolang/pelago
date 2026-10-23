fn main() {
    let (im, ia, ic) = (139968i64, 3877i64, 29573i64);
    let codes: [i64; 15] = [97, 99, 103, 116, 66, 68, 72, 75, 77, 78, 82, 83, 86, 87, 89];
    let cum: [i64; 15] = [270, 390, 510, 780, 800, 820, 840, 860, 880, 900, 920, 940, 960, 980, 1000];
    let mut seed = 42i64;
    let mut sum = 0i64;
    for _ in 0..5000000 {
        seed = (seed * ia + ic) % im;
        let r = seed * 1000;
        let mut k = 0;
        while r >= im * cum[k] {
            k += 1;
        }
        sum = (sum * 31 + codes[k]) & 2147483647;
    }
    println!("{} {}", sum, seed);
}
