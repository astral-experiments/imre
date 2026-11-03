from datetime import datetime, timedelta, timezone
from zoneinfo import ZoneInfo

key = "America/New_York"
zone = ZoneInfo(key)
assert ZoneInfo(key) is zone
assert ZoneInfo.no_cache(key) is not zone

# Both UTC instants map to 01:30 as the offset moves from EDT to EST.
for utc_hour, fold, offset in [(5, 0, -4), (6, 1, -5)]:
    instant = datetime(2020, 11, 1, utc_hour, 30, tzinfo=timezone.utc)
    local = instant.astimezone(zone)
    assert (local.hour, local.minute, local.fold) == (1, 30, fold)
    assert local.utcoffset() == timedelta(hours=offset)
    assert local.astimezone(timezone.utc) == instant

ZoneInfo.clear_cache(only_keys=["UTC"])
assert ZoneInfo(key) is zone
ZoneInfo.clear_cache(only_keys=[key])
assert ZoneInfo(key) is not zone
print("New York fold and cache passed")
