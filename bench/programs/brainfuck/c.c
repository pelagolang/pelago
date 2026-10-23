#include <stdio.h>
#include <string.h>
int main(void) {
    const char *prog = "-[>-[>-[>+<-]<-]<-]>>>.";
    int n = (int)strlen(prog);
    int code[64], jump[64], stack[64], sp = 0;
    for (int i = 0; i < n; i++) {
        char c = prog[i];
        code[i] = c == '+' ? 0 : c == '-' ? 1 : c == '<' ? 2 : c == '>' ? 3 : c == '[' ? 4 : c == ']' ? 5 : 6;
        jump[i] = 0;
    }
    for (int i = 0; i < n; i++) {
        if (code[i] == 4) stack[sp++] = i;
        else if (code[i] == 5) { int open = stack[--sp]; jump[open] = i; jump[i] = open; }
    }
    static long long tape[30000];
    long long ptr = 0, pc = 0, steps = 0, out = 0;
    while (pc < n) {
        int op = code[pc];
        steps++;
        if (op == 0) tape[ptr] = (tape[ptr] + 1) & 255;
        else if (op == 1) tape[ptr] = (tape[ptr] + 255) & 255;
        else if (op == 2) ptr--;
        else if (op == 3) ptr++;
        else if (op == 4) { if (tape[ptr] == 0) pc = jump[pc]; }
        else if (op == 5) { if (tape[ptr] != 0) pc = jump[pc]; }
        else out += tape[ptr];
        pc++;
    }
    long long sum = 0;
    for (int i = 0; i < 30000; i++) sum += tape[i];
    printf("%lld %lld %lld\n", steps, sum, out);
    return 0;
}
