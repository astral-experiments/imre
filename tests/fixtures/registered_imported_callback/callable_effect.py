from publish import publish


class Destination:
    def __init__(self, name: str, fail: bool):
        self.name = name
        self.fail = fail

    def __call__(self, value: int) -> None:
        print("entered", self.name, value)
        with open("callback.log", "ab") as record:
            record.write(b"called\n")
        if self.fail:
            raise ValueError("refused " + str(value))
        print("delivered", value)


destinations = {"save": Destination("save", False), "raise": Destination("raise", True)}
send = destinations[input()]
a = int(input())
b = int(input())
print("before")
publish(a, b, send)
print("after")
