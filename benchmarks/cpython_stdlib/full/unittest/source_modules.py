"""Run #578's module-value controls using the pinned source package."""
import shutil
import subprocess
import sys
from pathlib import Path

from suite_inventory import verify_files


FILES = {"test/__init__.py": "b93054b3ecf3a5af96f4772e7208e7a18b5dd4a4",
         "test/dis_module.py": "afbf600fdee22bbb87b2b4630c8e2ab2df2761f6"}


def verify(toolchain, root):
    import test

    source = Path(test.__file__).resolve().parent.parent
    verify_files(source, FILES)
    scope = root / "source_module_values"
    (scope / "test").mkdir(parents=True)
    for path in FILES:
        shutil.copyfile(source / path, scope / path)
    program = Path(__file__).with_name("module_reference_protocol.py").read_bytes()
    native = toolchain.build(program, scope)
    reference = subprocess.run([sys.executable, "-E", "-B", scope / "case.py"],
                               capture_output=True, timeout=30, check=True)
    observed = subprocess.run([native], capture_output=True, timeout=30, check=True)
    if (observed.stdout, observed.stderr) != (reference.stdout, reference.stderr):
        raise AssertionError(f"source module values differ: {observed!r} != {reference!r}")

    # A failed module initializer must prevent the subsequent attribute read.
    failure = root / "failed_source_module"
    failure.mkdir()
    (failure / "module_fails.py").write_text('print("initializing")\nraise ValueError("import failed")\n')
    native = toolchain.build(b'import module_fails\nprint(module_fails.__name__)\n', failure)
    expected = (1, b"initializing\n", [b"ValueError: import failed"])
    for command in ([sys.executable, "-E", "-B", failure / "case.py"], [native]):
        process = subprocess.run(command, capture_output=True, timeout=30)
        result = process.returncode, process.stdout, process.stderr.strip().splitlines()[-1:]
        if result != expected:
            raise AssertionError(f"failed module import: {result!r} != {expected!r}")
