#include <cstdio>
#include <unordered_map>
#include <vector>
int main() {
    const int n = 500000;
    std::vector<int> seq;
    long long s = 1;
    for (int i = 0; i < n; i++) { s = (s * 1103515245 + 12345) & 2147483647; seq.push_back((s >> 16) & 3); }
    for (int k : {1, 2, 4, 6, 9}) {
        std::unordered_map<long long, long long> m;
        long long mask = (1LL << (2 * k)) - 1, w = 0;
        for (int i = 0; i < n; i++) {
            w = ((w << 2) | seq[i]) & mask;
            if (i >= k - 1) m[w] += 1;
        }
        long long most = 0, total = 0;
        for (auto &kv : m) { total += kv.second; if (kv.second > most) most = kv.second; }
        std::printf("%d %zu %lld %lld\n", k, m.size(), most, total);
    }
    return 0;
}
