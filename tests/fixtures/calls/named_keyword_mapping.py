def total(base: int, *, factor: int = 2, offset: int = 0) -> int:
    return base * factor + offset


def count(*, factor: int, **others: int) -> int:
    others["factor"] = 9
    return factor + len(others)


def sample(key: str) -> None:
    options = {key: 3}
    print(total(7, **options))
    options["offset"] = 4
    print(total(7, **options), count(**options))
    print(options[key], len(options))
    try:
        total(7, factor=2, **options)
    except TypeError as exc:
        print(exc)
    options["other"] = 8
    try:
        total(7, **options)
    except TypeError as exc:
        print(exc)
    options.clear()
    try:
        count(**options)
    except TypeError as exc:
        print(exc)


sample("factor")
