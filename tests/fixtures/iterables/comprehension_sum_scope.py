def visit(outer: int, inner: int, values: list[int], events: list[str]) -> int:
    events.append(f"{outer}:{inner}")
    if outer == 1 and inner == 3:
        values.append(4)
    if outer == 1 and inner == 4:
        values[0] = 9
    return outer * inner


def finish(values: list[int], events: list[str], initial: int) -> int:
    events.append("start")
    while len(values) > 0:
        values.pop()
    return initial


def mutate(mapping: dict[str, int], key: str) -> int:
    value = mapping[key]
    mapping["added"] = 9
    return value


def fail_start(events: list[str]) -> int:
    events.append("start")
    raise ValueError("start must not run")


def scopes() -> None:
    # A helper's target shadows the first iterable's name only inside the loop.
    values = [4, 5]
    target = "outer target"
    print(sum([values for values in values]))
    print(len(values))
    print(sum([target for target in [1, 2] for target in range(target)]))
    print(target)

    # Nested iteration is live, and its inner source is evaluated afresh for
    # each outer iteration. Inline walrus writes survive private-target cleanup.
    outer = "outer"
    inner = "inner"
    seen = 1 << 100
    values = [3]
    events: list[str] = []
    print(
        sum(
            [
                (seen := seen + visit(outer, inner, values, events))
                for outer in [1, 2]
                for inner in values
            ],
            start=finish(values, events, seen),
        )
    )
    print(seen)
    print(outer)
    print(inner)
    print(len(values))
    for event in events:
        print(event)


scopes()

# A nested dictionary iterator can fail after exporting a BigInt assignment.
# Its private subtotal must be cleaned up, while the enclosing write survives.
for turn in range(12):
    mapping = {"first": 1 << 100, "second": 2}
    last = 0
    events: list[str] = []
    try:
        print(
            sum(
                [
                    (last := mutate(mapping, key))
                    for repeat in range(2)
                    for key in mapping
                ],
                fail_start(events),
            )
        )
    except RuntimeError as error:
        if turn == 11:
            print(error)
            print(last)
            print(len(events))
            print(len(mapping))
