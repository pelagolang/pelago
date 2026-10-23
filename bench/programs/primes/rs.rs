fn main() {
    let limit: i64 = 2000000;
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
    while r * r <= limit {
        if sieve[r as usize] == 1 {
            let mut k = r * r;
            while k <= limit {
                sieve[k as usize] = 0;
                k += r * r;
            }
        }
        r += 1;
    }
    let mut child: Vec<i64> = vec![0; 10];
    let mut cnt: Vec<i64> = vec![0];
    let (mut nodes, mut primes) = (1i64, 0i64);
    for p in 2..=limit {
        if p == 2 || p == 3 || (p >= 5 && sieve[p as usize] == 1) {
            primes += 1;
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
                    cnt.push(0);
                }
                node = child[node * 10 + digit] as usize;
                cnt[node] += 1;
                d /= 10;
            }
        }
    }
    let mut queue: Vec<usize> = vec![0];
    let mut head = 0;
    let mut check = 0i64;
    while head < queue.len() {
        let node = queue[head];
        head += 1;
        check = (check * 31 + cnt[node]) & 2147483647;
        for digit in 0..10 {
            if child[node * 10 + digit] != 0 {
                queue.push(child[node * 10 + digit] as usize);
            }
        }
    }
    println!("{} {} {}", primes, nodes, check);
}
