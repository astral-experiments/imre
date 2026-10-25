from collections.abc import Iterator, Mapping as ABC
from typing import Mapping as Alias


class Values(Alias[str, str]):
    def __getitem__(self, key: str) -> str:
        if key == "one":
            return "first"
        raise KeyError(key)

    def __iter__(self) -> Iterator[str]:
        yield from ["one"]

    def __len__(self) -> int:
        return 1


def first(values: ABC[str, object]) -> str:
    return str(values["one"])


def second(values: Alias[str, object]) -> str:
    return str(values["two"])


print(issubclass(ABC, ABC))
print(issubclass(ABC, Alias))
print(issubclass(Alias, Alias))
try:
    print(issubclass(Alias, ABC))
except TypeError as error:
    print(str(error))
print(issubclass(Values, ABC))
print(issubclass(Values, Alias))
print(issubclass(dict, Alias))
print(issubclass(list, Alias))
value = Values()
print(isinstance(value, ABC))
print(isinstance(value, Alias))
present = value.get("one")
if present is None:
    raise AssertionError("present key returned None")
print(present)
print(value.get("missing", "fallback"))
print(first({"one": 1}))
print(second({"two": 2}))
