// After kostya/benchmarks primes (MIT, Copyright (c) 2014 Konstantin Makarchev): sieve of Atkin, digit trie, breadth-first walk from a prefix, sorted result.
fn main() {
    let limit: i64 = 5000000;
    let prefix: i64 = 32338;
    let mut sieve = vec![0i64; (limit + 1) as usize];
    let mut x = 1;
    while x * x < limit {
        let mut y = 1;
        while y * y < limit {
            let mut n = 4 * x * x + y * y;
            if n <= limit && (n % 12 == 1 || n % 12 == 5) {
                sieve[n as usize] ^= 1;
            }
            n = 3 * x * x + y * y;
            if n <= limit && n % 12 == 7 {
                sieve[n as usize] ^= 1;
            }
            n = 3 * x * x - y * y;
            if x > y && n <= limit && n % 12 == 11 {
                sieve[n as usize] ^= 1;
            }
            y += 1;
        }
        x += 1;
    }
    let mut r = 5;
    while r * r < limit {
        if sieve[r as usize] == 1 {
            let mut k = r * r;
            while k < limit {
                sieve[k as usize] = 0;
                k += r * r;
            }
        }
        r += 1;
    }
    let mut child: Vec<i64> = vec![0; 10];
    let mut term: Vec<i64> = vec![0];
    let mut nodes = 1i64;
    for p in 2..=limit {
        if p == 2 || p == 3 || (p >= 5 && sieve[p as usize] == 1) {
            let mut d = 1;
            while d * 10 <= p {
                d *= 10;
            }
            let mut node = 0usize;
            while d > 0 {
                let digit = ((p / d) % 10) as usize;
                if child[node * 10 + digit] == 0 {
                    child[node * 10 + digit] = nodes;
                    nodes += 1;
                    child.resize(child.len() + 10, 0);
                    term.push(0);
                }
                node = child[node * 10 + digit] as usize;
                d /= 10;
            }
            term[node] = 1;
        }
    }
    let mut head: i64 = 0;
    let mut d = 1;
    while d * 10 <= prefix {
        d *= 10;
    }
    while d > 0 && head >= 0 {
        let next = child[head as usize * 10 + ((prefix / d) % 10) as usize];
        head = if next == 0 { -1 } else { next };
        d /= 10;
    }
    let mut found: Vec<i64> = Vec::new();
    if head >= 0 {
        let mut queue: Vec<usize> = vec![head as usize];
        let mut values: Vec<i64> = vec![prefix];
        let mut at = 0;
        while at < queue.len() {
            let node = queue[at];
            let value = values[at];
            at += 1;
            if term[node] == 1 {
                found.push(value);
            }
            for digit in 0..10 {
                if child[node * 10 + digit] != 0 {
                    queue.push(child[node * 10 + digit] as usize);
                    values.push(value * 10 + digit as i64);
                }
            }
        }
    }
    found.sort();
    let sum: i64 = found.iter().sum();
    if !found.is_empty() {
        println!("{} {} {} {}", found.len(), found[0], found[found.len() - 1], sum);
    } else {
        println!("0");
    }
}
