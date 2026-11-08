import _codecs
import codecs


def current():
    assert codecs.utf_16_decode is _codecs.utf_16_decode
    assert codecs.utf_16_decode(b"\xfe\xff\x00A\xd8\x00") == ("A", 4)
    assert codecs.utf_16_decode(b"\xff\xfeA\x00!", "replace", True) == ("A\ufffd", 5)
    for encoding, bom, data in (
        ("utf-16-le", b"\xff\xfe", b"A\x00=\xd8\r\xdc"),
        ("utf-16-be", b"\xfe\xff", b"\x00A\xd8=\xdc\r"),
        ("utf-32-le", b"\xff\xfe\x00\x00", b"A\x00\x00\x00\r\xf4\x01\x00"),
        ("utf-32-be", b"\x00\x00\xfe\xff", b"\x00\x00\x00A\x00\x01\xf4\r"),
    ):
        assert (bom + data).decode(encoding) == "\ufeffA🐍"
        assert (bom + data).decode(encoding[:6]) == "A🐍"
        assert data.decode(encoding) == "A🐍"
    assert b"\xd8=".decode("utf-16-be", "surrogatepass") == chr(0xd83d)
    assert b"\x0d\xdc\x00\x00".decode("utf-32-le", "surrogatepass") == chr(0xdc0d)
    malformed = b"\xfe\xff\xd8\x00!"
    try:
        malformed.decode("utf-16")
    except UnicodeDecodeError as error:
        assert (error.encoding, error.object, error.start, error.end, error.reason) == (
            "utf-16-be", malformed, 2, 5, "unexpected end of data",
        )
    else:
        raise AssertionError("accepted incomplete surrogate pair")
    seen = []

    def repair(error):
        seen.append((error.encoding, error.start, error.end, error.reason))
        error.object = b"_\x00B\x00"
        return chr(0xd800), -2

    codecs.register_error("wide_resume", repair)
    assert b"A\x00\x00\xdc".decode("utf-16-le", "wide_resume") == "A" + chr(0xd800) + "B"
    assert seen == [("utf-16-le", 2, 4, "illegal encoding")]
