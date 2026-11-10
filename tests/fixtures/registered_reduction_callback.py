def successor_sum(text: str) -> int:
    digits = list(map(int, text))
    return sum([digit * (digit + 1) for digit in digits])


def publish(a: int, b: int, send):
    total = a + b
    send(total)


def first(value):
    print("first", value)


def second(value):
    print("second", value)


handlers = {"first": first, "second": second}
send = handlers[input()]
publish(int(input()), int(input()), send)

print(successor_sum(input()))
