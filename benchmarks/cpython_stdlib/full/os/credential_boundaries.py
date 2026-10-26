"""Compare credential behavior that no upstream OS test observes directly."""
import hashlib
import subprocess
import sys

from suite_native import NativeCompileError

PROBES = {
    "implicit-exception-context": """import os

class Raises:
    def __index__(self) -> int:
        raise ValueError("from index")

try:
    os.setreuid(Raises(), -1)
except TypeError as exc:
    print(exc.__context__ is None, exc.__cause__ is None, exc.__suppress_context__)

try:
    raise ArithmeticError("active")
except ArithmeticError as active:
    try:
        os.setreuid(Raises(), -1)
    except TypeError as exc:
        print(exc.__context__ is active, exc.__cause__ is None, exc.__suppress_context__)
""",
    "deprecated-bool-index-result": """import os

class BoolIndex:
    def __index__(self) -> bool:
        return True

try:
    os.setregid(BoolIndex(), None)
except TypeError as error:
    print(str(error))
""",
}


def observe_boundaries(tools, root):
    observations = {}
    for name, text in PROBES.items():
        directory = root / name
        directory.mkdir()
        source = directory / "case.py"
        code = text.encode()
        source.write_bytes(code)

        def observe(argv):
            run = subprocess.run(argv, cwd=directory, start_new_session=True, capture_output=True,
                                 text=True, timeout=30)
            return {"exit": run.returncode, "stdout": run.stdout.splitlines(),
                    "stderr": run.stderr.replace(str(source), "case.py").splitlines()}

        reference = observe([sys.executable, "-IB", source])
        if reference["exit"]:
            raise AssertionError((name, reference))
        row = {"source_sha256": hashlib.sha256(code).hexdigest(), "reference": reference}
        try:
            binary = tools.build(code, directory)
        except NativeCompileError as exc:
            row.update(outcome="blocked", diagnostic=str(exc).replace(str(directory) + "/", "").strip().splitlines()[-1])
        else:
            native = observe([binary])
            row.update(outcome="pass" if native == reference else "failed", native=native)
        observations[name] = row
    return observations
