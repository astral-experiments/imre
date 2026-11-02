"""Run the pinned EnvironTests class with its fixtures in fresh Linux processes."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
PINS = json.loads((HERE / "upstream.json").read_text(encoding="utf-8"))
PREFIX = "test.test_os.EnvironTests."
FAULT = PREFIX + "test_items"


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def git_blob(data):
    return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()


def link_or_copy(source, dest):
    try:
        os.link(source, dest)
    except OSError:
        shutil.copyfile(source, dest)
    return dest


def stage(work, cpython, layout, wrong):
    test = cpython / "Lib" / "test"
    for path, expected in PINS["test_blobs"].items():
        if git_blob((test / path).read_bytes()) != expected:
            raise ValueError(f"pinned blob mismatch for test/{path}")
    root = work / "stage"
    package = root / "test"
    package.mkdir(parents=True)
    sources = (test / name for name in ("__init__.py", "mapping_tests.py", "support"))
    if layout == "full":
        sources = test.iterdir()
    for source in sources:
        if source.name in ("__pycache__", "test_os.py") or source.suffix == ".pyc":
            continue
        dest = package / source.name
        if source.is_dir():
            shutil.copytree(source, dest, copy_function=link_or_copy,
                            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"), symlinks=True)
        else:
            link_or_copy(source, dest)
    source = (test / "test_os.py").read_bytes()
    if layout == "retained-class":
        names = {
            "import asyncio", "import os", "import subprocess", "import sys",
            "import unittest", "import warnings", "from test import support",
            "from test import mapping_tests", "from test.support import os_helper",
            "from test.support import unix_shell", "tearDownModule", "EnvironTests",
        }
        found = {}
        for node in ast.parse(source).body:
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                name = ast.unparse(node)
            else:
                name = getattr(node, "name", None)
            if name in names:
                if name in found:
                    raise ValueError(f"duplicate upstream declaration {name}")
                found[name] = range(node.lineno, node.end_lineno + 1)
        if set(found) != names:
            raise ValueError(f"missing upstream declarations: {names - found.keys()}")
        kept = {line for span in found.values() for line in span}
        source = b"".join(
            line if number in kept else b"\n"
            for number, line in enumerate(source.splitlines(keepends=True), 1)
        )
    if wrong:
        old = b"            self.assertEqual(os.environ.get(key), value)\n"
        if source.count(old) != 1:
            raise ValueError("negative-control assertion no longer unique")
        source = source.replace(old, b'            self.assertEqual(os.environ.get(key), value + "!")\n')
    (package / "test_os.py").write_bytes(source)
    shutil.copyfile(HERE / "entry.py", root / "entry.py")
    return root


def invoke(argv, cwd, env, timeout):
    cwd.mkdir()
    started = time.monotonic()
    command = list(map(str, argv))
    try:
        child = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    except OSError as error:
        return {"argv": command, "cwd": str(cwd), "exit": None, "spawn_error": str(error)}
    expired = False
    try:
        out, err = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        expired = True
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        out, err = child.communicate()
    return {"argv": command, "cwd": str(cwd), "exit": child.returncode,
            "timed_out": expired, "seconds": round(time.monotonic() - started, 3),
            "stdout": out.decode("utf-8", "backslashreplace"), "stderr": err.decode("utf-8", "backslashreplace")}


def fields(result, name):
    return [line[len(name) + 1:] for line in result.get("stdout", "").splitlines()
            if line.startswith(name + " ")]


def raw_environment(result, label):
    parts = fields(result, label)
    if len(parts) != 1:
        return None
    try:
        snapshot = ast.literal_eval(parts[0])
    except (SyntaxError, ValueError):
        return None
    if not isinstance(snapshot, bytes) or not snapshot.endswith(b"\0"):
        return None
    entries = snapshot[:-1].split(b"\0")
    if any(b"=" not in entry for entry in entries):
        return None
    return sorted(entries)


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--cpython-source", required=True, type=Path)
    cli.add_argument("--python", required=True, type=Path, help="pinned CPython 3.14.5 executable")
    cli.add_argument("--engine", required=True, choices=("stock", "ordinary", "boxed"))
    cli.add_argument("--layout", choices=("full", "retained-class"), default="full")
    cli.add_argument("--level", choices=("0", "3"))
    cli.add_argument("--imre", type=Path)
    cli.add_argument("--runtime", type=Path)
    cli.add_argument("--clang", type=Path)
    cli.add_argument("--work", required=True, type=Path, help="new run directory")
    cli.add_argument("--case", action="append", default=[], help="original case id or method name")
    cli.add_argument("--wrong", action="store_true", help="one faulty expectation in a separate copy of test_items")
    args = cli.parse_args()
    native = args.engine != "stock"
    if sys.platform != "linux" or not Path("/usr/bin/env").is_file():
        cli.error("requires Linux and /usr/bin/env -0")
    if native and not all([args.imre, args.runtime, args.clang, args.level]):
        cli.error("native runs require --imre, --runtime, --clang, and --level")
    ids = PINS["ids"]
    wanted = {name if name.startswith(PREFIX) else PREFIX + name for name in args.case}
    if wanted - set(ids):
        cli.error(f"unknown case IDs: {wanted - set(ids)}")
    if args.wrong and wanted - {FAULT}:
        cli.error("--wrong runs only test_items")
    selected = [FAULT] if args.wrong else [case for case in ids if not wanted or case in wanted]
    parent = dict(os.environb)
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=False)
    root = stage(work, args.cpython_source.resolve(), args.layout, args.wrong)
    python = args.python.resolve()
    version = subprocess.check_output([str(python), "-V"], text=True).strip()
    if version != "Python 3.14.5":
        raise ValueError(f"unexpected reference version: {version}")
    entry = root / "entry.py"
    results = {"source_commit": PINS["source"], "source_blobs": PINS["test_blobs"],
               "source_layout": args.layout, "engine": args.engine, "level": args.level,
               "wrong_test_items": args.wrong, "staged_test_os_sha256": sha256(root / "test" / "test_os.py"),
               "driver_sha256": sha256(entry), "stock_python_sha256": sha256(python),
               "ordered_reference_ids": ids, "rows": {}}
    report = work / "result.json"

    def save():
        temp = report.with_suffix(".tmp")
        temp.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
        temp.replace(report)

    launcher = python
    prefix = [python, "-B", entry]
    flags = []
    if native:
        launcher, archive, clang = (path.resolve() for path in (args.imre, args.runtime, args.clang))
        results["toolchain_sha256"] = {str(p): sha256(p) for p in (launcher, archive, clang)}
        flags = ["--no-cache", "--runtime", str(archive), "--clang", str(clang), "--opt-level", args.level]
        binary = work / "environ-tests"
        if args.engine == "boxed":
            flags.append("--runtime-imports")
        command = [launcher, "build", entry, "--module-root", root, *flags, "-o", binary]
        buildenv = dict(os.environ)
        for key in ("PYTHONPATH", "IMREFLAGS", "IMRE_ENCODED_FLAGS"):
            buildenv.pop(key, None)
        buildenv["TMPDIR"] = str(work)
        results["build"] = invoke(command, work / "build", buildenv, 600)
        save()
        if results["build"]["exit"] != 0:
            return 1
        prefix = [binary]
        results["elf_sha256"] = sha256(binary)
    home = work / "home"
    home.mkdir()
    # Test processes receive a known input environment. The caller's environment is never serialized.
    env = {"PATH": os.defpath, "HOME": str(home), "LC_ALL": "C.UTF-8", "TMPDIR": str(work),
           "PYTHONDONTWRITEBYTECODE": "1", "IMRE_ENVIRON_BASELINE": "original"}
    if native:
        env.update(IMRE_PYTHON=str(launcher), IMRE_ENCODED_FLAGS="\x1f".join(flags))
    discovery = invoke([*prefix, "--list"], work / "discovery", env, 120)
    results["discovery"] = discovery
    results["exact_order"] = fields(discovery, "ID") == ids and discovery["exit"] == 0
    save()
    if not results["exact_order"]:
        return 1
    for number, identifier in enumerate(selected):
        execution = invoke([*prefix, identifier], work / f"case-{number:02d}", env, 240)
        before = raw_environment(execution, "BEFORE")
        restored = (before is not None and b"IMRE_ENVIRON_BASELINE=original" in before
                    and before == raw_environment(execution, "AFTER"))
        parent_ok = dict(os.environb) == parent
        one = fields(execution, "RAN") == ["1"] and fields(execution, "ID") == ids
        executable_ok = fields(execution, "EXECUTABLE") == [str(launcher)]
        outcome = fields(execution, "OUTCOME")
        if args.wrong:
            expected = (execution["exit"] == 1 and outcome == ["FAIL"]
                        and "VALUE1!" in execution.get("stderr", ""))
        else:
            expected = execution["exit"] == 0 and outcome == ["PASS"]
        text_view = fields(execution, "MAPPING_environ")
        bytes_view = fields(execution, "MAPPING_environb")
        verified = (one and restored and parent_ok and expected
                    and text_view == ["same"] and bytes_view == ["same"])
        if identifier == PREFIX + "test_putenv_unsetenv":
            verified = verified and executable_ok
        results["rows"][identifier] = {
            "execution": execution, "fixture_process_environment_restored": restored,
            "text_mapping": text_view[0] if len(text_view) == 1 else "unobserved",
            "bytes_mapping": bytes_view[0] if len(bytes_view) == 1 else "unobserved",
            "parent_environment_unchanged": parent_ok, "expected_python_executable": executable_ok,
            "outcome": outcome[0] if len(outcome) == 1 else "UNOBSERVED", "verified": verified,
        }
        if outcome == ["SKIP"]:
            results["rows"][identifier]["skip_reasons"] = fields(execution, "SKIP_REASON")
        save()
        print(identifier, results["rows"][identifier]["outcome"], verified, flush=True)
        if not parent_ok:
            raise RuntimeError("parent environment changed")
    return int(not all(row["verified"] for row in results["rows"].values()))


if __name__ == "__main__":
    sys.exit(main())
