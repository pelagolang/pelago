/* After kostya/benchmarks matmul (written by Attractive Chaos, MIT license): float matrices, second matrix transposed. */
#include <stdio.h>
#include <stdlib.h>
static double **init(int n) {
    double **m = malloc(n * sizeof(void *));
    for (int i = 0; i < n; i++) m[i] = calloc(n, sizeof(double));
    return m;
}
static void destroy(int n, double **m) {
    for (int i = 0; i < n; i++) free(m[i]);
    free(m);
}
static double **gen(int n, double seed) {
    double tmp = seed / n / n;
    double **m = init(n);
    for (int i = 0; i < n; i++)
        for (int j = 0; j < n; j++) m[i][j] = tmp * (i - j) * (i + j);
    return m;
}
static double **mul(int n, double *const *a, double *const *b) {
    double **m = init(n), **c = init(n);
    for (int i = 0; i < n; i++)
        for (int j = 0; j < n; j++) c[i][j] = b[j][i];
    for (int i = 0; i < n; i++) {
        double *p = a[i], *q = m[i];
        for (int j = 0; j < n; j++) {
            double t = 0.0, *r = c[j];
            for (int k = 0; k < n; k++) t += p[k] * r[k];
            q[j] = t;
        }
    }
    destroy(n, c);
    return m;
}
static double calc(int size) {
    int n = size / 2 * 2;
    double **a = gen(n, 1.0), **b = gen(n, 2.0), **m = mul(n, a, b);
    double result = m[n / 2][n / 2];
    destroy(n, a);
    destroy(n, b);
    destroy(n, m);
    return result;
}
int main(void) {
    printf("%f\n", calc(400));
    return 0;
}
