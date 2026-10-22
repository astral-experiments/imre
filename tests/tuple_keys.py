"""Compare immutable tuple hashing, equality, and stable ordering with CPython."""

from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import assignments

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "tuple_keys"
REJECTED = {
    "mutable_key": 'values: dict[tuple[list[int]], int] = {}',
    "mutable_nested_key": 'values: set[tuple[int, tuple[dict[str, int]]]] = set()',
    "float_key": 'values: dict[tuple[float], int] = {}',
    "record_key": 'class Item:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\nvalues: dict[tuple[Item], int] = {}',
    "nullable_order": 'values: list[tuple[int | None]] = [(None,), (1,)]\nsorted(values)',
    "float_order": 'sorted([(1.0,), (2.0,)])',
    "mutable_order": 'sorted([([1],), ([2],)])',
    "mixed_key_shape": 'values = {(1, "one"): 1}\nvalues[("one", 1)] = 2',
    "tuple_mutation": 'value = (1, "one")\nvalue[0] = 2',
}

if __name__ == "__main__":
    assignments.main(fixtures=FIXTURES, rejections=REJECTED, imported_rejections={}, errors={}, label="tuple keys", description=__doc__)
