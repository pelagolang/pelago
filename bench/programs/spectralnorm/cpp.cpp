#include <cmath>
#include <cstdio>
#include <vector>
static double a(long long i, long long j) { return 1.0 / (double)(((i + j) * (i + j + 1)) / 2 + i + 1); }
static void mul_av(const std::vector<double> &v, std::vector<double> &out) {
    for (size_t i = 0; i < v.size(); i++) { double s = 0.0; for (size_t j = 0; j < v.size(); j++) s += a(i, j) * v[j]; out[i] = s; }
}
static void mul_atv(const std::vector<double> &v, std::vector<double> &out) {
    for (size_t i = 0; i < v.size(); i++) { double s = 0.0; for (size_t j = 0; j < v.size(); j++) s += a(j, i) * v[j]; out[i] = s; }
}
int main() {
    const int n = 2000;
    std::vector<double> u(n, 1.0), v(n, 0.0), t(n, 0.0);
    for (int k = 0; k < 10; k++) { mul_av(u, t); mul_atv(t, v); mul_av(v, t); mul_atv(t, u); }
    double vbv = 0.0, vv = 0.0;
    for (int i = 0; i < n; i++) { vbv += u[i] * v[i]; vv += v[i] * v[i]; }
    std::printf("%f\n", std::sqrt(vbv / vv));
    return 0;
}
