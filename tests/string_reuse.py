"""Compare string concatenation ownership and evaluation order with CPython."""

import sys
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import main

if __name__ == "__main__":
    main(
        fixtures=Path(__file__).resolve().parent / "fixtures" / "string_reuse",
        rejections={},
        imported_rejections={},
        errors={},
        label="string concatenation ownership",
        description=__doc__,
    )
