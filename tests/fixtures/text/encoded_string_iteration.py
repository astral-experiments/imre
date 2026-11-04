def receiver(text: str) -> str:
    print("receiver")
    return text


def visit(text: str, stop: int) -> None:
    for byte in receiver(text).encode():
        text = "rebound"
        if byte == stop:
            break
        print(byte)
    else:
        print("exhausted")


visit("", 0)
visit("a\x00é雪🙂", 300)
visit("é雪", 169)
