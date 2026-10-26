from urllib.parse import parse_qsl, urljoin


def form(raw: str) -> None:
    print(parse_qsl(raw, keep_blank_values=True, errors="werkzeug.url_quote"))


def location(base: str, target: str) -> None:
    print(urljoin(base, target))


def mixed(base: str, target: bytes) -> str | bytes:
    return urljoin(base, target)


def show_mixed(value: str | bytes) -> None:
    if isinstance(value, bytes):
        print(value)
    else:
        print(value)


def observed(label: str, value: str) -> str:
    print(label)
    return value


form("a=one+two&a=%2B&blank=&=name&bare&symbol=%E2%98%83")
print(parse_qsl(b"x=%FF;;y=+", separator=";;"))
print(parse_qsl("x=%E9", encoding="latin1"))
print(parse_qsl("q=%FF&ok=%E2%98%83", errors="ignore"))
print(parse_qsl(None))
for query in ("a=1&bare", "a=1&"):
    try:
        print(parse_qsl(query, strict_parsing=True))
    except ValueError as error:
        print(str(error))
try:
    print(parse_qsl("a=1&&b=2", max_num_fields=2))
except ValueError as error:
    print(str(error))
try:
    print(parse_qsl("a=b", separator=""))
except ValueError as error:
    print(str(error))

for target in ("../next?x=1#top", "?", "#", "//other.test/d", "/signed%2Froute", "../café", "/a//b/../", ""):
    location("https://example.test/root/folder/item?old#prior", target)
print(urljoin(b"http://host/a/b", b"../p?#"))
print(urljoin("a/b", "../.", allow_fragments=False))
show_mixed(urljoin("", b"\xff"))
try:
    show_mixed(urljoin("a", b"b"))
except TypeError as error:
    print(str(error))


show_mixed(mixed("", b""))
show_mixed(mixed("a", b""))
print(urljoin("http://host/a/b#old", "#new", allow_fragments=False))
print(urljoin(url=observed("target", "../next"), base=observed("base", "http://host/a/b")))
print(parse_qsl(errors=observed("errors", "strict"), qs=observed("query", "x=2")))
print(parse_qsl("a=b", max_num_fields=10**100))
try:
    print(parse_qsl("good=first&bad=%FF", errors="unregistered-handler"))
except LookupError as error:
    print(str(error))
