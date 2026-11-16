def show(values: list[object]) -> None:
    values.append(7)
    print(values)
    print(repr(values))
    print(str(values))


values: list[object] = ["node"]
show(values)


def collect(labels: set[str]) -> list[str]:
    result = ["label"]
    for label in sorted(labels):
        result.extend([label])
    return result


marker = object()
globals()["sorted"] = lambda _: [marker]
assert collect({"source"})[1] is marker
