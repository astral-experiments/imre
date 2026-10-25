"""Compile each still-unobserved OS-suite method; never execute an unreviewed fixture."""
import argparse
import ast
from contextlib import chdir
import dis
from functools import cache
import hashlib
import inspect
import json
from pathlib import Path
import re
import shutil
import sys
import tempfile
import types

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, testcase_fixture


@cache
def methods(path):
    source = path.read_bytes()
    lines = source.splitlines(keepends=True)
    result = {}
    for cls in ast.parse(source).body:
        if not isinstance(cls, ast.ClassDef):
            continue
        for fn in cls.body:
            if isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
                body = b"".join(lines[fn.lineno - 1:fn.end_lineno])
                result[f"{cls.name}.{fn.name}"] = (body, fn.lineno)
                if cls.name == "TestInvalidFD" and fn.name == "get_single":
                    child, = [n for n in fn.body if isinstance(n, ast.FunctionDef) and n.name == "helper"]
                    result["TestInvalidFD.get_single.<locals>.helper"] = (
                        b"".join(lines[child.lineno - 1:child.end_lineno]), child.lineno)
    return result


def globals_read(code):
    names = {i.argval for i in dis.get_instructions(code) if i.opname in {"LOAD_GLOBAL", "LOAD_NAME"}}
    for child in code.co_consts:
        if isinstance(child, types.CodeType):
            names.update(globals_read(child))
    return names


def imports_for(fn):
    # Ask the original method for the names it actually reads, including in its
    # nested code. Helpers and constants are imported from their real release
    # modules; none are evaluated on CPython as a stand-in for a product call.
    imports = []
    for name in sorted(globals_read(fn.__code__) & fn.__globals__.keys()):
        if name == "__file__":
            continue
        obj = fn.__globals__[name]
        if isinstance(obj, types.ModuleType):
            imports.append(f"import {obj.__name__} as {name}")
        else:
            module = getattr(obj, "__module__", None)
            symbol = getattr(obj, "__name__", None)
            if not module or not symbol or not symbol.isidentifier():
                module, symbol = fn.__module__, name
            imports.append(f"from {module} import {symbol} as {name}")
    return imports


def preflight(case, source_paths, tools, work):
    method = inspect.unwrap(getattr(type(case), case._testMethodName))
    code = method.__code__
    body_path = Path(code.co_filename)
    source = "Lib/test/" + body_path.name
    closure = {}
    if method.__qualname__ == "TestInvalidFD.get_single.<locals>.helper" and code.co_freevars == ("f",):
        name = method.__closure__[0].cell_contents
        if type(name) is not str or case._testMethodName != "test_" + name:
            raise ValueError(f"unexpected release descriptor closure: {case.id()}")
        closure = {"f": name}
    elif code.co_freevars or source not in source_paths:
        return {"stage": "fixture", "reason": "The release constructs this method with closure inputs: "
                + ", ".join(code.co_freevars), "definition": method.__qualname__}
    body, line = methods(HERE / "upstream" / source)[method.__qualname__]
    imports = imports_for(method)
    fixture = testcase_fixture()
    if closure:
        # A class suite may start at the release helper's original indentation.
        # Transfer the captured operation name after imports, before the class.
        imports_src, cls_src = fixture.split("class NativeCase", 1)
        fixture = imports_src + f"f = {closure['f']!r}\n\nclass NativeCase" + cls_src
    prefix = ("\n".join(imports) + "\n" + fixture + "\n").encode()
    entry = prefix + body
    entry += f"\ncase = NativeCase()\ncase.{code.co_name}()\n".encode()
    record = {"source": f"{source}:{line}", "method_sha256": hashlib.sha256(body).hexdigest()}
    if closure:
        record["original_closure"] = closure
    try:
        tools.build(entry, work)
    except NativeCompileError as error:
        diagnostic = str(error).replace(str(work) + "/", "").strip()
        diagnostic = diagnostic.removeprefix(f"Compiling case.py (-O{tools.level})\n")
        position = re.search(r"^case\.py:(\d+):(\d+):", diagnostic)
        if position:
            generated_line = int(position[1])
            body_index = generated_line - prefix.count(b"\n") - 1
            original = body.splitlines()
            if 0 <= body_index < len(original):
                record.update(diagnostic_source=f"{source}:{line + body_index}:{position[2]}",
                              statement=original[body_index].decode().strip())
            elif 0 < generated_line <= len(prefix.splitlines()):
                record["fixture_statement"] = prefix.splitlines()[generated_line - 1].decode().strip()
        return {**record, "stage": "compile", "reason": diagnostic}
    return {**record, "stage": "built", "reason": "Compiled only; original instance state and execution still need verification."}


def paired_preflights(paths, revision):
    if not re.fullmatch(r"[0-9a-f]{40}", revision or ""):
        raise ValueError("record the exact Imre commit used for these probes")
    by_level = {}
    compilers, pins = set(), set()
    for path in paths:
        data = json.loads(path.read_bytes())
        level = data["opt_level"]
        if level in by_level or type(level) is not int or level not in (0, 3) or data["tests_executed"] != 0:
            raise ValueError(f"invalid method preflight at {path}")
        compilers.add(data["compiler_sha256"])
        pins.add(data["cpython_commit"])
        by_level[level] = data["cases"]
    if by_level.keys() != {0, 3} or by_level[0].keys() != by_level[3].keys() or len(compilers) != 1 or len(pins) != 1:
        raise ValueError("preflights need the same compiler, release and occurrences at both levels")
    rows = {}
    for name, first in sorted(by_level[0].items()):
        second = by_level[3][name]
        rows[name] = ([dict(first, opt_levels=[0, 3])] if first == second
                      else [dict(first, opt_levels=[0]), dict(second, opt_levels=[3])])
    return {"cpython_commit": pins.pop(), "compiler_commit": revision, "compiler_sha256": compilers.pop(),
            "tests_executed": 0, "cases": rows}


@cache
def source_digests(path):
    return {line: hashlib.sha256(data).hexdigest() for data, line in methods(path).values()}


def preflight_observation(probes, name, level, manifest, roles):
    if probes["cpython_commit"] != manifest["commit"] or probes["tests_executed"] != 0:
        raise ValueError("wrong CPython revision or execution mode for method probes")
    record, = [r for r in probes["cases"][name] if level in r["opt_levels"]]
    source, start = record["source"].rsplit(":", 1)
    if source not in manifest["files"] or source_digests(HERE / "upstream" / source).get(int(start)) != record["method_sha256"]:
        raise ValueError(f"{name}: attempted method does not match the pinned bytes")
    proof = {k: v for k, v in record.items() if k not in {"opt_levels", "reason"}}
    proof.update(artifact="method-preflight-linux.json", opt_level=level,
                 compiler_commit=probes["compiler_commit"], compiler_sha256=probes["compiler_sha256"])
    if record["stage"] == "compile":
        return {"outcome": "blocked", "reason": record["reason"], "evidence": proof, "roles": roles}
    if record["stage"] != "built":
        raise ValueError(f"{name}: unjustified fixture outcome {record['stage']}")
    return {"outcome": "not-run", "evidence": dict(proof, detail=record["reason"]), "roles": roles}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use the test-enabled CPython 3.14.5")
    lib = args.cpython_source.resolve() / "Lib"
    sys.path.insert(0, str(lib))
    tools = Toolchain(*(p.resolve() for p in (args.imre, args.runtime, args.clang)), args.opt_level)
    reports = {}
    with tempfile.TemporaryDirectory() as temp, chdir(temp):
        work = Path(temp)
        # Make pinned test helpers visible to the native compiler. Product
        # imports still use Imre's own stdlib. No compiled candidate is launched.
        shutil.copytree(lib / "test", work / "test", ignore=shutil.ignore_patterns("__pycache__"))
        for suite in ("os", "posix", "genericpath"):
            manifest = json.loads((HERE / f"{suite}.json").read_bytes())
            verify_files(HERE / "upstream", manifest["files"])
            verify_interpreter_tests(manifest)
            evidence = json.loads((HERE / f"evidence-{suite}-linux.json").read_bytes())
            level, = [r for r in evidence if r["opt_level"] == tools.level]
            rows = level["records"]
            found = cases(load_suite(manifest["module"]))
            for case, row in zip(found, rows, strict=True):
                if case.id() != row["id"] or row["occurrence"] != 1:
                    raise ValueError(f"unexpected release discovery: {case.id()}")
                if row["classification"] not in {"reference-only", "blocked"} or "private_api" in row.get("evidence", {}):
                    continue
                reports[case.id()] = preflight(case, manifest["files"], tools, work)
                print(case.id(), reports[case.id()]["stage"], flush=True)
    with tools.imre.open("rb") as binary:
        compiler_sha = hashlib.file_digest(binary, "sha256").hexdigest()
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "compiler_sha256": compiler_sha,
                                     "opt_level": tools.level,
                                     "tests_executed": 0, "cases": reports}, indent=2) + "\n")


if __name__ == "__main__":
    main()
