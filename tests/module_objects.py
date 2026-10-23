"""Check object reuse against clean split builds and whole-program code generation."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--compiler", required=True, type=Path)
    p.add_argument("--output", type=Path)
    args = p.parse_args()
    assert __debug__
    compiler = args.compiler.resolve()
    checks = {}
    with TemporaryDirectory(prefix="relar object checks ") as td:
        root = Path(td)
        project = root / "sources"
        project.mkdir()
        main = project / "main.py"
        leaf = project / "leaf.py"
        other = project / "unchanged.py"
        entry = "from leaf import amount\nfrom unchanged import twice\nprint(amount())\nprint(twice(5))\n"
        initial = "def amount() -> int:\n    return 7\n"
        main.write_text(entry)
        leaf.write_text(initial)
        other.write_text("def twice(x: int) -> int:\n    return x * 2\n")
        env = dict(os.environ, RELAR_CACHE_DIR=str(root / "cache"), RELAR_CACHE_TRACE="1")
        target = root / "program"

        def build(label, flags=(), path=target):
            r = subprocess.run([compiler, "build", "--quiet", main, *flags, "-o", path],
                               cwd=project, env=env, text=True, capture_output=True, check=True)
            assert not r.stdout
            events, units = [], {}
            for line in r.stderr.splitlines():
                family, action, *detail = line.removeprefix("relar ").split(" ")
                if family == "cache":
                    events.append(action)
                else:
                    assert family == "object" and action in ("hit", "miss") and len(detail) == 1, line
                    assert detail[0] not in units
                    units[detail[0]] = action
            checks[label] = {"events": events, "objects": units,
                             "binary_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            return events, units

        def output(path=target):
            return subprocess.run([path], text=True, capture_output=True, check=True).stdout

        events, units = build("initial")
        assert events == ["miss", "store"] and set(units) >= {"main", "leaf", "unchanged"}
        assert set(units.values()) == {"miss"} and output() == "7\n10\n"
        original = target.read_bytes()
        leaf.write_text(initial.replace("return 7", "return 8"))
        events, changed = build("body_edit")
        assert events == ["miss", "store"] and changed["leaf"] == "miss" and changed["unchanged"] == "hit"
        assert output() == "8\n10\n"

        leaf.write_text('def amount() -> str:\n    return "new ABI"\n')
        events, abi = build("callee_return_abi")
        assert events == ["miss", "store"] and abi["main"] == abi["leaf"] == "miss"
        assert output() == "new ABI\n10\n"
        clean = root / "clean"
        events, _ = build("clean_split", ["--no-cache"], clean)
        assert events == ["bypass"] and clean.read_bytes() == target.read_bytes()
        events, unsplit = build("whole_program", ["--whole-program"], clean)
        assert events == ["miss", "store"] and not unsplit and output(clean) == output()
        assert build("cached_whole_program", ["--whole-program"], clean) == (["hit"], {})
        assert build("cached_split") == (["hit"], {})

        leaf.write_text(initial)
        events, reverted = build("historical_objects_on_revert")
        assert events == ["miss", "store"] and set(reverted.values()) == {"hit"}
        assert target.read_bytes() == original

        main.write_text(entry + "# Source-only edit: every generated unit is the same.\n")
        events, comment = build("comment_only")
        assert events == ["miss", "store"] and set(comment.values()) == {"hit"}
        assert target.read_bytes() == original
        # Corrupt all cached object blobs, force the frontend to re-run, and
        # verify every required object is regenerated and the exact ELF is restored.
        objects = root / "cache/v1/objects"
        blobs = list(objects.glob("*.o"))
        assert blobs
        for f in blobs:
            f.write_bytes(b"damaged object")
        main.write_text(entry)
        events, corrupt = build("corrupt_objects")
        assert events == ["miss", "store"] and set(corrupt.values()) == {"miss"}
        assert target.read_bytes() == original and output() == "7\n10\n"
        events, low = build("different_optimization", ["--opt-level", "0"])
        assert events == ["miss", "store"] and set(low.values()) == {"miss"}
        assert output() == "7\n10\n"
    if args.output:
        args.output.write_text(json.dumps(checks, indent=2) + "\n")
    print(f"{len(checks)} cross-unit and cache checks passed")


if __name__ == "__main__":
    main()
