// After kostya/benchmarks primes (MIT, Copyright (c) 2014 Konstantin Makarchev): sieve of Atkin, digit trie, breadth-first walk from a prefix, sorted result.
#include <algorithm>
#include <cstdio>
#include <vector>
int main() {
    const long long limit = 5000000, prefix = 32338;
    std::vector<long long> sieve(limit + 1, 0);
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
    std::vector<long long> child(10, 0), term(1, 0);
    long long nodes = 1;
    for (long long p = 2; p <= limit; p++) {
        if (p == 2 || p == 3 || (p >= 5 && sieve[p] == 1)) {
            long long d = 1;
            while (d * 10 <= p) d *= 10;
            long long node = 0;
            while (d > 0) {
                long long digit = (p / d) % 10;
                if (child[node * 10 + digit] == 0) {
                    child[node * 10 + digit] = nodes++;
                    child.resize(child.size() + 10, 0);
                    term.push_back(0);
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
    std::vector<long long> found;
    if (head >= 0) {
        std::vector<long long> queue{head}, values{prefix};
        size_t at = 0;
        while (at < queue.size()) {
            long long node = queue[at], value = values[at];
            at++;
            if (term[node] == 1) found.push_back(value);
            for (int digit = 0; digit < 10; digit++)
                if (child[node * 10 + digit] != 0) {
                    queue.push_back(child[node * 10 + digit]);
                    values.push_back(value * 10 + digit);
                }
        }
    }
    std::sort(found.begin(), found.end());
    long long sum = 0;
    for (long long v : found) sum += v;
    if (!found.empty()) std::printf("%zu %lld %lld %lld\n", found.size(), found.front(), found.back(), sum);
    else std::printf("0\n");
    return 0;
}
