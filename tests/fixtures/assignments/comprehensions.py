def failing(value: int) -> int:
    if value == 2:
        raise ValueError("stop")
    return value


def collect() -> None:
    target = "outside"
    saved = 10
    values = [(saved := saved + target) for target in range(4)]
    print(saved)
    print(target)
    for value in values:
        print(value)
    filtered = [saved for index in range(6) if (saved := index * 2) > 4]
    print(saved)
    for value in filtered:
        print(value)
    empty = [(saved := index) for index in range(0)]
    print(len(empty))
    print(saved)
    mapping = {(key := str(index)): (value := len(key)) for index in range(4)}
    for name, count in mapping.items():
        print(name + ":" + str(count))
    seen = {(saved := index % 3) for index in range(8)}
    print(len(seen))
    print(saved)
    nested = [[(saved := saved + inner) for inner in range(3)] for outer in range(2)]
    print(saved)
    print(nested[1][2])
    prior = 40
    ignored = [(prior := index) for index in range(2)]
    nested_shadow = [
        [(saved := prior + inner) for inner in range(2)] for prior in range(3)
    ]
    print(prior)
    print(nested_shadow[2][1])
    print(saved)
    try:
        failing_values = [(saved := failing(index)) for index in range(5)]
        print(len(failing_values))
    except ValueError as error:
        print(str(error))
    print(saved)
    try:
        failing_filter = [
            index for index in range(4) if (saved := index) == failing(index)
        ]
        print(len(failing_filter))
    except ValueError as error:
        print(str(error))
    print(saved)
    before = "first "
    print(before + str(len([(before := str(index)) for index in range(3)])) + before)
    print(ignored[1])


def nested_generator_flow() -> None:
    saved: str | None = "initial"
    values = [
        (saved := None)
        for outer in range(1)
        if saved is not None
        for inner in range(2)
        if saved is not None
    ]
    print(len(values))
    print(saved is None)
    saved = "filter"
    filtered = [
        inner
        for outer in range(1)
        if saved is not None
        for inner in range(2)
        if saved is not None
        if (saved := None) is None
    ]
    print(len(filtered))
    written = -1
    lengths = [
        len(saved) + (written := inner)
        for outer in range(2)
        if (saved := "stable") is not None
        for inner in range(3)
    ]
    print(len(lengths))
    print(lengths[5])
    print(written)


collect()
nested_generator_flow()
