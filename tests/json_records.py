"""Compare the explicit typed JSON boundary with its checked CPython implementation."""

import sys
from pathlib import Path

if sys.flags.optimize:
    raise SystemExit("validation requires Python assertions")

sys.path.insert(0, str(Path(__file__).resolve().parent))
import assignments

ROOT = Path(__file__).resolve().parents[1]
assignments.BOOTSTRAP = (
    f"import sys; sys.path.insert(0, {str(ROOT / 'stdlib')!r});\n"
    + assignments.BOOTSTRAP
)
FIXTURES = Path(__file__).resolve().parent / "fixtures" / "json_records"
IMPORT = "from relar.json import decode\n"
ITEM = "class Item:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\n"

REJECTED = {
    "unknown_intrinsic": "from relar.json import loads\n",
    "bytes_input": IMPORT + ITEM + 'item = decode(b"{}", Item)\n',
    "missing_schema": IMPORT + 'item = decode("{}")\n',
    "extra_argument": IMPORT + ITEM + 'item = decode("{}", Item, False)\n',
    "keyword_schema": IMPORT + ITEM + 'item = decode("{}", schema=Item)\n',
    "instance_schema": IMPORT + ITEM + 'item = decode("{}", Item(1))\n',
    "container_schema": IMPORT + ITEM + 'item = decode("[]", list[Item])\n',
    "computed_constructor": IMPORT + ITEM.replace("self.value = value", "self.value = value + 1") + 'item = decode("{}", Item)\n',
    "calling_constructor": IMPORT + ITEM.replace("self.value = value", "self.value = int(value)") + 'item = decode("{}", Item)\n',
    "renamed_parameter": IMPORT + ITEM.replace("value: int) ->", "argument: int) ->").replace("self.value = value", "self.value = argument") + 'item = decode("{}", Item)\n',
    "extra_parameter": IMPORT + ITEM.replace("value: int) ->", "value: int, extra: int) ->") + 'item = decode("{}", Item)\n',
    "missing_parameter": IMPORT + "class Item:\n    value: int\n    def __init__(self) -> None:\n        self.value = 1\nitem = decode(\"{}\", Item)\n",
    "unpacked_fields": IMPORT + "class Item:\n    left: int\n    right: int\n    def __init__(self, left: int, right: int) -> None:\n        self.left, self.right = left, right\nitem = decode(\"{}\", Item)\n",
    "nonempty_list_default": IMPORT + "class Item:\n    values: list[int]\n    def __init__(self, values: list[int] | None = None) -> None:\n        self.values = [1] if values is None else values\nitem = decode(\"{}\", Item)\n",
    "truthiness_list_default": IMPORT + "class Item:\n    values: list[int]\n    def __init__(self, values: list[int] | None = None) -> None:\n        self.values = values or []\nitem = decode(\"{}\", Item)\n",
    "infinite_default": IMPORT + "class Item:\n    value: float\n    def __init__(self, value: float = 1e999) -> None:\n        self.value = value\nitem = decode(\"{}\", Item)\n",
    "nested_computed_constructor": IMPORT + ITEM.replace("self.value = value", "self.value = value + 1") + "class Outer:\n    items: list[Item]\n    def __init__(self, items: list[Item] | None = None) -> None:\n        self.items = [] if items is None else items\nouter = decode(\"{}\", Outer)\n",
}
for name, annotation in [
    ("bytes", "bytes"),
    ("dictionary", "dict[str, int]"),
    ("tuple", "tuple[int, str]"),
    ("set", "set[int]"),
    ("nested_bytes", "list[bytes]"),
]:
    REJECTED[name + "_field"] = (
        IMPORT
        + f"class Item:\n    value: {annotation}\n    def __init__(self, value: {annotation}) -> None:\n        self.value = value\n"
        + 'item = decode("{}", Item)\n'
    )

IMPORTED_REJECTIONS = {
    "intrinsic_shadow": {
        "main.py": "from relar.json import decode\n",
        "relar/__init__.py": "",
        "relar/json.py": "def decode(text: str, schema: int) -> int:\n    return schema\n",
    },
}

if __name__ == "__main__":
    assignments.main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors={},
        label="typed JSON",
        description=__doc__,
    )
