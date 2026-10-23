#include <stdio.h>
#include <stdlib.h>
#define CAP (1 << 21)
int main(void) {
    const int n = 500000;
    int *seq = malloc(n * sizeof(int));
    long long s = 1;
    for (int i = 0; i < n; i++) { s = (s * 1103515245 + 12345) & 2147483647; seq[i] = (s >> 16) & 3; }
    long long *keys = malloc(CAP * sizeof(long long));
    long long *cnt = malloc(CAP * sizeof(long long));
    int ks[5] = {1, 2, 4, 6, 9};
    for (int t = 0; t < 5; t++) {
        int k = ks[t];
        for (int i = 0; i < CAP; i++) { keys[i] = -1; cnt[i] = 0; }
        long long mask = (1LL << (2 * k)) - 1, w = 0, distinct = 0;
        for (int i = 0; i < n; i++) {
            w = ((w << 2) | seq[i]) & mask;
            if (i >= k - 1) {
                unsigned long long h = ((unsigned long long)w * 0x9E3779B97F4A7C15ULL) >> 43;
                while (keys[h] != -1 && keys[h] != w) h = (h + 1) & (CAP - 1);
                if (keys[h] == -1) { keys[h] = w; distinct++; }
                cnt[h]++;
            }
        }
        long long most = 0, total = 0;
        for (int i = 0; i < CAP; i++) if (keys[i] != -1) { total += cnt[i]; if (cnt[i] > most) most = cnt[i]; }
        printf("%d %lld %lld %lld\n", k, distinct, most, total);
    }
    return 0;
}
