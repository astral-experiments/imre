def flag_key(flag: bool | None) -> tuple[bool | None, int]:
    return (flag, 4)


def remove_key(keys: list[tuple[int, str]]) -> str:
    keys.pop()
    return "fallback"


def main() -> None:
    first: tuple[int, tuple[str, bytes], str | None] = (1000000000000000000000000, ("é", b"\x00\xff"), None)
    equal: tuple[int, tuple[str, bytes], str | None] = (1000000000000000000000000, ("é", b"\x00\xff"), None)
    values = {first: "original"}
    values[equal] = "replacement"
    print(len(values))
    print(values[first])
    print(values.get(equal, "missing"))
    print(first == equal)
    print(first != equal)
    flags: dict[tuple[bool | None, int], str] = {flag_key(None): "absent", flag_key(False): "false", flag_key(True): "true"}
    print(len(flags))
    options: list[bool | None] = [None, False, True]
    for flag in options:
        print(flags[flag_key(flag)])
    pairs = [((number, "key"), number * 2) for number in range(5)]
    indexed = {key: value for key, value in pairs}
    expanded = {**indexed, (99, "key"): 7}
    print(len(indexed))
    print(len(expanded))
    keys = {(number % 3, "key") for number in range(12)}
    keys.add((4, "key"))
    keys.discard((1, "key"))
    print(len(keys))
    print((2, "key") in keys)
    print((1, "key") not in keys)
    print(len({*keys, (4, "key")}))
    for key in sorted(keys):
        print(key[0])
    many: dict[tuple[int, str], int] = {}
    for number in range(100):
        many[(number, "stable")] = number
    for number in range(75):
        del many[(number, "stable")]
    for number in range(100, 150):
        many[(number, "stable")] = number
    total = 0
    for key, value in many.items():
        total = total + key[0] + value
    print(len(many))
    print(total)
    try:
        many[(999, "missing")]
    except KeyError as error:
        print(str(error))
    empty: tuple[()] = ()
    empty_keys = {empty: 3}
    print(empty_keys[()])
    present = {(1, "present"): "kept"}
    temporary = [(2, "absent")]
    print(present.get(temporary[0], remove_key(temporary)))
    print(len(temporary))


main()
