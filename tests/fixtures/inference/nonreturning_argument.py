def take(value: int) -> int:
    return value


def stop():
    print("before")
    raise ValueError("stopped")


try:
    print(take(stop()))
except ValueError as error:
    print(str(error))
print("after")
