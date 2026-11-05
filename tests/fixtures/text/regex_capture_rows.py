from re import findall


def rows(text: str):
    print(findall(r"([A-Za-z_]+)=([0-9]+)", text))
    print(findall(r"(?P<key>[a-z]+):([A-Z]+)([0-9]+)", text))
    print(findall(r"(aba)(ba)", text))
    print(findall(r"([^:]+):([0-9]+)", text))
    print(findall(r"(é)(🌳)", text))


def binary(data: bytes):
    pairs = findall(rb"([\x80-\xff]+)=(\x00+)", data)
    print(pairs)
    if len(pairs) > 1:
        first = pairs[0]
        print(first[0] is pairs[1][0])
        pairs[0] = pairs[1]
        print(first)
        print(pairs)


for value in ["", "bad= good=012 bad2=x k=3", "skip:key:AB12", "abaabababa", "é🌳é🌳", "👋:9\x00:0"]:
    rows(value)

binary(b"\xff=\x00 \xff=\x00\x00 \xff= \x80\xff=\x00")
