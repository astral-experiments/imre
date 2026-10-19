def piece(log: list[int], event: int) -> str:
    log.append(event)
    return str(event) + ":"


def text_source(log: list[int], text: str) -> str:
    log.append(2)
    return text


def position(log: list[int], index: int) -> int:
    log.append(3)
    return index


def byte_source(log: list[int], event: int) -> bytes:
    log.append(event)
    return b"\x01\xfe"


def coalesced_index_effects() -> None:
    for index in [1, 2]:
        text = "coalesced alphabet fragments:"
        log: list[int] = []
        try:
            text += (
                "abcd"[byte_source(log, 1)[0] % 4]
                + "wxyz"[byte_source(log, 2)[index] % 4]
                + "0123"[byte_source(log, 3)[0] % 4]
                + piece(log, 4)
            )
        except IndexError as problem:
            print(str(problem))
        print(text)
        for event in log:
            print(event)


def rebinding() -> None:
    alphabet = "aé😀"
    text = "captured:"
    text += alphabet[len(alphabet := "x")]
    print(text)
    print(alphabet)
    index = 0
    text += "abc"[(index := 1)]
    print(text)
    print(index)
    try:
        text += "abc"[(index := 99)]
    except IndexError as problem:
        print(str(problem))
        print(text)
        print(index)


for alphabet in ["abc", "aé😀", "\x00z", ""]:
    for index in [-4, -1, 0, 2, 3, 100000000000000000000]:
        text = "prefix:"
        log: list[int] = []
        try:
            text += (
                piece(log, 1)
                + text_source(log, alphabet)[position(log, index)]
                + piece(log, 4)
            )
        except IndexError as problem:
            print(str(problem))
        print(text)
        for event in log:
            print(event)

text = "é😀"
text += text[-1]
print(text)
text = "literal:"
text += "abc"[2] + "\x00x"[0]
print(text)
try:
    text += "abc"[100000000000000000000]
except IndexError as problem:
    print(str(problem))
    print(text)
rebinding()
coalesced_index_effects()
