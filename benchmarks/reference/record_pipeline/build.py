"""Build the independent record reference and record its sources, tools, and flags."""

import runpy
import sys
from pathlib import Path

if sys.flags.optimize:
    raise SystemExit("build validation requires Python assertions; do not use -O")

if __name__ == "__main__":
    root = Path(__file__).resolve().parent
    builder = root / "build_reference.py"
    if not builder.is_file():
        builder = root.parent / "build_reference.py"
    build = runpy.run_path(str(builder))["build_reference"]
    build(
        root,
        __doc__,
        entrypoint="record_pipeline",
        binaries={"reference": "record-reference"},
        comparison_domain=(
            "Payout records with signed ASCII decimal tokens of at most 4,300 digits. Rust uses checked "
            "native i64 arithmetic with BigInt fallback and per-call black_box. Complete output, "
            "repetition checksums, diagnostics, and file effects are compared with the independent "
            "oracle and CPython."
        ),
    )
