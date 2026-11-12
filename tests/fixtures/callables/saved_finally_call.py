def choose():
    try:
        return "wrong"
    finally:
        print("finally")
        raise SystemExit(0)


def take(value: int) -> int:
    return value


def relay(arg):
    return take(arg)


saved = choose
print(relay(saved()))
