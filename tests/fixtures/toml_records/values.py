from dataclasses import dataclass, field
from relar.toml import decode


@dataclass(slots=True)
class Item:
    name: str
    count: int = 7


@dataclass(slots=True)
class Config:
    title: str
    enabled: bool
    ratio: float
    limits: dict[str, int]
    items: list[Item] = field(default_factory=list)
    note: str | None = None
    flags: list[bool] = field(default_factory=list)
    optional_number: int | None = None
    optional_float: float | None = None
    optional_bool: bool | None = None
    optional_items: list[int] | None = field(default_factory=list)


class Manual:
    values: list[int]
    scale: float
    label: str

    def __init__(self, values: list[int] | None = None, scale: float = -0.0, label: str = "λ") -> None:
        self.values = [] if values is None else values
        self.scale = scale
        self.label = label


manual = decode('', Manual)
other = decode('', Manual)
manual.values.append(42)
print(repr(manual.values))
print(repr(other.values))
print(manual.scale)
print(manual.label)


text = '''
title = """native
TOML λ"""
enabled = true
ratio = -0.0
flags = [true, false, true]
optional_number = 12345678901234567890123456789012345678901234567890
optional_float = inf
optional_bool = false
optional_items = [1, 2]
ignored = [1, false, 1.5, 1979-05-27T07:32:00Z, { unknown = "valid" }]
[limits]
hex = 0xDEAD_BEEF
oct = 0o755
binary = 0b1010_1010
large = -12345678901234567890123456789012345678901234567890
positive = +42
[[items]]
name = 'first'
[[items]]
name = "second"
count = 99
'''
config = decode(text, Config)
print(config)
print(repr(config.optional_number))
print(repr(config.optional_float))
print(repr(config.optional_bool))
print(repr(config.optional_items))
print(config.limits["large"] * 10)
first = decode('title="one"\nenabled=false\nratio=1.5\nlimits={}', Config)
second = decode('title="two"\nenabled=true\nratio=2.5\nlimits={}', Config)
first.items.append(Item("added"))
first.flags.append(True)
optional_items = first.optional_items
if optional_items is not None:
    optional_items.append(3)
print(first)
print(second)
for token in ["inf", "+inf", "-inf", "nan", "+nan", "-nan", "1e999", "-1e999", "1e-999"]:
    number = decode("value=" + token, dict[str, float])["value"]
    print(number)
    print(number != number)

print(repr(decode('\ufeffvalue=1', dict[str, int])))
print(repr(decode('\ufeffvalue="\ufeff"', dict[str, str])))
for invalid_bom in ['\ufeff\ufeffvalue=1', 'value=1\ufeff']:
    try:
        decode(invalid_bom, dict[str, int])
        assert False, "unexpected byte order mark"
    except ValueError:
        print("invalid byte order mark")

print(repr(decode('table={\n "a=b" = [\n1,\n2], # =\n}', dict[str, dict[str, list[int]]])))
