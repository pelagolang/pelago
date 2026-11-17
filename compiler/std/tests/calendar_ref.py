"""Prints what compiler/std/tests/calendar.pel must print for its generated dates (checked against Python's datetime).
Run: python3 compiler/std/tests/calendar_ref.py > compiler/std/tests/calendar_dates.expected"""
from datetime import date, timedelta
import calendar as cal

EPOCH = date(1970, 1, 1)

def plus_months(d, n):
    t = d.year * 12 + d.month - 1 + n
    y, m = divmod(t, 12)
    m += 1
    return date(y, m, min(d.day, cal.monthrange(y, m)[1]))

for i in range(300):
    n = (i * 104729) % 3500000 - 700000
    d = EPOCH + timedelta(days=n)
    iy, iw, _ = d.isocalendar()
    p = plus_months(d, 13) if d.year < 9000 else d
    print(n, d.isoformat(), d.isoweekday(), d.timetuple().tm_yday, iy, iw, p.isoformat(), (d + timedelta(days=1000)).isoformat() if d.year < 9000 else d.isoformat())
