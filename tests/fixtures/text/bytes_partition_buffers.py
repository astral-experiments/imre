def without_zone(address: bytes) -> bytes:
    host, _, _ = address.partition(b"%")
    return host


print(without_zone(b"fe80::1%eth0"), without_zone(b"::1"))
source = b"ababa"
for separator in (b"ba", bytearray(b"ba"), memoryview(b"ba"), b"absent"):
    before, middle, after = bytes.partition(source, separator)
    print(before, bytes(middle), after, before is source, middle is separator)

for invalid in (b"", "a", memoryview(b"abc")[::2]):
    try:
        print(source.partition(invalid))
    except (TypeError, ValueError, BufferError) as error:
        print(type(error).__name__, error.args)
