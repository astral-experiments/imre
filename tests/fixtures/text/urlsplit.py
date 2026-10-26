from urllib.parse import SplitResult, SplitResultBytes, urlsplit


def request_target(target: str) -> None:
    parsed = urlsplit(target)
    if not parsed.scheme and parsed.netloc:
        path = f"/{parsed.netloc}{parsed.path}"
    else:
        path = parsed.path
    print((path, parsed.query, parsed.netloc if parsed.scheme else ""))


def inspect_bytes(target: bytes) -> None:
    parsed = urlsplit(target, b"HTTP")
    print(parsed)
    print((parsed.geturl(), parsed.username, parsed.password, parsed.hostname, parsed.port))


request_target("/health")
request_target("//opaque/path?a=%2F")
request_target("HTTP://user:pass@EXAMPLE.org:080/raw%2Furl?q=ONE#fragment")
print(urlsplit(" ////a?#"))
print(urlsplit("a/b?x#y", allow_fragments=False, scheme=" HTTPS\t"))
print(urlsplit("1http://example.com"))
print(urlsplit("git+ssh://a@b:c@EXAMPLE/"))
print((urlsplit("http://ÉΣ.example:0042").hostname,))
inspect_bytes(b"HTTP://a%40b:p@HOST:080/a?x#f")
inspect_bytes(b"http://[FE80::abc%ZoNE]:42/a")
inspect_bytes(b"relative")
parsed = urlsplit("/foo")
scheme, authority, path, query, fragment = parsed
print(parsed._fields)
print((scheme, authority, path, query, fragment))
print((parsed[2], len(parsed), parsed == ("", "", "/foo", "", ""), isinstance(parsed, SplitResult)))
print(SplitResultBytes(b"https", b"host", b"path", b"q", b"f").geturl())
print(isinstance(urlsplit(b"/"), SplitResultBytes))
for invalid in ("//[", "//[127.0.0.1]", "//[a::b::c]", "//[v.h]", "//[::1]junk",
                "//\uff0f", "//user\u2100example:443"):
    try:
        print(urlsplit(invalid))
    except ValueError as error:
        print(str(error))
for bad_port in ("//host:65536", "//host:\u0662", "//host:+80", "//host:00000", "//host:"):
    result = urlsplit(bad_port)
    print(result.netloc)
    try:
        print((result.port,))
    except ValueError as error:
        print(str(error))
try:
    print(urlsplit(b"/no\xff", "https"))  # ty: ignore[no-matching-overload]
except TypeError as error:
    print(str(error))
try:
    print(urlsplit("//host", b""))  # ty: ignore[no-matching-overload]
except TypeError as error:
    print(str(error))
try:
    print(urlsplit(b"a\xffb"))
except UnicodeDecodeError as error:
    print(str(error))
    print(error.args)
try:
    print(urlsplit(b"//host", b"ab\x80"))
except UnicodeDecodeError as error:
    print(str(error))
    print(error.args)
