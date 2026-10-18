import helpers
from facade import nested, produce, step


def main() -> None:
    offset = 1000000000000000000000000000000
    values = [step(item.value) + offset for item in produce(3)]
    for value in values:
        print(value)
    mapping = nested(4)
    for key, row in mapping.items():
        print(key)
        for value in row:
            print(value)
    items: list[helpers.Item] = [helpers.Item(value) for value in range(2)]
    print(items[1].value)


if __name__ == "__main__":
    entry = [step(value) for value in range(2)]
    print(entry[0])
    main()
