#!/usr/bin/env python3
"""Benchmark harness: Pelago against C, C++ and Rust, with several compilers per language.

Usage (from the repository root; needs python3 and at least one of gcc, clang, tcc, g++, clang++, rustc):
    python3 compiler/benches/harness.py [--runs N] [--warmups N] [--compile-runs N] [--pin CPU]
                                        [--json FILE] [--md FILE] [--timeout S] [name ...]

For every program in benches/*.pel it builds one executable per available contestant, checks that all of them print the
same output, then reports run time (median and minimum of N runs after discarded warm-ups, with the spread), compile time
(median of M builds), binary size and peak memory. The goal row is Pelago at or below the best of all other rows.

Contestants (a missing compiler just drops its rows; override a command with the environment variables CC, CXX, CLANG,
CLANGXX, TCC, RUSTC):
    pelago       target/release/pelago build --opt            (PELAGO_BUILD_FLAGS is added)
    gcc-O0 gcc-O2 gcc-O2-trapv clang-O2 tcc          C twins in benches/c
    g++-O2 clang++-O2                                 C++ twins in benches/cpp
    rustc-O rustc-O-checked                           Rust twins in benches/rs
The `-trapv` and `-checked` rows stop on overflow like Pelago does; they are the like-for-like rows.
"""
import argparse, json, math, os, platform, shutil, statistics, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(ROOT))
EXE = ".exe" if os.name == "nt" else ""


def tool(env, *names):
    if os.environ.get(env):
        return os.environ[env]
    for n in names:
        if shutil.which(n):
            return n
    return None


def contestants():
    """(label, language dir, source suffix, command builder or None when the tool is missing, checked)"""
    cc, clang, tcc = tool("CC", "gcc"), tool("CLANG", "clang"), tool("TCC", "tcc")
    cxx, clangxx, rustc = tool("CXX", "g++"), tool("CLANGXX", "clang++"), tool("RUSTC", "rustc")
    pel = os.path.join(REPO, "target", "release", "pelago" + EXE)
    flags = os.environ.get("PELAGO_BUILD_FLAGS", "").split()

    def c(tool_, *opts):
        return (lambda src, out: [tool_, *opts, "-o", out, src]) if tool_ else None

    def rs(*opts):
        return (lambda src, out: [rustc, *opts, "-o", out, src]) if rustc else None

    return [
        ("pelago", "", ".pel", lambda src, out: [pel, "build", "--opt", *flags, "-o", out, src], True),
        ("gcc-O0", "c", ".c", c(cc, "-O0"), False),
        ("gcc-O2", "c", ".c", c(cc, "-O2"), False),
        ("gcc-O2-trapv", "c", ".c", c(cc, "-O2", "-ftrapv"), True),
        ("clang-O2", "c", ".c", c(clang, "-O2"), False),
        ("tcc", "c", ".c", c(tcc), False),
        ("g++-O2", "cpp", ".cpp", c(cxx, "-O2", "-std=c++17"), False),
        ("clang++-O2", "cpp", ".cpp", c(clangxx, "-O2", "-std=c++17"), False),
        ("rustc-O", "rs", ".rs", rs("-O", "-C", "codegen-units=1", "-C", "overflow-checks=off"), False),
        ("rustc-O-checked", "rs", ".rs", rs("-O", "-C", "codegen-units=1", "-C", "overflow-checks=on"), True),
    ]


def version(cmd):
    try:
        out = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=20).stdout
        return out.strip().splitlines()[0]
    except Exception:
        return None


def environment():
    env = {"date": time.strftime("%Y-%m-%d"), "os": platform.platform(), "machine": platform.machine(), "cpus": os.cpu_count(),
           "python": platform.python_version()}
    try:
        with open("/proc/cpuinfo") as f:
            env["cpu"] = next(l.split(":", 1)[1].strip() for l in f if l.startswith("model name"))
    except Exception:
        env["cpu"] = platform.processor() or None
    for key, cmd in [("gcc", [tool("CC", "gcc") or "gcc", "--version"]), ("g++", [tool("CXX", "g++") or "g++", "--version"]),
                     ("clang", [tool("CLANG", "clang") or "clang", "--version"]),
                     ("clang++", [tool("CLANGXX", "clang++") or "clang++", "--version"]),
                     ("tcc", [tool("TCC", "tcc") or "tcc", "-v"]), ("rustc", [tool("RUSTC", "rustc") or "rustc", "--version"])]:
        env[key] = version(cmd)
    try:
        env["pelago"] = subprocess.run(["git", "-C", REPO, "rev-parse", "--short", "HEAD"], stdout=subprocess.PIPE, text=True).stdout.strip()
    except Exception:
        env["pelago"] = None
    return env


def run_once(path, timeout, pin):
    """One run: (seconds, peak resident KiB or None, stdout)."""
    cmd = [path]
    if pin is not None and shutil.which("taskset"):
        cmd = ["taskset", "-c", str(pin), path]
    start = time.perf_counter()
    if hasattr(os, "wait4"):
        p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stdin=subprocess.DEVNULL)
        try:
            out = p.stdout.read()
            _, status, ru = os.wait4(p.pid, 0)
        except Exception:
            p.kill()
            raise
        secs = time.perf_counter() - start
        p.returncode = os.waitstatus_to_exitcode(status)
        rss = ru.ru_maxrss // (1024 if sys.platform == "darwin" else 1)
        if secs > timeout:
            raise subprocess.TimeoutExpired(cmd, timeout)
        return secs, rss, out.replace(b"\r\n", b"\n")
    out = subprocess.run(cmd, stdout=subprocess.PIPE, stdin=subprocess.DEVNULL, timeout=timeout).stdout
    return time.perf_counter() - start, None, out.replace(b"\r\n", b"\n")


def measure(path, runs, warmups, timeout, pin):
    for _ in range(warmups):
        run_once(path, timeout, pin)
    times, rss, text = [], [], b""
    for _ in range(runs):
        s, r, text = run_once(path, timeout, pin)
        times.append(s)
        if r is not None:
            rss.append(r)
    med = statistics.median(times)
    spread = (max(times) - min(times)) / med if med else 0.0
    return {"median": med, "min": min(times), "spread": spread, "rss_kib": max(rss) if rss else None, "text": text}


def build(cmd, out, compile_runs):
    """Build `compile_runs` times; returns the median compile seconds, or an error string."""
    times = []
    for _ in range(compile_runs):
        if os.path.exists(out):
            os.remove(out)
        t = time.perf_counter()
        r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        times.append(time.perf_counter() - t)
        if r.returncode != 0:
            return r.stderr.strip().splitlines()[-1] if r.stderr.strip() else "build failed"
    return statistics.median(times)


def geomean(xs):
    return math.exp(sum(math.log(x) for x in xs) / len(xs)) if xs else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--runs", type=int, default=7)
    ap.add_argument("--warmups", type=int, default=2)
    ap.add_argument("--compile-runs", type=int, default=3)
    ap.add_argument("--timeout", type=float, default=120)
    ap.add_argument("--pin", type=int, default=None, help="run on this CPU with taskset (Linux)")
    ap.add_argument("--json", metavar="FILE")
    ap.add_argument("--md", metavar="FILE")
    a = ap.parse_args()

    names = a.names or sorted(f[:-4] for f in os.listdir(ROOT) if f.endswith(".pel"))
    subprocess.run(["cargo", "build", "--release", "-q"], cwd=REPO, check=True)
    cons = contestants()
    env = environment()
    results, bad = {}, []
    with tempfile.TemporaryDirectory() as tmp:
        for name in names:
            row = {}
            for label, lang, suffix, mk, _checked in cons:
                src = os.path.join(ROOT, lang, name + suffix)
                if mk is None or not os.path.exists(src):
                    continue
                out = os.path.join(tmp, f"{name}.{label.replace('+', 'p')}{EXE}")
                print(f"building {name} [{label}]", file=sys.stderr, flush=True)
                ct = build(mk(src, out), out, a.compile_runs)
                if isinstance(ct, str):
                    row[label] = {"error": ct}
                    continue
                print(f"running  {name} [{label}]", file=sys.stderr, flush=True)
                try:
                    m = measure(out, a.runs, a.warmups, a.timeout, a.pin)
                except subprocess.TimeoutExpired:
                    row[label] = {"error": "timeout"}
                    continue
                m["compile"] = ct
                m["size"] = os.path.getsize(out)
                row[label] = m
            ref = next((row[k]["text"] for k in ("gcc-O2", "clang-O2", "g++-O2", "rustc-O") if k in row and "text" in row[k]), None)
            for label, m in row.items():
                if "text" in m:
                    if ref is not None and m["text"] != ref:
                        m["mismatch"] = True
                        bad.append(f"{name} [{label}]")
                    del m["text"]
            results[name] = row

    labels = [c[0] for c in cons if any(c[0] in r and "median" in r[c[0]] for r in results.values())]
    others = [l for l in labels if l != "pelago"]

    def best_other(row):
        vals = [row[l]["median"] for l in others if l in row and "median" in row[l]]
        return min(vals) if vals else None

    lines = []
    emit = lines.append
    emit(f"Run time, median of {a.runs} after {a.warmups} warm-ups, seconds (spread = (max-min)/median; `!` marks > 10%)")
    emit("| program | " + " | ".join(labels) + " | pelago / best other |")
    emit("|---|" + "---:|" * (len(labels) + 1))
    ratios, below = [], 0
    for name, row in results.items():
        cells = []
        for l in labels:
            m = row.get(l)
            if not m:
                cells.append("-")
            elif "error" in m:
                cells.append("error")
            else:
                cells.append(f"{m['median']:.3f}" + ("!" if m["spread"] > 0.10 else "") + (" DIFF" if m.get("mismatch") else ""))
        b = best_other(row)
        p = row.get("pelago", {}).get("median")
        r = p / b if p and b else None
        if r is not None:
            ratios.append(r)
            below += r <= 1.0
        emit(f"| {name} | " + " | ".join(cells) + f" | {r:.2f}x |" if r is not None else f"| {name} | " + " | ".join(cells) + " | - |")
    gm = geomean(ratios)
    if gm:
        emit(f"\nGeometric mean of pelago / best other row: {gm:.2f}x; at or below every other row on {below} of {len(ratios)} programs.")
    for title, key, fmt in [("Compile time, median seconds", "compile", "{:.3f}"), ("Binary size, KiB", "size", None),
                            ("Peak memory, MiB", "rss_kib", None)]:
        emit(f"\n{title}")
        emit("| program | " + " | ".join(labels) + " |")
        emit("|---|" + "---:|" * len(labels))
        for name, row in results.items():
            cells = []
            for l in labels:
                v = row.get(l, {}).get(key)
                if v is None:
                    cells.append("-")
                elif key == "size":
                    cells.append(f"{v / 1024:.0f}")
                elif key == "rss_kib":
                    cells.append(f"{v / 1024:.1f}")
                else:
                    cells.append(fmt.format(v))
            emit(f"| {name} | " + " | ".join(cells) + " |")
    if bad:
        emit("\nOUTPUT DIFFERS: " + ", ".join(bad))
    text = "\n".join(lines)
    print(text)
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"environment": env, "runs": a.runs, "warmups": a.warmups, "results": results}, f, indent=1)
    if a.md:
        head = ["# Benchmark results", "", "Generated by `compiler/benches/harness.py`. Do not edit by hand.", "", "## Environment", "",
                "| | |", "|---|---|"] + [f"| {k} | {v} |" for k, v in env.items() if v] + ["", "## Results", ""]
        with open(a.md, "w") as f:
            f.write("\n".join(head) + "\n" + text + "\n")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
