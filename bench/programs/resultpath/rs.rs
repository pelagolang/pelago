#[derive(Clone, Copy)]
enum Bad { Odd, Big }
fn halve(x: i64) -> Result<i64, Bad> { if x & 1 == 1 { return Err(Bad::Odd); } Ok(x >> 1) }
fn bound(x: i64) -> Result<i64, Bad> { if x > 1000000000 { return Err(Bad::Big); } Ok(x) }
fn step(x: i64) -> Result<i64, Bad> {
    let h = halve(x)?;
    let b = bound(h * 3)?;
    Ok(b + 1)
}
fn main() {
    let (mut okc, mut bad, mut t) = (0i64, 0i64, 0i64);
    for i in 0..20000000i64 {
        match step(i) { Ok(v) => { okc += 1; t += v & 255; } Err(_) => bad += 1 }
    }
    println!("{} {} {}", okc, bad, t);
}
