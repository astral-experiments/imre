import sys


def successor_sum(text: str) -> int:
    digits = list(map(int, text))
    return sum([digit * (digit + 1) for digit in digits])


def report(stage):
    print("callback", stage)


def relay(text, callback):
    callback("before")
    try:
        return successor_sum(text)
    except ValueError as original:
        callback("caught")
        try:
            raise
        except ValueError as reraised:
            print("identity", original is reraised, reraised is sys.exception())
            raise


def outer():
    callbacks = {"selected": report}
    return relay("1x", callbacks["selected"])


outer()
