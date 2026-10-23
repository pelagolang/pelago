#include <cstdio>
#include <vector>
int main() {
    volatile long long seed = 1;
    long long s = seed;
    for (int i = 0; i < 1000; i++) s = (s * 1103515245 + 12345) & 2147483647;
    long long u = 20 + s % 30, r = (s >> 8) % 10000;
    std::vector<long long> a(10000, 0);
    for (long long i = 0; i < 10000; i++) {
        for (long long j = 0; j < 10000; j++) a[i] = a[i] + j % u;
        a[i] += r;
    }
    std::printf("%lld\n", a[r]);
    return 0;
}
