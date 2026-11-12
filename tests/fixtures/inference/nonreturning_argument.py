def take(value: int) -> int:
    return value


def stop():
    print("before")
    raise ValueError("stopped")


def through_stop():
    return stop()


try:
    print(take(stop()))
except ValueError as error:
    print(str(error))
try:
    print(take(through_stop()))
except ValueError as error:
    print(str(error))
try:
    take(print("unreached", stop()))
except ValueError as error:
    print(str(error))
print("after")
