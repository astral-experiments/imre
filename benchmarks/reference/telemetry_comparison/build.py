"""Build the independent telemetry reference and record its sources, tools, and flags."""

import runpy
from pathlib import Path

if __name__ == "__main__":
    root = Path(__file__).resolve().parent
    builder = root / "build_reference.py"
    if not builder.is_file():
        builder = root.parent / "build_reference.py"
    build = runpy.run_path(str(builder))["build_reference"]
    build(
        root,
        __doc__,
        entrypoint="telemetry",
        binaries={"reference": "telemetry-reference"},
        comparison_domain=(
            "Timestamp/value files with signed ASCII decimal tokens of at most 4,300 digits. Rust uses "
            "checked native i128 arithmetic with BigInt fallback and per-call black_box. Complete "
            "output, repetition checksums, diagnostics, and file effects are compared with the "
            "independent oracle and CPython."
        ),
    )
