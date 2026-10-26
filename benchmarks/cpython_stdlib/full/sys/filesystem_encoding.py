"""Compare startup encodings and their live string identities with the pinned interpreter."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from stderr_encoding import FILES, SHARED
from suite_inventory import verify_files
from suite_native import NativeCompileError, Toolchain

CASE = Path(__file__).with_name("filesystem_encoding_case.py")
NESTED_CALL = b'''import sys

def word(value: str, /) -> str:
    return value

print(sys.getfilesystemencoding(word(value="inside")))
'''


def configurations():
    for locale in ("C", "POSIX", "C.UTF-8"):
        for mode in (None, "0", "1"):
            env = {"LC_ALL": locale, "PYTHONCOERCECLOCALE": "0"}
            if mode is not None:
                env["PYTHONUTF8"] = mode
            yield "executable", env, [], "utf-8"
    for setting, arg in (("latin1:ignore", "ascii"), ("utf-8:strict", "surrogateescape"),
                         (":ignore", "utf-8")):
        yield "executable", {"LC_ALL": "C", "PYTHONUTF8": "0", "PYTHONIOENCODING": setting}, [], arg
    yield "executable", {"LC_CTYPE": "C", "LANG": "C.UTF-8", "PYTHONUTF8": "0",
                         "PYTHONCOERCECLOCALE": "0"}, [], "ascii"
    yield "executable", {"LANG": "C", "PYTHONUTF8": "0"}, [], "utf-8"
    yield "executable", {"LC_ALL": "C.UTF-8", "LC_CTYPE": "C", "PYTHONUTF8": "0"}, [], "surrogateescape"
    for args, mode in (([], "0"), (["-X", "utf8=0"], "1"), (["-X", "utf8"], "0"),
                       (["-I"], "0"), (["-I", "-X", "utf8=0"], "1")):
        yield "command", {"LC_ALL": "C", "PYTHONUTF8": mode, "PYTHONIOENCODING": "latin1:ignore"}, args, "ascii"
    for mode, stdio in (("0", "ascii:ignore"), ("0", ""), ("1", "latin1")):
        yield "executable", {"LC_ALL": "en_US.ISO-8859-1", "PYTHONUTF8": mode,
                             "PYTHONIOENCODING": stdio}, [], "iso8859-1"


def output(command, env):
    res = subprocess.run(command, env=env, capture_output=True, timeout=30)
    return {"returncode": res.returncode, "stdout": res.stdout.decode("ascii"), "stderr": res.stderr.decode("ascii")}


def measure(tools, locales):
    source = CASE.read_bytes()
    clean = os.environ.copy()
    for key in ("LC_ALL", "LC_CTYPE", "LANG", "PYTHONUTF8", "PYTHONIOENCODING", "PYTHONCOERCECLOCALE"):
        clean.pop(key, None)
    clean["LOCPATH"] = str(locales)
    clean["IMRE_ENCODED_FLAGS"] = "\x1f".join(("--quiet", "--no-cache", "--runtime", str(tools.runtime.resolve()),
                                               "--clang", str(tools.clang.resolve()), "--opt-level", str(tools.level)))
    records = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        negative = root / "nested"
        negative.mkdir()
        try:
            tools.build(NESTED_CALL, negative)
        except NativeCompileError as error:
            column = NESTED_CALL.splitlines()[5].index(b"value") + 1
            expected = f'{negative}/case.py:6:{column}: Imre: parameter "value" is positional-only'
            if str(error).splitlines()[-1] != expected:
                raise AssertionError((expected, str(error))) from error
        else:
            raise AssertionError("the outer encoding query suppressed the nested argument error")
        binary = tools.build(source, root)
        for mode, changes, flags, arg in configurations():
            env = dict(clean, **changes)
            if mode == "command":
                args = [*flags, "-c", source.decode(), arg]
                reference, native = [sys.executable, *args], [tools.imre, *args]
            else:
                reference, native = [sys.executable, CASE, arg], [binary, arg]
            expected, actual = output(reference, env), output(native, env)
            if expected["returncode"] or expected["stderr"] or actual != expected:
                raise AssertionError((tools.level, mode, changes, flags, arg, expected, actual))
            if changes.get("LC_ALL") == "en_US.ISO-8859-1" and changes["PYTHONUTF8"] == "0":
                if expected["stdout"].splitlines()[1] != "iso8859-1":
                    raise AssertionError("the reference did not select the generated Latin-1 locale")
            records.append({"mode": mode, "environment": changes, "flags": flags, "argument": arg, **actual})
    return records


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("run with CPython 3.14.5")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    for root in (SHARED / "upstream", args.cpython_source):
        verify_files(root, FILES)
    with tempfile.TemporaryDirectory() as tmp:
        locales = Path(tmp)
        subprocess.run(["localedef", "--no-archive", "-i", "en_US", "-f", "ISO-8859-1",
                        locales / "en_US.ISO-8859-1"], check=True)
        reports = {str(level): measure(Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level), locales)
                   for level in (0, 3)}
    args.result.write_text(json.dumps({"source_sha256": hashlib.sha256(CASE.read_bytes()).hexdigest(),
                                      "additional_upstream_occurrences": 0, "levels": reports}, indent=2) + "\n")


if __name__ == "__main__":
    main()
