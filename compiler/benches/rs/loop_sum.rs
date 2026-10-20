fn main() {
    let mut t = 0i64;
    for i in 0..300000000i64 { t += i ^ (t & 7); }
    println!("{}", t);
}
