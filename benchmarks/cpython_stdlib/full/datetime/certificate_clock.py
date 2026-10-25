from datetime import datetime as dt
from datetime import timedelta
from datetime import timezone
from datetime import UTC

# The two certificate validity operations in Werkzeug serving.py.
before = dt.now(timezone.utc)
after = before + timedelta(days=365)
print(before.tzinfo is UTC)
print(after.tzinfo is timezone.utc)
print(after > before)
print(timedelta(days=365).total_seconds())
print(timezone.utc.utcoffset(None).days)
print(timezone.utc.tzname(None))
base = dt(2024, 2, 29, 23, 59, 59, 999999, tzinfo=UTC)
print(base.isoformat())
print((base + timedelta(microseconds=1)).isoformat())
print((base + timedelta(days=365)).isoformat())
east = timezone(timedelta(hours=5, minutes=30), "IST")
same = dt(2024, 3, 1, 5, 29, 59, 999999, tzinfo=east)
print(same == base)
print(same.isoformat())
print(timedelta(microseconds=-1).days)
print(timedelta(microseconds=-1).seconds)
print(timedelta(microseconds=-1).microseconds)
