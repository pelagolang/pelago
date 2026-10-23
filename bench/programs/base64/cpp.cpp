// After kostya/benchmarks base64 (MIT, Copyright (c) 2014 Konstantin Makarchev): encode and decode a 128 KiB block into newly allocated buffers, with '=' padding.
#include <cstdio>
#include <vector>
using buf = std::vector<long long>;
static buf table(64), rev(256, 0);
static buf encode(const buf &src) {
    buf out;
    size_t i = 0, full = src.size() - src.size() % 3;
    while (i < full) {
        long long v = (src[i] << 16) | (src[i + 1] << 8) | src[i + 2];
        out.push_back(table[(v >> 18) & 63]);
        out.push_back(table[(v >> 12) & 63]);
        out.push_back(table[(v >> 6) & 63]);
        out.push_back(table[v & 63]);
        i += 3;
    }
    size_t rest = src.size() % 3;
    if (rest == 1) {
        long long v = src[i] << 16;
        out.push_back(table[(v >> 18) & 63]);
        out.push_back(table[(v >> 12) & 63]);
        out.push_back(61);
        out.push_back(61);
    } else if (rest == 2) {
        long long v = (src[i] << 16) | (src[i + 1] << 8);
        out.push_back(table[(v >> 18) & 63]);
        out.push_back(table[(v >> 12) & 63]);
        out.push_back(table[(v >> 6) & 63]);
        out.push_back(61);
    }
    return out;
}
static buf decode(const buf &src) {
    size_t size = src.size();
    while (size > 0 && (src[size - 1] == 61 || src[size - 1] == 10 || src[size - 1] == 13)) size--;
    buf out;
    size_t i = 0;
    while (i + 4 <= size) {
        long long v = (rev[src[i]] << 18) | (rev[src[i + 1]] << 12) | (rev[src[i + 2]] << 6) | rev[src[i + 3]];
        out.push_back((v >> 16) & 255);
        out.push_back((v >> 8) & 255);
        out.push_back(v & 255);
        i += 4;
    }
    size_t rest = size - i;
    if (rest == 2) {
        long long v = (rev[src[i]] << 18) | (rev[src[i + 1]] << 12);
        out.push_back((v >> 16) & 255);
    } else if (rest == 3) {
        long long v = (rev[src[i]] << 18) | (rev[src[i + 1]] << 12) | (rev[src[i + 2]] << 6);
        out.push_back((v >> 16) & 255);
        out.push_back((v >> 8) & 255);
    }
    return out;
}
int main() {
    for (int i = 0; i < 26; i++) { table[i] = 65 + i; table[26 + i] = 97 + i; }
    for (int i = 0; i < 10; i++) table[52 + i] = 48 + i;
    table[62] = 43;
    table[63] = 47;
    for (int i = 0; i < 64; i++) rev[table[i]] = i;
    buf text(131072, 97);
    buf encoded = encode(text);
    int ok = decode(encoded).size() == text.size();
    long long s_encoded = 0, s_decoded = 0;
    for (int t = 0; t < 256; t++) s_encoded += (long long)encode(text).size();
    for (int t = 0; t < 256; t++) s_decoded += (long long)decode(encoded).size();
    std::printf("%d %lld %lld\n", ok, s_encoded, s_decoded);
    return 0;
}
