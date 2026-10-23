// After kostya/benchmarks matmul (written by Attractive Chaos, MIT license): float matrices, second matrix transposed.
#include <cstdio>
#include <vector>
using matrix = std::vector<std::vector<double>>;
static matrix gen(int n, double seed) {
    double tmp = seed / n / n;
    matrix m(n, std::vector<double>(n));
    for (int i = 0; i < n; i++)
        for (int j = 0; j < n; j++) m[i][j] = tmp * (i - j) * (i + j);
    return m;
}
static matrix mul(int n, const matrix &a, const matrix &b) {
    matrix m(n, std::vector<double>(n)), c(n, std::vector<double>(n));
    for (int i = 0; i < n; i++)
        for (int j = 0; j < n; j++) c[i][j] = b[j][i];
    for (int i = 0; i < n; i++)
        for (int j = 0; j < n; j++) {
            double t = 0.0;
            for (int k = 0; k < n; k++) t += a[i][k] * c[j][k];
            m[i][j] = t;
        }
    return m;
}
static double calc(int size) {
    int n = size / 2 * 2;
    matrix a = gen(n, 1.0), b = gen(n, 2.0), m = mul(n, a, b);
    return m[n / 2][n / 2];
}
int main() {
    std::printf("%f\n", calc(400));
    return 0;
}
