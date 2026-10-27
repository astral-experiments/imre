"""Evidence for unsuccessful and platform-specific test_sys occurrences."""
import ast
import hashlib
from pathlib import Path

from exit_case import exit_observation
import intern_case
from ioencoding_case import observe as ioencoding_observation
from locale_stream_cases import observe as locale_stream_observations
from suite_evidence import roles
from suite_inventory import load_suite
from suite_native import NativeCompileError, method_bytes, native_method, run_in_fixtures, testcase_fixture

MODULE = "test.test_sys"
UNOBSERVED = roles(body="unobserved", assertions="unobserved", operation="unobserved",
                   fixture="unobserved")

# Each condition is from the pinned test, including skips raised inside a body.
SKIPS = {
    "test__enablelegacywindowsfsencoding": (1264, 'not hasattr(sys, "_enablelegacywindowsfsencoding")',
                                            "Linux filesystem encoding, no legacy Windows codec"),
    "test_emscripten_info": (741, "not support.is_emscripten", "Linux, not Emscripten"),
    "test_getandroidapilevel": (1214, 'sys.platform != "android"', "Linux, not Android"),
    "test_getobjects": (1304, 'not hasattr(sys, "getobjects")', "release build without Py_TRACE_REFS"),
    "test_getwindowsversion": (404, 'not hasattr(sys, "getwindowsversion")', "Linux, no Windows version API"),
    "test_sys_getwindowsversion_no_instantiation": (881, 'not hasattr(sys, "getwindowsversion")',
                                                     "Linux, no Windows version API"),
    "test_ioencoding_nonascii": (937, "sys.getfilesystemencoding() != locale.getpreferredencoding(False)",
                                 "CPython 3.14.5, LC_CTYPE=C.UTF-8, UTF-8 mode 0; filesystem utf-8, preferred UTF-8"),
    "test_pystats": (1325, 'not hasattr(sys, "_stats_on")', "release build without Py_STATS"),
}


def reference_skips(reference, level):
    fixture = MODULE + ".SysModuleTest"
    result = {}
    for name, (line, condition, config) in SKIPS.items():
        test_id = fixture + "." + name
        result[test_id, 1, level] = {
            "kind": "upstream-skip", "fixture_variant": fixture, "platform": reference["platform"],
            "configuration": config, "condition": condition,
            "reason": reference["reference"][test_id]["skipped"], "source": f"Lib/test/test_sys.py:{line}",
        }
    return result


def testcase_import_blocker(child, entry, fixture):
    """Recognize only the child traceback for the generated top-level TestCase import."""
    if child.returncode != 1 or child.stdout:
        return None
    for line, statement in enumerate(fixture.splitlines(), 1):
        if statement == "import unittest" and child.stderr.splitlines() == [
            "Traceback (most recent call last):",
            f'  File "{entry}", line {line}',
            "ModuleNotFoundError: No module named 'unittest'",
        ]:
            return line
    return None


def blocked_methods(toolchain, root, source, reference, selected, module):
    """Compile complete definitions where possible. A plan or proposal cannot earn a pass."""
    upstream = ast.parse(source.read_bytes())
    imports = {}
    for node in upstream.body:
        if not isinstance(node, (ast.Import, ast.ImportFrom)):
            continue
        for alias in node.names:
            spelling = alias.name if alias.asname is None else f"{alias.name} as {alias.asname}"
            statement = (f"import {spelling}" if isinstance(node, ast.Import)
                         else f"from {node.module} import {spelling}")
            imports[alias.asname or alias.name.split(".")[0]] = statement
    methods = {(cls.name, node.name): node for cls in upstream.body if isinstance(cls, ast.ClassDef)
               for node in cls.body if isinstance(node, ast.FunctionDef)}
    notes = {}
    for line in (Path(__file__).parent / "assessments.tsv").read_text().splitlines():
        if line and not line.startswith("#"):
            key, state, reason = line.split("\t")
            notes[MODULE + "." + key] = state, reason

    results = {}
    for name, baseline in reference["reference"].items():
        if name in selected or isinstance(baseline, dict):
            continue
        key = (name, 1)
        state, reason = notes[name]
        cls, method = name.split(".")[-2:]
        node = methods[cls, method]
        folder = root / (cls + "." + method)
        folder.mkdir()
        if cls == "SysModuleTest" and method == "test_exit":
            results[key] = exit_observation(module, toolchain, folder, source)
            continue
        if cls == "SysModuleTest" and method == "test_ioencoding":
            lines = source.read_bytes().splitlines(keepends=True)
            method_sha256 = hashlib.sha256(b"".join(lines[node.lineno - 1:node.end_lineno])).hexdigest()
            results[key] = ioencoding_observation(module, toolchain, method_sha256)
            continue
        used = {n.id for n in ast.walk(node) if isinstance(n, ast.Name)}
        preamble = "\n".join(imports[n] for n in sorted(used & imports.keys()))
        fixture = preamble + "\n" + testcase_fixture()
        outcome, locations = "blocked", dict(UNOBSERVED)
        proof = {"source": f"Lib/test/test_sys.py:{node.lineno}", "opt_level": toolchain.level}
        child_errors = []
        try:
            compiled = native_method(toolchain, folder, source, cls, name, fixture,
                                     on_child_error=child_errors.append)
        except NativeCompileError as exc:
            proof.update(stage="compile", diagnostic=str(exc).replace(str(folder), "[case]").strip())
            if name == intern_case.CASE:
                observed = intern_case.observation(module, toolchain, folder, source)
                observed["evidence"]["whole_native_body"] = proof
                results[key] = observed
                continue
        except ValueError as exc:
            if not node.decorator_list:
                raise
            proof.update(stage="original method decorator", diagnostic=str(exc),
                         decorators=[ast.unparse(d) for d in node.decorator_list])
        else:
            # A stateful size fixture cannot be replaced by an empty NativeCase.
            if cls == "SizeofTest":
                proof.update(stage="native fixture", diagnostic="requires original SizeofTest instance state")
            else:
                observed, failures, skips = run_in_fixtures(load_suite(MODULE), {name: compiled})
                if observed:
                    raise AssertionError(f"{name}: newly executable complete method; identify its product operation")
                if not failures:
                    raise AssertionError((name, "unreconciled fixture skip", skips))
                proof["method_sha256"] = hashlib.sha256(method_bytes(source, cls, method)).hexdigest()
                line = None
                if len(failures) == len(child_errors) == 1 and not skips:
                    line = testcase_import_blocker(child_errors[0], folder.resolve() / "case.py", fixture)
                if line is not None:
                    locations["fixture"] = "cpython"
                    reason = f"Native fixture stopped at case.py:{line} on import unittest before entering the method."
                    child = child_errors[0]
                    proof.update(stage="native fixture import", child_status=child.returncode,
                                 diagnostic=child.stderr.replace(str(folder), "[case]").strip())
                else:
                    outcome = "error"
                    locations = roles(body="unknown", assertions="unknown", operation="unknown", fixture="cpython")
                    reason = "\n".join(f"{case_id}: {trace.splitlines()[-1]}" for case_id, trace in failures)
                    reason = reason.replace(str(folder), "[case]")
                    proof.update(stage="native execution under original fixture", diagnostic=reason)
        if state == "proposed":
            proof["exception_proposal"] = "unreviewed; still in the denominator"
        results[key] = {"outcome": outcome, "roles": locations, "reason": reason, "evidence": proof}
    locale_stream_observations(module, toolchain, source, results)
    return results
