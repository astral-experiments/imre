import sys
import io
import itertools
import struct
import unicodedata
from unicodedata import normalize


class Text(str):
    pass


def current():
    assert unicodedata.unidata_version == "16.0.0"
    for ch in (Text("x"), "界", "！", "ｶ", "Ω", "\x00", "\ud800", "\udfff",
               "\U0001fa8a", "\U000323b0", "\U0002fffe"):
        print(unicodedata.east_asian_width(ch))
    for value in (itertools.count(), itertools.chain(), io.BytesIO(), struct.Struct("I")):
        try:
            unicodedata.east_asian_width(value)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("non-character accepted")
    assert sys.maxunicode == 0x10FFFF
    for value in (Text("\u01FA"), "\u2100", "\uAC00", "\uAC01", "\ud800", chr(sys.maxunicode)):
        print(unicodedata.decomposition(value))
    for value in (None, b"x", "", "ab"):
        try:
            unicodedata.decomposition(value)
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("invalid decomposition call succeeded")
    assert normalize is unicodedata.normalize
    for form in ("NFC", "NFD", "NFKC", "NFKD"):
        for value in ("", "plain", "é", "e\u0301", "①ﬂÅ", "각", "x\u0301", "a\u0315\u1acf\u0300", "\ua7f1"):
            result = normalize(form, value)
            print(form, result, result is value)
    for value in (Text(""), Text("plain"), Text("é")):
        result = normalize(Text("NFC"), value)
        print(type(result).__name__, result, result is value)
    print(normalize("invalid", ""))
    for call in (
        lambda: normalize(), lambda: normalize("NFC"), lambda: normalize("NFC", "a", 0),
        lambda: normalize(form="NFC", unistr="abc"), lambda: normalize("NFC", 1, x=0),
        lambda: normalize(None, ""), lambda: normalize(b"NFD", None),
        lambda: normalize("bad", None), lambda: normalize("NFC", b"text"),
        lambda: normalize("bad", "text"),
    ):
        try:
            call()
        except (TypeError, ValueError) as error:
            print(type(error).__name__, str(error))
        else:
            raise AssertionError("invalid normalization call succeeded")
