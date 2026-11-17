"""Generate an AOT-friendly unittest launcher for an explicit, ordered module list."""
from __future__ import annotations

import argparse
import keyword
from pathlib import Path

TEMPLATE = Path(__file__).with_name("literal_unittest_template.py")
MARKER = "    # LITERAL_MODULES\n"


def generate(modules: list[str]) -> str:
    if not modules:
        raise ValueError("supply at least one test module")
    if len(set(modules)) != len(modules):
        raise ValueError("duplicate test module")
    for module in modules:
        if not all(part.isidentifier() and not keyword.iskeyword(part) for part in module.split(".")):
            raise ValueError(f"invalid dotted Python module: {module!r}")
    imports = "".join(
        f"    import {module} as _module_{index}\n"
        f"    suite.addTests(loader.loadTestsFromModule(_module_{index}))\n"
        for index, module in enumerate(modules)
    )
    template = TEMPLATE.read_text(encoding="utf-8")
    if template.count(MARKER) != 1:
        raise ValueError("runner template marker is missing or repeated")
    return template.replace(MARKER, imports)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--module", action="append", required=True, help="literal dotted module; repeat in discovery order")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        source = generate(args.module)
    except ValueError as exc:
        parser.error(str(exc))
    args.output.write_text(source, encoding="utf-8")


if __name__ == "__main__":
    main()
