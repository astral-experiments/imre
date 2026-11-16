def publish(a: int, b: int, send):
    total = a + b
    send(total)


def first(value):
    print("first", value)


def second(value):
    print("second", value)


class Destination:
    def __init__(self, fail: bool):
        self.fail = fail

    def __call__(self, value: int) -> None:
        print("entered", value)
        with open("marker.bin", "ab") as marker:
            marker.write(b"called\n")
        if self.fail:
            raise ValueError("refused " + str(value))
        print("delivered", value)


def custom_add(self, other):
    with open("addition.bin", "ab") as record:
        record.write(b"override\n")
    print("override")
    return 314159

MarkedInt = type("MarkedInt", (int,), {"__add__": custom_add})

callbacks = {"first": first, "second": second, "save": Destination(False), "raise": Destination(True), "override": Destination(False)}
choice = input()
chosen = callbacks[choice]
value = int(input())
left = MarkedInt(value) if choice == "override" else value
publish(left, int(input()), chosen)
print("after")
