def sentinel(log: list[str]) -> str:
    log.append("sentinel")
    return "end"


def pull(source: list[str], log: list[str]) -> str:
    value = source.pop()
    log.append(value)
    return value


def rebinding() -> None:
    log: list[str] = []
    original = ["unread", "a"]
    current = original
    other = ["kept", "end", "b"]
    for value in iter(lambda: pull(current, log), sentinel(log)):
        print(value)
        current = other
    else:
        print("exhausted")
    print(" ".join(log))
    print(" ".join(original))
    print(" ".join(other))


def exhausted() -> str:
    try:
        raise StopIteration("producer")
    finally:
        print("producer finally")


def failing() -> str:
    raise ValueError("producer error")


def lambda_scope() -> None:
    label = "outer"
    chunks = [b"unused", b"end", b""]
    for chunk in iter(lambda: (label := chunks.pop()), b"end"):
        print(chunk)
    print(label)
    print(chunks.pop())


def materialized_before_delete() -> None:
    error = ValueError("first")
    flags = [False, True]
    for word in list(iter(lambda: str(error) if flags.pop() else "stop", "stop")):
        print(word)
        try:
            raise ValueError("second")
        except ValueError as error:
            pass


rebinding()
lambda_scope()
materialized_before_delete()

for value in iter(lambda: exhausted(), "end"):
    print(value)
else:
    print("producer exhausted")

try:
    for value in iter(lambda: "unused", exhausted()):
        print(value)
except StopIteration as error:
    print("construction: " + str(error))

try:
    for value in iter(lambda: "item", "end"):
        raise StopIteration("body")
except Exception as error:
    print("body: " + str(error))

try:
    for value in iter(lambda: failing(), "end"):
        print(value)
except ValueError as error:
    print(str(error))

with open("chunks", "wb") as out:
    out.write(b"aabb##cc")
with open("chunks", "rb") as stream:
    for chunk in iter(lambda: stream.read(2), b"##"):
        print(chunk)
        break
    else:
        print("unexpected exhaustion")
    for remaining in list(iter(lambda: stream.read(2), b"##")):
        print(remaining)
    print(stream.read())
