import re


def check(prefix: str) -> None:
    text = prefix + "ab界a🛠abcdef"
    for found in re.compile(r"(?P<first>a)(b)?").finditer(text):
        start, end = found.span("first")
        print(start)
        print(end)
        low, high = found.span(2)
        print(low)
        print(high)
    for found in re.compile(r"(?P<first>a)(b)(c)(d)(e)(f)").finditer(text):
        start, end = found.span(6)
        print(start)
        print(end)


def retained(text: str) -> None:
    matches = re.compile(r"(?P<first>a)").finditer(text)
    earlier = next(matches)
    later = next(matches)
    start, end = later.span("first")
    print(start)
    print(end)
    start, end = earlier.span("first")
    print(start)
    print(end)


check("é🛠")
check("é" * 32766)
check("é" * 40000)
retained("é界a🛠ab")
