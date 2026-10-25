"""Imported modules with identical function names and lines must keep distinct groups."""
import subprocess
import sys


def check_modules(lang, root):
    module = """from typing import overload
@overload
def choose(value: int) -> int: ...
@overload
def choose() -> int: ...
def choose(value: int = 8) -> int: return value
"""
    story = """from typing import clear_overloads, get_overloads
import left
import right
def verify() -> None:
    a = get_overloads(left.choose)
    b = get_overloads(right.choose)
    print(len(a))
    print(len(b))
    print(a[0].__module__)
    print(b[0].__module__)
    print(a[0] is b[0])
    print(a[0] is get_overloads(left.choose)[0])
    clear_overloads()
    print(len(a))
    print(len(get_overloads(right.choose)))
verify()
"""
    project = root / "multiple_modules"
    project.mkdir()
    for name in ("left", "right"):
        (project / (name + ".py")).write_text(module)
    src = project / "report.py"
    src.write_text(story)
    expected = subprocess.run([sys.executable, "-E", src], cwd=project, capture_output=True)
    if expected.returncode:
        raise AssertionError(expected.stderr.decode())
    for options in ([], ["--reachable-only"]):
        exe = project / "report"
        build = subprocess.run([lang.imre, "build", src, "-q", "--module-root", project, *options,
                        "--runtime", lang.runtime, "--clang", lang.clang, "--opt-level", str(lang.level),
                        "-o", exe, "--no-cache"], cwd=project, capture_output=True)
        if build.returncode:
            raise AssertionError((options, build.stderr.decode()))
        result = subprocess.run([exe], cwd=project, capture_output=True)
        if (result.returncode, result.stdout, result.stderr) != (0, expected.stdout, expected.stderr):
            raise AssertionError((options, result, expected))


def check_rejections(lang, root):
    # Don't interpret a bound receiver as source metadata or guess an erased ABI.
    skeleton = "from typing import Callable, get_overloads, overload\n"
    samples = {
        "bound-receiver": ("class Example:\n    def work(self) -> None: pass\ndef execute() -> None:\n    overload(Example().work)\nexecute()\n", "plain source functions"),
        "different-callables": ("def first() -> None: pass\ndef second(x: int) -> int: return x\ndef execute() -> None:\n    overload(first)\n    overload(second)\n    f: Callable[[int], int] = get_overloads(first)[0]\n    print(f(5))\nexecute()\n", "one proven registered source signature"),
        "noncontiguous-definition": ("@overload\ndef f() -> int: ...\nprevious = f\ndef f() -> int: return 3\n", "cannot be rebound"),
        "invalid-expanded-arguments": ("def f() -> None: pass\ndef execute() -> None:\n    dummy = overload(f)\n    dummy(**3)\nexecute()\n", "unpacking"),
        "nested-invalid-argument": ("def f() -> None: pass\ndef inner(x: int) -> int: return x\ndef execute() -> None:\n    dummy = overload(f)\n    dummy(arg=inner(unknown=3))\nexecute()\n", "unknown"),
    }
    for name, (source, diagnostic) in samples.items():
        path = root / (name + ".py")
        path.write_text(skeleton + source)
        result = subprocess.run([lang.imre, "check", path], capture_output=True, text=True)
        if not result.returncode or diagnostic not in result.stderr:
            raise AssertionError((name, result.returncode, result.stderr))
