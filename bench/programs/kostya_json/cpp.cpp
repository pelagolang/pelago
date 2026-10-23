// After kostya/benchmarks json (MIT, Copyright (c) 2014 Konstantin Makarchev): parse pretty-printed JSON of coordinates and average x, y, z. The text is built in memory, as byte codes, because the test's /tmp/1.json cannot be read yet. Keys are kept as hashes.
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

struct jv {
    enum kind { lit, num, str, arr, dict } type = lit;
    double f = 0;
    long long h = 0;
    std::vector<long long> keys;
    std::vector<jv> items;
};
using buf = std::vector<long long>;

static void put(buf &b, const char *s) {
    for (; *s; s++) b.push_back((unsigned char)*s);
}
static void digits(buf &b, long long v, int width) {
    long long d = 1;
    for (int i = 1; i < width; i++) d *= 10;
    while (d > 0) { b.push_back(48 + (v / d) % 10); d /= 10; }
}
static long long next(long long s) { return (s * 1103515245 + 12345) & 2147483647; }

static void skip(const buf &t, size_t &p) {
    while (p < t.size() && (t[p] == 32 || t[p] == 10 || t[p] == 13 || t[p] == 9)) p++;
}
static jv parse_value(const buf &t, size_t &p) {
    jv v;
    skip(t, p);
    long long c = t[p];
    if (c == 123) {
        v.type = jv::dict;
        p++;
        skip(t, p);
        if (t[p] == 125) { p++; return v; }
        bool more = true;
        while (more) {
            skip(t, p);
            p++;
            long long h = 0;
            while (t[p] != 34) { h = (h * 31 + t[p]) & 2147483647; p++; }
            p++;
            skip(t, p);
            p++;
            jv item = parse_value(t, p);
            v.keys.push_back(h);
            v.items.push_back(std::move(item));
            skip(t, p);
            if (t[p] == 44) p++; else { p++; more = false; }
        }
    } else if (c == 91) {
        v.type = jv::arr;
        p++;
        skip(t, p);
        if (t[p] == 93) { p++; return v; }
        bool more = true;
        while (more) {
            jv item = parse_value(t, p);
            v.keys.push_back(0);
            v.items.push_back(std::move(item));
            skip(t, p);
            if (t[p] == 44) p++; else { p++; more = false; }
        }
    } else if (c == 34) {
        v.type = jv::str;
        p++;
        long long h = 0;
        while (t[p] != 34) { h = (h * 31 + t[p]) & 2147483647; p++; }
        p++;
        v.h = h;
    } else if (c == 45 || (c >= 48 && c <= 57)) {
        bool neg = false;
        if (c == 45) { neg = true; p++; }
        long long m = 0;
        while (t[p] >= 48 && t[p] <= 57) { m = m * 10 + (t[p] - 48); p++; }
        double scale = 1.0;
        if (t[p] == 46) {
            p++;
            while (t[p] >= 48 && t[p] <= 57) { m = m * 10 + (t[p] - 48); scale = scale * 10.0; p++; }
        }
        double f = (double)m / scale;
        v.type = jv::num;
        v.f = neg ? 0.0 - f : f;
    } else {
        while (p < t.size() && t[p] >= 97 && t[p] <= 122) p++;
    }
    return v;
}
static std::string show(double v) {
    char b[64];
    std::snprintf(b, sizeof b, "%.6f", v);
    std::string s = b;
    while (!s.empty() && s.back() == '0') s.pop_back();
    if (!s.empty() && s.back() == '.') s.pop_back();
    return s;
}

int main() {
    const int records = 20000;
    const char *f_start = "{\n  \"coordinates\": [\n";
    const char *f_x = "    {\n      \"x\": -0.";
    const char *f_y = ",\n      \"y\": ";
    const char *f_z = ",\n      \"z\": 0.";
    const char *f_name = ",\n      \"name\": \"";
    const char *f_tail = "\",\n      \"opts\": {\n        \"1\": [\n          1,\n          true\n        ]\n      }\n    }";
    const char *f_sep = ",\n";
    const char *f_end = "\n  ],\n  \"info\": \"some info\"\n}\n";
    buf text;
    long long s = 1;
    put(text, f_start);
    for (int i = 0; i < records; i++) {
        if (i > 0) put(text, f_sep);
        put(text, f_x);
        s = next(s);
        digits(text, (s >> 8) % 1000000, 6);
        put(text, f_y);
        s = next(s);
        long long b = (s >> 8) % 1000000000;
        digits(text, b / 1000000, 1);
        text.push_back(46);
        digits(text, b % 1000000, 6);
        put(text, f_z);
        s = next(s);
        digits(text, (s >> 8) % 1000000, 6);
        put(text, f_name);
        for (int k = 0; k < 6; k++) { s = next(s); text.push_back(97 + (s >> 8) % 26); }
        text.push_back(32);
        s = next(s);
        digits(text, (s >> 8) % 10000, 4);
        put(text, f_tail);
    }
    put(text, f_end);

    size_t p = 0;
    jv root = parse_value(text, p);
    double sx = 0.0, sy = 0.0, sz = 0.0;
    long long n = 0;
    if (root.type == jv::dict) {
        for (size_t i = 0; i < root.items.size(); i++) {
            if (root.keys[i] != 1871919611) continue;
            const jv &arr = root.items[i];
            if (arr.type != jv::arr) continue;
            for (const jv &item : arr.items) {
                if (item.type != jv::dict) continue;
                n++;
                for (size_t j = 0; j < item.items.size(); j++) {
                    const jv &v = item.items[j];
                    if (v.type != jv::num) continue;
                    if (item.keys[j] == 120) sx += v.f;
                    else if (item.keys[j] == 121) sy += v.f;
                    else if (item.keys[j] == 122) sz += v.f;
                }
            }
        }
    }
    double len = (double)n;
    std::printf("%lld %s %s %s\n", n, show(sx / len).c_str(), show(sy / len).c_str(), show(sz / len).c_str());
    return 0;
}
