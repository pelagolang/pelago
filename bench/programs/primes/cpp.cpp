#include <cstdio>
#include <vector>
int main() {
    const long long limit = 2000000;
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
    for (long long r = 5; r * r <= limit; r++)
        if (sieve[r] == 1)
            for (long long k = r * r; k <= limit; k += r * r) sieve[k] = 0;
    std::vector<long long> child(10, 0), cnt(1, 0);
    long long nodes = 1, primes = 0;
    for (long long p = 2; p <= limit; p++) {
        if (p == 2 || p == 3 || (p >= 5 && sieve[p] == 1)) {
            primes++;
            long long d = 1;
            while (d * 10 <= p) d *= 10;
            long long node = 0;
            while (d > 0) {
                long long digit = (p / d) % 10;
                if (child[node * 10 + digit] == 0) {
                    child[node * 10 + digit] = nodes++;
                    child.resize(child.size() + 10, 0);
                    cnt.push_back(0);
                }
                node = child[node * 10 + digit];
                cnt[node]++;
                d /= 10;
            }
        }
    }
    std::vector<long long> queue{0};
    size_t head = 0;
    long long check = 0;
    while (head < queue.size()) {
        long long node = queue[head++];
        check = (check * 31 + cnt[node]) & 2147483647;
        for (int digit = 0; digit < 10; digit++)
            if (child[node * 10 + digit] != 0) queue.push_back(child[node * 10 + digit]);
    }
    std::printf("%lld %lld %lld\n", primes, nodes, check);
    return 0;
}
