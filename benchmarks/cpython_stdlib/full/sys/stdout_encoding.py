"""Compare native stdout writes, locale settings and the original test_ioencoding children."""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from stderr_encoding import FILES, SHARED, run
from suite_inventory import verify_files
from suite_native import Toolchain

META = """import sys
from sys import stdout as out
out.write("")
print(out.encoding + "|" + sys.__stdout__.errors)
print(out.errors is out.errors)
"""
WRITES = """import sys
from sys import stdout as out
out.write("")
out.flush()
out.write("before|")
try:
    count = sys.__stdout__.write("café ¢€🦀")
    print(count)
except UnicodeEncodeError as exc:
    sys.stderr.write(str(exc) + "\\n")
except LookupError as exc:
    sys.stderr.write(str(exc) + "\\n")
out.write("after|")
print(2**82)
print(False)
print(1.5)
print(b"ok")
out.flush()
"""


def original_children(source):
    tree = ast.parse(source.read_bytes())
    cls, = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "SysModuleTest"]
    method, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == "test_ioencoding"]
    setting = None
    for n in method.body:
        if not isinstance(n, ast.Assign):
            continue
        if isinstance(n.targets[0], ast.Subscript) and ast.literal_eval(n.targets[0].slice) == "PYTHONIOENCODING":
            setting = ast.literal_eval(n.value)
        if isinstance(n.value, ast.Call) and ast.unparse(n.value.func) == "subprocess.Popen":
            yield setting, ast.literal_eval(n.value.args[0].elts[2])


def level_report(tools, source):
    base = dict(os.environ, LC_ALL="C.UTF-8", LC_CTYPE="C.UTF-8", LANG="C.UTF-8")
    for key in ("PYTHONIOENCODING", "PYTHONUTF8", "PYTHONCOERCECLOCALE"):
        base.pop(key, None)
    options = ["--quiet", "--no-cache", "--runtime", str(tools.runtime.resolve()),
               "--clang", str(tools.clang.resolve()), "--opt-level", str(tools.level)]
    base["IMRE_ENCODED_FLAGS"] = "\x1f".join(options)
    controls = []

    def compare(label, code, command, env, flags=()):
        stock = run([sys.executable, *flags, "-c", code], env)
        native = run(command, env)
        if native != stock:
            raise AssertionError((label, env.get("PYTHONIOENCODING"), stock, native))
        controls.append({"case": label, "setting": env.get("PYTHONIOENCODING"), "flags": list(flags),
                         "source_sha256": hashlib.sha256(code.encode()).hexdigest(), **native})

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for label, code, settings in (
            ("metadata", META, (None, "", ":", ":ignore", "utf-8", "ascii:", "latin1", "ascii:no-such", "utf-8-sig")),
            ("writes and failures", WRITES, (None, "ascii:replace", "ascii", "latin1", "ascii:unknown-policy",
                                              "ascii:backslashreplace", "utf-8-sig")),
        ):
            binary = tools.build(code.encode(), work)
            for setting in settings:
                env = base if setting is None else dict(base, PYTHONIOENCODING=setting)
                compare(label, code, [binary], env)
        for label, flags, locale, setting in (
            ("legacy POSIX locale", ["-X", "utf8=0"], "POSIX", None),
            ("force UTF-8 mode", ["-X", "utf8"], "C", None),
            ("isolated command", ["-I"], "C.UTF-8", "no-such-codec"),
            ("policy without codec", ["-X", "utf8=0"], "C", ":ignore"),
        ):
            env = dict(base, LC_ALL=locale, PYTHONCOERCECLOCALE="0")
            if setting is not None:
                env["PYTHONIOENCODING"] = setting
            compare(label, META, [tools.imre, *flags, "-c", META], env, flags)
    children = []
    for setting, code in original_children(source):
        env = dict(base, PYTHONIOENCODING=setting)
        observations = {}
        for name, exe in (("cpython", sys.executable), ("imre", tools.imre)):
            one = run([exe, "-c", code], env)
            lines = bytes.fromhex(one.pop("stderr")).splitlines()
            observations[name] = one | {"last_stderr_line": lines[-1].decode("ascii", "backslashreplace") if lines else ""}
        equal = observations["cpython"] == observations["imre"]
        if setting.startswith("ascii") and not equal:
            raise AssertionError((setting, observations))
        children.append({"setting": setting, "source": code, "source_sha256": hashlib.sha256(code.encode()).hexdigest(),
                         "same_status_stdout_and_last_diagnostic_line": equal, **observations})
    if len(children) != 5:
        raise AssertionError(children)
    return {"controls": controls, "original_children": children}


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("run with CPython 3.14.5")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    for root in (SHARED / "upstream", args.cpython_source):
        verify_files(root, FILES)
    source = args.cpython_source / "Lib/test/test_sys.py"
    report = {"upstream_id": "test.test_sys.SysModuleTest.test_ioencoding", "whole_methods_added": 0,
              "files": FILES, "levels": {}}
    for level in (0, 3):
        tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
        report["levels"][str(level)] = level_report(tools, source)
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
