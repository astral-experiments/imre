def argument(text: str) -> str:
    print(text)
    return text


def failing_argument() -> int:
    print("evaluated failing argument")
    raise ValueError("argument failure")


print()
print(argument("left"), b"\xff", argument("right"), 12, 1.25, False, None)
try:
    print("no partial output", failing_argument())
except ValueError as error:
    print(str(error))
