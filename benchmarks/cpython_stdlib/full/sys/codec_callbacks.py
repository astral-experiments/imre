"""Compare codec entry calls and error callbacks in fresh native and CPython processes."""
import codecs
import io


def encoding() -> str:
    print("encoding")
    return "cp1251"


def errors() -> str:
    print("errors")
    return "replace"


def stream(s: io.BytesIO) -> io.BytesIO:
    print("stream")
    return s


def main() -> None:
    target = io.BytesIO()
    writer = codecs.lookup(encoding()).streamwriter(errors=errors(), stream=stream(target))
    writer.write("А☃")
    print(repr(target.getvalue()))
    for name in ("ascii", "cp1251", "utf8"):
        entry = codecs.lookup(name)
        for mode in ("namereplace", "xmlcharrefreplace", "missing-handler"):
            print(entry.decode(b"A", mode)[0])
            try:
                entry.decode(b"\x98", mode)
            except TypeError as error:
                print("TypeError: " + str(error))
            except LookupError as error:
                print("LookupError: " + str(error))


if __name__ == "__main__":
    main()
