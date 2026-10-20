fn digit_sum(start: i64) -> i64 {
    let (mut n, mut s) = (start, 0);
    while n != 0 { s += n % 10; n /= 10; }
    s
}
fn main() {
    let mut total = 0i64;
    for i in 1..3000001i64 { total += digit_sum(i) + i % 3 + i / 7; }
    println!("{}", total);
}
