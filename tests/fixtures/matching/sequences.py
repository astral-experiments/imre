def summarize(values: list[int]) -> str:
    match values:
        case []:
            return "empty"
        case [single]:
            return f"one:{single}"
        case [0, first, *middle, last]:
            return f"zero:{first}:{len(middle)}:{last}"
        case [first, *middle, last] as whole:
            whole[0] += 1
            return f"many:{first}:{len(middle)}:{last}:{values[0]}"
        case _:
            return "wrong sequence"


def tuple_shape(value: tuple[str, int, list[int]]) -> int:
    match value:
        case ("ready", count, [first, *rest]):
            return count + first + len(rest)
        case (_, count, _):
            return count
        case _:
            return -1


def optional(values: list[str | None] | None) -> str:
    match values:
        case None:
            return "missing"
        case [None, text] if text is not None:
            return text
        case [text, None] if text is not None:
            return text + "!"
        case [*rest]:
            return f"rest:{len(rest)}"
        case _:
            return "wrong optional"


def nested(values: list[list[int]]) -> int:
    match values:
        case [[first, *inner], *outer]:
            return first + len(inner) + len(outer)
        case _:
            return -1


print(summarize([]))
print(summarize([3]))
print(summarize([0, 4, 5, 6, 7]))
print(summarize([2, 4]))
print(tuple_shape(("ready", 7, [3, 4, 5])))
print(tuple_shape(("waiting", 7, [])))
print(optional(None))
print(optional([None, "yes"]))
print(optional(["yes", None]))
print(optional([]))
print(optional([None, None]))
print(nested([[2, 3, 4], [8]]))
print(nested([]))
match ("tag", 3, 4, 5):
    case ("tag", *numbers, last):
        print(numbers[0] + numbers[1] + last)
empty: list[str] = ["previous"]
match ():
    case [*empty]:
        print(len(empty))
match (1, None, 3):
    case [*maybe]:
        for item in maybe:
            if item is None:
                print("none")
            else:
                print(item)
match [1, 2, 3]:
    case [_, *_, last]:
        print(last)
rows = [[1], [2], [3]]
match rows:
    case [first, *rest]:
        rows[0] = [9]
        first.append(4)
        rest[0].append(5)
        rows.pop()
        print(first[0])
        print(len(rows[1]))
        print(rest[1][0])
        rest.append([6])
        print(len(rows))
        print(len(rest))
