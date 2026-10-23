// After kostya/benchmarks matmul (written by Attractive Chaos, MIT license): float matrices, second matrix transposed.
type Matrix = Vec<Vec<f64>>;

fn gen(n: usize, seed: f64) -> Matrix {
    let tmp = seed / n as f64 / n as f64;
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            m[i][j] = tmp * (i as f64 - j as f64) * (i as f64 + j as f64);
        }
    }
    m
}

fn mul(n: usize, a: &Matrix, b: &Matrix) -> Matrix {
    let mut m = vec![vec![0.0; n]; n];
    let mut c = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            c[i][j] = b[j][i];
        }
    }
    for i in 0..n {
        for j in 0..n {
            let mut t = 0.0;
            for k in 0..n {
                t += a[i][k] * c[j][k];
            }
            m[i][j] = t;
        }
    }
    m
}

fn calc(size: usize) -> f64 {
    let n = size / 2 * 2;
    let a = gen(n, 1.0);
    let b = gen(n, 2.0);
    let m = mul(n, &a, &b);
    m[n / 2][n / 2]
}

fn main() {
    println!("{:.6}", calc(400));
}
