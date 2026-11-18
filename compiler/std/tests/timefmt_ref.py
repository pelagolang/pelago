"""Prints what compiler/std/tests/timefmt.pel must print for its generated moments (Python's datetime as reference).
Run: python3 compiler/std/tests/timefmt_ref.py"""
from datetime import datetime, timedelta, timezone

OFFSETS = [0, 3600, -18000, 19800, -12600, 20700, 43200, -39600]
DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)

def off_text(o, colon=True):
    a = abs(o)
    return ("-" if o < 0 else "+") + "%02d" % (a // 3600) + (":" if colon else "") + "%02d" % (a % 3600 // 60)

for i in range(300):
    ms = (i * 2654435761) % 5000000000000 - 1500000000000     # 1922 to 2080
    o = OFFSETS[i % len(OFFSETS)]
    utc = EPOCH + timedelta(milliseconds=ms)
    d = utc + timedelta(seconds=o)
    frac = "%03d" % (d.microsecond // 1000)
    frac = frac.rstrip("0")
    base = "%04d-%02d-%02dT%02d:%02d:%02d" % (d.year, d.month, d.day, d.hour, d.minute, d.second)
    rfc = base + ("." + frac if frac else "") + ("Z" if o == 0 else off_text(o))
    r2822 = "%s, %d %s %04d %02d:%02d:%02d %s" % (DAYS[d.weekday()], d.day, MONTHS[d.month - 1], d.year, d.hour, d.minute, d.second, off_text(o, False))
    h = utc
    http = "%s, %02d %s %04d %02d:%02d:%02d GMT" % (DAYS[h.weekday()], h.day, MONTHS[h.month - 1], h.year, h.hour, h.minute, h.second)
    full = d.strftime("%A") + " %d " % d.day + d.strftime("%B") + " %04d, %02d:%02d:%02d.%03d %s %s" % (
        d.year, (d.hour % 12) or 12, d.minute, d.second, d.microsecond // 1000, "AM" if d.hour < 12 else "PM", off_text(o))
    print(ms, rfc, "|", r2822, "|", http, "|", full, "|", "true")
