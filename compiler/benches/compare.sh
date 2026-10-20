#!/bin/sh
# Native Pelago against C (gcc -O0 and -O2) and Rust (rustc -O) on every program in benches/ that has a C twin in benches/c.
# Usage: compiler/benches/compare.sh [--json] [--runs N] [name ...]   (run from the repository root; needs gcc and python3)
# On Windows (Git Bash, MinGW gcc) it builds `.exe` files and the C columns are MinGW gcc; the CI job `bench-windows` runs it there.
# One run of one executable may take BENCH_TIMEOUT seconds (default 120); after that the script names the program and exits with status 2.
# A Rust twin in benches/rs/<name>.rs adds the Rust columns when `rustc` is installed; without it the table is C only.
# The goal is a "vs -O2" ratio of 1.00x or less; those rows are marked `<= C`, and the last lines give the geometric means.
#   --json     also print one JSON line per program (name, pelago, c0, c2, rust seconds, ratios) after the table
#   --runs N   best of N runs per executable (default 7)
#   --checked  also time C with `-O2 -ftrapv` and Rust with `-O -C overflow-checks=on`, which stop on overflow like
#              Pelago does, and print a second table against them (C and Rust without checks are the "goal" rows)
#   name ...   only these programs (file names in benches/ without `.pel`)
# Each program must print the same output as its C twin; a mismatch is reported and the exit status is 1.
set -e
json=0
checked=0
runs=7
names=""
while [ $# -gt 0 ]; do
    case "$1" in
        --json) json=1 ;;
        --checked) checked=1 ;;
        --runs) shift; runs="$1" ;;
        *) names="$names $1" ;;
    esac
    shift
done
if [ -z "$names" ]; then
    for f in compiler/benches/c/*.c; do
        n=$(basename "$f" .c)
        [ -f "compiler/benches/$n.pel" ] && names="$names $n"
    done
fi
cargo build --release -q
have_rust=0
command -v rustc >/dev/null 2>&1 && have_rust=1
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
# Windows: executables need the `.exe` suffix, and python needs a path it understands.
exe=""
if [ "$OS" = "Windows_NT" ]; then
    exe=".exe"
    command -v cygpath >/dev/null 2>&1 && out=$(cygpath -m "$out")
fi
for name in $names; do
    echo "building $name" >&2
    target/release/pelago build --opt -o "$out/$name.pel$exe" "compiler/benches/$name.pel" >/dev/null
    gcc -O0 -o "$out/$name.c0$exe" "compiler/benches/c/$name.c"
    gcc -O2 -o "$out/$name.c2$exe" "compiler/benches/c/$name.c"
    if [ "$have_rust" = 1 ] && [ -f "compiler/benches/rs/$name.rs" ]; then
        rustc -O -C overflow-checks=off -o "$out/$name.rs$exe" "compiler/benches/rs/$name.rs"
    fi
    if [ "$checked" = 1 ]; then
        gcc -O2 -ftrapv -o "$out/$name.ct$exe" "compiler/benches/c/$name.c"
        if [ "$have_rust" = 1 ] && [ -f "compiler/benches/rs/$name.rs" ]; then
            rustc -O -C overflow-checks=on -o "$out/$name.rc$exe" "compiler/benches/rs/$name.rs"
        fi
    fi
done
python3 - "$json" "$runs" "$out" "$checked" "$exe" "${BENCH_TIMEOUT:-120}" $names <<'PY'
import json, subprocess, sys, time

as_json, runs, out, checked, exe, limit, names = sys.argv[1] == "1", int(sys.argv[2]), sys.argv[3], sys.argv[4] == "1", sys.argv[5], float(sys.argv[6]), sys.argv[7:]

def best(path):
    # Returns (best seconds of `runs`, stdout of the last run). A run that takes longer than `limit` ends the script.
    times, text = [], b""
    print("running " + path, file=sys.stderr, flush=True)
    for _ in range(runs):
        t = time.perf_counter()
        try:
            text = subprocess.run([path + exe], stdout=subprocess.PIPE, stdin=subprocess.DEVNULL, timeout=limit).stdout
        except subprocess.TimeoutExpired:
            print(f"TIMEOUT: {path} did not finish within {limit:.0f} s", file=sys.stderr, flush=True)
            sys.exit(2)
        times.append(time.perf_counter() - t)
    return min(times), text

import math, os

rows, bad = [], []
for name in names:
    (p, pt), (c0, _), (c2, ct) = (best(f"{out}/{name}.{k}") for k in ("pel", "c0", "c2"))
    rs = None
    if os.path.exists(f"{out}/{name}.rs{exe}"):
        rs, rt = best(f"{out}/{name}.rs")
        if rt != ct:
            bad.append(name + " (rust)")
    if pt != ct:
        bad.append(name)
    rows.append((name, p, c0, c2, rs))

def ratio(a, b):
    return f"{a / b:6.2f}x" if b else "     -"

with_rust = any(r[4] is not None for r in rows)
head = f"{'program':12s} {'pelago':>8s} {'C -O0':>8s} {'C -O2':>8s}"
if with_rust:
    head += f" {'Rust -O':>8s}"
head += f" {'vs -O0':>7s} {'vs -O2':>7s}"
if with_rust:
    head += f" {'vs Rust':>7s}"
print(head + f"   (best of {runs}, seconds; goal: vs -O2 <= 1.00x)")
for name, p, c0, c2, rs in rows:
    line = f"{name:12s} {p:8.3f} {c0:8.3f} {c2:8.3f}"
    if with_rust:
        line += f" {rs:8.3f}" if rs is not None else f" {'-':>8s}"
    line += f" {ratio(p, c0)} {ratio(p, c2)}"
    if with_rust:
        line += f" {ratio(p, rs) if rs is not None else '      -'}"
    line += "  <= C" if p <= c2 else ""
    line += "  OUTPUT DIFFERS" if name in bad else ""
    print(line)

def geomean(xs):
    return math.exp(sum(math.log(x) for x in xs) / len(xs))

at_or_below = sum(1 for r in rows if r[1] <= r[3])
print(f"geometric mean vs C -O2: {geomean([r[1] / r[3] for r in rows]):.2f}x; at or below C -O2 on {at_or_below} of {len(rows)} programs")
rr = [r for r in rows if r[4] is not None]
if rr:
    print(f"geometric mean vs Rust: {geomean([r[1] / r[4] for r in rr]):.2f}x over {len(rr)} programs")
elif with_rust is False:
    print("Rust columns skipped (no rustc or no benches/rs twins)")
if checked:
    print()
    print(f"{'program':12s} {'pelago':>8s} {'C trapv':>8s} {'Rust chk':>8s} {'vs C':>7s} {'vs Rust':>7s}   (same overflow checking; best of {runs}, seconds)")
    for name, p, c0, c2, rs in rows:
        ct, _ = best(f"{out}/{name}.ct")
        rc = best(f"{out}/{name}.rc")[0] if os.path.exists(f"{out}/{name}.rc{exe}") else None
        line = f"{name:12s} {p:8.3f} {ct:8.3f} " + (f"{rc:8.3f}" if rc is not None else f"{'-':>8s}")
        line += f" {ratio(p, ct)} " + (ratio(p, rc) if rc is not None else "      -")
        print(line)
if as_json:
    for name, p, c0, c2, rs in rows:
        d = {"name": name, "pelago": round(p, 4), "c_o0": round(c0, 4), "c_o2": round(c2, 4), "ratio_o2": round(p / c2, 2)}
        if rs is not None:
            d.update({"rust": round(rs, 4), "ratio_rust": round(p / rs, 2)})
        print(json.dumps(d))
sys.exit(1 if bad else 0)
PY
