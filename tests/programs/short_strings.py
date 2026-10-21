class Label:
    text: str

    def __init__(self, text: str) -> None:
        self.text = text


def main() -> None:
    # Each group spans 23, 24, and 25 UTF-8 bytes; len() counts code points.
    a23 = "abcdefghijklmnopqrstuvw"
    a24 = "abcdefghijklmnopqrstuvwx"
    a25 = "abcdefghijklmnopqrstuvwxy"
    u23 = "éééééééééééx"
    u24 = "éééééééééééé"
    u25 = "ééééééééééééx"
    e23 = "🙂🙂🙂🙂🙂abc"
    e24 = "🙂🙂🙂🙂🙂🙂"
    e25 = "🙂🙂🙂🙂🙂🙂x"
    n24 = a23 + "\0"
    n25 = a24 + "\0"
    values = ["", "\0", a23, a24, a25, u23, u24, u25, e23, e24, e25, n24, n25]
    for index in range(len(values)):
        value = values[index]
        print(value)
        print(len(value))
        print(value == value + "")

    print(a23 + "x" == a24)
    print(a24 + "y" == a25)
    print("ééééééééééé" + "é" == u24)
    print(u24 + "x" == u25)
    print("🙂🙂🙂🙂🙂" + "🙂" == e24)
    print(e24 + "x" == e25)
    print(a23 == a24)
    print(u24 == e24)
    print(n24 == a24)

    source = a23 + "\t" + a24 + "\n" + a25 + " " + u24 + " " + n25
    tokens = source.split()
    source = "replaced"
    alias = tokens
    first = tokens[0]
    saved: tuple[str, str, int] = (tokens[1], tokens[2], len(tokens))
    label = Label(tokens[3])
    from_field = label.text
    label.text = "replaced"
    last = alias.pop()
    tokens = []
    alias[0] = "replaced"
    alias = []

    # These values outlive the split source, list entries, and class field.
    print(first == a23)
    print(saved[0] == a24)
    print(saved[1] == a25)
    print(saved[-1])
    print(from_field == u24)
    print(last == n25)
    print(last)
    print(len(last))


main()
