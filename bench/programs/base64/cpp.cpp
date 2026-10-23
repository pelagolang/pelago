#include <cstdio>
#include <vector>
int main() {
    const int n = 3000000;
    std::vector<long long> table(64), rev(256, 0);
    for (int i = 0; i < 26; i++) { table[i] = 65 + i; table[26 + i] = 97 + i; }
    for (int i = 0; i < 10; i++) table[52 + i] = 48 + i;
    table[62] = 43;
    table[63] = 47;
    for (int i = 0; i < 64; i++) rev[table[i]] = i;
    std::vector<long long> data;
    long long s = 1;
    for (int i = 0; i < n; i++) { s = (s * 1103515245 + 12345) & 2147483647; data.push_back((s >> 8) & 255); }
    long long check = 0, bad = 0;
    for (int round = 0; round < 3; round++) {
        std::vector<long long> enc;
        for (int i = 0; i < n; i += 3) {
            long long v = (data[i] << 16) | (data[i + 1] << 8) | data[i + 2];
            enc.push_back(table[(v >> 18) & 63]);
            enc.push_back(table[(v >> 12) & 63]);
            enc.push_back(table[(v >> 6) & 63]);
            enc.push_back(table[v & 63]);
        }
        for (long long c : enc) check = (check * 31 + c) & 2147483647;
        for (size_t j = 0, i = 0; j < enc.size(); j += 4, i += 3) {
            long long v = (rev[enc[j]] << 18) | (rev[enc[j + 1]] << 12) | (rev[enc[j + 2]] << 6) | rev[enc[j + 3]];
            if (data[i] != ((v >> 16) & 255)) bad++;
            if (data[i + 1] != ((v >> 8) & 255)) bad++;
            if (data[i + 2] != (v & 255)) bad++;
        }
    }
    std::printf("%lld %lld\n", check, bad);
    return 0;
}
