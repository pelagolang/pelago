#!/bin/sh
# Native Pelago against C (gcc -O0 and -O2) on every program in benches/ that has a C twin in benches/c.
# Usage: compiler/benches/compare.sh [--json] [--runs N] [name ...]   (run from the repository root; needs gcc and python3)
#   --json     also print one JSON line per program (name, pelago, c0, c2 seconds, ratio to -O2) after the table
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
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
for name in $names; do
    target/release/pelago build --opt -o "$out/$name.pel" "compiler/benches/$name.pel" >/dev/null
    gcc -O0 -o "$out/$name.c0" "compiler/benches/c/$name.c"
    gcc -O2 -o "$out/$name.c2" "compiler/benches/c/$name.c"
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

rows, bad = [], []
for name in names:
    (p, pt), (c0, _), (c2, ct) = (best(f"{out}/{name}.{k}") for k in ("pel", "c0", "c2"))
    if pt != ct:
        bad.append(name)
    rows.append((name, p, c0, c2))
print(f"{'program':12s} {'pelago':>8s} {'C -O0':>8s} {'C -O2':>8s} {'vs -O0':>7s} {'vs -O2':>7s}   (best of {runs}, seconds)")
for name, p, c0, c2 in rows:
    flag = "  OUTPUT DIFFERS" if name in bad else ""
    print(f"{name:12s} {p:8.3f} {c0:8.3f} {c2:8.3f} {p / c0:6.2f}x {p / c2:6.2f}x{flag}")
if as_json:
    for name, p, c0, c2 in rows:
        print(json.dumps({"name": name, "pelago": round(p, 4), "c_o0": round(c0, 4), "c_o2": round(c2, 4), "ratio_o2": round(p / c2, 2)}))
sys.exit(1 if bad else 0)
PY
