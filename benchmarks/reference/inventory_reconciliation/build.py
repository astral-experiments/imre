"""Build the inventory BigInt reference with explicit, reproducible provenance."""

import runpy
from pathlib import Path


if __name__ == "__main__":
    root = Path(__file__).resolve().parent
    helper = root / "wrapped_reference.py"
    if not helper.is_file():
        helper = root.parent / helper.name
    build = runpy.run_path(str(helper))["build_reference"]
    build(
        root,
        __doc__,
        entrypoint="inventory",
        cargo_binary="inventory-reference",
        output_name="inventory-bigint",
        role="bigint",
    )
