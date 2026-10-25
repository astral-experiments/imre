"""Compare copymode when both paths are symlinks and Linux has no lchmod."""
import argparse
import json
import os
from pathlib import Path
import platform
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from copyfile import build
from lstat_fixture import observe

SOURCE = b"""import shutil
try:
    print(shutil.copymode('src-link', 'dst-link', follow_symlinks=False))
except OSError as error:
    print(repr(error))
    print(repr(error.filename))
"""


def run(argv, folder, remove):
    folder.mkdir()
    target = folder / "target"
    target.write_bytes(b"contents")
    target.chmod(0o640)
    src, dst = folder / "src-link", folder / "dst-link"
    src.symlink_to("target")
    dst.symlink_to("target")
    lookups = []

    def after_lookup(child, path, syscall, buffer, result):
        lookups.append([path, result])
        if path == "dst-link" and result == 0 and remove:
            src.unlink()

    output = observe(argv, ["src-link", "dst-link"], after_lookup, cwd=folder)
    assert lookups[:2] == [["src-link", 0], ["dst-link", 0]]
    assert src.is_symlink() != remove and dst.is_symlink()
    assert target.read_bytes() == b"contents" and target.stat().st_mode & 0o777 == 0o640
    return {"lookups": lookups, "output": output, "target_intact": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=lambda p: Path(p).resolve())
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        parser.error("run with the pinned CPython 3.14.5")
    if sys.platform != "linux" or platform.machine() not in ("x86_64", "aarch64") or hasattr(os, "lchmod"):
        parser.error("requires Linux without lchmod, on x86-64 or aarch64")
    report = {"optimization_levels": {}}
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        script = root / "probe.py"
        script.write_bytes(SOURCE)
        for level in (0, 3):
            folder = root / str(level)
            folder.mkdir()
            binary = build(args, folder, "native", SOURCE, level)
            cases = {}
            for removed in (False, True):
                name = "unlinked" if removed else "stable"
                ref = run([sys.executable, "-IB", script], folder / ("ref-" + name), removed)
                native = run([binary], folder / ("native-" + name), removed)
                assert ref["output"] == ["None"] and len(ref["lookups"]) == 2
                cases[name] = {"reference": ref, "native": native, "matches": native == ref}
            report["optimization_levels"][str(level)] = cases
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(not case["matches"] for cases in report["optimization_levels"].values()
                   for case in cases.values()))


if __name__ == "__main__":
    sys.exit(main())
