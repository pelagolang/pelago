#include <cstdio>
#include <vector>
using word = std::vector<long long>;
static long long dist(const word &a, const word &b) {
    word prev(b.size() + 1), cur(b.size() + 1, 0);
    for (size_t j = 0; j <= b.size(); j++) prev[j] = (long long)j;
    for (size_t i = 0; i < a.size(); i++) {
        cur[0] = (long long)i + 1;
        for (size_t j = 0; j < b.size(); j++) {
            long long cost = a[i] == b[j] ? 0 : 1;
            long long best = prev[j] + cost;
            if (prev[j + 1] + 1 < best) best = prev[j + 1] + 1;
            if (cur[j] + 1 < best) best = cur[j] + 1;
            cur[j + 1] = best;
        }
        prev = cur;
    }
    return prev[b.size()];
}
int main() {
    const int N = 600;
    std::vector<word> words;
    long long s = 7;
    for (int w = 0; w < N; w++) {
        s = (s * 1103515245 + 12345) & 2147483647;
        int len = 8 + (s >> 8) % 40;
        word x;
        for (int k = 0; k < len; k++) {
            s = (s * 1103515245 + 12345) & 2147483647;
            x.push_back(97 + (s >> 8) % 26);
        }
        words.push_back(std::move(x));
    }
    long long total = 0;
    for (int i = 0; i < N; i++)
        for (int j = i + 1; j < N; j++) total += dist(words[i], words[j]);
    std::printf("%lld\n", total);
    return 0;
}
