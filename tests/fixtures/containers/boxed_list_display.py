def show(values: list[object]) -> None:
    values.append(7)
    print(values)
    print(repr(values))
    print(str(values))


values: list[object] = ["node"]
show(values)
