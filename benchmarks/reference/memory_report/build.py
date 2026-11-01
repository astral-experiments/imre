"""Build the independent memory report reference and record its sources, tools, and flags."""

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
        entrypoint="memory_report",
        binaries={"reference": "memory-report-reference", "producer": "memory-report-producer"},
        comparison_domain=(
            "Schema-directed memory report JSON with arbitrary-precision integers, known-field duplicate "
            "rejection, scalar/list defaults, and depth 128. Rust stores native i64 values with checked "
            "BigInt fallback, stable sorting, and exact rational-to-binary64 rounding. All file-output "
            "modes finish rendering, write and close a staged file in a sibling temporary directory, "
            "then atomically replace the report entry. Prepared-project run mode validates a manifest "
            "and sequentially launches old/new executables with matching cwd/environment and captured "
            "UTF-8 text streams. Optional capture directories preserve the manifest and publish each "
            "decoded stream by staged replacement before checking status and decoding the report; "
            "completed captures survive later failures. Temporary workspaces clean up on normal and "
            "error exits. A separate compute mode repeats decode/compare/render over preloaded text with "
            "black-boxed input and an output-length checksum. Full output and filesystem effects are "
            "checked against an independent oracle, typed CPython, and atomic-output adapters using "
            "unchanged upstream models/rendering on the shared domain. The separately built producer "
            "emits deterministic captured bytes for every engine's process benchmarks."
        ),
    )
