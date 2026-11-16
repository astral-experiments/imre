def publish(a: int, b: int, send):
    total = a + b
    send(total)


def first(value):
    print("first", value)


def second(value):
    print("second", value)


def retired(self):
    globals()["values"] = destination
    print("finalized")


def accept(self, value):
    print("received", value)


handlers = {"first": first, "second": second}
send = handlers[input()]
publish(int(input()), int(input()), send)

destination = type("Destination", (), {"append": accept})()
previous = type("Previous", (), {"__del__": retired})
globals()["values"] = previous()
values: list[int] = [3]
values.append(True)
