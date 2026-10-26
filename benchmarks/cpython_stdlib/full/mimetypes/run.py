"""Measure the unchanged test_mimetypes occurrences under their real Linux fixtures."""
import argparse
import ast
from contextlib import ExitStack
from dataclasses import replace
from hashlib import sha256
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent), str(HERE.parents[1])]
from bridge import worker
from stage_tests import stage
from suite_evidence import build_report, occurrence_keys, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, run_in_fixtures

SOURCE = HERE / "upstream/Lib/test/test_mimetypes.py"
LINES = SOURCE.read_bytes().splitlines(keepends=True)
TOP = ast.parse(b"".join(LINES)).body
HERMETIC = b"mimetypes.knownfiles = []\nmimetypes.inited = False\nmimetypes._default_mime_types()\n"
TUPLES = {"test_case_sensitivity", "test_default_data", "test_bytes_path", "test_url"}
STRINGS = {"test_added_types_are_used", "test_init_reinitializes"}
HOSTED = {"test_case_sensitivity", "test_default_data", "test_data_urls", "test_file_parsing", "test_non_standard_types",
          "test_filename_with_url_delimiters", "test_url", "test_encoding", "test_guess_file_type",
          "test_preferred_extension", "test_keywords_args_api", "test_guess_all_types", "test_guess_known_extensions", *STRINGS}
UNOBSERVED = dict.fromkeys(("body", "assertions", "operation", "fixture"), "unobserved")


def raw(node):
    start = min([node.lineno, *(d.lineno for d in getattr(node, "decorator_list", ()))])
    return b"".join(LINES[start - 1:node.end_lineno])


def imports(nodes):
    names = {n.id for node in nodes for n in ast.walk(node) if isinstance(n, ast.Name)}
    result = b"import unittest\nimport mimetypes\n"
    for node in TOP:
        if isinstance(node, (ast.Import, ast.ImportFrom)) and any((a.asname or a.name.split(".")[0]) in names
                                                                 for a in node.names):
            result += raw(node)
    return result


def original_nodes(case):
    owner = type(case).__name__
    cls = next(n for n in TOP if isinstance(n, ast.ClassDef) and n.name == owner)
    method = next(n for n in cls.body if getattr(n, "name", "") == case._testMethodName)
    return cls, method


def source_for(case, typed=False):
    cls, method = original_nodes(case)
    owner = cls.name
    setup = [n for n in cls.body if getattr(n, "name", "") == "setUp"]
    source = imports([method, *setup])
    body = raw(method)
    if typed:
        assert setup and method.name in TUPLES | STRINGS
        kind = "tuple[str | None, str | None]" if method.name in TUPLES else "str | None"
        source += b"import sys\n" + f"class {owner}:\n".encode() + b"    db: mimetypes.MimeTypes\n"
        source += b"    def __init__(self) -> None:\n        self.setUp()\n"
        assertion = "assertSequenceEqual" if method.name == "test_url" else "assertEqual"
        msg = ", msg: str = ''" if method.name == "test_url" else ""
        mismatch = ("application/imre-mismatch", "imre-encoding") if method.name in TUPLES else "application/imre-mismatch"
        source += (f"    def {assertion}(self, left: {kind}, right: {kind}{msg}) -> None:\n"
                   f"        if len(sys.argv) > 1:\n            right = {mismatch!r}\n"
                   "        if left != right:\n"
                   "            raise AssertionError('MIME result mismatch')\n").encode()
    else:
        source += LINES[cls.lineno - 1]
    source += b"".join(raw(n) for n in setup) + raw(method)
    source += b"\n" + HERMETIC
    source += (f"{owner}().{method.name}()\n".encode() if typed else b"unittest.main()\n")
    return source, sha256(body).hexdigest()


def diagnostic(exc, work):
    return str(exc).strip().replace(str(work) + "/", "").replace(str(work), ".")


def run_method(tool, case, work, typed=False):
    src, digest = source_for(case, typed)
    exe = tool.build(src, work)
    process = subprocess.run([exe, *([] if typed else ["-v"])], capture_output=True, text=True, timeout=30)
    if process.returncode:
        raise AssertionError((case.id(), process.stdout, process.stderr))
    if typed:
        negative = subprocess.run([exe, "force-failure"], capture_output=True, text=True, timeout=10)
        assert negative.returncode and "MIME result mismatch" in negative.stderr, case.id()
    else:
        assert case._testMethodName in process.stderr and " ... ok" in process.stderr and "Ran 1 test" in process.stderr, process.stderr
    return dict(outcome="pass", roles=roles(body="imre", assertions="imre", operation="imre", fixture="mixed"),
                evidence=dict(method_sha256=digest, operation="bundled mimetypes under the unchanged native method", assertions="typed native equality with failing control" if typed else "bundled unittest",
                              fixture="original CPython hooks with native hermetic tables and class setup"))


def hosted(case, original, exe, work):
    import mimetypes
    count = {}
    with worker(exe, "builtins") as request, ExitStack() as patches:
        def call(op, arg="", strict=True, *extra):
            count[op] = count.get(op, 0) + 1
            return request(op, arg, strict, *extra)

        class HeldExtensions(list):
            def append(self, value):
                native = call("append-held", value)
                super().append(value)
                assert self == native

        class Database:
            def __init__(self, filenames=(), strict=True):
                if filenames:
                    [filename] = filenames
                    call("db-new", filename, strict)
            def guess_type(self, url, strict=True):
                return call("db-url", url, strict)
            def guess_file_type(self, path, *, strict=True):
                return call("db-file", path, strict)
            def guess_extension(self, type, strict=True):
                return call("db-extension", type, strict)
            def guess_all_extensions(self, type, strict=True):
                return HeldExtensions(call("db-extensions", type, strict))
            def add_type(self, type, ext, strict=True):
                return call("db-add", type, strict, ext)
            def readfp(self, fp, strict=True):
                stream = work / "readfp.txt"
                stream.write_text(fp.read(), encoding="utf-8")
                return call("db-readfp", str(stream), strict)

        if case._testMethodName == "test_guess_known_extensions":
            patches.enter_context(patch.object(mimetypes, "types_map", call("types-map")))
        case.db = Database()
        patches.enter_context(patch.object(mimetypes, "MimeTypes", Database))
        for attr, func in {
            "guess_type": lambda url, strict=True: call("url", url, strict),
            "guess_file_type": lambda path, *, strict=True: call("file", path, strict),
            "guess_extension": lambda type, strict=True: call("extension", type, strict),
            "add_type": lambda type, ext, strict=True: call("add", type, strict, ext),
            "init": lambda: call("init"),
        }.items():
            patches.enter_context(patch.object(mimetypes, attr, func))
        original()
        assert count
    return dict(outcome="pass", roles=roles(body="cpython", assertions="cpython", operation="imre", fixture="mixed"),
                evidence=dict(method_sha256=sha256(raw(original_nodes(case)[1])).hexdigest(),
                              calls=count, operation="all MIME calls in the original method served by the native child", fixture="original unittest hooks and case, native hermetic database"))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "output"):
        p.add_argument("--" + arg, type=Path, required=True)
    args = p.parse_args()
    assert sys.platform.startswith("linux") and sys.version_info[:3] == (3, 14, 5) and sys.implementation.name == "cpython"
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    report, starts = [], []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        staged = root / "staged"
        stage(staged)
        for level in (0, 3):
            work = root / str(level)
            work.mkdir()
            tool = Toolchain(args.imre, args.runtime, args.clang, level)
            hooks = [n for n in TOP if getattr(n, "name", "") in ("setUpModule", "tearDownModule")]
            for label, src, base in [
                ("full suite", b"import unittest\nimport test.test_mimetypes as m\nunittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromModule(m))\n", staged / "Lib"),
                ("original module hooks", imports(hooks) + b"".join(raw(n) for n in hooks) + b"\nsetUpModule()\ntearDownModule()\n", None),
            ]:
                for reachable in (False, True):
                    try:
                        replace(tool, reachable_only=reachable, module_root=base).build(src, base or work)
                    except NativeCompileError as exc:
                        why = diagnostic(exc, root)
                        if ": Imre:" not in why:
                            raise AssertionError(f"{label} did not reach the source: {why}") from exc
                        starts.append(dict(stage=label, opt_level=level, reachable_only=reachable,
                                           diagnostic=why))
                    else:
                        raise AssertionError(f"{label} now builds; execute the original fixture")
            wdir = work / "worker"
            wdir.mkdir()
            exe = tool.build((HERE / "worker.py").read_bytes(), wdir)
            suite = load_suite(manifest["module"])
            discovered = list(cases(suite))
            ids = list(occurrence_keys(c.id() for c in discovered))
            callbacks, skips = {}, {}
            for case, key in zip(discovered, ids, strict=True):
                owner, name = type(case).__name__, case._testMethodName
                ref = reference["reference"][case.id()]
                if isinstance(ref, dict):
                    assert ref == {"skipped": "Windows only"} and owner == "Win32MimeTypesTestCase"
                    cls = next(n for n in TOP if getattr(n, "name", "") == owner)
                    decorator = cls.decorator_list[0]
                    skips[(*key, level)] = dict(kind="upstream-skip", condition=ast.unparse(decorator),
                        reason=ref["skipped"], source=f"Lib/test/test_mimetypes.py:{decorator.lineno}",
                        platform=reference["platform"], fixture_variant=owner, configuration="ordinary CPython 3.14.5 on Linux")
                    continue
                def execute(case, original, *, name=name):
                    failures = []
                    for typed in ([False, True] if name in TUPLES | STRINGS else [False]):
                        try:
                            result = run_method(replace(tool, module_root=staged / "Lib"), case, staged / "Lib", typed)
                        except NativeCompileError as exc:
                            failures.append(diagnostic(exc, staged / "Lib"))
                        else:
                            break
                    else:
                        if name not in HOSTED:
                            return dict(outcome="blocked", roles=UNOBSERVED,
                                        reason=failures[-1], evidence=dict(stage="original method"))
                        result = hosted(case, original, exe, work)
                    if failures:
                        result["evidence"]["original_staging"] = failures
                    return result
                callbacks[case.id()] = execute
            observed, errors, unexpected = run_in_fixtures(suite, callbacks)
            if errors or unexpected:
                raise AssertionError([(case, text.splitlines()[-1][:500]) for case, text in errors] + unexpected)
            outcomes = {k: observed[k[0]][k[1] - 1] for k in ids if k[0] in observed}
            order = [(case.id(), type(case).__name__) for case in discovered]
            data = build_report(reference, level, order, outcomes, exceptions=skips)
            report.append(data)
            print(level, data["counts"], flush=True)
    args.output.mkdir(exist_ok=True)
    write_reports(args.output / "evidence-linux.json", report, reference)
    (args.output / "startup-linux.json").write_text(json.dumps(starts, indent=2) + "\n")


if __name__ == "__main__":
    main()
