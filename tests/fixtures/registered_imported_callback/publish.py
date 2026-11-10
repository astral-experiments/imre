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
