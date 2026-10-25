"""Exercise the pinned whitespace expression, its class assignment, and the stock module."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile


HERE = Path(__file__).resolve().parent
STOCK = HERE.parents[1] / "textwrap/upstream/Lib/textwrap.py"
BLOB = "41366fbf443a4fc46768a6745d0b9e64c1d3b056"
EXPECTED = b"[(9, 32), (10, 32), (11, 32), (12, 32), (13, 32), (32, 32)]\n"


def programs(source):
    digest = hashlib.sha1(b"blob " + str(len(source)).encode() + b"\0" + source).hexdigest()
    if digest != BLOB:
        raise ValueError(f"expected pinned textwrap blob {BLOB}, got {digest}")
    nodes = ast.parse(source).body
    lines = source.splitlines(keepends=True)

    def assignment(body, name):
        return next(n for n in body if isinstance(n, ast.Assign)
                    and any(isinstance(t, ast.Name) and t.id == name for t in n.targets))

    def segment(node):
        return b"".join(lines[node.lineno - 1:node.end_lineno])

    cls = next(n for n in nodes if isinstance(n, ast.ClassDef) and n.name == "TextWrapper")
    whitespace = segment(assignment(nodes, "_whitespace"))
    field = assignment(cls.body, "unicode_whitespace_trans")
    expression = ast.get_source_segment(source.decode(), field.value).encode()
    show = b"print(list(TextWrapper.unicode_whitespace_trans.items()))\n"
    return {
        "expression": whitespace + b"\nprint(list((" + expression + b").items()))\n",
        "class-initializer": whitespace + b"\n" + lines[cls.lineno - 1] + segment(field) + b"\n" + show,
        "whole-module": b"from stock_textwrap import TextWrapper\n" + show,
    }


def native(args, root, entry, level, reachable):
    binary = root / "case"
    command = [args.imre, "build", entry, "--module-root", root, "--no-cache", "--runtime", args.runtime,
               "--clang", args.clang, "--opt-level", str(level), "-o", binary]
    if reachable:
        command.append("--reachable-only")
    built = subprocess.run(command, cwd=root, capture_output=True)
    if built.returncode:
        return {"compile_exit": built.returncode, "outcome": "compile-blocked",
                "diagnostic": built.stderr.decode().replace(str(root), "<work>")}
    result = subprocess.run([binary], cwd=root, capture_output=True)
    return {"compile_exit": 0, "exit": result.returncode, "stdout": result.stdout.decode(),
            "stderr": result.stderr.decode(), "outcome": "matched" if
            (result.returncode, result.stdout, result.stderr) == (0, EXPECTED, b"") else "failed"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--clang", type=Path)
    parser.add_argument("--result", required=True, type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use pinned CPython 3.14.5 for the reference")
    if any((args.imre, args.runtime, args.clang)) and not all((args.imre, args.runtime, args.clang)):
        parser.error("pass --imre, --runtime and --clang together")
    if args.imre:
        args.imre, args.runtime, args.clang = (arg.resolve() for arg in (args.imre, args.runtime, args.clang))

    source = STOCK.read_bytes()
    report = {"textwrap_git_blob": BLOB, "probes": {}}
    for name, program in programs(source).items():
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            entry = root / "entry.py"
            entry.write_bytes(program)
            if name == "whole-module":
                (root / "stock_textwrap.py").write_bytes(source)
            ref = subprocess.run([sys.executable, "-E", "-B", entry], cwd=root, capture_output=True, check=True)
            if (ref.stdout, ref.stderr) != (EXPECTED, b""):
                raise ValueError(f"{name}: CPython produced {ref.stdout!r}, {ref.stderr!r}")
            report["probes"][name] = {
                "program_sha256": hashlib.sha256(program).hexdigest(), "reference": ref.stdout.decode().strip(),
                "modes": {f"O{level}-reachable-{int(reachable)}": native(args, root, entry, level, reachable)
                          for level in (0, 3) for reachable in (False, True)} if args.imre else {},
            }
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
