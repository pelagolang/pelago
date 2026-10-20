#include <stdio.h>
#include <stdint.h>

static int64_t digit_sum(int64_t start) {
    int64_t n = start, s = 0;
    while (n != 0) {
        s += n % 10;
        n = n / 10;
    }
    return s;
}

int main(void) {
    int64_t total = 0;
    for (int64_t i = 1; i < 3000001; i++) total += digit_sum(i) + i % 3 + i / 7;
    printf("%lld\n", (long long)total);
    return 0;
}
