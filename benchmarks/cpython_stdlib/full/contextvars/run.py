"""Run original ContextTest callbacks against a persistent native Context/Token store."""
import argparse
from collections import Counter
import hashlib
import inspect
import json
from pathlib import Path
import select
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes, run_evidence

CASES = (
    "test_context_copy_1", "test_context_getset_2", "test_context_getset_4",
    "test_context_run_6", "test_context_run_7",
    "test_token_contextmanager_with_default", "test_token_contextmanager_without_default",
    "test_token_contextmanager_on_exception", "test_token_contextmanager_reentrant",
    "test_token_contextmanager_multiple_c_set",
    "test_token_contextmanager_with_explicit_reset_the_same_token",
    "test_token_contextmanager_with_explicit_reset_another_token",
)


NATIVE = ("test_context_run_1", "test_context_typerrors_1")


class Calls:
    """Values, copies, entry, exit, and errors live in the native process."""
    def __init__(self, binary):
        self.child = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True)
        self.counts = Counter()

    def call(self, op, *args):
        self.counts[op] += 1
        fields = (op, *map(str, args))
        assert not any("\n" in part for part in fields), fields
        self.child.stdin.write("\n".join(fields) + "\n")
        self.child.stdin.flush()
        if not select.select([self.child.stdout], [], [], 12)[0]:
            raise TimeoutError(op)
        line = self.child.stdout.readline()
        if not line:
            raise AssertionError(self.child.stderr.read())
        line = line.removesuffix("\n")
        for error in (LookupError, KeyError, ValueError, RuntimeError):
            if line.startswith(error.__name__ + ":"):
                raise error(line.partition(":")[2])
        if line in ("entered", "left", "None"):
            return None if line == "None" else line
        return int(line)

    def close(self):
        if self.child.poll() is None:
            self.child.stdin.write("leave\n")
            self.child.stdin.flush()
        out, err = self.child.communicate(timeout=12)
        if out or err or self.child.returncode:
            raise AssertionError((out, err, self.child.returncode))

    def ContextVar(self, name, *, default="missing"):
        return Variable(self, self.call("var", name, default))

    def Context(self):
        return Context(self, self.call("ctx"))


class Handle:
    def __init__(self, calls, index):
        self.calls, self.index = calls, index

    def call(self, op, *args):
        return self.calls.call(op, self.index, *args)

    def leave(self, exc):
        if exc is None:
            assert self.calls.call("leave") == "left"
        elif type(exc) is ValueError:
            # Re-raise this original host callback error inside the native callback.
            try:
                self.calls.call("raise-value", str(exc))
            except ValueError as returned:
                assert str(returned) == str(exc)
            else:
                raise AssertionError("native Context suppressed the callback exception")
        else:
            self.calls.call("leave")


class Context(Handle):
    def run(self, body, *args, **kwargs):
        assert self.call("run") == "entered"
        try:
            result = body(*args, **kwargs)
        except BaseException as exc:
            self.leave(exc)
            raise
        self.leave(None)
        return result

    def get(self, variable):
        return self.call("ctx-get", variable.index)

    def __getitem__(self, variable):
        return self.call("item", variable.index)

    def copy(self):
        return Context(self.calls, self.call("copy"))


class Variable(Handle):
    def get(self):
        return self.call("get")

    def set(self, value):
        return Token(self.calls, self.call("set", value))

    def reset(self, token):
        self.call("reset", token.index)


class Token(Handle):
    def __enter__(self):
        assert self.call("with") == "entered"
        return self

    def __exit__(self, typ, exc, tb):
        self.leave(exc)


def exercise(binary, level):
    def run(case, original):
        mod = sys.modules[original.__module__]
        method = inspect.getsource(inspect.unwrap(original)).encode()
        bridge = Calls(binary)
        try:
            with patch.object(mod, "contextvars", bridge):
                original()
        finally:
            bridge.close()
        assert bridge.counts["var"] > 0 or bridge.counts["run"] > 0
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="mixed"),
                "evidence": {"operation": "Context, ContextVar and Token public methods",
                             "opt_level": level, "native_operations": dict(bridge.counts),
                             "method_sha256": hashlib.sha256(method).hexdigest(),
                             "fixture": "Original host method, decorators, callbacks and assertions; native objects and context switches in one process per method."}}
    return run


def native_case(tools, directory, name):
    # Both methods create their own fixtures and have no custom host hooks.
    method = method_bytes(HERE / "upstream/Lib/test/test_context.py", "ContextTest", name)
    source = b"import contextvars\nimport unittest\n\nclass Case(unittest.TestCase):\n" + method
    source += f"\nCase().{name}()\nprint('completed')\n".encode()
    program = tools.build(source, directory)
    def execute(case, original):
        result = subprocess.run([program], capture_output=True, timeout=10)
        if (result.returncode, result.stdout, result.stderr) != (0, b"completed\n", b""):
            raise AssertionError((name, result))
        return {"roles": roles(body="imre", assertions="imre", operation="imre", fixture="imre"),
                "evidence": {"operation": "Context errors and native unittest.assertRaisesRegex",
                             "fixture": "The byte-identical method creates its own native Context and assertion managers.",
                             "opt_level": tools.level, "method_sha256": hashlib.sha256(method).hexdigest()}}
    return execute


def bridge_source():
    # Native functions cannot recurse yet. The original selected fixtures enter at
    # most four nested contexts/with blocks; emit an acyclic transport stack.
    # Every level still crosses the public native Context.run or with Token.
    out = [(HERE / "bridge.py").read_text(),
           "def entered5() -> None:\n    raise AssertionError('unexpected bridge depth')\n"]
    for i in reversed(range(5)):
        out.append(f"""
def entered{i}() -> None:
    reply("entered")
    serve{i}()

def serve{i}() -> None:
    while True:
        action = input()
        if action == "leave":
            return
        if action == "raise-value":
            raise ValueError(input())
        try:
            if action == "run":
                contexts[int(input())].run(entered{i + 1})
                reply("left")
            elif action == "with":
                with tokens[int(input())]:
                    entered{i + 1}()
                reply("left")
            else:
                operation(action)
        except KeyError as exc:
            reply("KeyError:" + str(exc))
        except LookupError as exc:
            reply("LookupError:" + str(exc))
        except ValueError as exc:
            reply("ValueError:" + str(exc))
        except RuntimeError as exc:
            reply("RuntimeError:" + str(exc))
""")
    return ("\n".join(out) + "\nserve0()\n").encode()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + flag, required=True, type=Path)
    p.add_argument("--cpython-source", type=Path)
    args = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        p.error("use CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    spec = json.loads((HERE / "UPSTREAM.json").read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    verify_files(HERE / "upstream", spec["files"])
    verify_interpreter_tests(spec)
    reports = []
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            bridge = tools.build(bridge_source(), Path(tmp))
            cases = {spec["module"] + ".ContextTest." + name: exercise(bridge, level) for name in CASES}
            for name in NATIVE:
                work = Path(tmp) / name
                work.mkdir()
                cases[spec["module"] + ".ContextTest." + name] = native_case(tools, work, name)
            fixture = spec["module"] + ".HamtTest"
            skipped = {(fixture + ".test_hamt_stress", 1, level): {
                "kind": "upstream-skip", "platform": "linux", "fixture_variant": fixture,
                "configuration": "default CPython regrtest resources",
                "condition": "'cpu' not in test.support.use_resources",
                "reason": "resource 'cpu' is not enabled",
                "source": "Lib/test/test_context.py:799",
            }}
            report = run_evidence(load_suite(spec["module"]), cases, ref, level, exceptions=skipped)
            reports.append(report)
            print(f"O{level}: {report['counts']}", flush=True)
            for row in report["records"]:
                if row["outcome"] not in {"pass", "not-run"}:
                    print(row["id"], row.get("reason"), file=sys.stderr)
    write_reports(args.result, reports, ref)
    return int(any(r["counts"].get("host-driven-native-operation") != len(CASES) or
                   r["counts"].get("fully-native") != len(NATIVE) for r in reports))


if __name__ == "__main__":
    sys.exit(main())
