def current():
    class Two:
        def __index__(self):
            return 2

    class ByteString(bytes):
        pass

    class Mutable(bytearray):
        pass

    class OneChar(str):
        def __len__(self):
            return 1

    class OneByte(bytes):
        def __len__(self):
            return 1

    for factory in [bytes, bytearray, ByteString, Mutable]:
        data = factory(b"\x01\x02\x03\xfe\xff")
        assert data.hex() == "010203feff"
        assert data.hex(b":", Two()) == "01:0203:feff"
        assert factory.hex(data, sep=":", bytes_per_sep=-2) == "0102:03fe:ff"
        assert data.hex("\x00", 1) == "01\x0002\x0003\x00fe\x00ff"
        assert data.hex(b":", 0) == data.hex(bytes_per_sep=2) == data.hex()
        assert factory(b"").hex("-") == ""
        assert data.hex(OneChar(""), -3) == "010203\x00feff"
        assert data.hex(OneByte(b""), 3) == "0102\x0003feff"
        assert data.hex(OneChar("aé"), -3) == "010203afeff"
        for sep, error in [(OneChar("aĀ"), ValueError), (None, TypeError), ([1], TypeError), ("", ValueError),
                           (b"\x80", ValueError), ("é", ValueError)]:
            try:
                data.hex(sep)
            except error:
                pass
            else:
                raise AssertionError("invalid separator accepted")
        assert data.hex(":", -2**31) == data.hex(":", 2**31 - 1) == data.hex()
        try:
            data.hex(":", 2**40)
        except OverflowError:
            pass
        else:
            raise AssertionError("out-of-range group accepted")
        try:
            factory.hex(123)
        except TypeError:
            pass
        else:
            raise AssertionError("invalid descriptor receiver accepted")


if __name__ == "__main__":
    current()
