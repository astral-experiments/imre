from dataclasses import dataclass
from relar.toml import decode


@dataclass
class Item:
    value: int = 7


def repeat(text: str, count: int) -> str:
    result = ""
    for index in range(count):
        result += text
    return result


for depth in [62, 63, 64, 65, 100]:
    text = "ignored=" + repeat("[", depth) + "0" + repeat("]", depth)
    try:
        item = decode(text, Item)
        print(f"array accepted {depth}")
    except ValueError:
        print(f"array rejected {depth}")
    text = repeat("ignored.", depth) + "value=0"
    try:
        item = decode(text, Item)
        print(f"table accepted {depth}")
    except ValueError:
        print(f"table rejected {depth}")
