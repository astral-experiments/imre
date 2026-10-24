"""Compare Ruff's original memory report CLI with the native executable."""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "benchmarks"))
from memory_validate import equivalent_original, expected_projects, report

PACKAGE = ROOT / "examples/memory_report"
SOURCE = PACKAGE / "upstream/memory_report.py"
SENTINEL = b"existing report\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--source", type=Path, default=SOURCE)
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    if sys.flags.optimize:
        raise RuntimeError("validation requires Python assertions")
    source = args.source.resolve()
    if source == SOURCE:
        manifest = json.loads(source.with_name("UPSTREAM.json").read_bytes())
        assert (
            hashlib.sha256(source.read_bytes()).hexdigest()
            == manifest["files"][source.name]
        )
    commands = (
        [args.python, "-B", str(source)],
        [str(args.binary.resolve())],
    )
    env = {
        **os.environ,
        "PYTHONHASHSEED": "0",
        "COLUMNS": "80",
        "PYTHON_COLORS": "0",
        "NO_COLOR": "1",
    }
    count = 0
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        out = work / "report.md"
        link = work / "linked.md"
        link.symlink_to(out.name)

        def compare(flags, *, status=0, error=None, ties=None):
            nonlocal count
            outputs = []
            for command in commands:
                out.write_bytes(SENTINEL)
                p = subprocess.run(
                    [*command, *flags],
                    cwd=work,
                    env=env,
                    check=False,
                    capture_output=True,
                    timeout=30,
                )
                assert p.returncode == status, (command, flags, p.returncode, p.stderr)
                stderr = p.stderr
                if error:
                    # The native process prints the uncaught exception without CPython's stack frames.
                    message = stderr.rstrip(b"\n").splitlines()[-1]
                    kind, separator, detail = message.partition(b":")
                    assert separator and kind.rsplit(b".", 1)[-1] == error, (
                        command,
                        flags,
                        stderr,
                    )
                    stderr = error + separator + detail
                assert link.is_symlink(), (command, flags)
                if ties:
                    assert p.stdout.endswith(b"\n")
                    equivalent_original(p.stdout[:-1].decode(), ties)
                outputs.append((None if ties else p.stdout, stderr, out.read_bytes()))
            assert outputs[0] == outputs[1], (flags, outputs)
            stdout, stderr, contents = outputs[0]
            if status == 0 and "--output" in flags:
                target = flags[flags.index("--output") + 1]
                assert contents != SENTINEL and not stdout
                assert (
                    stderr
                    == f"Output written to {target}\n".encode() + contents + b"\n"
                )
            else:
                assert contents == SENTINEL
            count += 1

        for flags in (["--help"], ["compare", "--help"], ["run", "--help"]):
            compare(flags)
        for flags in ([], ["unknown"], ["compare", "--unknown"], ["compare", "--old"]):
            compare(flags, status=2)
        compare(["compare"], status=1)
        compare(["compare", "--old", "old.json", "--output", "report.md"], status=1)
        directories = ["compare", "--old-dir", "old", "--new-dir", "new"]
        (work / "old").mkdir()
        (work / "new").mkdir()
        compare(directories, status=1)
        data = PACKAGE / "sample"
        for stem in ("inventory", "ruff-scripts"):
            old, new = f"old/{stem}.json", f"new/{stem}.json"
            (work / old).write_bytes((data / f"{stem}-old.json").read_bytes())
            (work / new).write_bytes((data / f"{stem}.json").read_bytes())
            pair = ["compare", "--old", old, "--new", new]
            compare(pair)
            compare([*pair, "--output", "linked.md"])
        # The two real projects together expose a hash-order tie. Compare exact directory
        # bytes on this one-project input, and check all permitted ties explicitly below.
        (work / "new/inventory.json").rename(work / "new/unmatched.json")
        compare(directories)
        compare([*directories, "--output", "report.md"])

        before, after = work / "before.json", work / "project.json"
        flags = ["compare", "--old", before.name, "--new", after.name]
        before.write_text(json.dumps(report()))
        rows = [{"name": f"row {i}", "fields_bytes": 1} for i in range(16)]
        tied = report(10_001, structs=rows)
        after.write_text(json.dumps(tied))
        rendered, groups = expected_projects([("project", report(), tied)])
        compare(flags, ties={"rendered": rendered, "row_groups": groups})
        # Missing optional lists default to a fresh empty list; later duplicate JSON keys win.
        after.write_text(
            '{"total_bytes":0,"total_bytes":10000,"struct_metadata_bytes":0,'
            '"struct_fields_bytes":0,"memo_metadata_bytes":0,"memo_fields_bytes":0}'
        )
        compare(flags)
        # The compact report never indexes a row. Full comparison first reads its name.
        after.write_text(json.dumps(report(queries=[None])))
        compare(flags)
        after.write_text(json.dumps(report(10_001, queries=[None])))
        compare([*flags, "--output", "report.md"], status=1, error=b"TypeError")
        after.write_text('{"total_bytes":')
        compare([*flags, "--output", "report.md"], status=1, error=b"JSONDecodeError")
        after.write_text("{}")
        compare([*flags, "--output", "report.md"], status=1, error=b"KeyError")
    print(f"Ruff's memory reporter matched CPython on {count} compare and CLI cases")


if __name__ == "__main__":
    main()
