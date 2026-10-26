"""Exercise the native codecs used by filesystem encodings and JSON byte-order marks."""
import codecs
import sys
from codecs import BOM_UTF8, lookup


def encoding_name() -> str:
    print("receiver")
    return sys.getfilesystemencoding()


def text_input() -> str:
    print("input")
    return "café"


def error_policy() -> str:
    print("errors")
    return "replace"


def main() -> None:
    entry = lookup(sys.getfilesystemencoding())
    print(entry.name)
    print(len(entry))
    print(bool(entry))
    print(isinstance(entry, tuple))
    print(isinstance(entry, codecs.CodecInfo))
    print(entry is lookup("UTF--8"))
    print(entry is lookup("u8"))
    print(lookup(" ☃utf8 ") is lookup("utf8"))
    print(lookup("utf@8").name)
    print(lookup("ansi_x3.4-1968").name)
    print(lookup("csisolatin1").name)
    data, consumed = lookup(encoding_name()).encode(text_input(), error_policy())
    print(repr(data))
    print(consumed)
    decoded, consumed = entry.decode(data)
    print(repr(decoded))
    print(consumed)
    latin = lookup("latin-1")
    data, consumed = latin.encode("Aé日", "backslashreplace")
    print(repr(data))
    print(consumed)
    decoded, consumed = latin.decode(b"\xff\x80")
    print(repr(decoded))
    print(consumed)
    sig = lookup("utf-8-sig")
    data, consumed = sig.encode("")
    print(data == BOM_UTF8)
    print(consumed)
    decoded, consumed = sig.decode(BOM_UTF8 + b"A")
    print(decoded)
    print(consumed)
    ascii_codec = lookup("646")
    data, consumed = ascii_codec.encode("日é", "replace")
    print(repr(data))
    print(consumed)
    decoded, consumed = ascii_codec.decode(b"A\xff", "backslashreplace")
    print(repr(decoded))
    print(consumed)
    try:
        ascii_codec.encode("é")
    except UnicodeEncodeError as error:
        print(str(error))
    try:
        entry.decode(b"x\xff")
    except UnicodeDecodeError as error:
        print(str(error))
    try:
        lookup("an unknown codec")
    except LookupError as error:
        print(str(error))
    try:
        lookup("locale")
    except LookupError as error:
        print(str(error))
    try:
        lookup("utf8\x00")
    except ValueError as error:
        print(str(error))
    try:
        lookup()  # ty: ignore[missing-argument]
    except TypeError as error:
        print(str(error))
    try:
        lookup(12)  # ty: ignore[invalid-argument-type]
    except TypeError as error:
        print(str(error))
    print(repr(BOM_UTF8))
    print(repr(codecs.BOM_UTF16_BE))
    print(repr(codecs.BOM_UTF16_LE))
    print(repr(codecs.BOM_UTF32_BE))
    print(repr(codecs.BOM_UTF32_LE))


main()
