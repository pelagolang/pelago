#!/bin/sh
# Native Pelago against C (gcc -O0 and -O2) on the programs that have a C twin in benches/c.
# Usage: compiler/benches/compare.sh   (run from the repository root; needs gcc and python3)
set -e
cargo build --release -q
out=$(mktemp -d)
for name in collatz fib38; do
    target/release/pelago build --opt -o "$out/$name.pel" "compiler/benches/$name.pel" >/dev/null
    gcc -O0 -o "$out/$name.c0" "compiler/benches/c/$name.c"
    gcc -O2 -o "$out/$name.c2" "compiler/benches/c/$name.c"
    echo "$name (best of 7)"
    python3 - "$out/$name.pel" "$out/$name.c0" "$out/$name.c2" <<'PY'
import subprocess, sys, time
for exe in sys.argv[1:]:
    best = min(
        (lambda t: (subprocess.run([exe], stdout=subprocess.DEVNULL), time.perf_counter() - t)[1])(time.perf_counter())
        for _ in range(7)
    )
    print(f"  {exe.rsplit('.', 1)[-1]:5s} {best:.3f}s")
PY
done
rm -rf "$out"
