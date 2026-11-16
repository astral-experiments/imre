import time
from time import strptime

parsed = strptime("2024-02-29 +05:30", "%Y-%m-%d %z")
assert isinstance(parsed, time.struct_time)
assert tuple(parsed) == (2024, 2, 29, 0, 0, 0, 3, 60, -1)
assert parsed.tm_gmtoff == 19800
assert getattr(time, "strptime")("2000-02-29", "%Y-%m-%d").tm_yday == 60
assert time.strptime("UTC", "%Z").tm_zone == "UTC"

try:
    strptime("2023-02-29", "%Y-%m-%d")
except ValueError:
    pass
else:
    raise AssertionError("invalid date accepted")
