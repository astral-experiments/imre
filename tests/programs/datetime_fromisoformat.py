from datetime import datetime

module_parse = datetime.fromisoformat

def parse(text: str) -> datetime:
    return datetime.fromisoformat(text)

def saved_day(text: str) -> int:
    read = datetime.fromisoformat
    return read(text).day

for text in ["0001-01-01", "2000-02-29T12:34:56.123456+03:00",
             "2020-W53", "2020W537", "2020-W53-7🌙24:00Z", "20250101T235959-00:00:30.000005"]:
    stamp = parse(text)
    print(stamp.isoformat())
    print((stamp.year, stamp.month, stamp.day, stamp.hour, stamp.minute,
           stamp.second, stamp.microsecond, stamp.fold))

for text in ["bad'\t", "1900-02-29", "9999-12-31T24:00", "2025-01-01T24:01",
             "2024-01-01T24:01+25:00"]:
    try:
        print(parse(text))
    except ValueError as error:
        print(str(error))

print(saved_day("2020-W53-7"))
print(module_parse("2000-02-29T19:58+04:15").isoformat())
try:
    saved_day("bad")
except ValueError as error:
    print(str(error))
