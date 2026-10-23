// Brainfuck interpreter after kostya/benchmarks brainfuck/bf.c (MIT, Copyright (c) 2014 Konstantin Makarchev); program: brainfuck/bench.b
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum op_type { OP_INC, OP_MOVE, OP_LOOP, OP_PRINT };
struct op_list;
struct op {
    enum op_type type;
    long long offset;
    struct op_list *list;
};
struct op_list {
    struct op *ops;
    size_t cap, len;
};

static void push(struct op_list *l, struct op o) {
    if (l->len == l->cap) {
        l->cap = l->cap ? l->cap * 2 : 4;
        l->ops = realloc(l->ops, sizeof(struct op) * l->cap);
    }
    l->ops[l->len++] = o;
}

static void parse(const char **it, struct op_list *ops) {
    char c;
    while ((c = *(*it)++)) {
        struct op o = {OP_INC, 0, NULL};
        switch (c) {
        case '+': o.type = OP_INC; o.offset = 1; break;
        case '-': o.type = OP_INC; o.offset = -1; break;
        case '>': o.type = OP_MOVE; o.offset = 1; break;
        case '<': o.type = OP_MOVE; o.offset = -1; break;
        case '.': o.type = OP_PRINT; break;
        case '[':
            o.type = OP_LOOP;
            o.list = calloc(1, sizeof(struct op_list));
            parse(it, o.list);
            break;
        case ']': return;
        default: continue;
        }
        push(ops, o);
    }
}

struct tape {
    long long *cells;
    size_t cap, pos;
};
struct printer {
    long long sum1, sum2;
};

static void eval(const struct op_list *ops, struct tape *t, struct printer *p) {
    for (size_t i = 0; i < ops->len; i++) {
        const struct op *o = &ops->ops[i];
        switch (o->type) {
        case OP_INC: t->cells[t->pos] += o->offset; break;
        case OP_MOVE:
            t->pos += o->offset;
            if (t->pos >= t->cap) {
                t->cells = realloc(t->cells, sizeof(long long) * t->cap * 2);
                memset(t->cells + t->cap, 0, sizeof(long long) * t->cap);
                t->cap *= 2;
            }
            break;
        case OP_LOOP:
            while (t->cells[t->pos] > 0) eval(o->list, t, p);
            break;
        case OP_PRINT:
            p->sum1 = (p->sum1 + t->cells[t->pos]) % 255;
            p->sum2 = (p->sum2 + p->sum1) % 255;
            break;
        }
    }
}

int main(void) {
    const char *src = ">++[<+++++++++++++>-]<[[>+>+<<-]>[<+>-]++++++++[>++++++++<-]>.[-]<<>++++++++++[>++++++++++[>++++++++++[>++++++++++[>++++++++++[>++++++++++[>++++++++++[-]<-]<-]<-]<-]<-]<-]<-]++++++++++.";
    struct tape t = {calloc(1, sizeof(long long)), 1, 0};
    struct printer p = {0, 0};
    struct op_list ops = {NULL, 0, 0};
    const char *it = src;
    parse(&it, &ops);
    eval(&ops, &t, &p);
    printf("%lld\n", (p.sum2 << 8) | p.sum1);
    return 0;
}
