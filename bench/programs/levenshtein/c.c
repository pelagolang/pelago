#include <stdio.h>
#include <stdlib.h>
static long long dist(const long long *a, int na, const long long *b, int nb) {
    long long *prev = malloc((nb + 1) * sizeof(long long)), *cur = malloc((nb + 1) * sizeof(long long));
    for (int j = 0; j <= nb; j++) { prev[j] = j; cur[j] = 0; }
    for (int i = 0; i < na; i++) {
        cur[0] = i + 1;
        for (int j = 0; j < nb; j++) {
            long long cost = a[i] == b[j] ? 0 : 1;
            long long best = prev[j] + cost;
            if (prev[j + 1] + 1 < best) best = prev[j + 1] + 1;
            if (cur[j] + 1 < best) best = cur[j] + 1;
            cur[j + 1] = best;
        }
        for (int j = 0; j <= nb; j++) prev[j] = cur[j];
    }
    long long r = prev[nb];
    free(prev);
    free(cur);
    return r;
}
int main(void) {
    enum { N = 600 };
    long long *words[N];
    int lens[N];
    long long s = 7;
    for (int w = 0; w < N; w++) {
        s = (s * 1103515245 + 12345) & 2147483647;
        int len = 8 + (s >> 8) % 40;
        lens[w] = len;
        words[w] = malloc(len * sizeof(long long));
        for (int k = 0; k < len; k++) {
            s = (s * 1103515245 + 12345) & 2147483647;
            words[w][k] = 97 + (s >> 8) % 26;
        }
    }
    long long total = 0;
    for (int i = 0; i < N; i++)
        for (int j = i + 1; j < N; j++) total += dist(words[i], lens[i], words[j], lens[j]);
    printf("%lld\n", total);
    for (int w = 0; w < N; w++) free(words[w]);
    return 0;
}
