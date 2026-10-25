"""Attempt the host-driven Popen route for every eligible pinned subprocess ID."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from host_popen import HERE, verify
from suite_inventory import cases, load_suite


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=["0", "3"], required=True)
    args = parser.parse_args()
    manifest = verify()
    ref = json.loads((HERE / "reference-linux.json").read_text())["reference"]
    ids = [case.id() for case in cases(load_suite(manifest["module"]))]
    if len(ids) != len(set(ids)) or set(ids) != set(ref):
        raise ValueError("discovery differs from the complete pinned denominator")
    rows = {}
    with tempfile.TemporaryDirectory(prefix="imre-subprocess-audit-") as tmp:
        actors = Path(tmp, "actors")
        actors.mkdir()
        def attempt(name):
            reference = ref[name]
            if reference != "pass" and "resource 'cpu'" not in reference["skipped"]:
                return name, {"status": "reference-skip", "reason": reference["skipped"]}
            with tempfile.TemporaryDirectory(dir=tmp) as cwd:
                result = Path(cwd, "result.json")
                cmd = [sys.executable, "-B", HERE / "host_popen.py", "--imre", args.imre.resolve(),
                       "--runtime", args.runtime.resolve(), "--clang", args.clang.resolve(),
                       "--opt-level", args.opt_level, "--case", name, "--actors", actors,
                       "--result", result]
                # Some originals exhaust or replace descriptors, manipulate signals,
                # or can hang on a broken pipe implementation. Isolate and reap each.
                with tempfile.TemporaryFile() as log:
                    child = subprocess.Popen(cmd, cwd=cwd, stdout=log, stderr=log, start_new_session=True)
                    try:
                        child.wait(timeout=65)
                    except subprocess.TimeoutExpired:
                        return name, {"status": "failed", "reason": "isolated worker exceeded 65s"}
                    finally:
                        try:
                            os.killpg(child.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        child.wait()
                    if child.returncode or not result.exists():
                        log.seek(0)
                        tail = log.read().decode(errors="replace").strip().splitlines()[-1:]
                        return name, {"status": "failed", "reason": f"worker {child.returncode}: " + "".join(tail)}
                return name, json.loads(result.read_text())
        with ThreadPoolExecutor(max_workers=2) as pool:
            for name, row in pool.map(attempt, ids):
                rows[name] = row
                if len(rows) % 25 == 0:
                    print(f"{len(rows)} / {len(ids)}", flush=True)
    report = {"denominator": len(ids), "opt_level": args.opt_level, "rows": dict(sorted(rows.items()))}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
