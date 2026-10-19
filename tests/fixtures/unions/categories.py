class Box:
    value: int

    def __init__(self) -> None:
        self.value = 1


def inspect(error: Exception | None) -> None:
    print(isinstance(error, Exception))
    print(isinstance(error, BaseException))
    print(isinstance(error, ValueError))
    print(isinstance(error, (LookupError, ArithmeticError)))
    print(isinstance(error, str))


def failure() -> str | bytes:
    raise ValueError("test source")


print(isinstance(True, int))
print(isinstance(1, bool))
print(isinstance(1.0, float))
print(isinstance("text", bytes))
print(isinstance(["text"], list))
print(isinstance({"key": 1}, dict))
print(isinstance({1}, set))
print(isinstance((1, "text"), tuple))
print(isinstance(Box(), Box))
print(isinstance(None, (str, bytes)))
inspect(None)
inspect(ValueError("value"))
inspect(KeyError("key"))
inspect(ZeroDivisionError("zero"))
try:
    print(isinstance(failure(), str))
except ValueError:
    print("source failure preserved")
