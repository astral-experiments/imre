def current():
    class Packet(bytes):
        def lower(self):
            return b"override"

    data = b"\x00AZ@[az\xc3\xa9\xff\n"
    expected = b"\x00az@[az\xc3\xa9\xff\n"
    assert data.lower() == expected
    assert Packet(data).lower() == b"override"
    converted = bytes.lower(Packet(data))
    assert type(converted) is bytes
    assert converted == expected

    upper = b"\x00AZ@[AZ\xc3\xa9\xff\n"
    converted = bytes.upper(Packet(data))
    assert type(converted) is bytes and converted == upper
    for original, expected in ((data, upper), (b"", b"")):
        source = bytearray(original)
        converted = source.upper()
        assert type(converted) is bytearray and converted == expected
        converted.append(0)
        assert source == original


if __name__ == "__main__":
    current()
