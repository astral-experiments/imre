"""Compare guarded lookup, byte decoding, and integer loop optimizations with CPython."""

import sys
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

REJECTED = {
    "mapping_shadowed_by_target": (
        "def bad(mapping: dict[str, int]) -> list[int]:\n"
        '    return [mapping[mapping] for mapping in "aa"]\n'
    ),
}

# Module initialization rejects every possible early read, including helpers
# whose empty input or invalid UTF-8 would prevent that read at runtime.
IMPORTED_REJECTED = {
    f"early_global_{name}": {
        "main.py": "import library\n",
        "library.py": (
            "def read(data: bytes) -> list[int]:\n"
            "    return [TABLE[character] for character in data.decode()]\n"
            f"FIRST = read({raw})\n"
            'TABLE = {"a": 1}\n'
        ),
    }
    for name, raw in [("empty_bytes", "b''"), ("invalid_bytes", "b'x\\xff'")]
}

if __name__ == "__main__":
    main(
        fixtures=Path(__file__).resolve().parent / "fixtures" / "decoder_optimizations",
        rejections=REJECTED,
        imported_rejections=IMPORTED_REJECTED,
        errors={},
        label="decoder optimization semantics",
        description=__doc__,
    )
