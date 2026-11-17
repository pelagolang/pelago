"""Prints what compiler/std/tests/zone.pel must print for its generated checks (Python's zoneinfo as the reference).
Run: python3 compiler/std/tests/zone_ref.py > compiler/std/tests/zone.expected   (zone.out is that plus fixed cases)"""
from datetime import datetime, timedelta, timezone
from zoneinfo import ZoneInfo
import sys

# `bundled` prints the smaller sample that zone_bundled.pel checks (it runs on every CI machine, so it is kept short)
SMALL = len(sys.argv) > 1 and sys.argv[1] == "bundled"
N_RANDOM, N_FAR = (30, 40) if SMALL else (250, 600)

ZONES = ["Europe/Paris", "America/New_York", "Australia/Sydney", "Asia/Kolkata", "Asia/Kathmandu",
         "Asia/Tokyo", "Australia/Lord_Howe", "Europe/London", "Pacific/Auckland", "America/Sao_Paulo"]
EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)

def at(name, t):
    z = ZoneInfo(name)
    d = (EPOCH + timedelta(seconds=t)).astimezone(z)
    off = int(d.utcoffset().total_seconds())
    return off, d.tzname(), 1 if d.dst() else 0

for name in ZONES:
    for i in range(N_RANDOM):
        t = (i * 2654435761) % 6000000000 - 1500000000
        off, ab, dst = at(name, t)
        print(name, t, off, ab, dst)
    for k in range(N_FAR):
        t = 1798761600 + k * 40000000 // N_FAR       # 2027 .. 2028 .. well past the file's explicit transitions
        off, ab, dst = at(name, t)
        print(name, t, off, ab, dst)

# 2026-03-08, 03-29, 04-05, 09-27, 10-04, 10-25, 11-01 (UTC midnight) minus one day: each window is two days
WINDOWS = [(datetime(2026, m, d) - datetime(1970, 1, 1)).days * 86400 - 86400 for m, d in
           [(3, 8), (3, 29), (4, 5), (9, 27), (10, 4), (10, 25), (11, 1)]]

def local(name, L):
    z = ZoneInfo(name)
    base = datetime(1970, 1, 1) + timedelta(seconds=L)
    d0 = base.replace(tzinfo=z, fold=0)
    d1 = base.replace(tzinfo=z, fold=1)
    t0 = int(d0.timestamp())
    t1 = int(d1.timestamp())
    if d0.utcoffset() == d1.utcoffset():
        return "one", t0, t0
    back = datetime.fromtimestamp(t0, timezone.utc).astimezone(z).replace(tzinfo=None)
    if back == base:
        return "twice", min(t0, t1), max(t0, t1)
    return "never", t0, t1

for name in (["Europe/Paris", "Australia/Lord_Howe"] if SMALL else ["Europe/Paris", "America/New_York", "Australia/Sydney", "Australia/Lord_Howe", "America/Sao_Paulo"]):
    ones = 0
    for w in (WINDOWS[1:2] + WINDOWS[5:6] if SMALL else WINDOWS):
        for k in range(192):
            L = w + k * 900
            kind, a, b = local(name, L)
            if kind == "one":
                ones += 1
            else:
                print(name, L, kind, a, b)
    print(name, "ones", ones)
