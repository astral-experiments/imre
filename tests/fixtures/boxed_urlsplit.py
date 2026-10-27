import urllib
from urllib import parse
from urllib.parse import SplitResult, SplitResultBytes, unquote, urlsplit, urlunsplit


def current():
    values = ["first", "old"]
    def change_next(value, safe, encoding=None, errors=None):
        if value == "first":
            values[1] = "changed"
        return value
    print(parse.urlencode([("key", values)], doseq=True, quote_via=change_next))
    print(urllib.parse is parse, parse.urlsplit is urlsplit)
    from urllib.parse import uses_netloc
    print(uses_netloc is parse.uses_netloc, uses_netloc)
    parts = urlsplit("custom:/path")
    uses_netloc.append("custom")
    print(parts.geturl(), urlunsplit((b"custom", b"", b"/path", b"", b"")))
    uses_netloc.pop()
    print(parts.geturl())
    print(urllib.__package__, parse.__package__, parse.__name__, hasattr(urllib, "__path__"))
    for target in ("/health", "/hello%20there?suffix=%26&suffix=%C3%A9", "//first/second?x=1",
                   "https://user:pass@[FE80::1%25Scope]:443/a%2Fb?x=1#frag",
                   b"HTTP://User@EXAMPLE:00080/a%2Fb?#"):
        parts = urlsplit(target)
        print(isinstance(parts, tuple), isinstance(parts, SplitResult), isinstance(parts, SplitResultBytes))
        print(tuple(parts), parts[2], parts.hostname, parts.port, parts.username, parts.password)
        print(unquote(parts.path), urlunsplit(parts), parts.geturl())
    print(urlsplit(url=" /a\tb#f", scheme="http", allow_fragments=False))
    print(unquote(string="%FF+caf%C3%A9"), unquote(b"%FF", encoding="latin-1"))
    print(unquote("%", errors="werkzeug.url_quote"), unquote("untouched", encoding=None))
    parts = urlsplit("https://example:bad/path")
    print(parts.path, parts.hostname)
    try:
        print(parts.port)
    except ValueError as error:
        print(str(error))
    try:
        print(urlsplit("http://[::1"))
    except ValueError as error:
        print(str(error))
    for invalid in (None, ()):
        try:
            unquote(invalid)
        except (TypeError, AttributeError) as error:
            print(str(error))

    from urllib.parse import quote, quote_from_bytes, quote_plus, urlencode

    print(quote("Málaga <&> 東京", safe="!$&'()*+,/:;=@"), quote("/", safe="%!$&'()*+,/:=@"))
    print(quote_from_bytes(bytearray(b"/%\xff")), quote_plus("a b+c"))
    print(urlencode([("suffix", "a b"), ("suffix", "Málaga/+")]))
    print(parse.urlencode({b"q": [b"a/", b"\xff"]}, doseq=True, safe="/", quote_via=quote))
