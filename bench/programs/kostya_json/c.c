/* After kostya/benchmarks json (MIT, Copyright (c) 2014 Konstantin Makarchev): parse pretty-printed JSON of coordinates and average x, y, z. The text is built in memory, as byte codes, because the test's /tmp/1.json cannot be read yet. Keys are kept as hashes. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum kind { LIT, NUM, STR, ARR, DICT };
struct jv {
    enum kind kind;
    double f;
    long long h;
    long long *keys;
    struct jv *items;
    int n, cap;
};
struct buf {
    long long *v;
    long long n, cap;
};

static void push(struct buf *b, long long c) {
    if (b->n == b->cap) {
        b->cap = b->cap ? b->cap * 2 : 1024;
        b->v = realloc(b->v, b->cap * sizeof(long long));
    }
    b->v[b->n++] = c;
}
static void put(struct buf *b, const char *s) {
    for (; *s; s++) push(b, (unsigned char)*s);
}
static void digits(struct buf *b, long long v, int width) {
    long long d = 1;
    for (int i = 1; i < width; i++) d *= 10;
    while (d > 0) { push(b, 48 + (v / d) % 10); d /= 10; }
}
static long long next(long long s) { return (s * 1103515245 + 12345) & 2147483647; }

static void add(struct jv *v, long long key, struct jv item) {
    if (v->n == v->cap) {
        v->cap = v->cap ? v->cap * 2 : 4;
        v->items = realloc(v->items, v->cap * sizeof(struct jv));
        v->keys = realloc(v->keys, v->cap * sizeof(long long));
    }
    v->keys[v->n] = key;
    v->items[v->n++] = item;
}
static void skip(const long long *t, long long n, long long *p) {
    while (*p < n && (t[*p] == 32 || t[*p] == 10 || t[*p] == 13 || t[*p] == 9)) (*p)++;
}
static struct jv parse_value(const long long *t, long long n, long long *p) {
    struct jv v = {LIT, 0, 0, NULL, NULL, 0, 0};
    skip(t, n, p);
    long long c = t[*p];
    if (c == 123) {
        v.kind = DICT;
        (*p)++;
        skip(t, n, p);
        if (t[*p] == 125) { (*p)++; return v; }
        int more = 1;
        while (more) {
            skip(t, n, p);
            (*p)++;
            long long h = 0;
            while (t[*p] != 34) { h = (h * 31 + t[*p]) & 2147483647; (*p)++; }
            (*p)++;
            skip(t, n, p);
            (*p)++;
            struct jv item = parse_value(t, n, p);
            add(&v, h, item);
            skip(t, n, p);
            if (t[*p] == 44) (*p)++; else { (*p)++; more = 0; }
        }
    } else if (c == 91) {
        v.kind = ARR;
        (*p)++;
        skip(t, n, p);
        if (t[*p] == 93) { (*p)++; return v; }
        int more = 1;
        while (more) {
            struct jv item = parse_value(t, n, p);
            add(&v, 0, item);
            skip(t, n, p);
            if (t[*p] == 44) (*p)++; else { (*p)++; more = 0; }
        }
    } else if (c == 34) {
        v.kind = STR;
        (*p)++;
        long long h = 0;
        while (t[*p] != 34) { h = (h * 31 + t[*p]) & 2147483647; (*p)++; }
        (*p)++;
        v.h = h;
    } else if (c == 45 || (c >= 48 && c <= 57)) {
        int neg = 0;
        if (c == 45) { neg = 1; (*p)++; }
        long long m = 0;
        while (t[*p] >= 48 && t[*p] <= 57) { m = m * 10 + (t[*p] - 48); (*p)++; }
        double scale = 1.0;
        if (t[*p] == 46) {
            (*p)++;
            while (t[*p] >= 48 && t[*p] <= 57) { m = m * 10 + (t[*p] - 48); scale = scale * 10.0; (*p)++; }
        }
        double f = (double)m / scale;
        v.kind = NUM;
        v.f = neg ? 0.0 - f : f;
    } else {
        while (*p < n && t[*p] >= 97 && t[*p] <= 122) (*p)++;
    }
    return v;
}
static void show(double v) {
    char b[64];
    snprintf(b, sizeof b, "%.6f", v);
    int n = (int)strlen(b);
    while (n > 0 && b[n - 1] == '0') n--;
    if (n > 0 && b[n - 1] == '.') n--;
    b[n] = 0;
    printf("%s", b);
}

int main(void) {
    const int records = 20000;
    const char *f_start = "{\n  \"coordinates\": [\n";
    const char *f_x = "    {\n      \"x\": -0.";
    const char *f_y = ",\n      \"y\": ";
    const char *f_z = ",\n      \"z\": 0.";
    const char *f_name = ",\n      \"name\": \"";
    const char *f_tail = "\",\n      \"opts\": {\n        \"1\": [\n          1,\n          true\n        ]\n      }\n    }";
    const char *f_sep = ",\n";
    const char *f_end = "\n  ],\n  \"info\": \"some info\"\n}\n";
    struct buf text = {NULL, 0, 0};
    long long s = 1;
    put(&text, f_start);
    for (int i = 0; i < records; i++) {
        if (i > 0) put(&text, f_sep);
        put(&text, f_x);
        s = next(s);
        digits(&text, (s >> 8) % 1000000, 6);
        put(&text, f_y);
        s = next(s);
        long long b = (s >> 8) % 1000000000;
        digits(&text, b / 1000000, 1);
        push(&text, 46);
        digits(&text, b % 1000000, 6);
        put(&text, f_z);
        s = next(s);
        digits(&text, (s >> 8) % 1000000, 6);
        put(&text, f_name);
        for (int k = 0; k < 6; k++) { s = next(s); push(&text, 97 + (s >> 8) % 26); }
        push(&text, 32);
        s = next(s);
        digits(&text, (s >> 8) % 10000, 4);
        put(&text, f_tail);
    }
    put(&text, f_end);

    long long p = 0;
    struct jv root = parse_value(text.v, text.n, &p);
    double sx = 0.0, sy = 0.0, sz = 0.0;
    long long n = 0;
    if (root.kind == DICT) {
        for (int i = 0; i < root.n; i++) {
            if (root.keys[i] != 1871919611) continue;
            struct jv *arr = &root.items[i];
            if (arr->kind != ARR) continue;
            for (int a = 0; a < arr->n; a++) {
                struct jv *item = &arr->items[a];
                if (item->kind != DICT) continue;
                n++;
                for (int j = 0; j < item->n; j++) {
                    struct jv *v = &item->items[j];
                    if (v->kind != NUM) continue;
                    if (item->keys[j] == 120) sx += v->f;
                    else if (item->keys[j] == 121) sy += v->f;
                    else if (item->keys[j] == 122) sz += v->f;
                }
            }
        }
    }
    double len = (double)n;
    printf("%lld ", n);
    show(sx / len);
    printf(" ");
    show(sy / len);
    printf(" ");
    show(sz / len);
    printf("\n");
    return 0;
}
