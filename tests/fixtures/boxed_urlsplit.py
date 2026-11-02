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
    for url in ("tel:123-4;phone-context=+1-650-516", b"http://example/a;b/c;p?q#frag"):
        parsed = parse.urlparse(url)
        result_type = parse.ParseResultBytes if isinstance(url, bytes) else parse.ParseResult
        print(type(parsed) is result_type, isinstance(parsed, result_type), tuple(parsed), parsed.params)
        print(parsed.geturl(), parse.urlunparse(parsed))
    class CustomSplit(str):
        def lstrip(self, chars):
            return self
        def replace(self, old, new):
            return self
        def split(self, sep, maxsplit):
            return ('changed?', 'override')
    assert parse.urldefrag(CustomSplit('path#frag')).geturl() == 'changed?#override'

    assert parse.unwrap(' < URL:https://example.test/a?b#c > ') == 'https://example.test/a?b#c'
    assert parse.unwrap('url:path') == 'url:path'

    class Redirect(str):
        def lstrip(self, chars):
            return 'ALT:path?#changed'

    assert parse.urldefrag(Redirect('other:path#old')).geturl() == 'alt:path?#changed'
    regular = Redirect('other:path')
    assert parse.urldefrag(regular).url is regular
    for url in ("doc?#", "doc?#part", b"doc?#", b"doc?#part"):
        result = parse.urldefrag(url)
        result_type = parse.DefragResultBytes if isinstance(url, bytes) else parse.DefragResult
        print(type(result) is result_type, isinstance(result, tuple), result._fields)
        print(tuple(result), result.url, result.fragment, result.geturl())
    try:
        print(parse.urlparse("http://example:oracle").port)
    except ValueError as error:
        print(str(error))
    from urllib.parse import uses_netloc
    print(uses_netloc is parse.uses_netloc, uses_netloc)
    parts = urlsplit("custom:/path")
    uses_netloc.append("custom")
    print(parts.geturl(), urlunsplit((b"custom", b"", b"/path", b"", b"")))
    uses_netloc.pop()
    print(parts.geturl())
    join = parse.urljoin
    try:
        parse.urljoin = None
        assert parse.urljoin is None
        del parse.urljoin
        assert not hasattr(parse, "urljoin")
    finally:
        parse.urljoin = join
    assert parse.urljoin is join
    print(parse.urljoin("http://a/b/c/d?x#y", "?"))
    print(parse.urljoin(b"http://a/b/", b"../g#"))
    print(parse.urljoin(None, b"path"))
    parse.uses_relative.append("custom")
    uses_netloc.append("custom")
    print(parse.urljoin("custom://a/b", "c"))
    uses_netloc.pop()
    print(parse.urljoin("custom://a/b", "c"))
    parse.uses_relative.pop()
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
    pairs = [("k", ["first", "second"])]
    def detach(value, *options):
        if value == "k":
            pairs.clear()
            [[n] for n in range(4800)]
        return value
    print(urlencode(pairs, doseq=True, quote_via=detach))
    class NoLength(TypeError):
        pass
    class Scalar:
        def __len__(self):
            raise NoLength()
        def __str__(self):
            return "a b"
    print(urlencode([("q", Scalar())], doseq=True))
    print(urlencode([("suffix", "a b"), ("suffix", "Málaga/+")]))
    print(parse.urlencode({b"q": [b"a/", b"\xff"]}, doseq=True, safe="/", quote_via=quote))

    class Text(str):
        pass
    class Data(bytes):
        pass

    print(quote(Text("a b")), urlencode([("q", Data(b"a b"))], doseq=True))
