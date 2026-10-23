#include <stdio.h>
int main(void) {
    const long long im = 139968, ia = 3877, ic = 29573;
    long long codes[15] = {97, 99, 103, 116, 66, 68, 72, 75, 77, 78, 82, 83, 86, 87, 89};
    long long cum[15] = {270, 390, 510, 780, 800, 820, 840, 860, 880, 900, 920, 940, 960, 980, 1000};
    long long seed = 42, sum = 0;
    for (long long i = 0; i < 5000000; i++) {
        seed = (seed * ia + ic) % im;
        long long r = seed * 1000;
        int k = 0;
        while (r >= im * cum[k]) k++;
        sum = (sum * 31 + codes[k]) & 2147483647;
    }
    printf("%lld %lld\n", sum, seed);
    return 0;
}
