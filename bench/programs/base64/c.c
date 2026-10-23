#include <stdio.h>
#include <stdlib.h>
int main(void) {
    const int n = 3000000;
    int table[64], rev[256] = {0};
    for (int i = 0; i < 26; i++) { table[i] = 65 + i; table[26 + i] = 97 + i; }
    for (int i = 0; i < 10; i++) table[52 + i] = 48 + i;
    table[62] = 43;
    table[63] = 47;
    for (int i = 0; i < 64; i++) rev[table[i]] = i;
    long long *data = malloc(n * sizeof(long long));
    long long s = 1;
    for (int i = 0; i < n; i++) { s = (s * 1103515245 + 12345) & 2147483647; data[i] = (s >> 8) & 255; }
    long long check = 0, bad = 0;
    for (int round = 0; round < 3; round++) {
        long long *enc = malloc(n / 3 * 4 * sizeof(long long));
        int e = 0;
        for (int i = 0; i < n; i += 3) {
            long long v = (data[i] << 16) | (data[i + 1] << 8) | data[i + 2];
            enc[e++] = table[(v >> 18) & 63];
            enc[e++] = table[(v >> 12) & 63];
            enc[e++] = table[(v >> 6) & 63];
            enc[e++] = table[v & 63];
        }
        for (int k = 0; k < e; k++) check = (check * 31 + enc[k]) & 2147483647;
        for (int j = 0, i = 0; j < e; j += 4, i += 3) {
            long long v = ((long long)rev[enc[j]] << 18) | ((long long)rev[enc[j + 1]] << 12) | ((long long)rev[enc[j + 2]] << 6) | rev[enc[j + 3]];
            if (data[i] != ((v >> 16) & 255)) bad++;
            if (data[i + 1] != ((v >> 8) & 255)) bad++;
            if (data[i + 2] != (v & 255)) bad++;
        }
        free(enc);
    }
    free(data);
    printf("%lld %lld\n", check, bad);
    return 0;
}
