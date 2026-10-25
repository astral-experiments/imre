"""Compare the complete Windows-1251 byte mapping with the pinned registry."""
from codecs import lookup


def main() -> None:
    entry = lookup("cp1251")
    for alias in ("cp1251", "1251", "windows-1251", "WINDOWS_1251"):
        info = lookup(alias)
        print(info.name)
        print(info is entry)
        print(info is lookup("windows-1251"))

    valid = bytes([value for value in range(256) if value != 0x98])
    decoded, size = entry.decode(valid)
    print(decoded)
    print(size)
    encoded, size = entry.encode(decoded)
    print(encoded == valid)
    print(size)
    encoded, size = entry.encode("до свидания\n")
    print(repr(encoded))
    print(size)

    invalid = b"A\x98\x98\xc0"
    for errors in ("ignore", "replace", "backslashreplace"):
        decoded, size = entry.decode(invalid, errors)
        print(repr(decoded))
        print(size)
    try:
        entry.decode(invalid)
    except UnicodeDecodeError as error:
        print(str(error))
    try:
        entry.decode(invalid, "xmlcharrefreplace")
    except TypeError as error:
        print(str(error))
    for errors in ("ignore", "replace", "backslashreplace", "xmlcharrefreplace"):
        encoded, size = entry.encode("А日☃Я", errors)
        print(repr(encoded))
        print(size)
    for text in ("А¡Я", "А日☃Я"):
        try:
            entry.encode(text)
        except UnicodeEncodeError as error:
            print(str(error))
    try:
        entry.encode("日", "absent-here")
    except LookupError as error:
        print(str(error))
    encoded, size = entry.encode("Я", "absent-here")
    print(repr(encoded))
    print(size)


main()
