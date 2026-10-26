"""Compare compiled dataclasses and statically bound descriptors with CPython."""

import argparse
import ast
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
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
FIXTURES = Path(__file__).resolve().parent / "fixtures" / "dataclasses"
IMPORT = "from dataclasses import dataclass, field\n"
ITEM = IMPORT + "@dataclass\nclass Item:\n    value: int\n"
FROZEN = ITEM.replace("@dataclass\n", "@dataclass(frozen=True)\n")
METHOD = ITEM + "    @classmethod\n    def make(cls, value: int) -> \"Item\":\n        return cls(value)\n"
PROPERTY = ITEM + "    @property\n    def double(self) -> int:\n        return self.value * 2\n"
ITEMS_METHOD = ITEM + "    def items(self, value: int) -> list[int]:\n        return [value]\n"

SLOTS = ITEM.replace("@dataclass\n", "@dataclass(slots=True)\n")
ERASE = "\ndef hold(value: object) -> object:\n    return value\n"
STRUCT = "from imre import struct\n@struct\nclass Point:\n    x: float\n"
REJECTED = {
    "boxed_unslotted": ITEM + ERASE + "hold(Item(2))\n",
    "boxed_mutable_list_field": SLOTS.replace("value: int", "value: list[int]") + ERASE + "hold(Item([2]))\n",
    "boxed_special_method": SLOTS + "    def __bool__(self) -> bool:\n        return False\n" + ERASE + "hold(Item(2))\n",
    "boxed_disabled_equality": SLOTS.replace("slots=True", "slots=True, eq=False") + ERASE + "hold(Item(2))\n",
    "boxed_unchecked_attribute": SLOTS + "def observe(value: object) -> int:\n    return value.other\nobserve(Item(2))\n",
    "boxed_bad_index": SLOTS + ERASE + "def observe(value: object) -> object:\n    return value['invalid']\nobserve(hold(Item(2)))\n",
    "struct_identity": STRUCT + "p = Point(1.0)\nprint(p is p)\n",
    "struct_id": STRUCT + "print(id(Point(1.0)))\n",
    "struct_field_write": STRUCT + "p = Point(1.0)\np.x = 2.0\n",
    "struct_subclass": STRUCT + "class Child(Point):\n    pass\n",
    "struct_owning_field": STRUCT.replace("x: float", "x: list[float]"),
    "struct_python_int": STRUCT.replace("x: float", "x: int"),
    "struct_weakref": STRUCT + "from weakref import ref\nref(Point(1.0))\n",
    "struct_optional_identity": STRUCT + "def same(a: Point | None, b: Point | None) -> bool:\n    return a is b\n",
    "struct_wrong_argument": STRUCT + "Point(True)\n",
    "struct_custom_eq": STRUCT + "    def __eq__(self, other: object) -> bool:\n        return True\n",
    "struct_post_init": STRUCT + "    def __post_init__(self) -> None:\n        print(self.x)\n",
    "struct_erased_identity": STRUCT + ERASE + "p = Point(1.0)\nprint(hold(p) is hold(p))\n",
    "unproven_post_init_conversion": """from dataclasses import dataclass
@dataclass(frozen=True)
class Wrapper:
    value: dict[str, int]
    def __post_init__(self) -> None:
        print("wrapper callback")
@dataclass(frozen=True)
class Holder:
    data: Wrapper
    def __post_init__(self) -> None:
        if isinstance(self.data, dict):
            object.__setattr__(self, "data", Wrapper(value=self.data))
Holder({"a": 1})
""",
    "asdict_copy_unknown_keyword": """from dataclasses import asdict, dataclass
from typing import Self
@dataclass(frozen=True)
class Option:
    preview: bool = False
    def updated(self: Self, **kwargs) -> Self:
        return type(self)(**{**asdict(self), **kwargs})
Option().updated(unknown=True)
""",
    "post_init_parameter": ITEM + "    def __post_init__(self, argument: int) -> None:\n        pass\n",
    "post_init_return": ITEM + "    def __post_init__(self) -> int:\n        return 1\n",
    "post_init_static": ITEM + "    @staticmethod\n    def __post_init__(self: int) -> None:\n        pass\n",
    "wrong_default": IMPORT + "@dataclass\nclass Item:\n    value: int = True\n",
    "wrong_field_assignment": ITEM + "item = Item(1)\nitem.value = False\n",
    "wrong_constructor": ITEM + "item = Item(True)\n",
    "missing_field": ITEM + "item = Item()\n",
    "extra_field": ITEM + "item = Item(1, extra=2)\n",
    "keyword_only_positional": ITEM.replace("@dataclass\n", "@dataclass(kw_only=True)\n") + "Item(1)\n",
    "field_keyword_only": IMPORT + "@dataclass\nclass Item:\n    value: int = field(kw_only=True)\nItem(1)\n",
    "non_default_after_default": IMPORT + "@dataclass\nclass Item:\n    first: int = 1\n    second: int\n",
    "mutable_default": IMPORT + "@dataclass\nclass Item:\n    values: list[int] = []\n",
    "wrong_factory": IMPORT + "@dataclass\nclass Item:\n    values: list[int] = field(default_factory=dict)\n",
    "called_factory": IMPORT + "@dataclass\nclass Item:\n    values: list[int] = field(default_factory=list())\n",
    "custom_factory": IMPORT + "def values() -> list[int]:\n    return [1]\n@dataclass\nclass Item:\n    values: list[int] = field(default_factory=values)\n",
    "default_and_factory": IMPORT + "@dataclass\nclass Item:\n    values: list[int] = field(default=None, default_factory=list)\n",
    "field_init_false": IMPORT + "@dataclass\nclass Item:\n    value: int = field(default=1, init=False)\n",
    "field_metadata": IMPORT + "@dataclass\nclass Item:\n    value: int = field(metadata={})\n",
    "field_hash": IMPORT + "@dataclass\nclass Item:\n    value: int = field(hash=False)\n",
    "nonliteral_option": ITEM.replace("@dataclass\n", "@dataclass(frozen=bool(1))\n"),
    "ordering": ITEM.replace("@dataclass\n", "@dataclass(order=True)\n"),
    "unsafe_hash": ITEM.replace("@dataclass\n", "@dataclass(unsafe_hash=True)\n"),
    "weakref_slot": ITEM.replace("@dataclass\n", "@dataclass(slots=True, weakref_slot=True)\n"),
    "no_initializer": ITEM.replace("@dataclass\n", "@dataclass(init=False)\n"),
    "duplicate_decorator": ITEM.replace("@dataclass\n", "@dataclass\n@dataclass\n"),
    "frozen_assignment": FROZEN + "item = Item(1)\nitem.value = 2\n",
    "frozen_augmentation": FROZEN + "item = Item(1)\nitem.value += 2\n",
    "frozen_unpacked_target": FROZEN + "item = Item(1)\nitem.value, other = (2, 3)\n",
    "frozen_method_assignment": FROZEN + "    def change(self) -> None:\n        self.value = 2\n",
    "frozen_explicit_initializer": FROZEN + "    def __init__(self, value: int) -> None:\n        self.value = value\n",
    "property_assignment": PROPERTY + "item = Item(1)\nitem.double = 3\n",
    "property_augmentation": PROPERTY + "item = Item(1)\nitem.double += 3\n",
    "property_call": PROPERTY + "Item(1).double()\n",
    "property_extra_parameter": PROPERTY.replace("def double(self)", "def double(self, extra: int)"),
    "frozen_property_setter_assignment": PROPERTY.replace("@dataclass\n", "@dataclass(frozen=True)\n")
        + "    @double.setter\n    def double(self, value: int) -> None:\n        print(value)\nItem(1).double = 4\n",
    "cached_property_has_no_setter": "from functools import cached_property\n"
        + PROPERTY.replace("@property", "@cached_property")
        + "    @double.setter\n    def double(self, value: int) -> None:\n        self.value = value // 2\n",
    "property_recursion": PROPERTY.replace("return self.value * 2", "return self.double"),
    "classmethod_recursion": METHOD.replace("return cls(value)", "return cls.make(value)"),
    "classmethod_missing_receiver": METHOD.replace("def make(cls, value: int)", "def make(value: int)"),
    "classmethod_wrong_receiver": METHOD.replace("def make(cls,", "def make(self,").replace("return cls(value)", "return Item(value)"),
    "classmethod_receiver_rebind": METHOD.replace("return cls(value)", "cls = Item\n        return cls(value)"),
    "mixed_descriptors": METHOD.replace("    @classmethod", "    @staticmethod\n    @classmethod"),
    "self_outside_method": "from typing import Self\ndef identity(value: Self) -> Self:\n    return value\n",
    "compared_float_list": IMPORT + "@dataclass\nclass Item:\n    values: list[float]\n",
    "compared_float_tuple": IMPORT + "@dataclass\nclass Item:\n    values: tuple[float, int]\n",
    "compared_optional_float_map": IMPORT + "@dataclass\nclass Item:\n    values: dict[str, float | None]\n",
    "loop_method_starred_arguments": ITEMS_METHOD + "for value in Item(1).items(*[2]):\n    print(value)\n",
    "loop_method_expanded_keywords": ITEMS_METHOD + "for value in Item(1).items(**{'value': 2}):\n    print(value)\n",
}

IMPORTED_REJECTIONS = {
    "dataclasses_shadow": {
        "main.py": "from dataclasses import dataclass\n",
        "dataclasses.py": "def dataclass(value: int) -> int:\n    return value\n",
    },
    "unsupported_import": {"main.py": "from dataclasses import replace\n"},
}

ERRORS = {
    "property_failure": (
        ITEM + "    @property\n    def bad(self) -> int:\n        print(\"getter\")\n        raise ValueError(\"getter failed\")\nprint(\"before\")\nprint(Item(1).bad)\n",
        "ValueError",
    ),
    "staticmethod_receiver_failure": (
        ITEM + "    @staticmethod\n    def identity(value: int) -> int:\n        return value\n"
        + "def receiver() -> Item:\n    print(\"receiver\")\n    raise ValueError(\"receiver failed\")\n"
        + "def argument() -> int:\n    print(\"must not evaluate argument\")\n    return 1\n"
        + "print(receiver().identity(argument()))\n",
        "ValueError",
    ),
    "classmethod_receiver_failure": (
        METHOD
        + "def receiver() -> Item:\n    print(\"receiver\")\n    raise ValueError(\"receiver failed\")\n"
        + "def argument() -> int:\n    print(\"must not evaluate argument\")\n    return 1\n"
        + "print(receiver().make(argument()))\n",
        "ValueError",
    ),
    "constructor_argument_failure": (
        IMPORT + "@dataclass\nclass Item:\n    name: str\n    value: int\n"
        + "def name() -> str:\n    print(\"name\")\n    return \"owned name beyond compact string storage\"\n"
        + "def value() -> int:\n    print(\"value\")\n    raise ValueError(\"argument failed\")\n"
        + "Item(name(), value())\n",
        "ValueError",
    ),
}


def write_upstream_teststats(fixtures: Path) -> None:
    """Exercise the unchanged dataclass and its three methods from pinned libregrtest."""
    suite = ROOT / "benchmarks/cpython_stdlib/full/shared"
    key = "Lib/test/libregrtest/result.py"
    source = (suite / "upstream" / key).read_bytes()
    git_blob = hashlib.sha1(b"blob " + str(len(source)).encode() + b"\0" + source).hexdigest()
    assert git_blob == json.loads((suite / "UPSTREAM.json").read_text())["files"][key]
    cls = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == "TestStats")
    start = min(d.lineno for d in cls.decorator_list) - 1
    original = b"".join(source.splitlines(keepends=True)[start:cls.end_lineno])
    driver = """
@dataclasses.dataclass(slots=True)
class UnittestResult:
    testsRun: int
    failures: list[str]
    skipped: list[str]

@dataclasses.dataclass(slots=True)
class DoctestResult:
    attempted: int
    failed: int
    skipped: int

def retain(value: object) -> object:
    return value

def display(value: object, original: TestStats) -> None:
    print(repr(value))
    print(str(value))
    print(value is original)
    print(value == original)
    print(isinstance(value, tuple))
    if isinstance(value, TestStats):
        print(value.tests_run)
        print(value.failures)
        print(value.skipped)

stats = TestStats.from_unittest(UnittestResult(3, ["failed"], []))
stored = retain(stats)
display(stored, stats)
stats.accumulate(TestStats.from_doctest(DoctestResult(5, 0, 2)))
display(stored, stats)
display((8, 1, 2), stats)
"""
    (fixtures / "upstream_teststats.py").write_bytes(b"import dataclasses\n\n" + original + driver.encode())


def run_suite(fixtures: Path) -> None:
    assignments.main(
        fixtures=fixtures,
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTIONS,
        errors=ERRORS,
        label="dataclass and descriptor",
        description=__doc__,
    )


if __name__ == "__main__":
    # Python 3.13 changed distinct dataclass instances from tuple comparison to
    # fieldwise comparison. Only the shared-NaN differential needs that oracle;
    # portable NaN, record identity, and runtime unit coverage run on every host.
    options = argparse.ArgumentParser(add_help=False)
    options.add_argument("--python", default=sys.executable)
    selected, _ = options.parse_known_args()
    version = subprocess.run(
        [selected.python, "-I", "-c", "import sys; print(sys.version_info >= (3, 13))"],
        text=True, capture_output=True, check=True, timeout=30,
    ).stdout.strip()
    with tempfile.TemporaryDirectory(prefix="imre-dataclass-oracle-") as directory:
        fixtures = Path(directory) / "dataclasses"
        shutil.copytree(FIXTURES, fixtures, ignore=shutil.ignore_patterns("__pycache__"))
        write_upstream_teststats(fixtures)
        if version != "True":
            (fixtures / "nan_alias.py").unlink()
            print("Shared-NaN differential requires CPython >=3.13; other dataclass checks remain enabled")
        run_suite(fixtures)
