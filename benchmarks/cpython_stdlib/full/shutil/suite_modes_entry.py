"""Run fresh complete-method attempts and operation evidence for one shared suite_modes invocation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=lambda p: Path(p).resolve(), required=True)
    args = parser.parse_args()
    mode = os.environ.get("IMRE_SUITE_MODE")
    compiler = os.environ.get("IMRE_SUITE_COMPILER")
    if mode not in ("ordinary", "reachable") or not compiler:
        parser.error("run this entry through benchmarks/cpython_stdlib/suite_modes.py")
    tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip()
    attempts = args.result.with_name(f"method-attempts-{mode}.json")
    if attempts.exists():
        parser.error(f"refusing to reuse {attempts}")
    common = ["--imre", args.imre, "--runtime", args.runtime, "--clang", args.clang]
    env = {**os.environ, "PYTHONPATH": str(args.cpython_source / "Lib")}
    python = [sys.executable, "-B"]
    subprocess.run([*python, HERE / "attempt_methods.py", *common,
                    "--compiler-tree", tree, "--result", attempts], env=env, check=True)
    report = json.loads(attempts.read_bytes())
    # The shared launcher forwards to the real executable and records every build.
    # Keep both digests so a forwarding script is never reported as the compiler.
    assert report["compiler_sha256"] == sha256(args.imre)
    report["compiler_forwarder_sha256"] = report["compiler_sha256"]
    report["compiler_sha256"] = sha256(Path(compiler))
    report["compiler_mode"] = mode
    attempts.write_text(json.dumps(report, indent=2) + "\n")
    subprocess.run([*python, HERE / "evidence.py", *common, "--cpython-source", args.cpython_source,
                    "--method-attempts", attempts, "--result", args.result], env=env, check=True)


if __name__ == "__main__":
    main()
