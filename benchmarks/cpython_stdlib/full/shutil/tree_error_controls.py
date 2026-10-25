"""Check real copytree error entries, continued sibling copies, and validation order."""
import os
import subprocess
import sys


SOURCE = r'''import shutil
import sys

try:
    if sys.argv[1] == "bytes":
        shutil.copytree(b"src", b"dst")
    else:
        shutil.copytree("src", "dst")
except shutil.Error as exc:
    args = exc.args
    print(len(args))
    print(repr(exc.args))
    entries = args[0]
    print(repr(entries))
    print(str(exc))
    print(repr(exc))
    assert len(entries) == 2
    entries.pop()
    entries.pop()
    print(str(exc))
    print(repr(exc))
    print(repr(exc.args))

try:
    shutil.copytree("missing", "invalid\0destination")
except FileNotFoundError as exc:
    print(repr(exc.errno))
    print(repr(exc.filename))
try:
    shutil.copytree("src", "invalid\0destination")
except ValueError as exc:
    print(str(exc))
'''


def run(tools, work):
    work.mkdir()
    exe = tools.build(SOURCE.encode(), work)
    for kind in ("str", "bytes"):
        observed = []
        for label, command in (("reference", [sys.executable, "-IB", work / "case.py"]),
                               ("native", [exe])):
            folder = work / (kind + label)
            source = folder / "src"
            (source / "nested").mkdir(parents=True)
            os.mkfifo(source / "one")
            os.mkfifo(os.fsencode(source / "nested") + (b"/two-\xff" if kind == "bytes" else b"/two"))
            (source / "sibling").write_bytes(b"kept")
            completed = subprocess.run([*command, kind], cwd=folder, capture_output=True, check=True, timeout=30)
            assert (folder / "dst/sibling").read_bytes() == b"kept"
            observed.append((completed.stdout, completed.stderr))
        assert observed[0] == observed[1], (kind, observed)
