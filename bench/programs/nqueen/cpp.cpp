#include <cstdio>
static long long solve(int n, long long cols, long long d1, long long d2) {
    long long all = (1LL << n) - 1;
    if (cols == all) return 1;
    long long count = 0, avail = all ^ (cols | d1 | d2);
    while (avail != 0) {
        long long bit = avail & -avail;
        avail = avail ^ bit;
        count += solve(n, cols | bit, ((d1 | bit) << 1) & all, (d2 | bit) >> 1);
    }
    return count;
}
int main(void) {
    const int n = 14;
    std::printf("%lld\n", solve(n, 0, 0, 0));
    return 0;
}
