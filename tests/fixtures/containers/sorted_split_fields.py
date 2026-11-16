def direct(text: str) -> None:
    for field in sorted(text.split("|")):
        print("d[" + field + "]")


def assigned(text: str) -> None:
    fields = text.split("|")
    text = "rebound"
    for field in sorted(fields):
        print("a[" + field + "]")
    print(text)


def short(text: str) -> None:
    fields = text.split("|")
    for field in sorted(fields):
        print("b[" + field + "]")
        break


def aliased(text: str) -> None:
    fields = text.split("|")
    other = fields
    other.append("between")
    for field in sorted(fields):
        print("x[" + field + "]")
    print(len(other))


def reverse_order(text: str) -> None:
    fields = text.split("|")
    for field in sorted(fields, reverse=True):
        print("r[" + field + "]")


def reused(text: str) -> None:
    fields = text.split("|")
    for field in sorted(fields):
        print("s[" + field + "]")
    print(len(fields))


def variable(text: str, separator: str) -> None:
    for field in sorted(text.split(separator)):
        print("v[" + field + "]")


def main() -> None:
    for text in ["", "a", "|", "|||", "|a||", "same|same|x|same",
                 "🐍|é|🙂|a|aa|a|\0|\r\n|z||", "β₿🙂β₿Aβ₿a",
                 "z|Y|x|W|v|U|t|S|r|Q|p|O|n|M|l|K|j|I|h|G|f|E|d|C|b|A",
                 "Za|Zb|Za|Zc|ZZ|ab|a|w|b|z|c|z|d|z|e|z|f|z|g|z|h|"]:
        direct(text)
        assigned(text)
        short(text)
        aliased(text)
        reverse_order(text)
        reused(text)
        variable(text, "β₿")


if __name__ == "__main__":
    main()
