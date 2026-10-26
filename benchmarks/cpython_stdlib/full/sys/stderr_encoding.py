"""Compare stderr metadata and bytes with the unchanged test_exit child sources."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

SHARED = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SHARED))
from suite_inventory import verify_files
from suite_native import Toolchain

CASE = "test.test_sys.SysModuleTest.test_exit"
FILES = {"Lib/test/test_sys.py": "0db0369500a7c7c965386bda5646046978fec3c7",
         "LICENSE": "20cf39097c68baa17cc566b64e76d34ebf034044"}
METADATA = """import sys
from sys import stderr as err
err.write("")
err.write(err.encoding + "|" + sys.__stderr__.errors + "|")
size = err.write("café €🦀")
sys.__stderr__.write("|" + str(size) + "\\n")
err.flush()
"""
REPEATED = """import sys
sys.stderr.write("")
sys.stderr.flush()
sys.stderr.write("one")
sys.stderr.write("two")
sys.stderr.flush()
"""
REPRESENTATION = """import sys
from sys import stderr as err
print(repr(err))
print(str(sys.__stderr__))
print("stream:", sys.stderr, err)
"""
SHADOWED_REPR = """import sys
def render(repr: str) -> str:
    return repr(sys.stderr)
print(render("local"))
"""


def run(command, env):
    child = subprocess.run(command, env=env, capture_output=True, timeout=45)
    return {"status": child.returncode, "stdout": child.stdout.hex(), "stderr": child.stderr.hex()}


def exit_children(source):
    cls, = [n for n in ast.parse(source.read_bytes()).body if isinstance(n, ast.ClassDef) and n.name == "SysModuleTest"]
    method, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == "test_exit"]
    for stmt in method.body:
        if isinstance(stmt, ast.Expr) and isinstance(call := stmt.value, ast.Call) and isinstance(call.func, ast.Name) and call.func.id == "check_exit_message":
            code, expected = [ast.literal_eval(arg) for arg in call.args]
            yield code, expected, {kw.arg: ast.literal_eval(kw.value) for kw in call.keywords}


def compare(toolchain, source):
    base = dict(os.environ, LC_ALL="C.UTF-8", LANG="C.UTF-8")
    base.pop("PYTHONIOENCODING", None)
    base.pop("PYTHONUTF8", None)
    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)

        def binary(program, name):
            path = root / name
            path.mkdir()
            return toolchain.build(program.encode(), path)

        def expect(label, code, command, setting, flags=()):
            env = base.copy()
            if setting is not None:
                env["PYTHONIOENCODING"] = setting
            env["IMRE_ENCODED_FLAGS"] = "\x1f".join(["--quiet", "--no-cache", "--runtime", str(toolchain.runtime),
                                                   "--clang", str(toolchain.clang), "--opt-level", str(toolchain.level)])
            native = run(command, env)
            stock = run([sys.executable, *flags, "-c", code], env)
            if native != stock:
                raise AssertionError((label, setting, stock, native))
            row = {"case": label, "setting": setting, "source_sha256": hashlib.sha256(code.encode()).hexdigest(), **native}
            rows.append(row)
            return row

        direct = binary(METADATA, "metadata")
        for setting in (None, "LATIN-1", "ascii:replace", "ascii:ignore", "ascii:unknown-policy",
                        "utf_8", "UTF-8-SIG", ":surrogateescape", "ASCII:", ":"):
            expect("stderr metadata and writes", METADATA, [direct], setting)
        repeated = binary(REPEATED, "signature")
        expect("one signature across writes and flushes", REPEATED, [repeated], "utf-8-sig")
        representation = binary(REPRESENTATION, "representation")
        for setting in (None, "LATIN-1", "US-ASCII:replace", "UTF_8_SIG"):
            expect("repr, str and positional stderr", REPRESENTATION, [representation], setting)
        shadow = root / "shadow.py"
        shadow.write_text(SHADOWED_REPR)
        stock = run([sys.executable, "-c", SHADOWED_REPR], base)
        rejected = subprocess.run([toolchain.imre, "check", shadow], capture_output=True, timeout=30)
        if (stock["status"] != 1 or b"TypeError: 'str' object is not callable" not in bytes.fromhex(stock["stderr"])
                or rejected.returncode == 0 or b"shadow.py:3:12:" not in rejected.stderr):
            raise AssertionError((stock, rejected))
        rows.append({"case": "shadowed repr remains an ordinary invalid call", "stage": "compile",
                     "source_sha256": hashlib.sha256(SHADOWED_REPR.encode()).hexdigest(),
                     "diagnostic": rejected.stderr.decode().replace(str(root), "[case]").strip()})
        isolated = 'import os\n' + METADATA + '\nsetting = os.getenv("PYTHONIOENCODING")\nif setting is not None:\n    err.write(setting)\n'
        expect("isolated command preserves environment and ignores codec", isolated,
               [toolchain.imre, "-I", "-c", isolated], "not-a-codec", ["-I"])
        for index, (code, expected, extra) in enumerate(exit_children(source)):
            if extra or expected == b"unflushed,message":
                row = expect(CASE + " child", code, [binary(code, f"exit{index}")], extra.get("PYTHONIOENCODING"))
                if row["status"] != 1 or not bytes.fromhex(row["stderr"]).startswith(expected):
                    raise AssertionError(row)
        if sum(row["case"] == CASE + " child" for row in rows) != 2:
            raise AssertionError("expected the original flushing and Latin-1 children")
        diagnostic = 'raise ValueError("hé€")'
        program = binary(diagnostic, "diagnostic")
        for setting in ("ascii:ignore", "latin1", "utf-8"):
            env = dict(base, PYTHONIOENCODING=setting)
            lines = []
            for command in ([program], [sys.executable, "-c", diagnostic]):
                outcome = run(command, env)
                if outcome["status"] != 1 or outcome["stdout"]:
                    raise AssertionError(outcome)
                lines.append(bytes.fromhex(outcome["stderr"]).splitlines()[-1])
            if lines[0] != lines[1] or not lines[0].startswith(b"ValueError: "):
                raise AssertionError(lines)
            rows.append({"case": "uncaught exception diagnostic (last line)", "setting": setting,
                         "status": 1, "line": lines[0].hex()})
        env = dict(base, PYTHONIOENCODING="no-such-sys-codec")
        for command in ([direct], [sys.executable, "-c", METADATA]):
            bad = run(command, env)
            if bad["status"] != 1 or bad["stdout"] or b"unknown encoding: no-such-sys-codec" not in bytes.fromhex(bad["stderr"]):
                raise AssertionError(bad)
        rows.append({"case": "unknown codec before script output", "setting": env["PYTHONIOENCODING"],
                     "status": 1, "stdout": "", "diagnostic_contains": "unknown encoding: no-such-sys-codec"})
    return rows


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("run with the pinned CPython 3.14.5")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    for root in (SHARED / "upstream", args.cpython_source):
        verify_files(root, FILES)
    source = args.cpython_source / "Lib/test/test_sys.py"
    report = {"cpython_commit": "5607950ef232dad16d75c0cf53101d9649d89115", "files": FILES,
              "upstream_id": CASE, "whole_methods_added": 0, "levels": {}}
    for level in (0, 3):
        tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
        report["levels"][str(level)] = compare(tools, source)
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
