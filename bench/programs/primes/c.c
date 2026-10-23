/* After kostya/benchmarks primes (MIT, Copyright (c) 2014 Konstantin Makarchev): sieve of Atkin, digit trie, breadth-first walk from a prefix, sorted result. */
#include <stdio.h>
#include <stdlib.h>
static int cmp(const void *a, const void *b) {
    long long x = *(const long long *)a, y = *(const long long *)b;
    return x < y ? -1 : x > y;
}
int main(void) {
    const long long limit = 5000000, prefix = 32338;
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
    for (long long r = 5; r * r < limit; r++)
        if (sieve[r] == 1)
            for (long long k = r * r; k < limit; k += r * r) sieve[k] = 0;
    long long cap = 1 << 16, nodes = 1;
    long long *child = calloc(cap * 10, sizeof(long long)), *term = calloc(cap, sizeof(long long));
    for (long long p = 2; p <= limit; p++) {
        if (p == 2 || p == 3 || (p >= 5 && sieve[p] == 1)) {
            long long d = 1;
            while (d * 10 <= p) d *= 10;
            long long node = 0;
            while (d > 0) {
                long long digit = (p / d) % 10;
                if (child[node * 10 + digit] == 0) {
                    if (nodes == cap) {
                        child = realloc(child, cap * 2 * 10 * sizeof(long long));
                        term = realloc(term, cap * 2 * sizeof(long long));
                        for (long long i = cap * 10; i < cap * 20; i++) child[i] = 0;
                        for (long long i = cap; i < cap * 2; i++) term[i] = 0;
                        cap *= 2;
                    }
                    child[node * 10 + digit] = nodes++;
                }
                node = child[node * 10 + digit];
                d /= 10;
            }
            term[node] = 1;
        }
    }
    long long head = 0, d = 1;
    while (d * 10 <= prefix) d *= 10;
    while (d > 0 && head >= 0) {
        long long next = child[head * 10 + (prefix / d) % 10];
        head = next == 0 ? -1 : next;
        d /= 10;
    }
    long long *found = malloc(nodes * sizeof(long long)), nfound = 0;
    if (head >= 0) {
        long long *queue = malloc(nodes * sizeof(long long)), *values = malloc(nodes * sizeof(long long));
        long long at = 0, tail = 0;
        queue[tail] = head;
        values[tail++] = prefix;
        while (at < tail) {
            long long node = queue[at], value = values[at];
            at++;
            if (term[node] == 1) found[nfound++] = value;
            for (int digit = 0; digit < 10; digit++)
                if (child[node * 10 + digit] != 0) {
                    queue[tail] = child[node * 10 + digit];
                    values[tail++] = value * 10 + digit;
                }
        }
    }
    qsort(found, nfound, sizeof(long long), cmp);
    long long sum = 0;
    for (long long i = 0; i < nfound; i++) sum += found[i];
    if (nfound > 0) printf("%lld %lld %lld %lld\n", nfound, found[0], found[nfound - 1], sum);
    else printf("0\n");
    return 0;
}
