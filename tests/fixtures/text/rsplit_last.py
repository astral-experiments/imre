def source(events: list[str]) -> str:
    events.append("source")
    return "an owned source,with a final field"


def separator(events: list[str]) -> str:
    events.append("separator")
    return ","


def remove_owner(values: list[str]) -> str:
    values.pop()
    return ","


def bad_separator() -> str:
    raise RuntimeError("separator callback")


def optional_separator(value: str, sep: str | None) -> str:
    return value.rsplit(sep, 1)[-1]


def index(events: list[str]) -> int:
    events.append("index")
    return -1


for value in ["", "a", "aaa", "ababa", "a:b:", "λ🙂λ🙂tail", "\0a\0", "a λ tail "]:
    for sep in [",", "aa", "aba", ":", "λ🙂", "\0", "🙂", " "]:
        selected = value.rsplit(sep, 1)[-1]
        stored = value.rsplit(sep, 1)
        print(repr(selected))
        print(selected == stored[-1])

events: list[str] = []
print(source(events).rsplit(separator(events), maxsplit=1)[-1])
print(repr(events))
events = []
print(source(events).rsplit(maxsplit=1, sep=separator(events))[-1])
print(repr(events))

for counter in range(30):
    values = ["a dynamically owned source " + str(counter) + ",tail " + str(counter)]
    print(values[0].rsplit(remove_owner(values), 1)[-1])
    print(len(values))
    values = ["an owned source without a separator " + str(counter)]
    print(values[0].rsplit(remove_owner(values), 1)[-1])
    print(len(values))
    try:
        print(("owned source " + str(counter)).rsplit(bad_separator(), 1)[-1])
    except RuntimeError as error:
        print(str(error))
    try:
        print(("owned source " + str(counter)).rsplit("", 1)[-1])
    except ValueError as error:
        print(str(error))

print(repr(optional_separator(" a b ", None)))
try:
    print(optional_separator(" \t", None))
except IndexError as error:
    print(str(error))

original = "an independently owned value " + str(len(events))
selected = original.rsplit("missing separator", 1)[-1]
original += " changed"
print(selected)
print(original)

events = []
try:
    print("abc".rsplit("", 1)[index(events)])
except ValueError as error:
    print(str(error))
print(repr(events))
