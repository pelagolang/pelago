#include <cmath>
#include <cstdio>
static double x[5] = {0.0, 4.84143144246472090, 8.34336671824457987, 12.8943695621391310, 15.3796971148509165};
static double y[5] = {0.0, -1.16032004402742839, 4.12479856412430479, -15.1111514016986312, -25.9193146099879641};
static double z[5] = {0.0, -0.103622044471123109, -0.403523417114321381, -0.223307578892655734, 0.179258772950371181};
static double vx[5], vy[5], vz[5], m[5];
static double energy(void) {
    double e = 0.0;
    for (int i = 0; i < 5; i++) {
        e += 0.5 * m[i] * (vx[i] * vx[i] + vy[i] * vy[i] + vz[i] * vz[i]);
        for (int j = i + 1; j < 5; j++) {
            double dx = x[i] - x[j], dy = y[i] - y[j], dz = z[i] - z[j];
            e -= m[i] * m[j] / std::sqrt(dx * dx + dy * dy + dz * dz);
        }
    }
    return e;
}
int main(void) {
    const double pi = 3.141592653589793, sm = 4.0 * pi * pi, dpy = 365.24;
    vx[1] = 0.00166007664274403694 * dpy; vx[2] = -0.00276742510726862411 * dpy; vx[3] = 0.00296460137564761618 * dpy; vx[4] = 0.00268067772490389322 * dpy;
    vy[1] = 0.00769901118419740425 * dpy; vy[2] = 0.00499852801234917238 * dpy; vy[3] = 0.00237847173959480950 * dpy; vy[4] = 0.00162824170038242295 * dpy;
    vz[1] = -0.0000690460016972063023 * dpy; vz[2] = 0.0000230417297573763929 * dpy; vz[3] = -0.0000296589568540237556 * dpy; vz[4] = -0.0000951592254519715870 * dpy;
    m[0] = sm; m[1] = 0.000954791938424326609 * sm; m[2] = 0.000285885980666130812 * sm; m[3] = 0.0000436624404335156298 * sm; m[4] = 0.0000515138902046611451 * sm;
    double px = 0.0, py = 0.0, pz = 0.0;
    for (int i = 0; i < 5; i++) { px += vx[i] * m[i]; py += vy[i] * m[i]; pz += vz[i] * m[i]; }
    vx[0] = -px / sm; vy[0] = -py / sm; vz[0] = -pz / sm;
    std::printf("%f\n", energy());
    for (int step = 0; step < 200000; step++) {
        for (int i = 0; i < 5; i++)
            for (int j = i + 1; j < 5; j++) {
                double dx = x[i] - x[j], dy = y[i] - y[j], dz = z[i] - z[j];
                double d2 = dx * dx + dy * dy + dz * dz;
                double mag = 0.01 / (d2 * std::sqrt(d2));
                vx[i] -= dx * m[j] * mag; vy[i] -= dy * m[j] * mag; vz[i] -= dz * m[j] * mag;
                vx[j] += dx * m[i] * mag; vy[j] += dy * m[i] * mag; vz[j] += dz * m[i] * mag;
            }
        for (int i = 0; i < 5; i++) { x[i] += 0.01 * vx[i]; y[i] += 0.01 * vy[i]; z[i] += 0.01 * vz[i]; }
    }
    std::printf("%f\n", energy());
    return 0;
}
