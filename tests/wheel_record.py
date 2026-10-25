"""Run PyPA installer's unchanged WheelFile.validate_record on real ZIP members."""
import argparse
import ast
import base64
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from zipfile import ZIP_DEFLATED, ZIP_STORED, ZipFile

EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "wheel_record"
UPSTREAM = EXAMPLE / "upstream"
DRIVER = EXAMPLE / "validate.py"
DIST = "demo-1.0.dist-info"
QUOTED = 'demo/comma,"quote".txt'


def verify_vendor():
    for name, expected in json.loads((EXAMPLE / "UPSTREAM.json").read_bytes())["sources"].items():
        data = (UPSTREAM / name.removeprefix("src/")).read_bytes()
        actual = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        if actual != expected:
            raise AssertionError(f"upstream file differs: {name}")


def make_wheel(path, compression, case):
    path.parent.mkdir(parents=True)
    members = {
        "demo/__init__.py": b"x = 1\n", QUOTED: "café".encode(),
        f"{DIST}/METADATA": b"Metadata-Version: 2.1\nName: demo\nVersion: 1.0\n",
        f"{DIST}/WHEEL": b"Wheel-Version: 1.0\nTag: py3-none-any\n",
    }
    record = f"{DIST}/RECORD"
    buf = io.StringIO(newline="")
    writer = csv.writer(buf)
    for name, data in members.items():
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
        if name == QUOTED and case == "missing-row":
            continue
        size = -1 if name == QUOTED and case == "invalid-size" else len(data)
        if name == QUOTED and case == "wrong-size":
            size += 1
        writer.writerow((name, "sha256=" + digest, size))
    writer.writerow((record, "", ""))
    members[record] = buf.getvalue().encode()
    if case == "tampered":
        members[QUOTED] = b"C" + members[QUOTED][1:]
    with ZipFile(path, "w", compression=compression) as out:
        for name, data in members.items():
            out.writestr(name, data)
    with ZipFile(path) as archive:
        assert archive.testzip() is None
    return path


def tamper_real(source, out):
    out.parent.mkdir(parents=True)
    with ZipFile(source) as original, ZipFile(out, "w") as changed:
        name = next(m.filename for m in original.infolist()
                    if m.filename.endswith(".py") and m.file_size)
        for m in original.infolist():
            data = original.read(m)
            if m.filename == name:
                data = bytes((data[0] ^ 1,)) + data[1:]
            changed.writestr(m, data)
    with ZipFile(out) as check:
        assert check.testzip() is None
    return name


def observe(command, path, env=None):
    run = subprocess.run([*command, path], cwd=path.parent, env=env,
                         capture_output=True, timeout=150)
    if run.returncode == 0:
        return run.returncode, run.stdout, run.stderr
    # CPython writes a traceback plus a qualified exception; Imre uses a native
    # stack. Keep the exact exception class, message and continuation lines.
    lines = run.stderr.splitlines(keepends=True)
    cls = b"_WheelFileValidationError:"
    for i, line in enumerate(lines):
        stripped = line.removeprefix(b"installer.sources.")
        if stripped.startswith(cls):
            return run.returncode, run.stdout, stripped + b"".join(lines[i + 1:])
    raise AssertionError((path, run.returncode, run.stderr))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-levels", nargs="+", default=("0", "3"))
    parser.add_argument("--real-wheel", type=Path)
    parser.add_argument("--reference-only", action="store_true")
    args = parser.parse_args()
    if not args.reference_only and not (args.compiler and args.runtime):
        parser.error("native checks require --compiler and --runtime")
    verify_vendor()
    env = dict(os.environ, PYTHONPATH=str(UPSTREAM), PYTHONNOUSERSITE="1")
    expected_issue = {"valid": "", "tampered": "didn't match RECORD", "wrong-size": "didn't match RECORD",
                      "missing-row": "is not mentioned in RECORD", "invalid-size": "invalid"}
    with tempfile.TemporaryDirectory(prefix="imre-wheel-record-") as tmp:
        root = Path(tmp)
        cases = []
        for fmt, compression in [("stored", ZIP_STORED), ("deflated", ZIP_DEFLATED)]:
            for case, issue in expected_issue.items():
                path = root / (fmt + "-" + case) / "demo-1.0-py3-none-any.whl"
                cases.append((make_wheel(path, compression, case), issue, QUOTED))
        if args.real_wheel:
            wheel = args.real_wheel.resolve()
            bad = root / "real-tampered" / wheel.name
            name = tamper_real(wheel, bad)
            cases += [(wheel, "", ""), (bad, "didn't match RECORD", name)]
        reference = []
        for path, issue, member in cases:
            actual = observe([args.python, "-S", "-B", DRIVER], path, env)
            if issue:
                assert actual[0] == 1 and not actual[1], (path, actual)
                issues = ast.literal_eval(actual[2].split(b": ", 1)[1].decode())
                assert any(issue in entry and member in entry for entry in issues), (path, actual)
            else:
                assert actual == (0, b"", b""), (path, actual)
            reference.append(actual)
        print(f"CPython: {len(cases)} unchanged PyPA validation outcomes")
        if args.reference_only:
            return
        # Imre requires the entry source and its imported package under the
        # same module root. Keep the vendored source tree free of local code.
        module_root = root / "src"
        shutil.copytree(UPSTREAM, module_root)
        entry = Path(shutil.copy2(DRIVER, module_root))
        for level in args.opt_levels:
            binary = root / ("validate-O" + level)
            subprocess.run([args.compiler.resolve(), "build", entry, "--module-root", module_root,
                            "--reachable-only", "--no-cache", "--quiet", "--runtime", args.runtime.resolve(),
                            "--clang", args.clang, "--opt-level", level, "-o", binary],
                           cwd=root, check=True, timeout=240)
            for (path, _, _), expected in zip(cases, reference, strict=True):
                actual = observe([binary], path)
                if actual != expected:
                    raise AssertionError((level, path, actual, expected))
            print(f"O{level}: {len(cases)} matched status, stdout, and complete exception messages")


if __name__ == "__main__":
    main()
