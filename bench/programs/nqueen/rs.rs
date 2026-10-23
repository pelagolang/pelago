fn solve(n: i64, cols: i64, d1: i64, d2: i64) -> i64 {
    let all = (1i64 << n) - 1;
    if cols == all {
        return 1;
    }
    let mut count = 0;
    let mut avail = all ^ (cols | d1 | d2);
    while avail != 0 {
        let bit = avail & -avail;
        avail ^= bit;
        count += solve(n, cols | bit, ((d1 | bit) << 1) & all, (d2 | bit) >> 1);
    }
    count
}

fn main() {
    let n = 14;
    println!("{}", solve(n, 0, 0, 0));
}
