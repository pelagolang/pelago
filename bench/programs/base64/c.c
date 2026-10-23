/* After kostya/benchmarks base64 (MIT, Copyright (c) 2014 Konstantin Makarchev): encode and decode a 128 KiB block into newly allocated buffers, with '=' padding. */
#include <stdio.h>
#include <stdlib.h>
static long long table[64], rev[256];
static long long *encode(const long long *src, int n, int *out_n) {
    long long *out = malloc((n / 3 * 4 + 8) * sizeof(long long));
    int o = 0, i = 0, full = n - n % 3;
    while (i < full) {
        long long v = (src[i] << 16) | (src[i + 1] << 8) | src[i + 2];
        out[o++] = table[(v >> 18) & 63];
        out[o++] = table[(v >> 12) & 63];
        out[o++] = table[(v >> 6) & 63];
        out[o++] = table[v & 63];
        i += 3;
    }
    int rest = n % 3;
    if (rest == 1) {
        long long v = src[i] << 16;
        out[o++] = table[(v >> 18) & 63];
        out[o++] = table[(v >> 12) & 63];
        out[o++] = 61;
        out[o++] = 61;
    } else if (rest == 2) {
        long long v = (src[i] << 16) | (src[i + 1] << 8);
        out[o++] = table[(v >> 18) & 63];
        out[o++] = table[(v >> 12) & 63];
        out[o++] = table[(v >> 6) & 63];
        out[o++] = 61;
    }
    *out_n = o;
    return out;
}
static long long *decode(const long long *src, int n, int *out_n) {
    int size = n;
    while (size > 0 && (src[size - 1] == 61 || src[size - 1] == 10 || src[size - 1] == 13)) size--;
    long long *out = malloc((size / 4 * 3 + 8) * sizeof(long long));
    int o = 0, i = 0;
    while (i + 4 <= size) {
        long long v = (rev[src[i]] << 18) | (rev[src[i + 1]] << 12) | (rev[src[i + 2]] << 6) | rev[src[i + 3]];
        out[o++] = (v >> 16) & 255;
        out[o++] = (v >> 8) & 255;
        out[o++] = v & 255;
        i += 4;
    }
    int rest = size - i;
    if (rest == 2) {
        long long v = (rev[src[i]] << 18) | (rev[src[i + 1]] << 12);
        out[o++] = (v >> 16) & 255;
    } else if (rest == 3) {
        long long v = (rev[src[i]] << 18) | (rev[src[i + 1]] << 12) | (rev[src[i + 2]] << 6);
        out[o++] = (v >> 16) & 255;
        out[o++] = (v >> 8) & 255;
    }
    *out_n = o;
    return out;
}
int main(void) {
    for (int i = 0; i < 26; i++) { table[i] = 65 + i; table[26 + i] = 97 + i; }
    for (int i = 0; i < 10; i++) table[52 + i] = 48 + i;
    table[62] = 43;
    table[63] = 47;
    for (int i = 0; i < 64; i++) rev[table[i]] = i;
    const int n = 131072;
    long long *text = malloc(n * sizeof(long long));
    for (int i = 0; i < n; i++) text[i] = 97;
    int en, dn;
    long long *encoded = encode(text, n, &en);
    long long *decoded = decode(encoded, en, &dn);
    int ok = dn == n;
    free(decoded);
    long long s_encoded = 0, s_decoded = 0;
    for (int t = 0; t < 256; t++) { int m; long long *e = encode(text, n, &m); s_encoded += m; free(e); }
    for (int t = 0; t < 256; t++) { int m; long long *d = decode(encoded, en, &m); s_decoded += m; free(d); }
    printf("%d %lld %lld\n", ok, s_encoded, s_decoded);
    return 0;
}
