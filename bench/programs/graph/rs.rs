fn main() {
    let n = 200000usize;
    let mut adj = Vec::new();
    let mut s: i64 = 99;
    for _ in 0..n * 4 { s = (s * 1103515245 + 12345) & 2147483647; adj.push(s as usize % n); }
    let mut dist = vec![-1i64; n];
    let mut queue = vec![0usize; n];
    let (mut head, mut tail) = (0usize, 1usize);
    dist[0] = 0;
    while head < tail {
        let u = queue[head]; head += 1;
        for k in 0..4 { let v = adj[u * 4 + k]; if dist[v] < 0 { dist[v] = dist[u] + 1; queue[tail] = v; tail += 1; } }
    }
    let mut t = 0i64; for i in 0..n { t += dist[i] + 1; }
    println!("{} {}", tail, t);
}
