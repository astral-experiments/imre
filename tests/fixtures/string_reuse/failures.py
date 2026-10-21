def part(log: list[int], event: int) -> str:
    log.append(event)
    return "temporary part " + str(event)


def fail(log: list[int]) -> str:
    log.append(9)
    raise ValueError("RHS failed")


def parameter(text: str, log: list[int]) -> str:
    try:
        text += fail(log)
    finally:
        print(text)
    return text


def rebinding(log: list[int]) -> None:
    text = "before rebinding"
    try:
        text += (text := "replacement remains after failure") + fail(log)
    except ValueError:
        print(text)
    finally:
        print(text)


log: list[int] = []
text = "unchanged heap-sized string before any RHS failure"
for index in range(2):
    try:
        if index == 0:
            text += fail(log)
        else:
            text = text + part(log, 1) + (fail(log) + part(log, 2))
    except ValueError as problem:
        print(str(problem))
        print(text)
    finally:
        print(text)

original = text
try:
    print(parameter(original, log))
except ValueError:
    print(original)

rebinding(log)
for event in log:
    print(event)
