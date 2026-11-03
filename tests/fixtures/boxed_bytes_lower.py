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


if __name__ == "__main__":
    current()
