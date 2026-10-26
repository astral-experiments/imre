"""Compare redirected native input with pinned CPython, including chunk boundaries."""
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
from suite_native import Toolchain

CASE = Path(__file__).with_name("stdin_case.py")
BOM = b"\xef\xbb\xbf"


def inputs():
    yield "empty", None, b""
    yield "blank lines and a final partial line", None, b"\n\r\nA\rB\nlast"
    yield "Latin-1", "latin1", b"caf\xe9\n\xff\n"
    yield "ASCII strict reads past the first line", "ascii", b"ok\n\xff\n"
    for handler in ("ignore", "replace", "backslashreplace", "unknown", "z"):
        yield "ASCII invalid byte with " + handler, "ascii:" + handler, b"one\xfftwo\n"
    yield "unknown handler is consulted lazily", "ascii:z", b"valid\n"
    yield "UTF-8 strict reads past the first line", "utf-8", b"ok\n\xff\n"
    yield "UTF-8 invalid continuation", "utf-8:backslashreplace", b"\xc3A\xff\n"
    yield "incomplete character at EOF", "utf-8", b"\xe2\x82"
    yield "BOM only", "utf-8-sig", BOM
    yield "only the initial BOM is removed", "utf-8-sig", BOM + b"first\n" + BOM + b"second\n"
    for prefix in (8189, 8190, 8191):
        yield f"four-byte character split at {8192-prefix}", "utf-8", b"a" * prefix + "🦀\n".encode()
    yield "BOM on the second raw read is data", "utf-8-sig", BOM + b"x" * 8188 + b"\n" + BOM + b"z\n"
    yield "all complete lines in the first chunk precede error", "utf-8", b"x" * 8191 + b"\n\xff"
    yield "failure discards the newly read suffix", "utf-8", b"ok\n\xff\xe2"
    yield "failure preserves the previous incomplete prefix", "utf-8", b"x" * 8191 + b"\xe2\xff\xc3"


def observe(command, env, data):
    # A file makes the 8192-byte read boundary independent of pipe scheduling.
    with tempfile.TemporaryFile() as raw:
        raw.write(data)
        raw.seek(0)
        result = subprocess.run(command, stdin=raw, capture_output=True, env=env, timeout=30)
    return {"returncode": result.returncode, "stdout": result.stdout.hex(), "stderr": result.stderr.hex()}


def level_report(tools):
    source = CASE.read_bytes()
    base = dict(os.environ, LC_ALL="C.UTF-8", LC_CTYPE="C.UTF-8", LANG="C.UTF-8")
    for key in ("PYTHONIOENCODING", "PYTHONUTF8", "PYTHONCOERCECLOCALE"):
        base.pop(key, None)
    base["IMRE_ENCODED_FLAGS"] = "\x1f".join((
        "--quiet", "--no-cache", "--runtime", str(tools.runtime.resolve()),
        "--clang", str(tools.clang.resolve()), "--opt-level", str(tools.level),
    ))
    controls = []
    with tempfile.TemporaryDirectory() as tmp:
        binary = tools.build(source, Path(tmp))
        for label, setting, data in inputs():
            env = base if setting is None else dict(base, PYTHONIOENCODING=setting)
            stock = observe([sys.executable, CASE], env, data)
            native = observe([binary], env, data)
            if stock["returncode"] or stock != native:
                raise AssertionError((label, stock, native))
            controls.append({"case": label, "setting": setting, "input_sha256": hashlib.sha256(data).hexdigest(), **native})

        # In the original locale helper a trailing -I belongs to the script.
        for locale, setting, flags, args in (
            ("C", None, ["-X", "utf8=0"], ["-I"]),
            ("POSIX", ":ignore", ["-X", "utf8=0"], []),
            ("C", "latin1:", ["-X", "utf8=0"], []),
            ("C.UTF-8", "ascii:ignore", ["-I", "-X", "utf8"], []),
        ):
            env = dict(base, LC_ALL=locale, PYTHONCOERCECLOCALE="0")
            if setting is not None:
                env["PYTHONIOENCODING"] = setting
            data = b"ab\r\n"
            command = [*flags, "-c", source.decode(), *args]
            stock = observe([sys.executable, *command], env, data)
            native = observe([tools.imre, *command], env, data)
            if stock["returncode"] or stock != native:
                raise AssertionError((locale, setting, flags, args, stock, native))
            controls.append({"case": "CLI startup", "locale": locale, "setting": setting,
                             "flags": flags, "script_arguments": args,
                             "input_sha256": hashlib.sha256(data).hexdigest(), **native})

        data = b"\xff\n"
        env = dict(base, PYTHONIOENCODING="utf-8:surrogateescape")
        stock = observe([sys.executable, CASE], env, data)
        native = observe([binary], env, data)
        if native != stock:
            assert bytes.fromhex(stock["stdout"]).endswith(b"1:56575\nEOFError: EOF when reading a line\nEOFError: EOF when reading a line\n")
            assert bytes.fromhex(native["stdout"]).endswith(
                b"NotImplementedError: native strings cannot represent surrogate-escaped input bytes\n"
                b"EOFError: EOF when reading a line\n"
            )
        return {"controls": controls, "surrogateescape": {"matches": native == stock, "cpython": stock, "imre": native}}


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("run with CPython 3.14.5")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    for root in (SHARED / "upstream", args.cpython_source):
        verify_files(root, FILES)
    report = {"source_sha256": hashlib.sha256(CASE.read_bytes()).hexdigest(),
              "whole_methods_added": 0, "levels": {}}
    for level in (0, 3):
        tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level)
        report["levels"][str(level)] = level_report(tools)
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
