"""Run the complete pinned CSV module through unittest's native module loader."""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

from suite_evidence import build_report, occurrence_keys, reference_cases, roles, write_reports
from suite_inventory import verify_files

HERE = Path(__file__).resolve().parent
CPYTHON_ONLY = {
    "test.test_csv.MiscTestCase.test_disallow_instantiation",
    "test.test_csv.MiscTestCase.test_lazy_import",
}
FAULT = "test.test_csv.TestDialectExcel.test_single"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def stage(lib, pin):
    for directory in (HERE, HERE.parent / "shared"):
        manifest = json.loads((directory / "UPSTREAM.json").read_bytes())
        if manifest["commit"] != pin:
            raise ValueError(f"CPython pin mismatch in {directory}")
        files = {name: blob for name, blob in manifest["files"].items()
                 if name in {"Lib/test/test_csv.py", "Lib/test/__init__.py"} or name.startswith("Lib/test/support/")}
        verify_files(directory / "upstream", files)
        for name in files:
            dest = lib.parent / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(directory / "upstream" / name, dest)
    shutil.copyfile(HERE / "suite_loader.py", lib / "loader.py")


def check(command, cwd, expected, negative, timeout):
    run = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    ids = [name for name, _ in expected]
    records = re.findall(r"^test_\w+ \((test\.test_csv\.[\w.]+)\)(?:\n[^\n]*?)? \.\.\. ([^\n]+)$", run.stderr, re.M)
    skips = sum(status.startswith("skipped ") for _, status in expected)
    summary = f"FAILED (failures=1, skipped={skips})" if negative else f"OK (skipped={skips})"
    discovery = ["IMRE_DISCOVERY_ID " + name for name in ids] + [f"IMRE_DISCOVERY_COUNT {len(ids)}"]
    if (run.returncode != int(negative) or run.stdout.splitlines() != discovery or records != expected
            or run.stderr.splitlines()[-1:] != [summary]
            or re.findall(r"^Ran (\d+) tests? in ", run.stderr, re.M) != [str(len(ids))]
            or negative and ("AssertionError:" not in run.stderr or f"FAIL: test_single ({FAULT})" not in run.stderr)):
        raise ValueError(f"{command} exited {run.returncode}\n{run.stdout}{run.stderr}")
    return records, {"exit": run.returncode, "summary": summary, "stdout_sha256": hashlib.sha256(run.stdout.encode()).hexdigest(),
                     "stderr_sha256": hashlib.sha256(run.stderr.encode()).hexdigest()}


def run_suite(args, reference_skips):
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    if sys.platform != "linux" or sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("use Linux CPython 3.14.5")
    if args.jobs < 1 or args.timeout < 1:
        raise SystemExit("--jobs and --timeout must be positive")
    expected = []
    for (name, _), status in reference_cases(reference).items():
        expected.append((name, "ok" if status == "pass" else "skipped " + repr(status["skipped"])))
    if args.negative_control:
        expected = [(name, "FAIL" if name == FAULT else status) for name, status in expected]
    native = [(name, "skipped 'implementation detail specific to cpython'" if name in CPYTHON_ONLY else status)
              for name, status in expected]
    imre, runtime, clang = (path.resolve() for path in (args.imre, args.runtime, args.clang))
    env = {k: v for k, v in os.environ.items() if k not in {"IMREFLAGS", "IMRE_ENCODED_FLAGS"}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        lib = work / "Lib"
        stage(lib, reference["cpython_commit"])
        source = lib / "test/test_csv.py"
        entry = lib / "loader.py"
        proof = {"compiler_sha256": digest(imre), "runtime_sha256": digest(runtime),
                 "loader_sha256": digest(entry), "upstream_suite_sha256": digest(source)}
        if args.negative_control:
            old = b"self.readerAssertEqual('abc', [['abc']])"
            data = source.read_bytes()
            if data.count(old) != 1:
                raise ValueError("expected exactly one TestDialectExcel.test_single assertion")
            source.write_bytes(data.replace(old, b"self.readerAssertEqual('abc', [['def']])"))
            proof["mutant_suite_sha256"] = digest(source)
        _, proof["cpython"] = check([sys.executable, "-E", "-B", entry], work, expected, args.negative_control, args.timeout)

        def measure(level):
            directory = work / f"ordinary-O{level}"
            directory.mkdir()
            binary = directory / "suite"
            command = [imre, "build", entry, "--module-root", lib, "--no-cache", "--runtime", runtime,
                       "--clang", clang, "--opt-level", str(level), "-o", binary]
            print(f"Building CSV -O{level}", flush=True)
            subprocess.run(command, cwd=directory, env=env, check=True, timeout=args.timeout)
            records, receipt = check([binary], directory, native, args.negative_control, args.timeout)
            receipt["executable_sha256"] = digest(binary)
            print(f"CSV -O{level}: {len(records)} tests, {receipt['summary']}", flush=True)
            return records, receipt

        levels = [0] if args.negative_control else [0, 3]
        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            runs = dict(zip(levels, pool.map(measure, levels), strict=True))
    if args.negative_control:
        proof["target"] = FAULT
        proof["ordinary_O0"] = runs[0][1]
        args.result.write_text(json.dumps(proof, indent=2) + "\n")
        return 0
    reports = []
    for level in (0, 3):
        records, ordinary = runs[level]
        discovered = [(name, name.rsplit(".", 1)[0]) for name, _ in records]
        observations = {}
        for key, (_, status) in zip(occurrence_keys(name for name, _ in records), records, strict=True):
            if status == "ok":
                obs = {"outcome": "pass", "roles": roles(body="imre", assertions="imre", operation="imre", fixture="imre"),
                       "evidence": {"operation": "CSV operations in the original test body",
                                    "fixture": "Native unittest module loader, fixtures and runner"}}
            else:
                obs = {"outcome": "skip", "reason": status.removeprefix("skipped '").removesuffix("'"),
                       "roles": roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="imre")}
            observations[key] = obs
        report = build_report(reference, level, discovered, observations, exceptions=reference_skips(level))
        report["intact_suite"] = {**proof, "ordinary": ordinary}
        reports.append(report)
    write_reports(args.result, reports, reference)
    return 0
