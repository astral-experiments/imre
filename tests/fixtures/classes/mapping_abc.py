from collections.abc import Iterator, Mapping


class Section(Mapping[str, str]):
    data: dict[str, str]
    names: list[str]

    def __init__(self, data: dict[str, str], names: list[str]) -> None:
        self.data = data
        self.names = names

    def __getitem__(self, key: str) -> str:
        if key == "broken":
            raise ValueError("broken lookup")
        return self.data[key]

    def __len__(self) -> int:
        return len(self.data)

    def __iter__(self) -> Iterator[str]:
        yield from self.names


class Special(Section):
    def __iter__(self) -> Iterator[str]:
        print("iterating special")
        yield from ["b2", "a1"]

    def __getitem__(self, key: str) -> str:
        if key == "help":
            return "custom"
        raise KeyError(key)

    def __bool__(self) -> bool:
        return False

    def __contains__(self, key) -> bool:
        return key == "synthetic"


class BrokenLength(Section):
    def __len__(self) -> int:
        return -1


class HugeLength(Section):
    def __len__(self) -> int:
        return 1267650600228229401496703205376


class Missing(Mapping[str, str]):
    def __len__(self) -> int:
        return 0


class Complete(Missing):
    def __getitem__(self, key: str) -> str:
        return key

    def __iter__(self) -> Iterator[str]:
        yield from ["complete"]


def walk_completed(section: Missing) -> None:
    for name in section:
        print(name)
    print(sorted(section, key=lambda name: name[1:]))


def truth(section: Section) -> bool:
    return bool(section)


def optional_truth(section: Section | None) -> bool:
    return bool(section)


def contains(section: Section, key: str) -> bool:
    return key in section


def first_truthy(left: Section, right: Section) -> Section:
    return left or right

def query(section: Section, key: str) -> str:
    return section.get(key, "fallback")


def walk(section: Section) -> None:
    for name in section:
        print(name)
    print(sorted(section, key=lambda name: name[1:]))


def marker(value: str) -> str:
    print(value)
    return value


data: dict[str, str] = {"help": "green"}
plain = Section(data, ["help"])
other = Special(data, ["help"])
print(isinstance(plain, Mapping))
print(isinstance(data, Mapping))
print(isinstance(3, Mapping))
print(issubclass(Special, Mapping))
print(issubclass(dict, Mapping))
print(issubclass(list, Mapping))
print(len(plain))
print(plain["help"])
print("help" in plain)
print("missing" not in plain)
print(query(plain, "help"))
print(query(other, "help"))
print(query(other, "missing"))
print(plain.get("missing") is None)
print(plain.get(marker("help"), marker("eager default")))
print(plain.get(default=marker("first"), key=marker("help")))
print(contains(other, "help"))
print(contains(other, "synthetic"))
print(truth(plain))
print(truth(other))
print(truth(Section({}, [])))
print(optional_truth(other))
print(optional_truth(None))
print(first_truthy(other, plain)["help"])
try:
    truth(BrokenLength(data, ["help"]))
except ValueError as error:
    print(str(error))
try:
    len(HugeLength(data, ["help"]))
except OverflowError as error:
    print(str(error))
try:
    Missing()
except TypeError as error:
    print(str(error))
walk(plain)
walk(other)
walk_completed(Complete())
try:
    query(plain, "broken")
except ValueError as error:
    print(str(error))
