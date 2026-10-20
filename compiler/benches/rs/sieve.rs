fn main() {
    let n = 5000000usize;
    let mut f = vec![1i64; n + 1];
    let mut c = 0i64;
    for i in 2..=n {
        if f[i] == 1 {
            c += 1;
            let mut j = i * i;
            while j <= n { f[j] = 0; j += i; }
        }
    }
    println!("{}", c);
}
