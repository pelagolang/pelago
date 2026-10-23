#include <stdio.h>
#include <stdlib.h>
int main(void) {
    const long long limit = 2000000;
    long long *sieve = calloc(limit + 1, sizeof(long long));
    for (long long x = 1; x * x < limit; x++)
        for (long long y = 1; y * y < limit; y++) {
            long long n = 4 * x * x + y * y;
            if (n <= limit && (n % 12 == 1 || n % 12 == 5)) sieve[n] ^= 1;
            n = 3 * x * x + y * y;
            if (n <= limit && n % 12 == 7) sieve[n] ^= 1;
            n = 3 * x * x - y * y;
            if (x > y && n <= limit && n % 12 == 11) sieve[n] ^= 1;
        }
    for (long long r = 5; r * r <= limit; r++)
        if (sieve[r] == 1)
            for (long long k = r * r; k <= limit; k += r * r) sieve[k] = 0;
    long long cap = 1 << 16, nodes = 1, primes = 0;
    long long *child = calloc(cap * 10, sizeof(long long)), *cnt = calloc(cap, sizeof(long long));
    for (long long p = 2; p <= limit; p++) {
        if (p == 2 || p == 3 || (p >= 5 && sieve[p] == 1)) {
            primes++;
            long long d = 1;
            while (d * 10 <= p) d *= 10;
            long long node = 0;
            while (d > 0) {
                long long digit = (p / d) % 10;
                if (child[node * 10 + digit] == 0) {
                    if (nodes == cap) {
                        child = realloc(child, cap * 2 * 10 * sizeof(long long));
                        cnt = realloc(cnt, cap * 2 * sizeof(long long));
                        for (long long i = cap * 10; i < cap * 20; i++) child[i] = 0;
                        for (long long i = cap; i < cap * 2; i++) cnt[i] = 0;
                        cap *= 2;
                    }
                    child[node * 10 + digit] = nodes++;
                }
                node = child[node * 10 + digit];
                cnt[node]++;
                d /= 10;
            }
        }
    }
    long long *queue = malloc(nodes * sizeof(long long));
    long long head = 0, tail = 0, check = 0;
    queue[tail++] = 0;
    while (head < tail) {
        long long node = queue[head++];
        check = (check * 31 + cnt[node]) & 2147483647;
        for (int digit = 0; digit < 10; digit++)
            if (child[node * 10 + digit] != 0) queue[tail++] = child[node * 10 + digit];
    }
    printf("%lld %lld %lld\n", primes, nodes, check);
    return 0;
}
