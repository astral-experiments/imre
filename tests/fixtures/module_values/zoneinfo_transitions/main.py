"""Compare DST transitions using the bundled IANA 2026b TZif files."""

from datetime import UTC, datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError, reset_tzpath


def transition(zone, instant, wall_time):
    for seconds in (-1, 0, 1):
        utc = instant + timedelta(seconds=seconds)
        local = utc.astimezone(zone)
        print(utc.isoformat(), local.isoformat(), local.fold, local.astimezone(UTC) == utc)

    for fold in (0, 1):
        local = wall_time.replace(tzinfo=zone, fold=fold)
        utc = local.astimezone(UTC)
        back = utc.astimezone(zone)
        print(local.isoformat(), fold, utc.isoformat(), back.isoformat(), back.fold)

    start = wall_time.replace(hour=0, minute=0, tzinfo=zone)
    end = start + timedelta(days=1)
    print(end - start, end.astimezone(UTC) - start.astimezone(UTC))


def check_zone(key, spring, gap, autumn, overlap):
    zone = ZoneInfo(key)
    print(key, ZoneInfo(key) is zone)
    uncached = ZoneInfo.no_cache(key)
    print(uncached is zone, ZoneInfo.no_cache(key) is uncached, ZoneInfo(key) is zone)
    transition(zone, spring, gap)
    transition(zone, autumn, overlap)


reset_tzpath([str(Path(__file__).with_name("tzif"))])
check_zone(
    "America/New_York",
    datetime(2020, 3, 8, 7, tzinfo=UTC), datetime(2020, 3, 8, 2, 30),
    datetime(2020, 11, 1, 6, tzinfo=UTC), datetime(2020, 11, 1, 1, 30),
)
check_zone(
    "Australia/Lord_Howe",
    datetime(2020, 10, 3, 15, 30, tzinfo=UTC), datetime(2020, 10, 4, 2, 15),
    datetime(2020, 4, 4, 15, tzinfo=UTC), datetime(2020, 4, 5, 1, 45),
)

for key in ("/absolute", "America/../America/New_York", "Etc/No_Such_Zone"):
    try:
        ZoneInfo(key)
    except (ValueError, ZoneInfoNotFoundError) as error:
        print(key, type(error).__name__)
