#!/usr/bin/env python3
"""Builds compiler/std/tzdata.pel, the tz database bundled with `std.zone` for systems that have none (Windows).

Usage: python3 compiler/std/tools/gen_tzdata.py [/usr/share/zoneinfo] > compiler/std/tzdata.pel

Each distinct zone is stored once in a compact form (types, delta-coded transitions, the POSIX footer rule) and
every name (aliases included) points at it. Transitions after the last one the footer rule does not already predict
are dropped, so a modern zone is a few dozen bytes; the generator checks the rule against every dropped transition.
Format of one zone (all integers are LEB128 varints, signed ones zigzag):
  ntypes, then per type: utoff (signed), isdst (0/1), abbreviation length + ASCII;
  ntrans, then per transition: time delta from the previous one (signed), type index;
  rule length + ASCII (the POSIX TZ string, empty when the zone has none).
"""
import base64, hashlib, os, struct, sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/zoneinfo"


def read_tzif(b):
    assert b[:4] == b"TZif"
    ver = b[4]
    counts = struct.unpack(">6l", b[20:44])
    at = 0
    wide = False
    if ver >= 0x32:
        isut, isstd, leap, timecnt, typecnt, charcnt = counts
        at = 44 + timecnt * 5 + typecnt * 6 + charcnt + leap * 8 + isstd + isut
        counts = struct.unpack(">6l", b[at + 20:at + 44])
        wide = True
    isut, isstd, leap, timecnt, typecnt, charcnt = counts
    p = at + 44
    ts = 8 if wide else 4
    trans = list(struct.unpack(">%d%s" % (timecnt, "q" if wide else "l"), b[p:p + timecnt * ts]))
    p += timecnt * ts
    kinds = list(b[p:p + timecnt])
    p += timecnt
    types = []
    for i in range(typecnt):
        off, dst, ai = struct.unpack(">lBB", b[p + i * 6:p + i * 6 + 6])
        types.append((off, dst, ai))
    p += typecnt * 6
    chars = b[p:p + charcnt]
    p += charcnt + leap * (ts + 4) + isstd + isut
    out = []
    for off, dst, ai in types:
        end = chars.index(b"\0", ai)
        out.append((off, dst, chars[ai:end].decode()))
    foot = ""
    if wide and b[p:p + 1] == b"\n":
        foot = b[p + 1:b.index(b"\n", p + 1)].decode()
    return trans, kinds, out, foot


# ---- POSIX rule evaluation (mirrors compiler/std/zone.pel `ruleAt`, so a dropped transition is provably redundant)
def days_from_civil(y, m, d):
    y -= m <= 2
    era = y // 400
    yoe = y - era * 400
    doy = (153 * (m + (-3 if m > 2 else 9)) + 2) // 5 + d - 1
    doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return era * 146097 + doe - 719468


def civil_year(days):
    z = days + 719468
    era = z // 146097
    doe = z - era * 146097
    yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
    doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
    mp = (5 * doy + 2) // 153
    m = mp + 3 if mp < 10 else mp - 9
    return yoe + era * 400 + (1 if m <= 2 else 0)


def leap(y):
    return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)


def dim(y, m):
    return 29 if m == 2 and leap(y) else [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m - 1]


class Rule:
    def __init__(self, text):
        self.s = text
        self.i = 0
        self.std = self.name()
        self.stdoff = -self.clock()
        self.dst = None
        if self.i < len(text):
            self.dstname = self.name()
            self.dstoff = self.stdoff + 3600
            if text[self.i] != ",":
                self.dstoff = -self.clock()
            assert text[self.i] == ","
            self.i += 1
            self.a = self.edge()
            assert text[self.i] == ","
            self.i += 1
            self.b = self.edge()
            self.dst = True
        assert self.i == len(text), text

    def name(self):
        s = self.s
        if s[self.i] == "<":
            j = s.index(">", self.i)
            r = s[self.i + 1:j]
            self.i = j + 1
            return r
        j = self.i
        while j < len(s) and s[j].isalpha():
            j += 1
        r = s[self.i:j]
        self.i = j
        return r

    def num(self):
        j = self.i
        while j < len(self.s) and self.s[j].isdigit():
            j += 1
        r = int(self.s[self.i:j])
        self.i = j
        return r

    def clock(self):
        sign = 1
        if self.s[self.i] in "+-":
            sign = -1 if self.s[self.i] == "-" else 1
            self.i += 1
        v = self.num() * 3600
        if self.i < len(self.s) and self.s[self.i] == ":":
            self.i += 1
            v += self.num() * 60
            if self.i < len(self.s) and self.s[self.i] == ":":
                self.i += 1
                v += self.num()
        return sign * v

    def edge(self):
        s = self.s
        if s[self.i] == "M":
            self.i += 1
            m = self.num(); self.i += 1
            w = self.num(); self.i += 1
            d = self.num()
            e = ("M", m, w, d)
        elif s[self.i] == "J":
            self.i += 1
            e = ("J", self.num())
        else:
            e = ("N", self.num())
        at = 7200
        if self.i < len(s) and s[self.i] == "/":
            self.i += 1
            at = self.clock()
        return (e, at)

    def day(self, e, year):
        if e[0] == "M":
            _, m, w, d = e
            first = days_from_civil(year, m, 1)
            dow1 = (first + 4) % 7
            day = 1 + (d - dow1 + 7) % 7 + (w - 1) * 7
            if day > dim(year, m):
                day -= 7
            return first + day - 1
        jan1 = days_from_civil(year, 1, 1)
        if e[0] == "J":
            return jan1 + e[1] - 1 + (1 if leap(year) and e[1] >= 60 else 0)
        return jan1 + e[1]

    def at(self, t):
        if not self.dst:
            return (self.stdoff, 0, self.std)
        year = civil_year((t + self.stdoff) // 86400)
        start = self.day(self.a[0], year) * 86400 + self.a[1] - self.stdoff
        end = self.day(self.b[0], year) * 86400 + self.b[1] - self.dstoff
        d = (start <= t < end) if start < end else not (end <= t < start)
        return (self.dstoff, 1, self.dstname) if d else (self.stdoff, 0, self.std)


def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def zigzag(n):
    return varint((n << 1) ^ (n >> 63) if n >= 0 else ((-n) << 1) - 1)


def compact(b):
    trans, kinds, types, foot = read_tzif(b)
    if foot:
        rule = Rule(foot)
        full = (list(trans), list(kinds))

        def table_at(t):
            """What the original file says at `t` (before the first transition: the first type that is not daylight time)."""
            lo, hi = 0, len(full[0])
            while lo < hi:
                mid = (lo + hi) // 2
                if full[0][mid] <= t:
                    lo = mid + 1
                else:
                    hi = mid
            if lo == 0:
                first = next((i for i, x in enumerate(types) if x[1] == 0), 0)
                return types[first]
            return types[full[1][lo - 1]]

        def good(keep):
            """Whether keeping `keep` transitions and letting the rule take over after the last one changes nothing."""
            if keep >= len(trans):
                return True
            start = trans[keep - 1] if keep > 0 else trans[0] - 400 * 86400
            end = max(trans[-1], 2524608000) + 800 * 86400          # to 2050 and past the last transition
            ts = set(range(start, end, 86400))
            for x in trans[max(keep - 1, 0):]:
                ts.update((x - 1, x, x + 1, x + 3600, x + 7200))
            for t in ts:
                if t >= start and rule.at(t) != table_at(t):
                    return False
            return True

        # the rule predicts the tail: find the smallest suffix it reproduces, then confirm by sampling every day
        k = len(trans)
        while k > 0 and (rule.at(trans[k - 1]) == table_at(trans[k - 1])):
            k -= 1
        keep = min(len(trans), k + 1) if len(trans) else 0
        while not good(keep):
            keep += 1
        trans, kinds = trans[:keep], kinds[:keep]
    out = bytearray()
    used = sorted(set(kinds))
    # keep every type that a kept transition uses, plus type 0 and the first non-dst one (the pre-first-transition type)
    first = next((i for i, t in enumerate(types) if t[1] == 0), 0)
    used = sorted(set(used) | {0, first})
    remap = {old: new for new, old in enumerate(used)}
    # the pre-first-transition rule in the reader is "first type that is not daylight time, else 0": keep order
    out += varint(len(used))
    for old in used:
        off, dst, ab = types[old]
        out += zigzag(off) + bytes([dst]) + varint(len(ab)) + ab.encode()
    out += varint(len(trans))
    prev = 0
    for t, kd in zip(trans, kinds):
        out += zigzag(t - prev) + varint(remap[kd])
        prev = t
    out += varint(len(foot.encode())) + foot.encode()
    return bytes(out)


def main():
    names = {}
    for r, d, f in os.walk(ROOT):
        rel = os.path.relpath(r, ROOT)
        if rel.split(os.sep)[0] in ("posix", "right"):
            continue
        for n in f:
            p = os.path.join(r, n)
            name = n if rel == "." else rel + "/" + n
            if name in ("posixrules", "localtime", "Factory") or "." in n:
                continue
            raw = open(p, "rb").read()
            if raw[:4] != b"TZif":
                continue
            names[name] = compact(raw)
    uniq = {}
    for blob in names.values():
        uniq.setdefault(hashlib.md5(blob).hexdigest(), blob)
    # layout: each unique blob padded to a multiple of 3 bytes so its base64 text stands alone
    order = sorted(uniq)
    pos = {}
    text = []
    cur = 0
    for h in order:
        b = uniq[h]
        pad = (-len(b)) % 3
        enc = base64.b64encode(b + b"\0" * pad).decode()
        pos[h] = (cur, len(b))
        text.append(enc)
        cur += len(enc)
    index = []
    for name in sorted(names):
        h = hashlib.md5(names[name]).hexdigest()
        index.append("%s %d %d" % (name, pos[h][0], pos[h][1]))
    version = open(os.path.join(ROOT, "tzdata.zi")).readline().split()[-1]
    print("/// The IANA time zone database %s, bundled for systems that have none (generated by tools/gen_tzdata.py; do" % version)
    print("/// not edit). `std.zone` reads it when no zone file is found, or on request with `zone.bundled(name)`.")
    print("///")
    print("/// %d names, %d distinct zones, %d bytes of data." % (len(names), len(uniq), cur // 4 * 3))
    print()
    print("//~ `name offset length` lines, sorted: where each zone's data starts in `data()` (base64 characters) and how many bytes it is.")
    print('pub fn index() -> str = "%s"' % "\\n".join(index))
    print()
    print("//~ The data of every distinct zone, base64.")
    print('pub fn data() -> str = "%s"' % "".join(text))


main()
