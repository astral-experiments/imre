"""Compare two original Werkzeug access logs in a standalone native executable."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate


SOURCE_HASH = "411ca77d410e9e01009a6e2ace196e7db864d4afa7f74d50d4a4162b39ec68d5"
EXPECTED = (0, "True\nTrue\n1 True\nTrue\nTrue\n", "GET /health 200\nGET /missing 404\n")


def capture(command, cwd, env):
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, timeout=120)
    return result.returncode, result.stdout.decode(), result.stderr.decode()


def main():
    args = argparse.ArgumentParser(description=__doc__)
    for option in ("compiler", "runtime", "wheelhouse", "output"):
        args.add_argument(f"--{option}", type=Path, required=True)
    opts = args.parse_args()
    compiler, runtime = opts.compiler.resolve(), opts.runtime.resolve()
    env = {k: v for k, v in os.environ.items() if not k.startswith(("PYTHON", "IMRE"))}
    rows = {}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        site = work / "site"
        wheels = gate.stage(opts.wheelhouse.resolve(), site)
        source = site / "werkzeug/_internal.py"
        if gate.digest(source) != SOURCE_HASH:
            raise ValueError("expected the locked Werkzeug 3.1.8 _internal.py")
        # The neutral name isolates this consumer from earlier package initialization.
        shutil.copyfile(source, site / "wheel_werkzeug_internal.py")
        fixture = Path(__file__).with_name("werkzeug_access_log_consumer.py")
        entry = site / "access_log.py"
        shutil.copyfile(fixture, entry)
        reference = capture([sys.executable, "-ESB", entry], site, env)
        if reference != EXPECTED:
            raise RuntimeError(f"unexpected CPython reference {reference!r}")
        for opt in (0, 3):
            binary = work / f"access-O{opt}"
            llvm = binary.with_suffix(".ll")
            build = capture(
                [compiler, "build", entry, "--runtime-imports", "--module-root", site,
                 "--runtime", runtime, "--opt-level", str(opt), "--no-cache",
                 "--emit-llvm", llvm, "-o", binary],
                site, env,
            )
            row = rows[f"O{opt}"] = {"build": build, "matches_reference": False}
            if build[0] == 0:
                ir = llvm.read_bytes()
                if b"call ptr @imre_dyn_new" not in ir:
                    raise RuntimeError("the access log did not use the boxed execution path")
                run = capture([binary], work, env | {"PATH": "/nonexistent"})
                row.update(run=run, matches_reference=run == reference,
                           executable_sha256=gate.digest(binary), llvm_sha256=gate.digest(llvm))
    report = {"python": sys.version.split()[0], "wheels": wheels, "source_sha256": SOURCE_HASH,
              "fixture_sha256": gate.digest(fixture), "compiler_sha256": gate.digest(compiler),
              "runtime_sha256": gate.digest(runtime), "reference": reference, "native": rows}
    opts.output.write_text(json.dumps(report, indent=2) + "\n")
    return 0 if all(row["matches_reference"] for row in rows.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
