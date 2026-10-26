"""Reproduce the collections ledger from explicit Imre artifacts and a pinned CPython."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--clang", type=Path, default=shutil.which("clang-18") or shutil.which("clang"))
    parser.add_argument("--compiler-revision", required=True)
    parser.add_argument("--compare", type=Path, help="optional independent four-mode report for previously verified methods")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or args.clang is None:
        parser.error("use test-enabled CPython 3.14.5 and supply clang")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    imre, runtime, clang = (p.resolve() for p in (args.imre, args.runtime, args.clang))
    revision, tree, *parents = subprocess.check_output(
        ["git", "-C", REPO, "show", "-s", "--format=%H %T %P", args.compiler_revision], text=True
    ).split()
    pin = json.loads((HERE / "UPSTREAM.json").read_bytes())["commit"]
    source = {"compiler_revision": revision, "tree": tree, "parents": parents, "cpython_commit": pin,
              "imre_sha256": digest(imre), "runtime_sha256": digest(runtime)}
    (out / "source.json").write_text(json.dumps(source, indent=2) + "\n")
    common = ["--imre", imre, "--runtime", runtime, "--clang", clang, "--compiler-revision", revision]

    def run(script, *arguments, expected=(0,), env=None):
        result = subprocess.run([sys.executable, "-B", script, *arguments], cwd=REPO, env=env)
        if result.returncode not in expected:
            raise subprocess.CalledProcessError(result.returncode, result.args)

    def level(n):
        for name in ("routines", "doctests", "counter_host", "defaultdict_factory_host", "deque_index_host", "sequence_predicates_host"):
            result = out / f"{name}-O{n}.json"
            run(HERE / (name + ".py"), *common, "--opt-level", str(n), "--result", result,
                expected=(0, 1) if name == "doctests" else (0,))
            raw = json.loads(result.read_bytes())
            assert raw["compiler_revision"] == revision and raw["opt_level"] == n
            if name == "doctests":
                assert all(row["status"] in {"compile-blocked", "native-body/host-output-check"}
                           for row in raw["tests"].values()), raw["tests"]
        run(HERE / "default_methods.py", "--imre", imre, "--runtime", runtime, "--source", out / "source.json",
            "--clang", clang, "--opt-level", str(n), "--result", out / f"native-default-O{n}.json")

    with ThreadPoolExecutor(max_workers=2) as workers:
        list(workers.map(level, (0, 3)))

    def levels(name):
        return [out / f"{name}-O{n}.json" for n in (0, 3)]

    run(HERE / "native_report.py", "--routines", *levels("routines"), "--doctests", *levels("doctests"),
        "--host", *levels("counter_host"), "--host", *levels("defaultdict_factory_host"),
        "--host", *levels("deque_index_host"), "--host", *levels("sequence_predicates_host"),
        "--result", out / "fixture-roles.json")
    run(HERE / "combine_defaults.py", out, "--result", out / "evidence-linux.json")
    compare = ["--compare", args.compare.resolve()] if args.compare else []
    run(HERE / "verify_defaults.py", *common, *compare, "--evidence", out / "evidence-linux.json",
        "--result", out / "verified-defaults.json")
    run(HERE / "fromkeys/run.py", *common, "--result", out / "fromkeys-original.json",
        env={**os.environ, "PYTHONPATH": str(HERE / "fromkeys")})
    for folder, report in ((HERE, "evidence-linux.json"), (HERE / "fromkeys", "evidence-fromkeys-linux.json")):
        run(HERE.parents[1] / "suite_evidence.py", folder / "reference-linux.json", out / report)
    assert digest(imre) == source["imre_sha256"] and digest(runtime) == source["runtime_sha256"]


if __name__ == "__main__":
    main()
