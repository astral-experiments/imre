def source(events: list[str]) -> str:
    events.append("source")
    return "left,right,tail"


def separator(events: list[str]) -> str:
    events.append("separator")
    return ","


def limit(events: list[str]) -> int:
    events.append("limit")
    return 1


def remove_owner(values: list[str]) -> str:
    values.pop()
    return ","


def bad_separator() -> str:
    print("separator failed")
    raise RuntimeError("argument error")


events: list[str] = []
print(repr(source(events).split(maxsplit=limit(events), sep=separator(events))))
print(repr(events))
events = []
print(repr(source(events).rsplit(sep=separator(events), maxsplit=limit(events))))
print(repr(events))


values: list[str] = ["a long owned receiver,with more than inline storage"]
print(repr(values[0].split(remove_owner(values))))
values = ["another owned receiver,with an independently rooted field"]
print(repr(values[0].partition(remove_owner(values))))


for index in range(40):
    try:
        print(repr(("owned receiver " + str(index)).partition(bad_separator())))
    except RuntimeError as error:
        print(str(error))
    try:
        print(repr(("owned receiver " + str(index)).partition("")))
    except ValueError as error:
        print(str(error))
    try:
        print(repr(("owned receiver " + str(index)).split(sep="", maxsplit=0)))
    except ValueError as error:
        print(str(error))

parts: list[tuple[str, str, str]] = []
for index in range(20):
    parts.append(("left:" + str(index) + ":right").partition(":"))
for part in parts:
    print(repr(part))
    print(part[0] + part[1] + part[2])
print(repr(parts.pop()))

for index in range(30):
    original = "a dynamically owned λ🙂 source " + str(index)
    fields = original.split("missing separator")
    joined = "unused separator".join(fields)
    fields[0] = fields[0] + " field changed"
    original += " original changed"
    print(joined)
    print(fields[0])
    print(original)
    unchanged = original.rsplit("source", 0)
    kept = "".join(unchanged)
    unchanged.pop()
    original = "rebound"
    print(kept)
