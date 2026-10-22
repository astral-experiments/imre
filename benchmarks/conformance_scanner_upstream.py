"""Feed Ruff's original pattern through the same observable capture workload."""

import re
import runpy
from pathlib import Path

from conformance_report import parse
from conformance_report.original import load_upstream

upstream = load_upstream()


def scan_annotation(line):
    if match := re.search(upstream.CONFORMANCE_ERROR_PATTERN, line):
        return parse.Annotation(
            match.start(), match.end(), match.group("optional") is not None,
            match.group("tag"), match.group("multi") is not None,
            match.group("description") or "Missing",
        )
    return None


parse.scan_annotation = scan_annotation
runpy.run_path(str(Path(__file__).with_name("conformance_scanner.py")), run_name="__main__")
