from functools import cmp_to_key


def number(log: list[int], value: int) -> int:
    log.append(1)
    return value


def spelling(log: list[int], value: str) -> str:
    log.append(2)
    return value


def exercise() -> None:
    def numeric(left, right):
        left, right = int(left), int(right)
        return (left > right) - (left < right)

    events: list[int] = []
    large = 10**70
    values = [spelling(events, '02'), number(events, 2), spelling(events, '-3'),
              number(events, large), spelling(events, str(large)), number(events, 0)]
    print(events)
    ordered = sorted(values, key=cmp_to_key(numeric))
    for value in ordered:
        print(isinstance(value, str))
        print(int(value))
    print([int(value) for value in values])
    invalid = [1, 'bad integer']
    try:
        invalid.sort(key=cmp_to_key(numeric))
    except ValueError as error:
        print(str(error))
    print(int(invalid[0]))


exercise()
