NAMES = {"ABC", "SSS", "abc", "ÉTA"}
LONG_NAMES = {"ABC", "SNAP", "SSS", "NUL"}
MUTABLE = {"abc"}
WITH_EMPTY = {"", "abc"}


def first_name(text: str) -> str | None:
    name = text.split(".", 1)[0].rstrip(" ").upper()
    if name in NAMES:
        return name
    return None


def lower_name(text: str) -> bool:
    return text.rstrip().lower() in NAMES


def long_name(text: str) -> bool:
    return text.split(".", 1)[0].rstrip(" ").upper() in LONG_NAMES


def mutable_name(text: str) -> bool:
    return text.split(".", 1)[0].lower() in MUTABLE


def empty_name(text: str) -> bool:
    return text.split(".", 1)[0].lower() in WITH_EMPTY


def main() -> None:
    for text in ("aBc  .x", "ABC.extra", "zzz", "A.x", "", "ßs", "abcß", "éta.tail", ".name"):
        print(first_name(text))
        print(lower_name(text))
        print(empty_name(text))
    for text in ("AbC.tail", "cba", "ſnap", "ßs", "aß", "", "ab", "NUL .tail", "xxx", "é"):
        print(long_name(text))
    alias = MUTABLE
    print(mutable_name("xyz.suffix"))
    alias.add("xyz")
    print(mutable_name("xyz.suffix"))
    alias.discard("xyz")
    print(mutable_name("xyz.suffix"))


main()
