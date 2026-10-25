"""Check that name discovery cannot discard observable local class construction."""
from pathlib import Path
import subprocess
import sys
from textwrap import dedent

from suite_native import NativeCompileError


def verify(lang, root):
    folder = root / "loader_local_classes"
    folder.mkdir()
    source = Path(__file__).with_name("loader_local_classes.py").read_bytes()
    binary = lang.build(source, folder)
    expected = subprocess.run([sys.executable, "-EB", folder / "case.py"], cwd=folder,
                              capture_output=True, check=True, timeout=30)
    actual = subprocess.run([binary], cwd=folder, capture_output=True, check=True, timeout=30)
    if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
        raise AssertionError(f"local class discovery differs: {actual!r} != {expected!r}")

    cases = {
        "forward": ("""
            def inspect() -> None:
                print(Later.__name__)
                class Later: pass
            inspect()
        """, "", "UnboundLocalError", "local class is referenced before its definition"),
        "default": ("""
            def observe() -> int:
                print("default evaluated")
                return 3
            def inspect() -> None:
                class Case:
                    def method(self, argument=observe()): pass
                print(Case.__name__)
            inspect()
        """, "default evaluated\nCase\n", None, "local class body requires plain methods without defaults or inert class attributes"),
        "branch": ("""
            def inspect(flag: bool) -> None:
                if flag:
                    class Case: pass
                print("finished")
            inspect(True)
        """, "finished\n", None, "local class declarations must appear directly in the enclosing function's suite"),
        "rebind": ("""
            def inspect() -> None:
                class Case: pass
                Case = 7
                print(Case)
            inspect()
        """, "7\n", None, "rebinding a builtin, definition, or imported module is unsupported"),
    }
    for name, (source, stdout, error, diagnostic) in cases.items():
        folder = root / ("local-class-" + name)
        folder.mkdir()
        source = dedent(source).lstrip().encode()
        program = folder / "reference.py"
        program.write_bytes(source)
        oracle = subprocess.run([sys.executable, "-EB", program], capture_output=True, text=True, timeout=30)
        last_line = oracle.stderr.splitlines()[-1:] or [""]
        correct_exit = (oracle.returncode == 1 and last_line[0].startswith(error + ":")
                        if error else oracle.returncode == 0 and not oracle.stderr)
        if oracle.stdout != stdout or not correct_exit:
            raise AssertionError(f"{name}: unexpected CPython result {oracle!r}")
        try:
            lang.build(source, folder)
        except NativeCompileError as exc:
            if diagnostic not in str(exc):
                raise AssertionError(f"{name}: unexpected native diagnostic: {exc}") from exc
        else:
            raise AssertionError(f"{name}: native compilation erased the class boundary")
