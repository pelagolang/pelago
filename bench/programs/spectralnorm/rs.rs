fn a(i: i64, j: i64) -> f64 {
    1.0 / (((i + j) * (i + j + 1)) / 2 + i + 1) as f64
}

fn mul_av(v: &[f64], out: &mut [f64]) {
    for i in 0..v.len() {
        let mut s = 0.0;
        for j in 0..v.len() {
            s += a(i as i64, j as i64) * v[j];
        }
        out[i] = s;
    }
}

fn mul_atv(v: &[f64], out: &mut [f64]) {
    for i in 0..v.len() {
        let mut s = 0.0;
        for j in 0..v.len() {
            s += a(j as i64, i as i64) * v[j];
        }
        out[i] = s;
    }
}

fn main() {
    let n = 2000;
    let mut u = vec![1.0f64; n];
    let mut v = vec![0.0f64; n];
    let mut t = vec![0.0f64; n];
    for _ in 0..10 {
        mul_av(&u, &mut t);
        mul_atv(&t, &mut v);
        mul_av(&v, &mut t);
        mul_atv(&t, &mut u);
    }
    let (mut vbv, mut vv) = (0.0, 0.0);
    for i in 0..n {
        vbv += u[i] * v[i];
        vv += v[i] * v[i];
    }
    println!("{:.6}", (vbv / vv).sqrt());
}
