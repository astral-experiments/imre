"""Check strict registry errors, including BOM-relative offsets."""
import codecs


def encode_error(name: str, value: str) -> None:
    try:
        codecs.lookup(name).encode(value)
    except UnicodeEncodeError as error:
        print(repr(error.args))


def decode_error(name: str, value: bytes) -> None:
    try:
        codecs.lookup(name).decode(value)
    except UnicodeDecodeError as error:
        print(repr(error.args))


encode_error("ascii", "xé€")
encode_error("latin-1", "xé€")
decode_error("ascii", b"a\xff")
decode_error("utf-8", b"a\xff")
decode_error("utf-8-sig", b"a\xff")
decode_error("utf-8-sig", b"\xef\xbb\xbfa\xff")
