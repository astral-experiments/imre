"""Evidence for unsuccessful and platform-specific test_sys occurrences."""
import ast
import hashlib
from pathlib import Path
import subprocess
from types import SimpleNamespace
from unittest.mock import patch

from exit_case import exit_observation
from suite_evidence import roles
from suite_inventory import load_suite
from suite_native import NativeCompileError, native_method, run_in_fixtures, testcase_fixture

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


def argv_invocation(module, toolchain, folder):
    """Run the full original method and record the actual native command-line failure."""
    test_id = MODULE + ".SysModuleTest.test_orig_argv"

    def execute(case, original):
        with patch.object(module, "sys", SimpleNamespace(executable=str(toolchain.imre.resolve()))):
            try:
                original()
            except subprocess.CalledProcessError as exc:
                error = exc.stderr.splitlines()[0] if exc.stderr else "no diagnostic"
                return {"result": exc.returncode, "stdout": exc.stdout, "diagnostic": error, "arguments": exc.cmd[1:]}
        return {"result": 0}

    data, failures, skips = run_in_fixtures(load_suite(MODULE), {test_id: execute})
    if failures or skips:
        raise AssertionError((failures, skips))
    observation, = data[test_id]
    if not observation["result"]:
        raise AssertionError("native Python command invocation now succeeds; register its complete passing adapter")
    args = observation["arguments"]
    if args[:4] != ["-I", "-X", "utf8", "-c"] or observation["stdout"] or not observation["diagnostic"].startswith("Usage: imre "):
        raise AssertionError(("the launch failure changed; check which operations actually ran", observation))
    def reference_control(case, original):
        calls = []
        def capture(command, **kwargs):
            result = subprocess.run(command, **kwargs)
            calls.append((command, result))
            return result
        with patch.object(module, "subprocess", SimpleNamespace(run=capture)):
            original()
        (command, result), = calls
        argv, original_argv = map(ast.literal_eval, result.stdout.splitlines())
        if command[1:] != args or original_argv != command or result.stderr:
            raise AssertionError((command, result))
        return {"returncode": result.returncode, "argv": argv, "orig_argv": ["<cpython>", *original_argv[1:]]}

    checked, errors, skipped = run_in_fixtures(load_suite(MODULE), {test_id: reference_control})
    if errors or skipped:
        raise AssertionError((errors, skipped))
    reference, = checked[test_id]
    child_source = args[4].encode()
    folder.mkdir()
    binary = toolchain.build(child_source, folder)
    entry = str(folder / "case.py")
    compiler = str(toolchain.imre.resolve())
    runtime, clang = str(toolchain.runtime.resolve()), str(toolchain.clang.resolve())
    paths = {str(binary): "<native-child>", entry: "<child-source>", compiler: "<imre>",
             runtime: "<runtime>", clang: "<clang>"}
    flags = ["--runtime", runtime, "--clang", clang, "--opt-level", str(toolchain.level), "--quiet", "--no-cache"]
    controls = {}
    for mode, invocation, argv0 in (
        ("executable", [str(binary), *args[5:]], str(binary)),
        ("imre run", [compiler, "run", entry, *flags, "--", *args[5:]], entry),
    ):
        run = subprocess.run(invocation, capture_output=True, text=True, check=True, timeout=45)
        argv, orig_argv = map(ast.literal_eval, run.stdout.splitlines())
        if run.stderr or argv != [argv0, *args[5:]] or orig_argv != invocation:
            raise AssertionError((mode, argv, orig_argv, run.stderr))
        controls[mode] = {"returncode": run.returncode, "argv": [paths.get(x, x) for x in argv],
                          "orig_argv": [paths.get(x, x) for x in orig_argv]}
    return {"outcome": "error",
            "roles": roles(body="cpython", assertions="unobserved", operation="unobserved", fixture="cpython"),
            "reason": f"Imre exited {observation['result']} before executing the -c child: {observation['diagnostic']}",
            "evidence": {"stage": "CLI option dispatch, before the child source or its two sys reads",
                         "arguments": args, "stdout": observation["stdout"], "diagnostic": observation["diagnostic"],
                         "child_sha256": hashlib.sha256(child_source).hexdigest(), "native_calls": 0,
                         "stock_cpython_control": reference, "independent_launch_controls": controls,
                         "returncode": observation["result"], "opt_level": toolchain.level,
                         "fixture": "upstream CPython SysModuleTest lifecycle and subprocess.run(check=True)"}}


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
        if state == "failed" and name.endswith(".test_orig_argv"):
            results[key] = argv_invocation(module, toolchain, root / "argv-child")
            continue
        cls, method = name.split(".")[-2:]
        node = methods[cls, method]
        folder = root / (cls + "." + method)
        folder.mkdir()
        if cls == "SysModuleTest" and method == "test_exit":
            results[key] = exit_observation(module, toolchain, folder, source)
            continue
        used = {n.id for n in ast.walk(node) if isinstance(n, ast.Name)}
        preamble = "\n".join(imports[n] for n in sorted(used & imports.keys()))
        fixture = preamble + "\n" + testcase_fixture()
        outcome, locations = "blocked", dict(UNOBSERVED)
        proof = {"source": f"Lib/test/test_sys.py:{node.lineno}", "opt_level": toolchain.level}
        try:
            compiled = native_method(toolchain, folder, source, cls, name, fixture)
        except NativeCompileError as exc:
            proof.update(stage="compile", diagnostic=str(exc).replace(str(folder), "[case]").strip())
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
                outcome = "error"
                locations = roles(body="unknown", assertions="unknown", operation="unknown", fixture="cpython")
                reason = str(failures).replace(str(folder), "[case]")
                proof.update(stage="native execution under original fixture", diagnostic=reason)
        if state == "proposed":
            proof["exception_proposal"] = "unreviewed; still in the denominator"
        results[key] = {"outcome": outcome, "roles": locations, "reason": reason, "evidence": proof}
    return results
