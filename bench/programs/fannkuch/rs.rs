fn main() {
    let n: usize = 9;
    let mut perm1: Vec<i64> = (0..n as i64).collect();
    let mut perm = vec![0i64; n];
    let mut count = vec![0i64; n];
    let (mut maxflips, mut checksum, mut permcount) = (0i64, 0i64, 0i64);
    let mut r = n;
    let mut done = false;
    while !done {
        while r != 1 { count[r - 1] = r as i64; r -= 1; }
        perm.copy_from_slice(&perm1);
        let mut flips = 0i64;
        let mut k = perm[0] as usize;
        while k != 0 {
            let (mut i, mut j) = (0usize, k);
            while i < j { perm.swap(i, j); i += 1; j -= 1; }
            flips += 1;
            k = perm[0] as usize;
        }
        if flips > maxflips { maxflips = flips; }
        if permcount % 2 == 0 { checksum += flips; } else { checksum -= flips; }
        let mut more = true;
        while more {
            if r == n { done = true; more = false; } else {
                let p0 = perm1[0];
                let mut i = 0;
                while i < r { perm1[i] = perm1[i + 1]; i += 1; }
                perm1[r] = p0;
                count[r] -= 1;
                if count[r] > 0 { more = false; } else { r += 1; }
            }
        }
        permcount += 1;
    }
    println!("{} {}", checksum, maxflips);
}
