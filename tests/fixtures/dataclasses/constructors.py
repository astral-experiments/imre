from dataclasses import dataclass, field


@dataclass
class Empty:
    """A record with no fields."""


@dataclass(slots=True)
class ScalarDefaults:
    name: str
    number: int = -1267650600228229401496703205376
    flag: bool = True
    real: float = -0.0
    raw: bytes = b"x\x00"
    optional: str | None = None
    marker: int = field(default=9, kw_only=True)


@dataclass(kw_only=True, slots=False)
class Keywords:
    name: str
    first: int = field(kw_only=False)
    second: int = 2


@dataclass
class SelfNamed:
    self: int = 1


def argument(label: str, value: int) -> int:
    print(label)
    return value


print(Empty())
print(ScalarDefaults("λ"))
print(ScalarDefaults("literal", optional="kept", marker=4))
print(Keywords(argument("positional", 1), second=argument("second", 3), name="named"))
print(SelfNamed(self=9))
