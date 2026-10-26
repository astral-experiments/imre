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
IMPORT = "from imre.json import decode\n"
ITEM = "class Item:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\n"

REJECTED = {
    "mutate_json_rows": 'from typing import Any\ndef mutate(rows: list[dict[str, Any]], other: dict[str, Any]) -> None:\n    rows[0] = other\n',
    "sort_json_alias": 'from typing import Any\ndef mutate(rows: list[dict[str, Any]]) -> None:\n    rows.sort(key=lambda row: 1)\ndef pass_alias(rows: list[dict[str, Any]]) -> None:\n    alias = rows\n    mutate(alias)\n',
    "append_json_rows": 'from typing import Any\ndef mutate(rows: list[dict[str, Any]]) -> None:\n    rows.append(rows[0])\n',
    "dynamic_json_key": 'import json\nkey = "field"\nrows = json.loads("[]")\nvalue = rows[0][key]\n',
    "write_json_object": 'from typing import Any\ndef write(row: dict[str, Any]) -> None:\n    row["field"] = 1\n',
    "nested_json_list_mutation": 'import json\nfrom typing import Any\nrows: list[dict[str, Any]] = json.loads("{}").get("rows", [])\nrows.pop()\n',
    "nested_json_item_mutation": 'import json\nfrom typing import Any\nrows: list[dict[str, Any]] = json.loads("{}").get("rows", [])\nrows[0] = rows[1]\n',
    "root_json_cycle": 'import json\nroot = json.loads("[]")\nroot.append(root)\n',
    "general_any": "from typing import Any\ndef identity(value: Any) -> Any:\n    return value\n",
    "json_ordering": 'import json\nrows = json.loads("[]")\nprint(rows[0] < rows[1])\n',
    "json_instances": 'import json\nfor value in json.loads("[]"):\n    print(isinstance(value, str))\n',
    "json_options": 'import json\njson.loads("[]", strict=False)\n',
    "unknown_intrinsic": "from imre.json import loads\n",
    "bytes_input": IMPORT + ITEM + 'item = decode(b"{}", Item)\n',
    "missing_schema": IMPORT + 'item = decode("{}")\n',
    "extra_argument": IMPORT + ITEM + 'item = decode("{}", Item, False)\n',
    "keyword_schema": IMPORT + ITEM + 'item = decode("{}", schema=Item)\n',
    "instance_schema": IMPORT + ITEM + 'item = decode("{}", Item(1))\n',
    "quoted_schema": IMPORT + ITEM + 'item = decode("{}", "Item")\n',
    "shadowed_builtin_schema": IMPORT + 'from typing import Union as list\nitems = decode("[1]", list[int])\n',
    "quoted_element_schema": IMPORT + ITEM + 'items = decode("[]", list["Item"])\n',
    "quoted_key_schema": IMPORT + ITEM + 'items = decode("{}", dict["str", Item])\n',
    "shadowed_typing_schema": IMPORT + 'import typing as cls\n' + ITEM + '    @classmethod\n    def parse(cls, text: str) -> None:\n        decode(text, list[cls.Union[int, None]])\n',
    "scalar_schema": IMPORT + 'item = decode("1", int)\n',
    "non_string_dictionary": IMPORT + 'item = decode("{}", dict[int, int])\n',
    "bytes_list_schema": IMPORT + 'item = decode("[]", list[bytes])\n',
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
    ("tuple", "tuple[int, str]"),
    ("set", "set[int]"),
    ("nested_bytes", "list[bytes]"),
]:
    REJECTED[name + "_field"] = (
        IMPORT
        + f"class Item:\n    value: {annotation}\n    def __init__(self, value: {annotation}) -> None:\n        self.value = value\n"
        + 'item = decode("{}", Item)\n'
    )

for name, expression in {
    "encode_bytes": 'encode(b"bytes")',
    "encode_tuple": 'encode((1, "one"))',
    "encode_set": 'encode({1, 2})',
    "encode_integer_keys": 'encode({1: "one"})',
    "encode_indent_type": 'encode(1, indent=1.0)',
    "encode_ascii_type": 'encode(1, ensure_ascii=1)',
    "encode_positional_indent": 'encode(1, 2)',
    "encode_unknown_keyword": 'encode(1, sort_keys=True)',
    "encode_missing_value": 'encode()',
}.items():
    REJECTED[name] = "from imre.json import encode\n" + expression + "\n"

IMPORTED_REJECTIONS = {
    "intrinsic_shadow": {
        "main.py": "from imre.json import decode\n",
        "imre/__init__.py": "",
        "imre/json.py": "def decode(text: str, schema: int) -> int:\n    return schema\n",
    },
    "schema_namespace_shadow": {
        "main.py": IMPORT + 'import models as cls\n' + ITEM + '    @classmethod\n    def parse(cls, text: str) -> cls.Item:\n        return decode(text, cls.Item)\n',
        "models.py": ITEM,
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
        thinlto_build_timeout=120,
    )
