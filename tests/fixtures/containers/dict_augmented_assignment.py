def change(values: dict[str, int], mode: int) -> int:
    if mode == 0:
        values.clear()
        values["new"] = 99
    elif mode == 1:
        del values["key"]
        values["new"] = 99
        values["key"] = 100
    elif mode == 2:
        values["key"] = 100
    else:
        raise ValueError("stop")
    return 3


for mode in range(4):
    values = {"key": 7, "other": 2}
    try:
        values["key"] += change(values, mode)
    except ValueError as error:
        print(str(error))
    print(values)
    try:
        values["missing"] += change(values, 2)
    except KeyError:
        print(values["key"])
