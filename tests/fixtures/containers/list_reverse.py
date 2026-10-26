def receiver(events: list[int], values: list[int]) -> list[int]:
    events.append(1)
    return values


def argument(events: list[int]) -> int:
    events.append(2)
    return 23


def main():
    values = [1, 0, 10 ** 80, -5]
    alias = values
    print(values.reverse())
    print(alias)
    print(alias is values)
    saved = values.reverse
    values = [9]
    print(saved())
    print(values)
    print(alias)
    print(list.reverse(alias))
    print(alias)
    empty: list[str] = []
    print(empty.reverse())
    print(empty)
    word = ["listed"]
    print(word.reverse())
    print(word)
    words = ["α", "b", "c"]
    text_alias = words
    saved_text = words.reverse
    words = ["new"]
    saved_text()
    print(text_alias)
    print(words)
    first = [1]
    second = [2]
    rows = [first, second, second]
    rows.reverse()
    print(rows[0] is second)
    print(rows[2] is first)
    rows[0].append(9)
    print(rows[1])
    values = [1, 2, 3, 4]
    seen: list[int] = []
    for item in values:
        seen.append(item)
        if len(seen) == 1:
            values.reverse()
    print(seen)
    print(values)
    events: list[int] = []
    try:
        receiver(events, values).reverse(argument(events))
    except TypeError as error:
        print(str(error))
    print(events)
    print(values)
    events.clear()
    try:
        receiver(events, values).reverse(bad=argument(events))
    except TypeError as error:
        print(str(error))
    print(events)
    try:
        list.reverse()
    except TypeError as error:
        print(str(error))
    try:
        list.reverse(123, argument(events))
    except TypeError as error:
        print(str(error))
    print(events)
    try:
        list.reverse({3})
    except TypeError as error:
        print(str(error))
    try:
        list.reverse(values, [], bad=argument(events))
    except TypeError as error:
        print(str(error))
    print(events)
    print(values)


main()
