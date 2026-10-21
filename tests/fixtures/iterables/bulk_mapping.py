class Source:
    text: str

    def __init__(self, text: str) -> None:
        self.text = text


def take(source: Source, events: list[str]) -> str:
    events.append("source")
    captured = source.text
    source.text = "89"
    return captured


def start(events: list[str]) -> int:
    events.append("start")
    return 10


def show(values: list[int]) -> None:
    print(len(values))
    for value in values:
        print(value)


source = Source("١２𝟛")
alias = source.text
events: list[str] = []
values = list(map(int, take(source, events)))
other = list(map(int, alias))
values[0] = 99
show(values)
show(other)
print(alias)
print(source.text)
print(len(events))

empty = list(map(int, ""))
another_empty = list(map(int, ""))
empty.append(7)
show(empty)
show(another_empty)

# The outer sum's start runs after eager list conversion, but before lazy map
# conversion. In both cases the original string survives its owner's rebinding.
for eager in [True, False]:
    events = []
    source = Source("١xZ")
    try:
        if eager:
            print(sum(list(map(int, take(source, events))), start(events)))
        else:
            print(sum(map(int, take(source, events)), start(events)))
    except ValueError as error:
        print(error)
        print(error.__cause__ is None)
        print(error.__suppress_context__)
    for event in events:
        print(event)
    print(source.text)

# Fail after both ASCII and non-ASCII prefixes. The first invalid character is
# observable, including its repr; a failed assignment preserves the old list.
for text in [
    "1x٢y",
    "١٢xY",
    "x١y",
    "١\0x",
    "١'\\",
    "١\\'",
    "١\u0301x",
    "١\U000e0100x",
    "١\U00011de0x",
    "١²x",
    "١ x",
    "١\t2",
    "١+2",
    "١-2",
    "١_2",
]:
    retained = [77, 88]
    retained_alias = retained
    for turn in range(12):
        try:
            retained = list(map(int, text))
        except ValueError as error:
            if turn == 11:
                print(error)
    retained_alias[0] = 99
    print(retained[0])

text = ""
for index in range(64):
    text += "0١２𝟛"
for turn in range(12):
    values = list(map(int, text))
print(len(values))
print(sum(values))
