def marked(log, text):
    log.append(text)
    result = ""
    for index in range(1024):
        result += text
    return result


def failed(log: list[str]) -> str:
    log.append("failure")
    raise ValueError("argument failure")


def joined(first, *, second):
    return first + second


def cleanup(log):
    try:
        return marked(log, "return")
    finally:
        log.append("finally")


def target(value):
    return [value]


def assign_after_rhs() -> None:
    value: int | None = None
    target(value)[0] = (value := 1)
    print(value)


log: list[str] = []
result = joined(second=marked(log, "second"), first=marked(log, "first"))
print(len(result))
for item in log:
    print(item)
for turn in range(24):
    try:
        result = joined(second=marked(log, "owned"), first=failed(log))
    except ValueError as error:
        if turn == 23:
            print(error)
print(len(log))
print(len(cleanup(log)))
print(log[-2])
print(log[-1])
assign_after_rhs()
