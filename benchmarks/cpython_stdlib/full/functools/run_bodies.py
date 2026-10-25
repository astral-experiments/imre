"""Attempt every pinned functools test body with Imre and the original host fixtures."""

import argparse
import ast
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import re
import sys
import tempfile
from textwrap import dedent

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures

SOURCE = HERE / "upstream/Lib/test/test_functools.py"
MODULE = "test.test_functools"

# These native fixtures compare the actual operands, using ordinary equality
# and unittest's failure formatting. The method source is copied unchanged.
EQUALITY = """    def assertEqual(self, first: OPERAND, second: OPERAND, msg: str | None = None) -> None:
        if first != second:
            raise AssertionError(unittest.TestCase()._formatMessage(msg, f"{first!r} != {second!r}"))
"""


def only_string_assertions(method):
    """Every use of self is an assertEqual call with an exact string expectation."""
    uses = {n for n in ast.walk(method) if isinstance(n, ast.Name) and n.id == "self"}
    assertions = {
        call.func.value for call in ast.walk(method) if isinstance(call, ast.Call)
        and isinstance(call.func, ast.Attribute) and call.func.attr == "assertEqual"
        and isinstance(call.func.value, ast.Name) and call.func.value.id == "self"
        and len(call.args) == 2 and not call.keywords
        and isinstance(call.args[1], ast.Constant) and type(call.args[1].value) is str
    }
    return bool(uses) and assertions == uses


def declared(nodes):
    """Index the bindings available to a method, including the accelerator-only declarations."""
    bindings = {}
    for node in nodes:
        if isinstance(node, (ast.FunctionDef, ast.ClassDef)):
            bindings[node.name] = node
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            for name in node.names:
                bindings.setdefault(name.asname or name.name.split(".")[0], node)
        elif isinstance(node, ast.Assign):
            for name in node.targets:
                if isinstance(name, ast.Name):
                    bindings[name.id] = node
        elif isinstance(node, ast.If):
            bindings.update(declared(node.body))
    return bindings


class Fixtures:
    def __init__(self):
        source = SOURCE.read_bytes()
        self.lines = source.decode("utf-8").splitlines(keepends=True)
        self.globals = declared(ast.parse(source).body)
        # This fixture comes from the same pinned interpreter as the host hooks.
        # Send only its original declaration to the native compiler.
        import test

        manifest = json.loads((HERE / "UPSTREAM.json").read_text())
        verify_interpreter_tests({"files": manifest["interpreter_fixtures"]})
        support = (Path(test.__file__).resolve().parent / "support" / "__init__.py").read_bytes()
        support_lines = support.decode("utf-8").splitlines(keepends=True)
        helper = declared(ast.parse(support).body)["EqualToForwardRef"]
        self.forward_ref_helper = "".join(support_lines[helper.lineno - 1:helper.end_lineno])

    def source(self, node):
        first = min([node.lineno, *(d.lineno for d in getattr(node, "decorator_list", ()))])
        return "".join(self.lines[first - 1:node.end_lineno])

    def for_case(self, case):
        cls = type(case)
        method = case._testMethodName
        owner = next(c for c in cls.__mro__ if method in vars(c))
        node = declared(self.globals[owner.__name__].body)[method]
        # All 325 Linux reference cases run without a custom setUp/tearDown.
        # The shared runner still invokes the original unittest hooks.
        class_members = {}
        for base in reversed(cls.__mro__):
            definition = self.globals.get(base.__name__)
            if isinstance(definition, ast.ClassDef):
                class_members.update(declared(definition.body))
        names, members = set(), set()
        helper_nodes = {}
        def collect(tree):
            for use in ast.walk(tree):
                if isinstance(use, ast.Name) and isinstance(use.ctx, ast.Load):
                    names.add(use.id)
                if (isinstance(use, ast.Attribute) and isinstance(use.value, ast.Name)
                        and use.value.id == "self"):
                    members.add(use.attr)

        collect(node)
        # Bind public operations to actual native functools, retaining subclass
        # fixtures where present. No CPython implementation is installed in a child.
        member_texts = {}
        while members - member_texts.keys():
            member = sorted(members - member_texts.keys())[0]
            member_texts[member] = ""
            if member == "module" and member in class_members:
                names.add("functools")
                member_texts[member] = "    module = functools\n"
            elif member in {"partial", "reduce", "cmp_to_key"} and member in class_members:
                names.add("functools")
                target = "functools." + member
                if member == "partial" and cls.__name__.endswith("Subclass"):
                    target = "CPartialSubclass" if "CSubclass" in cls.__name__ else "PyPartialSubclass"
                    names.add(target)
                member_texts[member] = f"    {member} = staticmethod({target})\n"
            elif member in class_members and member != method:
                helper = class_members[member]
                collect(helper)
                helper_nodes[helper.lineno] = self.source(helper)
        # A class-level fixture may construct an earlier nested class (e.g. A()).
        for name in list(names & class_members.keys()):
            helper = class_members[name]
            if isinstance(helper, ast.ClassDef):
                helper_nodes[helper.lineno] = self.source(helper)
                collect(helper)

        seen, declarations, external = set(), {}, []
        imports = {"import unittest"}
        while names - seen:
            name = sorted(names - seen)[0]
            seen.add(name)
            if name in {"functools", "py_functools", "c_functools"}:
                imports.add("import functools" if name == "functools" else f"import functools as {name}")
                continue
            if name == "EqualToForwardRef":
                imports.add("import annotationlib")
                external.append(self.forward_ref_helper)
                continue
            source_node = self.globals.get(name)
            if source_node is None or source_node is node:
                continue
            if isinstance(source_node, (ast.Import, ast.ImportFrom)):
                imports.add(self.source(source_node).strip())
            else:
                collect(source_node)
                # Conditional global declarations are dedented to module scope.
                declarations[source_node.lineno] = dedent(self.source(source_node))
        equality = ({"test_with_bound_and_unbound_methods": "str",
                     "test_get_annotations": "dict[str, type]",
                     "test_lru_cache_parameters": ""}.get(method))
        if equality is None and only_string_assertions(node):
            equality = "str"
        fixture = "\n".join(sorted(imports)) + "\n\n"
        fixture += "\n".join(external + [declarations[k] for k in sorted(declarations)])
        fixture += "\nclass NativeCase" + (":\n" if equality is not None else "(unittest.TestCase):\n")
        fixture += "".join(member_texts.values())
        fixture += "".join(helper_nodes[k] for k in sorted(helper_nodes))
        if equality is not None:
            fixture += EQUALITY.replace(": OPERAND", f": {equality}" if equality else "")
        return owner.__name__, node, fixture


def compile_case(toolchain, fixtures, case, root):
    name = case.id()
    directory = root / name
    directory.mkdir()
    owner, node, fixture = fixtures.for_case(case)
    try:
        execute = native_method(toolchain, directory, SOURCE, owner, name, fixture)
    except ValueError as error:
        # native_method preserves upstream decorators by requiring a dedicated adapter.
        return name, None, {"status": "blocked", "stage": "method decorator", "diagnostic": str(error)}
    except RuntimeError as error:
        output = str(error).replace(str(directory), "<case>")
        lines = [s.strip() for s in output.splitlines() if s.strip() and not s.lstrip().startswith("Compiling ")]
        diag = next((s for s in lines if "Imre:" in s or s.startswith("error[")), lines[0])
        match = re.search(r"(?:<case>/|\b)case\.py:(\d+):\d+", output)
        start = fixture.count("\n") + 2
        line = int(match.group(1)) if match else 0
        body = start <= line <= start + node.end_lineno - node.lineno
        result = {"status": "blocked", "stage": "method compilation" if body else "fixture compilation",
                  "diagnostic": diag}
        if body:
            result["upstream_line"] = node.lineno + line - start
        return name, None, result
    return name, execute, None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, required=True, type=Path)
    parser.add_argument("--compiler-commit", required=True, help="Git commit used to build the supplied Imre binary")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("fixture and discovery require CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE / "reference-linux.json").read_text())
    fixtures = Fixtures()
    suite = list(cases(load_suite(MODULE)))
    if len(suite) != reference["denominator"] or {c.id() for c in suite} != reference["reference"].keys():
        raise ValueError("the pinned reference inventory has changed")
    report = {"module": MODULE, "cpython_commit": manifest["commit"], "denominator": len(suite),
              "compiler_commit": args.compiler_commit,
              "optimization_levels": {}}
    failed = False
    for level in (0, 3):
        toolchain = Toolchain(args.imre, args.runtime, args.clang, level)
        executions, results = {}, {}
        with tempfile.TemporaryDirectory() as temp, ThreadPoolExecutor(max_workers=2) as pool:
            for name, execute, result in pool.map(lambda c: compile_case(toolchain, fixtures, c, Path(temp)), suite):
                if execute:
                    executions[name] = execute
                else:
                    results[name] = result
            evidence, failures, skips = run_in_fixtures(load_suite(MODULE), executions)
            if skips:
                raise ValueError(f"unexplained reference skips: {skips}")
            for name, entries in evidence.items():
                if entries:
                    entry, = entries
                    results[name] = {"status": "native_body_host_fixture", **entry}
            for name, diagnostic in failures:
                results[name] = {"status": "failed", "diagnostic": diagnostic.strip().splitlines()[-1]}
                failed = True
        if results.keys() != reference["reference"].keys():
            raise RuntimeError("missing native-body attempts")
        report["optimization_levels"][str(level)] = dict(sorted(results.items()))
        print(f"-O{level}: {Counter(row['status'] for row in results.values())}", flush=True)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
