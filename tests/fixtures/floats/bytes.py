def parse(value: bytes) -> float:
    return float(value)


values = [
    b"",
    b" \t+1_2.5e-1\r\n",
    b"-0.0",
    b".5",
    b"1.",
    b"Infinity",
    b"-INF",
    b"+NaN",
    b"1e9999",
    b"-1e-9999",
    b"1__2",
    b"0x1p0",
    b"1\x00",
    b"\xff",
    b"\xa01.5",
    b"\xc2\xa01.5",
    b"\xd9\xa1.5",
]
for value in values:
    try:
        print(parse(value))
    except ValueError:
        print("invalid")

for byte in range(256):
    try:
        print(parse(bytes([byte])))
    except ValueError:
        print("invalid")

mapping: dict[bytes, float | None] = {b"zero": -0.0, b"missing": None}
for key, item in mapping.items():
    print(key)
    if item is not None:
        print(item)
