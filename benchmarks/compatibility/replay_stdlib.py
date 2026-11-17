"""Replay an intact CPython 3.14.5 unittest FILE at stock, Imre O0 and O3."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
from pathlib import Path

from scorecard import COHORT, HERE, ROOT, SCHEMA, parse_result, require

RAN = re.compile(r"^Ran (\d+) tests? in ", re.M)


def invoke(argv, env, timeout):
    try:
        p = subprocess.run([str(x) for x in argv], env=env, capture_output=True, timeout=timeout)
        return p.returncode, p.stdout, p.stderr, False
    except subprocess.TimeoutExpired as e:
        return None, e.stdout or b"", e.stderr or b"", True


def write_log(path, data): path.write_bytes(data)


def parse(stderr, module, complete=True):
    text = stderr.decode("utf-8", errors="replace"); results = []; occurrences = {}
    for line_num, line in enumerate(text.splitlines(), 1):
        result = parse_result(line, module)
        if not result: continue
        test_id, outcome = result
        occurrences[test_id] = occurrences.get(test_id, 0) + 1
        results.append((test_id, occurrences[test_id], outcome, line_num))
    if complete:
        totals = RAN.findall(text)
        require(len(totals) == 1 and int(totals[0]) == len(results), f"unparsed unittest output: reported {totals}, parsed {len(results)}")
    return results


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ("python", "cpython", "imre", "runtime", "clang", "manifest", "family"):
        p.add_argument("--" + flag, required=True)
    p.add_argument("--source", help="CPython repository-relative FILE; defaults to manifest module path")
    p.add_argument("--include-module", action="append", default=[])
    p.add_argument("--timeout", type=int, default=600)
    p.add_argument("--output", type=Path, help="JSONL path (default stdlib/FAMILY.jsonl)")
    a = p.parse_args()
    manifest = json.loads(Path(a.manifest).read_text()); module = manifest["module"]
    require(manifest["commit"] == COHORT["cpython_commit"], "manifest is not pinned CPython")
    source_path = a.source or "Lib/" + module.replace(".", "/") + ".py"
    require(source_path in manifest["files"], "source not in upstream manifest; use --source")
    source = Path(a.cpython).resolve() / source_path; data = source.read_bytes()
    blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
    require(blob == manifest["files"][source_path], "upstream source blob differs from manifest")
    version = subprocess.check_output([a.python, "-c", "import platform; print(platform.python_version())"], text=True).strip()
    require(version == COHORT["cpython_version"], f"need exact Python {COHORT['cpython_version']}, got {version}")
    full_version = subprocess.check_output([a.python, "-VV"], text=True).strip()
    evidence = HERE / "evidence" / a.family; evidence.mkdir(parents=True, exist_ok=True)
    output = a.output or HERE / "stdlib" / f"{a.family}.jsonl"
    cpython = Path(a.cpython).resolve()
    fixture_lib = cpython.parent / "imre-cpython-test-fixtures" / "Lib"
    if not (fixture_lib / "test").exists():
        fixture_lib.mkdir(parents=True, exist_ok=True)
        shutil.copytree(cpython / "Lib/test", fixture_lib / "test", symlinks=True)
    native_source = fixture_lib / source_path.removeprefix("Lib/")
    require(native_source.read_bytes() == data, "staged source differs from upstream")
    temp = cpython.parent / "imre-tmp"; temp.mkdir(exist_ok=True)
    env = os.environ.copy(); env.update(PYTHONPATH=str(cpython / "Lib"), PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(temp))
    native_env = env.copy(); native_env["PYTHONPATH"] = str(fixture_lib)
    def recorded_env(e): return {k: e[k] for k in ("PYTHONPATH", "PYTHONDONTWRITEBYTECODE", "TMPDIR")}
    run_env = recorded_env(env); native_run_env = recorded_env(native_env)
    stock_cmd = [a.python, "-B", str(source), "-v"]
    code, stdout, stderr, timed = invoke(stock_cmd, env, a.timeout)
    write_log(evidence / "stock.stdout", stdout); write_log(evidence / "stock.stderr", stderr)
    require(not timed, "stock timed out; no complete inventory")
    originals = parse(stderr, module)
    require(originals, "stock registered no originals")
    require(code in (0, 1) and (code != 0 or all(r[2] in {"pass", "skip"} for r in originals)), "stock exit status contradicts results")
    all_runs = {"stock": {"argv": stock_cmd, "environment": run_env, "exit_code": code, "version": version, "full_version": full_version}}
    observations = {"stock": {(i, o): (status, line) for i, o, status, line in originals}}
    for tier, opt in (("O0", "0"), ("O3", "3")):
        binary = evidence / f"{tier}.exe"
        build = [a.imre, "build", str(native_source), "--module-root", str(fixture_lib), "--no-cache", "--opt-level", opt, "--runtime", a.runtime, "--clang", a.clang, "-o", str(binary)]
        for name in a.include_module: build += ["--include-module", name]
        bc, bo, be, bt = invoke(build, native_env, a.timeout)
        write_log(evidence / f"{tier}.build.stdout", bo); write_log(evidence / f"{tier}.build.stderr", be)
        cmd = [str(binary), "-v"]
        meta = {"argv": cmd, "build_argv": build, "environment": native_run_env, "build_exit_code": bc}
        result = {}
        if bc != 0 or bt:
            meta["reason"] = "build timed out before test execution" if bt else "build failed before test execution (see build stderr)"
        else:
            rc, so, se, rt = invoke(cmd, native_env, a.timeout)
            write_log(evidence / f"{tier}.stdout", so); write_log(evidence / f"{tier}.stderr", se)
            has_summary = bool(RAN.search(se.decode("utf-8", errors="replace")))
            parsed = parse(se, module, complete=has_summary and not rt)
            require(rc in (0, 1, None), f"{tier}: unexpected process exit {rc}")
            require(rc != 0 or all(r[2] in {"pass", "skip"} for r in parsed), f"{tier}: exit status contradicts results")
            result = {(i, o): (status, line) for i, o, status, line in parsed}
            require(set(result) <= set(observations["stock"]), f"{tier}: IDs outside stock inventory")
            meta.update(exit_code=rc, timed_out=rt)
            if rt: meta["reason"] = "suite timed out before this original reported an outcome"
            elif not has_summary: meta["reason"] = "suite exited before reporting a unittest summary"
            elif len(result) != len(originals): meta["reason"] = "suite did not report this original"
        all_runs[tier] = meta; observations[tier] = result
        if binary.exists(): binary.unlink()
    rel = evidence.relative_to(ROOT).as_posix()
    records = []
    for test_id, occurrence, _, _ in originals:
        runs = {}
        for tier in ("stock", "O0", "O3"):
            meta = all_runs[tier]; found = observations[tier].get((test_id, occurrence))
            run = {"argv": meta["argv"], "environment": meta["environment"]}
            if tier == "stock": run.update(version=version, full_version=full_version)
            else: run["build_argv"] = meta["build_argv"]
            if found: run.update(outcome=found[0], evidence=f"{rel}/{tier}.stderr#L{found[1]}")
            else: run.update(outcome="not_run", reason=meta.get("reason", "no individual result"), evidence=f"{rel}/{tier}.build.stderr")
            runs[tier] = run
        records.append({"schema": SCHEMA, "kind": "stdlib", "family": a.family, "id": test_id, "occurrence": occurrence, "source": {"repo": "https://github.com/python/cpython", "commit": manifest["commit"], "path": source_path, "git_blob": blob, "sha256": hashlib.sha256(data).hexdigest()}, "imre_revision": COHORT["imre_revision"], "platform": {"os": platform.system().lower(), "arch": platform.machine(), "release": platform.release()}, "runs": runs, "omission_reason": None, "paired": all(runs[t]["outcome"] == "pass" for t in ("stock", "O0", "O3"))})
    output.parent.mkdir(parents=True, exist_ok=True); output.write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in records))
    (evidence / "run.json").write_text(json.dumps({"imre_revision": COHORT["imre_revision"], "source_blob": blob, "platform": records[0]["platform"], "runs": all_runs}, indent=2) + "\n")
    print(f"{len(records)} original receipts, {sum(r['paired'] for r in records)} paired: {output}")


if __name__ == "__main__": main()
