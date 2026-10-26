def current():
    from fnmatch import filter

    names = ["__jinja2_東京.cache", "keep", "__jinja2_.cache", "__jinja2_x.cache.tmp"]
    print(filter(names, "__jinja2_*.cache"))
    print(filter(pat="[!a]?", names=("ab", ".x", "é/", "é/")))
    blob = bytearray(b"\xff")
    matches = filter((b"\xc3\xa9", blob), b"?")
    print(len(matches), matches[0] is blob)

    class Pattern:
        def __fspath__(self):
            print("resolve pattern")
            return "*.cache"

    print(filter(names, Pattern()))
    for items, pattern in ((["ok", b"bytes"], "*"), (["text"], b"*"), ([42], "*"), (7, "*"), ([], 7)):
        try:
            filter(items, pattern)
        except TypeError as error:
            print(error.args)
    try:
        filter([], "*", pat="?")
    except TypeError as error:
        print(error.args)
