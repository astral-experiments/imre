import _imp
import codecs
import os


class Text(str):
    pass


class EchoSpec:
    def __format__(self, spec):
        return spec


def affixes():
    word = Text("é\udfff🙂")
    assert word.startswith("é") and word.endswith("🙂")
    assert not word.startswith("é🙂") and not word.endswith("é🙂")
    assert word.startswith(Text("\udfff🙂"), 1)
    assert word.endswith("é\udfff", 0, -1)
    assert not word.startswith("\udffe", 1) and not word.endswith("\udffe🙂")
    assert not "é🙂".startswith("\udfff") and not "é🙂".endswith("\udfff")
    assert word.startswith(("", 0), 3) and not word.startswith("", 4)
    assert word.endswith(("\udfff", "🙂", 0))
    try:
        word.startswith(("\udffe", 0, "é"))
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a non-string tuple member")

    seen = []

    class Bound:
        def __init__(self, value):
            self.value = value

        def __index__(self):
            seen.append(self.value)
            if self.value is None:
                raise RuntimeError("bound sentinel")
            return self.value

    large = 10 ** 100
    assert word.startswith("é", Bound(-large), Bound(large))
    assert seen == [-large, large]
    assert not word.endswith("", large, None)
    seen.clear()
    try:
        word.endswith(0, Bound(0), Bound(None))
    except RuntimeError as error:
        assert str(error) == "bound sentinel" and seen == [0, None]
    else:
        raise AssertionError("affix type checked before end bound")


def current():
    affixes()
    low = "\udcff"
    high = "\U0000d800"
    path = "dir/" + low
    assert path.split("/")[::-1] == [low, "dir"]
    assert (low * 5).rsplit(low * 2, 1) == [low * 3, ""]
    assert (high + "\x1c" + low).split(None, 1) == [high, low]
    assert str.split(Text(path), low) == ["dir/", ""]
    field = Text("field" + low)
    assert field.split("/")[0] is not field.split("/")[0]
    assert field.rsplit("separator longer than field")[0] is field
    replacement = "\ufffd"
    assert type(low) is str and low.__class__ is str
    assert type(os.fsdecode(b"\xff")) is str
    subtype = Text(low)
    assert _imp.is_frozen(low) is False and _imp.is_frozen(subtype) is False
    assert type(subtype) is Text and isinstance(subtype, str)
    scalar = Text("🙂")
    source = Text(low + low + low + scalar)
    assert source.count(Text(low + low)) == 1
    assert source.count(scalar, -2) == 1 and str.count(scalar, subtype) == 0
    assert source.count(replacement) == 0 and str.count(high + "\udc00", "𐀀") == 0
    assert source.count("", 4, 10**50) == 1 and source.count("", 5) == 0
    assert source.count(subtype, -(10**50), 2) == 2
    assert str(subtype) == low and type(str(subtype)) is str
    assert os.fsdecode(subtype) is subtype and os.fsencode(subtype) == b"\xff"
    assert replacement == "�"
    assert [ord(c) for c in "\uecff\udcff\N{REPLACEMENT CHARACTER}"] == [0xecff, 0xdcff, 0xfffd]
    assert low != replacement and low != r"\udcff"
    assert r"\udcff" == "\\udcff"
    assert [ord(char) for char in low + high + replacement] == [0xdcff, 0xd800, 0xfffd]
    assert "é" + low + "中" == "é\udcff中"
    text = "é" + high + low
    assert text[1] == chr(0xd800)
    assert text[::-1] == low + high + "é"
    assert list(reversed(text)) == [low, high, "é"]
    assert sorted([replacement, low, high, "🙂"]) == [high, low, replacement, "🙂"]
    values = {low: "path", replacement: "decoded"}
    assert values[chr(0xdcff)] == "path" and high not in values
    assert ascii("é'\udcff") == r'''"\xe9'\udcff"'''
    assert repr("\ud800" "\udfff") == r"'\ud800\udfff'"
    assert len("\ud800" "\udfff") == 2
    assert r"\udcff" "\udcff" == "\\udcff" + low
    assert f"{os.devnull}\udfff" == os.devnull + "\udfff"
    assert f"{{\udcff}}{3}{{\ufffd\ud800}}" == "{" + low + "}3{" + replacement + high + "}"
    assert f"{EchoSpec():\udcff}" == low
    assert f"{low!r}" == repr(low) and f"{text!a}" == ascii(text)
    assert rf"\uDCFF{'\ud800'}" == r"\uDCFF" + high

    for raw in (b"ascii", b"\x80", b"\xff\xc3\xa9\xfe", b"\xe4\xb8\xad\xe2\x82", b"\xed\xa0\x80"):
        escaped = raw.decode("utf-8", "surrogateescape")
        assert os.fsdecode(raw) == escaped
        assert os.fsdecode(escaped) is escaped
        assert os.fsencode(escaped) == raw
        assert escaped.encode("utf-8", "surrogateescape") == raw
        assert codecs.encode(obj=escaped, encoding="UTF8", errors="surrogateescape") == raw
        assert codecs.decode(raw, "UTF8", "surrogateescape") == escaped
        assert bytes(escaped, "utf-8", "surrogateescape") == raw
        assert bytearray(escaped, "utf-8", "surrogateescape") == bytearray(raw)
        assert codecs.lookup("utf-8").decode(raw, "surrogateescape") == (escaped, len(raw))

    assert b"\xff".decode("utf-8", "surrogateescape") == low
    assert b"\xff".decode("utf-8", "replace") == replacement
    assert replacement.encode() == b"\xef\xbf\xbd"
    for text, errors in (("é" + low + high + "a", "strict"),
                         ("é" + high, "surrogateescape"), ("\udc7f", "surrogateescape")):
        try:
            text.encode("utf-8", errors)
        except UnicodeEncodeError as exc:
            assert exc.object is text
            print(exc.encoding, exc.start, exc.end, exc.reason)
        else:
            raise AssertionError("unencodable surrogate accepted")

    raw = b"\xed\xa0\x80x\xed\xb3\xbf"
    assert codecs.encode(high + "x" + low, "utf-8", "surrogatepass") == raw
    assert codecs.decode(raw, "utf-8", "surrogatepass") == high + "x" + low
    for invalid in (b"\xed\xa0", b"\xed\xa0X", raw + b"\xff"):
        try:
            codecs.decode(invalid, "utf-8", "surrogatepass")
        except UnicodeDecodeError as exc:
            assert exc.object == invalid
            print(exc.start, exc.end, exc.reason)
        else:
            raise AssertionError("malformed input accepted")

    class Mapped(str):
        def title(self):
            return "override"

        def swapcase(self):
            return "override"

    word = Mapped("AΣ\ud800ß")
    assert word.title() == word.swapcase() == "override"
    title = str.title.__get__(word)
    swapcase = str.swapcase.__get__(word)
    assert title() == "Aς\ud800Ss" and type(title()) is str
    assert swapcase() == "aς\ud800SS" and type(swapcase()) is str
    assert "A\ud800Σ".title() == "A\ud800Σ"
    assert "A\ud800Σ".swapcase() == "a\ud800σ"
    signed = Mapped("!")
    assert str.title(signed) is not str.title(signed)
    assert str.swapcase(signed) is not str.swapcase(signed)
    assert str.title.__objclass__ is str
    assert str.swapcase.__text_signature__ == "($self, /)"
    for method in (title, swapcase):
        for args, kw in (((1,), {}), ((), {"unknown": True})):
            try:
                method(*args, **kw)
            except TypeError as error:
                assert "takes no " in str(error)
            else:
                raise AssertionError("unexpected accepted case-map arguments")

    class Alias:
        f = str.title

    try:
        Alias().f(1)
    except TypeError as error:
        assert str(error) == "descriptor 'title' for 'str' objects doesn't apply to a 'Alias' object"
    else:
        raise AssertionError("accepted a non-string descriptor receiver")
