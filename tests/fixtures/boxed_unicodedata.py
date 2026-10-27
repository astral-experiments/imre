import unicodedata
from unicodedata import normalize


class Text(str):
    pass


def current():
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
