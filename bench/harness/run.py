#!/usr/bin/env python3
"""Benchmark harness: Pelago against C, C++ and Rust, with several compilers per language.

Usage (from the repository root; needs python 3.11+ and at least one of gcc, clang, tcc, g++, clang++, rustc):
    python3 bench/harness/run.py [--runs N] [--warmups N] [--compile-runs N] [--pin CPU] [--size small,medium,large]
                                 [--save] [--json FILE] [--md FILE] [--site FILE] [--history FILE] [--bless]
                                 [--timeout S] [name ...]

Each program is a directory bench/programs/<name>/ with pelago.pel, c.c, cpp.cpp, rs.rs (a twin may be missing), sizes.toml
(optional) and expected*.txt (written by --bless). The harness builds one executable per available contestant, checks that all
print the same output, then reports run time (median and minimum of N runs after discarded warm-ups, with the spread), compile
time (median of M builds), binary size and peak memory. The goal row is Pelago at or below the best of all other rows.

Contestants (a missing compiler just drops its rows; override a command with CC, CXX, CLANG, CLANGXX, TCC, RUSTC):
    pelago       target/release/pelago build --opt            (PELAGO_BUILD_FLAGS is added)
    gcc-O0 gcc-O2 gcc-O2-trapv clang-O2 tcc          c.c
    g++-O2 clang++-O2                                 cpp.cpp
    g++-O2-expert                                     cpp-expert.cpp (arena, flat tables; only where the program has one)
    rustc-O rustc-O-checked                           rs.rs
The `-trapv` and `-checked` rows stop on overflow like Pelago does; they are the like-for-like rows.
Sizes: sizes.toml gives, per size, text replacements applied to every twin; `medium` is the committed source. Programs
without a sizes.toml run at medium only. `--bless` writes expected.txt (expected.small.txt, ...) from the reference row (gcc -O2,
else clang, g++, rustc); later runs compare every row to the file.
`--save` writes bench/results/<date>-<machine>.json and bench/results.md. `--site FILE` writes the medium results in the
`website/content/bench.json` format; `--history FILE` appends one JSON line. bench/toolchains.toml lists the pinned versions;
the results say when a tool differs.
"""
import argparse, json, tomllib, math, os, platform, shutil, statistics, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)  # bench/
REPO = os.path.dirname(ROOT)
PROGRAMS = os.path.join(ROOT, "programs")
EXE = ".exe" if os.name == "nt" else ""


def tool(env, *names):
    if os.environ.get(env):
        return os.environ[env]
    for n in names:
        if shutil.which(n):
            return n
    return None


def contestants():
    """(label, source file name in a program directory, command builder or None when the tool is missing, checked)"""
    cc, clang, tcc = tool("CC", "gcc"), tool("CLANG", "clang"), tool("TCC", "tcc")
    cxx, clangxx, rustc = tool("CXX", "g++"), tool("CLANGXX", "clang++"), tool("RUSTC", "rustc")
    pel = os.path.join(REPO, "target", "release", "pelago" + EXE)
    flags = os.environ.get("PELAGO_BUILD_FLAGS", "").split()

    def c(tool_, *opts):
        return (lambda src, out: [tool_, *opts, "-o", out, src]) if tool_ else None

    def rs(*opts):
        return (lambda src, out: [rustc, *opts, "-o", out, src]) if rustc else None

    return [
        ("pelago", "pelago.pel", lambda src, out: [pel, "build", "--opt", *flags, "-o", out, src], True),
        ("gcc-O0", "c.c", c(cc, "-O0"), False),
        ("gcc-O2", "c.c", c(cc, "-O2"), False),
        ("gcc-O2-trapv", "c.c", c(cc, "-O2", "-ftrapv"), True),
        ("clang-O2", "c.c", c(clang, "-O2"), False),
        ("tcc", "c.c", c(tcc), False),
        ("g++-O2", "cpp.cpp", c(cxx, "-O2", "-std=c++17"), False),
        ("clang++-O2", "cpp.cpp", c(clangxx, "-O2", "-std=c++17"), False),
        ("g++-O2-expert", "cpp-expert.cpp", c(cxx, "-O2", "-std=c++17"), False),
        ("rustc-O", "rs.rs", rs("-O", "-C", "codegen-units=1", "-C", "overflow-checks=off"), False),
        ("rustc-O-checked", "rs.rs", rs("-O", "-C", "codegen-units=1", "-C", "overflow-checks=on"), True),
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


def peak_rss_kib(path, timeout):
    """Peak resident KiB of one extra run. Linux: poll VmHWM in /proc (ru_maxrss would be inflated by this Python process, which
    the child inherits at fork); elsewhere ru_maxrss from run_once."""
    if not os.path.exists("/proc/self/status"):
        return None
    p = subprocess.Popen([path], stdout=subprocess.DEVNULL, stdin=subprocess.DEVNULL)
    best, end = 0, time.time() + timeout
    while p.poll() is None and time.time() < end:
        try:
            with open(f"/proc/{p.pid}/status") as f:
                for line in f:
                    if line.startswith("VmHWM:"):
                        best = max(best, int(line.split()[1]))
        except OSError:
            pass
        time.sleep(0.0005)
    p.kill()
    p.wait()
    return best or None


def measure(path, runs, warmups, timeout, pin):
    for _ in range(warmups):
        run_once(path, timeout, pin)
    times, rss, text = [], [], b""
    for _ in range(runs):
        s, r, text = run_once(path, timeout, pin)
        times.append(s)
        if r is not None:
            rss.append(r)
    hwm = peak_rss_kib(path, timeout)
    if hwm is not None:
        rss = [hwm]
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


def sized_source(src, name, size, repl, tmp):
    """Path of the source for `size`: the file itself for medium, else a copy with the size's replacements applied."""
    pairs = repl.get(name, {}).get(size)
    if size == "medium" or pairs is None:
        return src if size == "medium" else None
    with open(src) as f:
        text = f.read()
    applied = 0
    for old, new in pairs:  # a pair may be missing from a twin (sieve's `n + 1` is written differently in each language)
        if old in text:
            text = text.replace(old, new)
            applied += 1
    if not applied:
        raise SystemExit(f"sizes.json: no replacement for {name} {size} applies to {src}")
    d = os.path.join(tmp, "src", size, name)
    os.makedirs(d, exist_ok=True)
    out = os.path.join(d, os.path.basename(src))
    with open(out, "w") as f:
        f.write(text)
    return out


def geomean(xs):
    return math.exp(sum(math.log(x) for x in xs) / len(xs)) if xs else None


def check_pins(env):
    """Names of tools whose version string does not contain the version pinned in bench/toolchains.toml."""
    path = os.path.join(ROOT, "toolchains.toml")
    if not os.path.exists(path):
        return ""
    with open(path, "rb") as f:
        pins = tomllib.load(f).get("pinned", {})
    drift = [f"{k} (pinned {v})" for k, v in pins.items() if v and env.get(k) and v not in env[k]]
    return ", ".join(drift)


def run_size(size, names, cons, repl, a, tmp):
    """Build, check and time every program at one size; returns {name: {label: result}} and the list of problems."""
    results, bad = {}, []
    for name in names:
        row = {}
        for label, fname, mk, _checked in cons:
            src = os.path.join(PROGRAMS, name, fname)
            if mk is None or not os.path.exists(src):
                continue
            src = sized_source(src, name, size, repl, os.path.join(tmp, size))
            if src is None:
                break
            out = os.path.join(tmp, f"{name}.{size}.{label}{EXE}")
            print(f"building {name} {size} [{label}]", file=sys.stderr, flush=True)
            ct = build(mk(src, out), out, a.compile_runs)
            if isinstance(ct, str):
                row[label] = {"error": ct}
                continue
            print(f"running  {name} {size} [{label}]", file=sys.stderr, flush=True)
            try:
                m = measure(out, a.runs, a.warmups, a.timeout, a.pin)
            except subprocess.TimeoutExpired:
                row[label] = {"error": "timeout"}
                continue
            m["compile"] = ct
            m["size"] = os.path.getsize(out)
            row[label] = m
        if not row:
            continue
        exp_path = os.path.join(PROGRAMS, name, "expected.txt" if size == "medium" else f"expected.{size}.txt")
        ref = next((row[k]["text"] for k in ("gcc-O2", "clang-O2", "g++-O2", "rustc-O") if k in row and "text" in row[k]), None)
        if a.bless and ref is not None:
            with open(exp_path, "wb") as f:
                f.write(ref)
        elif os.path.exists(exp_path):
            with open(exp_path, "rb") as f:
                ref = f.read()
        for label, m in row.items():
            if "text" in m:
                if ref is not None and m["text"] != ref:
                    m["mismatch"] = True
                    bad.append(f"{name} {size} [{label}]")
                del m["text"]
        results[name] = row
    return results, bad


def report(results, labels, a, size):
    others = [l for l in labels if l != "pelago"]
    lines = []
    emit = lines.append
    emit(f"### Size: {size}\n")
    emit(f"Run time, median of {a.runs} after {a.warmups} warm-ups, seconds (`!` marks a spread over 10%: (max-min)/median)\n")
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
        vals = [row[l]["median"] for l in others if l in row and "median" in row[l]]
        p = row.get("pelago", {}).get("median")
        r = p / min(vals) if p and vals else None
        if r is not None:
            ratios.append(r)
            below += r <= 1.0
        emit(f"| {name} | " + " | ".join(cells) + (f" | {r:.2f}x |" if r is not None else " | - |"))
    gm = geomean(ratios)
    if gm:
        emit(f"\nGeometric mean of pelago / best other row: {gm:.2f}x; at or below every other row on {below} of {len(ratios)} programs.")
    emit("\nCost of overflow checks (checked / unchecked run time; Pelago always checks) and Pelago against the checked rows\n")
    emit("| program | gcc -ftrapv / gcc | rustc checked / unchecked | pelago / gcc -ftrapv | pelago / rustc checked |")
    emit("|---|---:|---:|---:|---:|")
    def rat(row, x, y):
        a, b = row.get(x, {}).get("median"), row.get(y, {}).get("median")
        return f"{a / b:.2f}x" if a and b else "-"
    for name, row in results.items():
        emit(f"| {name} | {rat(row, 'gcc-O2-trapv', 'gcc-O2')} | {rat(row, 'rustc-O-checked', 'rustc-O')} | "
             f"{rat(row, 'pelago', 'gcc-O2-trapv')} | {rat(row, 'pelago', 'rustc-O-checked')} |")
    for title, key in [("Compile time, median seconds", "compile"), ("Binary size, KiB", "size"), ("Peak memory, MiB", "rss_kib")]:
        emit(f"\n{title}\n")
        emit("| program | " + " | ".join(labels) + " |")
        emit("|---|" + "---:|" * len(labels))
        for name, row in results.items():
            cells = []
            for l in labels:
                v = row.get(l, {}).get(key)
                cells.append("-" if v is None else f"{v / 1024:.0f}" if key == "size" else f"{v / 1024:.1f}" if key == "rss_kib" else f"{v:.3f}")
            emit(f"| {name} | " + " | ".join(cells) + " |")
    return "\n".join(lines), gm, below, len(ratios)


def site_json(results, env, a):
    """The medium results in the format of website/content/bench.json."""
    rows, rc, rr = [], [], []
    for name, row in results.items():
        p, c2, rs = (row.get(k, {}).get("median") for k in ("pelago", "gcc-O2", "rustc-O"))
        if not (p and c2):
            continue
        d = {"name": name, "pelago": round(p, 4), "c_o2": round(c2, 4), "ratio_o2": round(p / c2, 2)}
        rc.append(p / c2)
        if rs:
            d.update({"rust": round(rs, 4), "ratio_rust": round(p / rs, 2)})
            rr.append(p / rs)
        rows.append(d)
    return {"date": env["date"], "runs": a.runs, "machine": f"{env.get('cpus')}-core {env.get('machine')} {env.get('os')}",
            "gcc": (env.get("gcc") or "").split()[-1] if env.get("gcc") else None,
            "rustc": (env.get("rustc") or "").split()[1] if env.get("rustc") else None,
            "geomean_c": round(geomean(rc), 2) if rc else None, "geomean_rust": round(geomean(rr), 2) if rr else None,
            "at_or_below_c": sum(1 for r in rc if r <= 1.0), "rows": rows}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--runs", type=int, default=7)
    ap.add_argument("--warmups", type=int, default=2)
    ap.add_argument("--compile-runs", type=int, default=3)
    ap.add_argument("--timeout", type=float, default=120)
    ap.add_argument("--pin", type=int, default=None, help="run on this CPU with taskset (Linux)")
    ap.add_argument("--size", default="medium", help="comma list of small, medium, large (default medium)")
    ap.add_argument("--bless", action="store_true", help="write expected/<name>.<size>.txt from the reference row")
    ap.add_argument("--save", action="store_true", help="write bench/results/<date>-<machine>.json and bench/results.md")
    ap.add_argument("--json", metavar="FILE")
    ap.add_argument("--md", metavar="FILE")
    ap.add_argument("--site", metavar="FILE", help="write the medium results as website/content/bench.json")
    ap.add_argument("--history", metavar="FILE", help="append one JSON line (date, geomean, per-program ratios)")
    a = ap.parse_args()

    sizes = [s.strip() for s in a.size.split(",")]
    names = a.names or sorted(d for d in os.listdir(PROGRAMS) if os.path.exists(os.path.join(PROGRAMS, d, "pelago.pel")))
    repl = {}
    for n in names:
        sp = os.path.join(PROGRAMS, n, "sizes.toml")
        if os.path.exists(sp):
            with open(sp, "rb") as f:
                repl[n] = {sz: v["replace"] for sz, v in tomllib.load(f).items()}
    subprocess.run(["cargo", "build", "--release", "-q"], cwd=REPO, check=True)
    cons = contestants()
    env = environment()
    env["toolchain_drift"] = check_pins(env) or None
    all_results, all_bad, parts, summary = {}, [], [], {}
    with tempfile.TemporaryDirectory() as tmp:
        for size in sizes:
            results, bad = run_size(size, names, cons, repl, a, tmp)
            all_results[size] = results
            all_bad += bad
            # Print each size as soon as it is done, so a long run shows its progress (and a killed run keeps what finished).
            labels = [c[0] for c in cons if any(c[0] in r and "median" in r[c[0]] for r in results.values())]
            text, gm, below, n = report(results, labels, a, size)
            summary[size] = {"geomean": gm, "below": below, "programs": n}
            parts.append(text)
            print(text, file=sys.stderr, flush=True)
    if all_bad:
        parts.append("OUTPUT DIFFERS: " + ", ".join(all_bad))
    text = "\n\n".join(parts)
    print(text)
    raw = {"environment": env, "runs": a.runs, "warmups": a.warmups, "results": all_results}
    if a.save:
        os.makedirs(os.path.join(ROOT, "results"), exist_ok=True)
        machine = (env.get("machine") or "machine").replace(" ", "-")
        a.json = a.json or os.path.join(ROOT, "results", f"{env['date']}-{machine}-{env.get('os', '').split('-')[0].lower()}.json")
        a.md = a.md or os.path.join(ROOT, "results.md")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(raw, f, indent=1)
    if a.md:
        head = ["# Benchmark results", "", "Generated by `bench/harness/run.py`. Do not edit by hand.", "", "## Environment", "",
                "| | |", "|---|---|"] + [f"| {k} | {v} |" for k, v in env.items() if v] + ["", "## Results", ""]
        with open(a.md, "w") as f:
            f.write("\n".join(head) + "\n" + text + "\n")
    if a.site and "medium" in all_results:
        with open(a.site, "w") as f:
            json.dump(site_json(all_results["medium"], env, a), f, indent=1)
            f.write("\n")
    if a.history:
        with open(a.history, "a") as f:
            f.write(json.dumps({"date": env["date"], "pelago": env.get("pelago"), "machine": env.get("machine"), "summary": summary}) + "\n")
    sys.exit(1 if all_bad else 0)


if __name__ == "__main__":
    main()
