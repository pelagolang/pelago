#include <math.h>
#include <stdio.h>
#include <stdlib.h>
static double a(long long i, long long j) { return 1.0 / (double)(((i + j) * (i + j + 1)) / 2 + i + 1); }
static void mul_av(const double *v, double *out, int n) {
    for (int i = 0; i < n; i++) { double s = 0.0; for (int j = 0; j < n; j++) s += a(i, j) * v[j]; out[i] = s; }
}
static void mul_atv(const double *v, double *out, int n) {
    for (int i = 0; i < n; i++) { double s = 0.0; for (int j = 0; j < n; j++) s += a(j, i) * v[j]; out[i] = s; }
}
int main(void) {
    const int n = 2000;
    double *u = malloc(n * sizeof(double)), *v = calloc(n, sizeof(double)), *t = calloc(n, sizeof(double));
    for (int i = 0; i < n; i++) u[i] = 1.0;
    for (int k = 0; k < 10; k++) { mul_av(u, t, n); mul_atv(t, v, n); mul_av(v, t, n); mul_atv(t, u, n); }
    double vbv = 0.0, vv = 0.0;
    for (int i = 0; i < n; i++) { vbv += u[i] * v[i]; vv += v[i] * v[i]; }
    printf("%f\n", sqrt(vbv / vv));
    return 0;
}
