"""Run the unedited frozen/source PEP 3147 methods with observable native calls."""
import ast
from collections import Counter
import hashlib
import os
from pathlib import Path, PurePath
import subprocess
import sys
import textwrap
import warnings

from suite_evidence import roles

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "upstream/Lib/test/test_importlib/test_util.py"
LINES = SOURCE.read_text().splitlines(keepends=True)
OWNER, = (n for n in ast.parse("".join(LINES)).body if isinstance(n, ast.ClassDef) and n.name == "PEP3147Tests")
METHODS = {m.name: m for m in OWNER.body if isinstance(m, ast.FunctionDef)}
BEARISH, = (n for n in ast.walk(METHODS["test_cache_from_source_override"]) if isinstance(n, ast.ClassDef) and n.name == "Bearish")
ERRORS = {cls.__name__: cls for cls in (DeprecationWarning, NotImplementedError, RuntimeError, TypeError, ValueError)}


def build_worker(lang, directory):
    directory.mkdir()
    source = (HERE / "cache_worker.py").read_text()
    original = textwrap.dedent("".join(LINES[BEARISH.lineno - 1:BEARISH.end_lineno]))
    return lang.build(source.replace("# UPSTREAM_BEARISH", original).encode(), directory)


class Utility:
    """Calls the native operation once with the live TestCase settings and cwd."""
    def __init__(self, binary):
        self.binary = binary
        self.calls = Counter()

    def _run(self, name, path, rest):
        def setting(v):
            if v is None:
                return ["none"]
            assert type(v) is str
            return ["str", v]
        # The unchanged deprecated-argument tests push an unscoped simplefilter.
        # Forward that fixture setting before native emission; native code decides its effect.
        first = warnings.filters[0] if warnings.filters else None
        policy = first[0] if first and first[1:] == (None, Warning, None, 0) else "native-default"
        kind = "pure" if isinstance(path, PurePath) else "str"
        assert kind == "pure" or type(path) is str
        fields = [*setting(sys.implementation.cache_tag), *setting(sys.pycache_prefix), policy, name, kind, str(path), *rest]
        assert all("\n" not in field for field in fields)
        run = subprocess.run([self.binary], input="\n".join(fields) + "\n", capture_output=True, text=True, timeout=15)
        if run.returncode or run.stderr or len(run.stdout.splitlines()) != 2:
            raise AssertionError((fields, run))
        status, payload = run.stdout.splitlines()
        self.calls[f"{name}:{kind}:{status}"] += 1
        value = ast.literal_eval(payload)
        if status == "ok":
            return value
        raise ERRORS[status](value)

    def cache_from_source(self, path, debug_override=None, *, optimization=None):
        kind, extra = "none", []
        if type(debug_override) is bool:
            kind, extra = "bool", ["true" if debug_override else "false"]
        elif type(debug_override) is list and all(type(v) is int for v in debug_override):
            kind, extra = "list", [str(len(debug_override)), *map(str, debug_override)]
        elif debug_override is not None:
            assert type(debug_override).__name__ == BEARISH.name and type(debug_override).__bool__.__code__.co_firstlineno == BEARISH.body[0].lineno
            kind = "bearish"
        if optimization is None:
            opt_kind, value = "none", ""
        elif type(optimization) is int:
            opt_kind, value = "int", str(optimization)
        else:
            assert type(optimization) is str
            opt_kind, value = "str", optimization
        return self._run("forward", path, [kind, opt_kind, value, *extra])

    def source_from_cache(self, path):
        return self._run("reverse", path, [])


def cache_method(binary, method, level, fault=None):
    node = METHODS[method]
    body = "".join(LINES[node.lineno - 1:node.end_lineno]).encode()

    def execute(case, original):
        util = Utility(binary)
        if fault == "result":
            util.cache_from_source = lambda *args, **kwargs: "a deliberately wrong result"
        elif fault == "exception":
            def wrong(*args, **kwargs):
                raise LookupError("a deliberately wrong exception")
            util.source_from_cache = wrong
        saved = case.util
        try:
            case.util = util
            original()
        finally:
            case.util = saved
        if not util.calls:
            raise AssertionError(f"{case.id()} did not invoke a native cache operation")
        return {
            "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="mixed"),
            "evidence": {
                "method_sha256": hashlib.sha256(body).hexdigest(), "opt_level":level,
                "operation": "The native worker constructs or rejects the cache path, including its path protocol and error behavior.",
                "fixture": "CPython runs the original frozen/source case, decorators, cwd and sys setting changes, warning filter scopes, and assertions. The worker receives the current tag, prefix, cwd, filter and typed arguments; PurePath and the verbatim Bearish truth method run natively.",
                "native_calls": dict(sorted(util.calls.items())),
            },
        }
    return execute


def check_environment(lang, root):
    # -I ignores the prefix for startup while the process environment still retains it.
    code = """import os
import sys
from importlib.util import cache_from_source
print(sys.pycache_prefix is None)
sys.implementation.cache_tag = "target"
print(cache_from_source("p.py"))
sys.pycache_prefix = None
print(cache_from_source("p.py"))
print(os.getenv("PYTHONPYCACHEPREFIX", ""))
"""
    flags = ["--runtime", str(lang.runtime.resolve()), "--clang", str(lang.clang.resolve()), "--opt-level", str(lang.level), "--no-cache"]
    env = dict(os.environ, PYTHONPYCACHEPREFIX=str(root / "prefix"), IMRE_ENCODED_FLAGS="\x1f".join(flags))
    def compare(options, source, environment):
        args = [*options, "-c", source]
        params = dict(cwd=root, env=environment, capture_output=True, timeout=45)
        ref = subprocess.run([sys.executable, *args], **params)
        actual = subprocess.run([lang.imre.resolve(), *args], **params)
        assert ref.returncode == actual.returncode == 0 and (actual.stdout, actual.stderr) == (ref.stdout, ref.stderr), (actual,ref)

    for options in (["-B"], ["-I", "-B"]):
        compare(options, code, env)
    # -I ignores PYTHONOPTIMIZE, while explicit cache options override it.
    source = (HERE / "cache_command.py").read_text()
    for options, level in (([], ""), (["-O"], ""), (["-OO"], ""), ([], "4"), (["-I"], "4")):
        compare(options, source, dict(env, PYTHONOPTIMIZE=level))


def register(lang, root, level, executions):
    check_environment(lang, root)
    controls = root / "cache-controls"
    controls.mkdir()
    direct = HERE / "cache_controls.py"
    native = lang.build(direct.read_bytes(), controls)
    params = dict(input=b"selected-314\n/target\n", capture_output=True, timeout=15)
    reference = subprocess.run([sys.executable, "-I", "-S", direct], **params)
    actual = subprocess.run([native], **params)
    assert reference.returncode == actual.returncode == 0 and (actual.stdout, actual.stderr) == (reference.stdout, reference.stderr), (actual, reference)
    binary = build_worker(lang, root / "cache-paths")
    for method in METHODS:
        if method == "test_sep_altsep_and_sep_cache_from_source":
            continue  # The pinned decorator records both genuine POSIX skips in the full ledger.
        for variant in ("Frozen", "Source"):
            executions[f"test.test_importlib.test_util.{variant}_PEP3147Tests.{method}"] = cache_method(binary, method, level)
    from test.test_importlib import test_util
    for method, fault, expected in (("test_cache_from_source","result",AssertionError), ("test_source_from_cache_bad_path","exception",LookupError)):
        case = test_util.Frozen_PEP3147Tests(method)
        try:
            cache_method(binary,method,level,fault)(case, getattr(case,method))
        except expected:
            pass
        else:
            raise AssertionError("wrong product behavior must fail the original assertion")
