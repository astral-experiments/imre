"""Run complete CPython itertools methods and account for every discovered ID."""

import argparse
import ast
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import doctest
from functools import cache
import hashlib
import itertools
import json
import os
from pathlib import Path
import subprocess
import symtable
import sys
import tempfile
import textwrap

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_evidence import ROLES, roles, write_reports, validate_pair
from suite_native import Toolchain, NativeCompileError, compile_blocker, native_method, run_evidence, testcase_fixture

PIN = HERE / "upstream/Lib/test/test_itertools.py"
PREFIX = "test.test_itertools."
# A passing build is credited only after reviewing the original method and
# helpers. The tuple lists the actual native operations, including builtin-only IDs.
REVIEWED = {
    "LengthTransparency.test_repeat": ("int", ["repeat", "operator.length_hint"]),
    "LengthTransparency.test_repeat_with_negative_times": ("int", ["repeat", "operator.length_hint"]),
    "SubclassWithKwargsTest.test_keywords_in_subclass": (None, [
        "repeat", "builtins.zip", "builtins.filter", "filterfalse", "chain", "builtins.map",
        "starmap", "islice", "takewhile", "dropwhile", "cycle", "compress",
        "iterator subclass construction, keyword rejection and overridden __init__/__new__",
    ]),
    "TestBasicOps.test_accumulate": (None, ["accumulate, numeric and string addition, binary functions and initial values"]),
    "TestBasicOps.test_batched": (None, ["batched, strict tails, iterator errors and argument validation"]),
    "TestBasicOps.test_combinations": (None, ["combinations, permutations, combinations_with_replacement and pure Python equivalents"]),
    "TestBasicOps.test_combinations_with_replacement": (None, ["combinations_with_replacement, combinations, groupby and product"]),
    "TestBasicOps.test_combinatorics": (None, ["product, permutations, combinations and combinations_with_replacement"]),
    "TestBasicOps.test_compress": (None, ["compress, chain and repeat"]),
    "TestBasicOps.test_dropwhile": (None, ["dropwhile"]),
    "TestBasicOps.test_filterfalse": (None, ["filterfalse, count and islice"]),
    "TestBasicOps.test_map": (None, ["builtins.map, count and islice"]),
    "TestBasicOps.test_permutations": (None, ["permutations, product and pure Python equivalents"]),
    "TestBasicOps.test_product": (None, ["product, repeated pools and both original reference generators"]),
    "TestBasicOps.test_repeat_with_negative_times": ("str", ["repeat", "repr"]),
    "TestBasicOps.test_starmap": (None, ["starmap"]),
    "TestBasicOps.test_takewhile": (None, ["takewhile"]),
    "TestBasicOps.test_tee_del_backward": ("int", ["repeat", "tee", "any", "del"]),
    "TestBasicOps.test_zip_longest_bad_iterable": (None, ["zip_longest propagates the original __iter__ exception"]),
    "TestBasicOps.test_ziplongest": (None, ["zip_longest, fill values and uneven iterables"]),
    "TestExamples.test_accumulate": ("list[int]", ["accumulate"]),
    "TestExamples.test_chain": ("str", ["chain"]),
    "TestExamples.test_chain_from_iterable": ("str", ["chain.from_iterable"]),
    "TestExamples.test_combinations_with_replacement": ("list[tuple[str, str]]", ["combinations_with_replacement"]),
    "TestExamples.test_compress": ("list[str]", ["compress"]),
    "TestExamples.test_count": ("list[int]", ["count", "islice"]),
    "TestExamples.test_cycle": ("list[str]", ["cycle", "islice"]),
    "TestExamples.test_dropwhile": ("list[int]", ["dropwhile"]),
    "TestExamples.test_filter": ("list[int]", ["builtins.filter"]),
    "TestExamples.test_filterfalse": ("list[int]", ["filterfalse"]),
    "TestExamples.test_islice": ("list[str]", ["islice"]),
    "TestExamples.test_repeat": ("list[int]", ["repeat"]),
    "TestExamples.test_takewhile": ("list[int]", ["takewhile"]),
    "TestExamples.test_zip": ("list[tuple[str, str]]", ["builtins.zip"]),
    "TestExamples.test_zip_longest": ("list[tuple[str, str]]", ["zip_longest"]),
    "TestPurePythonRoughEquivalents.test_batched_recipe": (None, ["batched compared with the original Python recipe using islice"]),
    "TestVariousIteratorArgs.test_accumulate": (None, ["accumulate"]),
    "TestVariousIteratorArgs.test_batched": (None, ["batched"]),
    "TestVariousIteratorArgs.test_compress": (None, ["compress"]),
    "TestVariousIteratorArgs.test_dropwhile": (None, ["dropwhile"]),
    "TestVariousIteratorArgs.test_filterfalse": (None, ["filterfalse"]),
    "TestVariousIteratorArgs.test_map": (None, ["builtins.map"]),
    "TestVariousIteratorArgs.test_starmap": (None, ["starmap"]),
    "TestVariousIteratorArgs.test_takewhile": (None, ["takewhile"]),
    "TestVariousIteratorArgs.test_ziplongest": (None, ["zip_longest"]),
}
# These original methods exercise the same makecycle helper and contain no assertions.
GC_CYCLES = {"test_accumulate", "test_batched", "test_compress", "test_dropwhile",
             "test_filterfalse", "test_takewhile", "test_zip_longest"}
EXAMPLE_TYPES = {
    "test_accumulate": "list[int]", "test_combinations_with_replacement": "list[tuple[str, str]]",
    "test_dropwhile": "list[int]", "test_filterfalse": "list[int]", "test_map": "list[int]",
    "test_stapmap": "list[int]", "test_takewhile": "list[int]", "test_zip_longest": "list[tuple[str, str]]",
}


class OriginalSource:
    """Gather the actual upstream globals and self helpers used by each method."""

    def __init__(self, path):
        data = path.read_bytes()
        self.lines = data.splitlines(keepends=True)
        self.providers = {}
        self.methods = {}
        for node in ast.parse(data).body:
            if isinstance(node, (ast.FunctionDef, ast.ClassDef)):
                self.providers[node.name] = node
                if isinstance(node, ast.ClassDef):
                    self.methods[node.name] = {
                        member.name: member for member in node.body if isinstance(member, ast.FunctionDef)
                    }
            elif isinstance(node, (ast.Import, ast.ImportFrom)):
                for alias in node.names:
                    if alias.name != "*":
                        self.providers[alias.asname or alias.name] = node
            elif isinstance(node, ast.Assign):
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        self.providers[target.id] = node

    def region(self, node, *, with_decorators=False):
        start = node.decorator_list[0].lineno if with_decorators and node.decorator_list else node.lineno
        return b"".join(self.lines[start - 1:node.end_lineno]).decode()

    @cache
    def globals(self, node, with_decorators):
        source = self.region(node, with_decorators=with_decorators)
        table = symtable.symtable(textwrap.dedent(source), "<fixture>", "exec")

        def visit(scope):
            names = {s.get_name() for s in scope.get_symbols() if s.is_global() and s.is_referenced()}
            for child in scope.get_children():
                names.update(visit(child))
            return names

        return visit(table)

    def fixture(self, owner, method):
        functions = self.methods[owner]
        node = functions[method]
        names, helpers, globals_, exports = set(), {}, {}, set()
        visited = set()

        def collect(decl, *, with_decorators=False):
            if decl in visited:
                return
            visited.add(decl)
            for name in sorted(self.globals(decl, with_decorators)):
                names.add(name)
                value = self.providers.get(name)
                if value is None:
                    if name in vars(itertools) and not name.startswith("_"):
                        exports.add(name)
                elif isinstance(value, (ast.Import, ast.ImportFrom)):
                    # The only multi-name import in the original is from test.support.
                    # An unused sibling may not be supported and is not part of the fixture.
                    globals_[value.lineno, name] = (
                        f"from {value.module} import {name}\n" if isinstance(value, ast.ImportFrom)
                        else f"import {name}\n"
                    )
                else:
                    globals_[value.lineno, name] = self.region(value) + "\n"
                    collect(value)
            for member in ast.walk(decl):
                if (isinstance(member, ast.Attribute) and isinstance(member.value, ast.Name)
                        and member.value.id == "self" and member.attr != method and member.attr in functions):
                    helpers[member.attr] = functions[member.attr]
                    collect(functions[member.attr], with_decorators=True)

        collect(node)
        preamble = ("from itertools import " + ", ".join(sorted(exports)) + "\n") if exports else ""
        preamble += "".join(value for _, value in sorted(globals_.items()))
        reviewed = REVIEWED.get(owner + "." + method)
        equality = reviewed[0] if reviewed else EXAMPLE_TYPES.get(method) if owner == "TestExamples" else None
        prefix = typed_case(equality) if equality else testcase_fixture()
        fixture = preamble + prefix + "".join(
            self.region(h, with_decorators=True) + "\n" for h in sorted(helpers.values(), key=lambda n: n.lineno)
        )
        return fixture, names


def typed_case(ty):
    # The current native TestCase has a single equality signature. These typed
    # operands use the native recursive comparison, including unequal lengths.
    if ty.startswith("list["):
        check = """        if len(first) != len(second):
            raise AssertionError("unequal lengths")
        for i in range(len(first)):
            if first[i] != second[i]:
                raise AssertionError("unequal items")
"""
    else:
        check = """        if first != second:
            raise AssertionError("unequal operands")
"""
    return f"\nclass NativeCase:\n    def assertEqual(self, first: {ty}, second: {ty}) -> None:\n" + check


def private_reason(owner, method, node):
    decorators = {ast.unparse(d) for d in node.decorator_list}
    tested = {"test_product_sizeof", "test_combinations_sizeof",
              "test_combinations_with_replacement_sizeof", "test_permutations_sizeof"}
    if (owner == "SizeofTest" and method in tested
            and any(isinstance(n, ast.Attribute) and n.attr == "check_sizeof" for n in ast.walk(node))):
        return "The @support.cpython_only class asserts CPython object header and C array sizes with support.check_sizeof."
    if "support.impl_detail('tuple reuse is specific to CPython')" in decorators and method.endswith("_tuple_reuse"):
        return "The upstream implementation-detail guard and assertions specifically require CPython tuple identity reuse."
    if ("support.cpython_only" in decorators and method.endswith("_result_gc")
            and any(isinstance(n, ast.Attribute) and n.attr == "is_tracked" for n in ast.walk(node))):
        return "The guarded method calls gc.is_tracked on a CPython result tuple after recycling."
    return None


def checked_doctest(manifest, case, source_root=None):
    info = manifest["doctest_source"]
    excerpt = (HERE / "upstream" / info["excerpt"]).read_bytes()
    if hashlib.sha256(excerpt).hexdigest() != info["excerpt_sha256"]:
        raise ValueError("the upstream batched doctest excerpt differs")
    if source_root:
        source = (source_root / info["path"]).read_bytes()
        actual = hashlib.sha1(f"blob {len(source)}\0".encode() + source).hexdigest()
        start = info["first_line"] - 1
        if actual != info["git_blob"] or excerpt != b"".join(source.splitlines(keepends=True)[start:start + info["line_count"]]):
            raise ValueError("the batched doctest does not match its pinned source blob")
    parsed, = doctest.DocTestParser().get_examples(excerpt.decode())
    discovered, = case._dt_test.examples
    if (parsed.source, parsed.want, parsed.options, parsed.exc_msg) != (
        discovered.source, discovered.want, discovered.options, discovered.exc_msg
    ):
        raise ValueError("the interpreter's discovered doctest differs from its upstream source")
    return discovered


def batched_example(tools, directory, example):
    """A statement-only example: the host launches native batched and compares its output."""
    if example.options or example.exc_msg or not isinstance(ast.parse(example.source).body[0], ast.For):
        raise ValueError("review the doctest interactive mode before compiling this changed example")
    data = example.source.encode()
    binary = tools.build(b"from itertools import batched\n" + data, directory)

    def execute(case, original):
        run = subprocess.run([binary], capture_output=True, text=True, timeout=60)
        if run.returncode or run.stderr or not doctest.OutputChecker().check_output(example.want, run.stdout, 0):
            raise AssertionError(f"native batched example: {run.returncode}, {run.stdout!r}, {run.stderr!r}")
        return {"observation": {"roles": roles(body="imre", operation="imre", assertions="cpython", fixture="mixed"),
                "evidence": {"operation": "itertools.batched; iteration and tuple printing in Imre",
                             "fixture": "original CPython DocTestCase hooks and OutputChecker; native import in child",
                             "example_sha256": hashlib.sha256(data).hexdigest(), "opt_level": tools.level}}}
    return execute


def blocked(exc, directory, node=None):
    lines = [s for s in str(exc).splitlines() if s.strip() and not s.strip().startswith("Compiling ")]
    reason = "\n".join(lines).replace(str(directory) + "/", "").replace(str(directory), "<case>")
    row = compile_blocker(reason)
    if isinstance(exc, ValueError):
        row["evidence"]["stage"] = "original fixture/decorator"
    if node is not None:
        row["evidence"]["upstream_method_line"] = node.lineno
    return row


def fixture_import_blocker(directory, fixture, node):
    """Recognize a verified fixture import failing before the original method runs."""
    first_line = fixture.partition("\n")[0]
    if not first_line.startswith("from itertools import "):
        return None
    result = subprocess.run([directory / "case"], cwd=directory, capture_output=True, text=True, timeout=60)
    if result.returncode != 1 or result.stdout:
        return None
    entry = (directory / "case.py").resolve()
    prefix = f'Traceback (most recent call last):\n  File "{entry}", line 1\n'
    # The original count method uses support.MAX_Py_ssize_t. Its provider
    # is not present in a standalone native fixture. Require the exact frame
    # and statement: a failure inside the method must reach the review guard.
    imports = fixture.splitlines()
    if "from test import support" in imports:
        line = imports.index("from test import support") + 1
        error = "ModuleNotFoundError: No module named 'test'"
        traceback = (f'Traceback (most recent call last):\n  File "{entry}", line {line}, in <module>\n'
                     f"    from test import support\n{error}\n")
        if result.stderr == traceback:
            return {"outcome": "blocked", "reason": f"case.py:{line}: {error}",
                    "roles": dict.fromkeys(ROLES, "unobserved"),
                    "evidence": {"stage": "native fixture import", "source": imports[line - 1],
                                 "upstream_method_line": node.lineno}}
    names = first_line.removeprefix("from itertools import ").split(", ")
    errors = ['ModuleNotFoundError: no module named "itertools"']
    missing = next((name for name in names if name != "count"), None)
    if missing in {"accumulate", "combinations", "islice", "permutations", "product"}:
        errors.append(f'ModuleNotFoundError: parent of "itertools.{missing}" is not a package')
        for origin in ("unknown location", "<imre stdlib>/itertools.py"):
            errors.append(f"ImportError: cannot import name '{missing}' from 'itertools' ({origin})")
    for error in errors:
        if result.stderr == f"{prefix}{error}\n":
            return {"outcome": "blocked", "reason": f"case.py:1: {error}",
                    "roles": dict.fromkeys(ROLES, "unobserved"),
                    "evidence": {"stage": "native fixture import", "source": first_line,
                                 "upstream_method_line": node.lineno}}
    return None


def identity_gated_method(tools, directory, src, case, node, fixture, decorators):
    """Run the original host decorators; compile the entire unchanged def only if they are identities."""
    from test import support
    known = {"support.skip_if_pgo_task": support.skip_if_pgo_task,
             "support.requires_resource('cpu')": support.requires_resource("cpu"),
             "support.cpython_only": support.cpython_only}
    original = getattr(case, case._testMethodName).__func__
    if (not decorators or any(d not in known or known[d](original) is not original for d in decorators)
            or getattr(original, "__unittest_skip__", False) or hasattr(original, "__wrapped__")):
        raise ValueError("original method decorators require a dedicated fixture adapter")
    body = src.region(node).encode()
    marker = "completed " + case.id()
    binary = tools.build(fixture.encode() + b"\n" + body +
                         f"\ncase = NativeCase()\ncase.{node.name}()\nprint({marker!r})\n".encode(), directory)

    def execute(upstream_case, bound):
        result = subprocess.run([binary], capture_output=True, text=True, timeout=60)
        if result.returncode or result.stderr or result.stdout != marker + "\n":
            raise AssertionError(f"native exit {result.returncode}: {result.stdout!r} {result.stderr!r}")
        if node.name != "test_long_chain_of_empty_iterables":
            raise ValueError(f"review newly compiled decorated method {upstream_case.id()}")
        return {"roles": roles(body="imre", assertions="imre", operation="imre", fixture="mixed"),
                "evidence": {"operation": "chain.from_iterable across ten million empty generators; next",
                    "fixture": "original CPython decorators resolve to identities with CPU enabled; native test instance",
                    "method_sha256": hashlib.sha256(body).hexdigest(), "opt_level": tools.level}}
    return execute


def run(args, reference, level):
    from test import support

    support.use_resources = ["cpu"]
    suite = load_suite(reference["module"])
    discovered = list(cases(suite))
    if Counter(c.id() for c in discovered) != Counter(reference["reference"].keys()) or len(discovered) != 137:
        raise ValueError("discovery differs from the 137 original IDs/occurrences")
    src = OriginalSource(PIN)
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    tools = Toolchain(args.imre, args.runtime, args.clang, level)

    preflight, executions, exceptions = {}, {}, {}
    workers = min(4, os.process_cpu_count() or 1)
    # Prepare independent native fixtures concurrently. The original host
    # callbacks still run in discovery order after preflight.
    with tempfile.TemporaryDirectory() as work, ThreadPoolExecutor(max_workers=workers) as pool:
        def prepare(entry):
            i, case = entry
            name, variant = case.id(), f"{type(case).__module__}.{type(case).__qualname__}"
            key = name, 1
            exception = None
            directory = Path(work) / str(i)
            directory.mkdir()
            if name == "itertools.batched":
                example = checked_doctest(manifest, case, args.cpython_source)
                try:
                    return key, batched_example(tools, directory, example), None
                except NativeCompileError as exc:
                    return key, blocked(exc, directory), None
            owner, method = name.rsplit(".", 2)[1:]
            short = owner + "." + method
            node = src.methods[owner][method]
            loc = f"Lib/test/test_itertools.py:{node.lineno}"
            decorators = [ast.unparse(d) for d in node.decorator_list]
            ref = reference["reference"][name]
            if isinstance(ref, dict) and "skipped" in ref:
                condition = next((d for d in decorators if d == "support.bigaddrspacetest"
                                  or d.startswith("support.requires_resource(")), None)
                if not condition:
                    raise ValueError(f"unreviewed reference skip: {name}")
                exception = {"kind": "upstream-skip", "reason": ref["skipped"],
                    "condition": condition, "source": loc, "platform": reference["platform"],
                    "configuration": "Pinned Linux x86_64 CPython 3.14.5, default regrtest resources",
                    "fixture_variant": variant}
                if condition == "support.bigaddrspacetest":
                    return key, None, exception
                # The same unchanged CPU case passes under -u cpu. Attempt the native
                # body under that resource even though the default reference skipped.
            if reason := private_reason(owner, method, node):
                condition = (", ".join(decorators) if owner != "SizeofTest"
                             else "The original SizeofTest class is decorated with support.cpython_only")
                row = {"outcome": "excluded", "reason": reason,
                       "roles": dict.fromkeys(ROLES, "unobserved")}
                exception = {"kind": "reviewed-exclusion", "condition": condition,
                    "reason": reason, "source": loc, "platform": reference["platform"], "fixture_variant": variant,
                    "configuration": "Imre native runtime on Linux x86_64 versus the pinned CPython object layout",
                    "review": f"Inspected {short} and its original CPython implementation guard at {loc}.",
                    "revisit": f"Run the unchanged {name} when Imre exposes the CPython identity, GC or object-size behavior asserted at {loc}."}
                return key, row, exception
            fixture, dependencies = src.fixture(owner, method)
            reviewed = REVIEWED.get(short)
            cycle = owner == "TestGC" and method in GC_CYCLES
            if cycle:
                reviewed = (None, [method.removeprefix("test_"), "makecycle appends the iterator, pulls once and drops references"])
            try:
                if decorators and short in {"RegressionTests.test_long_chain_of_empty_iterables",
                                            "TestBasicOps.test_immutable_types"}:
                    execute = identity_gated_method(tools, directory, src, case, node, fixture, decorators)
                else:
                    execute = native_method(tools, directory, PIN, owner, name, fixture,
                                            operation=("imre", ", ".join(reviewed[1])) if reviewed else None,
                                            assertions="none" if cycle else "imre")
            except (NativeCompileError, ValueError) as exc:
                row = blocked(exc, directory, node)
                if owner == "TestVariousIteratorArgs":
                    row["evidence"]["original_source_fixtures"] = sorted(dependencies & {"G", "I", "Ig", "S", "L", "R", "X", "N", "E", "E2"})
                if owner == "TestGC" and "makecycle" in src.region(node):
                    row["evidence"]["source_fixture"] = "makecycle appends the iterator, pulls once, then deletes its local references."
                if key[0].endswith(".test_long_chain_of_empty_iterables"):
                    row["evidence"]["cpu_enabled_reference"] = "pass"
                return key, row, exception
            if not reviewed and short != "RegressionTests.test_long_chain_of_empty_iterables":
                if startup := fixture_import_blocker(directory, fixture, node):
                    return key, startup, exception
                raise ValueError(f"Complete method now compiles: review its operations and assertion fixture: {name}")
            return key, execute, exception

        for key, result, exception in pool.map(prepare, enumerate(discovered), buffersize=workers):
            if callable(result):
                executions[key] = result
            elif result is not None:
                preflight[key] = result
            if exception is not None:
                exceptions[(*key, level)] = exception
        return run_evidence(suite, executions, reference, level, preflight=preflight, exceptions=exceptions)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for opt in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + opt, type=Path, required=True)
    p.add_argument("--cpython-source", type=Path, help="verify the whole C docstring blob; supply Lib/test if needed")
    p.add_argument("--baseline", type=Path, help="require the saved classifications and passing fixture roles at both levels")
    args = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        p.error("use the pinned, test-enabled CPython 3.14.5 interpreter")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reports = [run(args, reference, level) for level in (0, 3)]
    write_reports(args.result, reports, reference)
    if args.baseline:
        previous = json.loads(args.baseline.read_text())
        validate_pair(previous, reference)
        for old, new in zip(previous, reports, strict=True):
            current = {(r["id"], r["occurrence"]): r for r in new["records"]}
            for r in old["records"]:
                now = current[r["id"], r["occurrence"]]
                if r["outcome"] == "pass" and (now["outcome"] != "pass" or now["roles"] != r["roles"]):
                    p.error(f"native regression at O{old['opt_level']}: {r['id']}")
                if now["classification"] != r["classification"]:
                    p.error(f"stale classification at O{old['opt_level']}: {r['id']}: "
                            f"{r['classification']} -> {now['classification']}")
    for report in reports:
        print(f"-O{report['opt_level']}: {report['denominator']} occurrences; {report['counts']}")
    return int(any(r["classification"] in {"failed", "unexpected-skip", "reference-disagreement"}
                   for report in reports for r in report["records"]))


if __name__ == "__main__":
    sys.exit(main())
