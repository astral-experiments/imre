events = []


class Gate:
    def __bool__(self):
        events.append("truth")
        return False


def rejected():
    raise AssertionError("evaluated right operand")


def consume(gate, right):
    while gate and right():
        print("unexpected body")
    else:
        print("exhausted", events)


def current():
    events.clear()
    consume(Gate(), rejected)
    assert events == ["truth"]
