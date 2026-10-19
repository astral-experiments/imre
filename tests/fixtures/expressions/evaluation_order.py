"""Conditional and slice evaluation preserve effects and earlier operand owners."""


def condition(log: list[str], flag: bool) -> bool:
    log.append("condition")
    return flag


def branch(log: list[str], name: str) -> str:
    log.append(name)
    return "owned " + name


def replace(values: list[int]) -> int:
    values[0] = 0
    return 1000000000000000000000000000000


def replace_condition(values: list[int]) -> bool:
    values[0] = 0
    return True


def receiver(log: list[str], sources: list[list[int]]) -> list[int]:
    log.append("receiver")
    return sources[0]


def start(log: list[str], values: list[int]) -> int | None:
    log.append("start")
    values.pop()
    return None


def stop(log: list[str], sources: list[list[int]]) -> int | None:
    log.append("stop")
    sources[0] = [90]
    return None


def step(log: list[str], values: list[int]) -> int:
    log.append("step")
    values.append(88)
    return -1


def failing_bound(log: list[str], position: int, failure: int) -> int:
    log.append(str(position))
    if position == failure:
        raise ValueError("bound " + str(position))
    return 1000000000000000000000000000000


def text_receiver(log: list[str], source: list[str]) -> str:
    log.append("text receiver")
    return source[0]


def replace_text(log: list[str], source: list[str]) -> int:
    log.append("replace text")
    source[0] = "replacement"
    return 1


def main() -> None:
    log: list[str] = []
    for flag in [False, True]:
        print(branch(log, "yes") if condition(log, flag) else branch(log, "no"))
    for entry in log:
        print(entry)

    values = [1000000000000000000000000000000]
    print(values[0] == (replace(values) if True else 0))
    values[0] = 1000000000000000000000000000000
    print(
        values[0]
        == (1000000000000000000000000000000 if replace_condition(values) else 0)
    )

    sources = [[0, 1, 2, 3]]
    original = sources[0]
    log = []
    copied = receiver(log, sources)[
        start(log, original) : stop(log, sources) : step(log, original)
    ]
    for entry in log:
        print(entry)
    for item in copied:
        print(item)
    print(sources[0][0])

    text = ["aé🌍z"]
    log = []
    print(text_receiver(log, text)[replace_text(log, text) : -1])
    for entry in log:
        print(entry)
    print(text[0])
    text[0] = "aé🌍z"
    log = []
    print(text_receiver(log, text)[replace_text(log, text)])
    for entry in log:
        print(entry)

    for failure in range(3):
        log = []
        try:
            _discarded = receiver(log, sources)[
                failing_bound(log, 0, failure) : failing_bound(
                    log, 1, failure
                ) : failing_bound(log, 2, failure)
            ]
        except ValueError as error:
            print(str(error))
        finally:
            for entry in log:
                print(entry)


main()
