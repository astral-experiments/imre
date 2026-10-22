"""Compare native typed TOML with CPython's maintained TOML 1.1 parser."""

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
FIXTURES = Path(__file__).resolve().parent / "fixtures" / "toml_records"
IMPORT = "from relar.toml import decode\n"
ITEM = "class Item:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\n"

REJECTED = {
    "unknown_intrinsic": "from relar.toml import loads\n",
    "encoding_unavailable": "from relar.toml import encode\n",
    "bytes_input": IMPORT + ITEM + 'decode(b"value=1", Item)\n',
    "missing_schema": IMPORT + 'decode("value=1")\n',
    "extra_argument": IMPORT + ITEM + 'decode("value=1", Item, False)\n',
    "keyword_schema": IMPORT + ITEM + 'decode("value=1", schema=Item)\n',
    "instance_schema": IMPORT + ITEM + 'decode("value=1", Item(1))\n',
    "quoted_schema": IMPORT + ITEM + 'decode("value=1", "Item")\n',
    "scalar_schema": IMPORT + 'decode("value=1", int)\n',
    "list_root": IMPORT + 'decode("value=[1]", list[int])\n',
    "non_string_keys": IMPORT + 'decode("value=1", dict[int, int])\n',
    "quoted_key_schema": IMPORT + 'decode("value=1", dict["str", int])\n',
    "quoted_element_schema": IMPORT + ITEM + 'decode("value=1", dict[str, "Item"])\n',
    "computed_constructor": IMPORT + ITEM.replace("self.value = value", "self.value = value + 1") + 'decode("value=1", Item)\n',
    "renamed_parameter": IMPORT + ITEM.replace("value: int) ->", "argument: int) ->").replace("self.value = value", "self.value = argument") + 'decode("value=1", Item)\n',
    "nonempty_default": IMPORT + "class Item:\n    values: list[int]\n    def __init__(self, values: list[int] | None = None) -> None:\n        self.values = [1] if values is None else values\ndecode(\"\", Item)\n",
    "dynamic_schema": IMPORT + ITEM + 'def parse(Item: int) -> None:\n    decode("value=1", Item)\n',
}
for name, annotation in (
    ("bytes", "bytes"),
    ("tuple", "tuple[int, str]"),
    ("set", "set[int]"),
    ("union", "int | str"),
    ("nested_bytes", "list[bytes]"),
):
    REJECTED[name + "_field"] = (
        IMPORT
        + f"class Item:\n    value: {annotation}\n    def __init__(self, value: {annotation}) -> None:\n        self.value = value\n"
        + 'decode("value=1", Item)\n'
    )

IMPORTED_REJECTIONS = {
    "intrinsic_shadow": {
        "main.py": IMPORT,
        "relar/__init__.py": "",
        "relar/toml.py": "def decode(text: str, schema: int) -> int:\n    return schema\n",
    },
}

if __name__ == "__main__":
    assignments.main(
        fixtures=FIXTURES,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors={},
        label="typed TOML",
        description=__doc__,
    )
