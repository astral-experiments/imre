import _codecs
import codecs
import encodings


def search(name):
    calls.append(name)
    if enabled and name == "user_test":
        return codecs.CodecInfo(
            name=name,
            encode=lambda value: (value[::-1].encode(), len(value)),
            decode=lambda value: (bytes(value)[::-1].decode(), len(value)),
        )


def miss(name):
    calls.append("miss:" + name)


def custom_escape(name):
    if name == "raw_unicode_escape":
        calls.append(name)
        return (None, lambda value: ("custom", len(value)), None, None)


assert codecs.register is _codecs.register
assert codecs.unregister is _codecs.unregister
assert codecs.lookup is _codecs.lookup

calls = []
enabled = False

codecs.register(miss)
codecs.register(search)
try:
    codecs.lookup("utf-8")
    assert calls == []
    try:
        codecs.lookup("User-Test")
    except LookupError:
        pass
    else:
        raise AssertionError("disabled codec was found")

    enabled = True
    info = codecs.lookup("USER test")
    assert calls == ["miss:user_test", "user_test"] * 2
    assert info.name == "user_test"
    assert codecs.lookup("user-test") is info
    assert codecs.encode("abc", "User-Test") == b"cba"
    assert codecs.decode(b"cba", "user test") == "abc"
    assert len(calls) == 4

    codecs.unregister(lambda name: None)
    assert codecs.lookup("user_test") is info
    codecs.unregister(miss)
    assert codecs.lookup("user_test") is not info
    assert calls[-1] == "user_test"
finally:
    codecs.unregister(miss)
    codecs.unregister(search)

try:
    codecs.lookup("user-test")
except LookupError:
    pass
else:
    raise AssertionError("unregistered codec was still cached")


calls.clear()
codecs.register(custom_escape)
try:
    assert codecs.decode(br"\u0042", "raw-unicode-escape") == "B"
    assert calls == []
    codecs.unregister(encodings.search_function)
    try:
        assert codecs.decode(br"\u0042", "raw-unicode-escape") == "custom"
        assert codecs.lookup("raw_unicode_escape")[1](b"x") == ("custom", 1)
        assert calls == ["raw_unicode_escape"]
    finally:
        codecs.register(encodings.search_function)
finally:
    codecs.unregister(custom_escape)

assert codecs.decode(br"\u0042", "raw-unicode-escape") == "B"
