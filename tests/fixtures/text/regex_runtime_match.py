def show_result(match: object) -> None:
    print(bool(match))
    print(repr(match))


def check(parts: list[str], text: str) -> None:
    import re as regex
    pattern = "|".join(parts)
    show_result(regex.match(pattern, text))
    show_result(regex.search(pattern, text))


def translate_blocks(pattern: str) -> None:
    from fnmatch import _translate as split_pattern
    parts, stars = split_pattern(pattern, "*", ".")
    print(repr(parts))
    print(repr(stars))


check([r"(?s:(?>.*?a)b)\z", r"z"], "aab")
check([r"(?s:(?>.*?a)b)\z", r"z"], "\nab")
check([r"(?s:.*)"], "ä\n" * 30)
translate_blocks("a**b?")
