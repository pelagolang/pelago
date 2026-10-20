#!/bin/sh
# Native Pelago against C (gcc -O0 and -O2) and Rust (rustc -O) on every program in benches/ that has a C twin in benches/c.
# Usage: compiler/benches/compare.sh [--json] [--runs N] [name ...]   (run from the repository root; needs gcc and python3)
# A Rust twin in benches/rs/<name>.rs adds the Rust columns when `rustc` is installed; without it the table is C only.
# The goal is a "vs -O2" ratio of 1.00x or less; those rows are marked `<= C`, and the last lines give the geometric means.
#   --json     also print one JSON line per program (name, pelago, c0, c2, rust seconds, ratios) after the table
#   --runs N   best of N runs per executable (default 7)
#   name ...   only these programs (file names in benches/ without `.pel`)
# Each program must print the same output as its C twin; a mismatch is reported and the exit status is 1.
set -e
json=0
runs=7
names=""
while [ $# -gt 0 ]; do
    case "$1" in
        --json) json=1 ;;
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
for name in $names; do
    target/release/pelago build --opt -o "$out/$name.pel" "compiler/benches/$name.pel" >/dev/null
    gcc -O0 -o "$out/$name.c0" "compiler/benches/c/$name.c"
    gcc -O2 -o "$out/$name.c2" "compiler/benches/c/$name.c"
    if [ "$have_rust" = 1 ] && [ -f "compiler/benches/rs/$name.rs" ]; then
        rustc -O -C overflow-checks=off -o "$out/$name.rs" "compiler/benches/rs/$name.rs"
    fi
done
python3 - "$json" "$runs" "$out" $names <<'PY'
import json, subprocess, sys, time

as_json, runs, out, names = sys.argv[1] == "1", int(sys.argv[2]), sys.argv[3], sys.argv[4:]

def best(exe):
    # Returns (best seconds of `runs`, stdout of the last run).
    times, text = [], b""
    for _ in range(runs):
        t = time.perf_counter()
        text = subprocess.run([exe], stdout=subprocess.PIPE).stdout
        times.append(time.perf_counter() - t)
    return min(times), text

import math, os

rows, bad = [], []
for name in names:
    (p, pt), (c0, _), (c2, ct) = (best(f"{out}/{name}.{k}") for k in ("pel", "c0", "c2"))
    rs = None
    if os.path.exists(f"{out}/{name}.rs"):
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
if as_json:
    for name, p, c0, c2, rs in rows:
        d = {"name": name, "pelago": round(p, 4), "c_o0": round(c0, 4), "c_o2": round(c2, 4), "ratio_o2": round(p / c2, 2)}
        if rs is not None:
            d.update({"rust": round(rs, 4), "ratio_rust": round(p / rs, 2)})
        print(json.dumps(d))
sys.exit(1 if bad else 0)
PY
