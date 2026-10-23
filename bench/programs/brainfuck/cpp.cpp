#include <cstdio>
#include <string>
#include <vector>
int main() {
    const std::string prog = "-[>-[>-[>+<-]<-]<-]>>>.";
    std::vector<int> code, jump(prog.size(), 0), stack;
    for (char c : prog) code.push_back(c == '+' ? 0 : c == '-' ? 1 : c == '<' ? 2 : c == '>' ? 3 : c == '[' ? 4 : c == ']' ? 5 : 6);
    for (size_t i = 0; i < code.size(); i++) {
        if (code[i] == 4) stack.push_back((int)i);
        else if (code[i] == 5) { int open = stack.back(); stack.pop_back(); jump[open] = (int)i; jump[i] = open; }
    }
    std::vector<long long> tape(30000, 0);
    long long ptr = 0, steps = 0, out = 0;
    size_t pc = 0;
    while (pc < code.size()) {
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
    for (long long v : tape) sum += v;
    std::printf("%lld %lld %lld\n", steps, sum, out);
    return 0;
}
