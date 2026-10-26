"""Compare fixed-class programs with CPython and reject unsafe class shapes."""

import argparse
import ast
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tokenize

sys.path.insert(0, str(Path(__file__).resolve().parent))
from class_data_sources import write_class_data_sources


FIXTURES = Path(__file__).resolve().parent / "fixtures" / "classes"
PROJECT = FIXTURES.parents[2]
CELL = """class Cell:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value

    def add(self, amount: int) -> int:
        self.value += amount
        return self.value

"""


HOOK = """class Build:
    value: int
    def __init__(self) -> None:
        self.prepare()
    def prepare(self) -> None:
        self.value = 1
"""


SUBCLASS_LAYOUT = """class Base: pass
class A(Base):
    label: str
    def __init__(self) -> None: self.label = "only A"
class B(Base): pass
"""


FORMAT_INIT = '''class Formatted:
    def __init__(self):
        self.path = "{v}".format(v=1)
'''


INVALID_OBJECT_BASES = {
    "non_type_object": 'object = 7\nclass Child(object): pass\nprint("done")\n',
    "duplicate_object": 'class Child(object, object): pass\nprint("done")\n',
    "inconsistent_object_mro": 'class Base: pass\nclass Child(object, Base): pass\nprint("done")\n',
}


REJECTED = {
    **INVALID_OBJECT_BASES,
    "weak_proxy_callback": "import weakref\nclass Target:\n    pass\ndef callback(proxy: Target) -> None:\n    pass\ntarget = Target()\nweakref.proxy(target, callback)\n",
    "inferred_field_reads_itself": FORMAT_INIT.replace("v=1", "v=self.path"),
    "inferred_field_invalid_format_arg": FORMAT_INIT.replace("v=1", "v=[1]"),
    "inferred_field_type_is_shadowed": "class str: pass\n" + FORMAT_INIT,
    "inferred_field_incompatible_store": FORMAT_INIT + "item = Formatted()\nitem.path = False\n",
    "inferred_field_custom_format": (
        "class Other:\n    def format(self) -> int: return 7\n"
        "class C:\n    def __init__(self, other: Other): self.path = other.format()\n"
    ),
    "partially_initialized_self_in_class_data_store": 'class C:\n    shared = [1]\n    n: int\n    def __init__(self):\n        self.shared = expose(self)\n        self.n = 0\ndef expose(v: C) -> list[int]: return [v.n]\nC()\n',
    "class_local_shadows_module_callable": 'def make() -> int: return 1\nclass C:\n    make = [1]\n    wrong = make()\n',
    "read_computed_class_data_before_definition": 'def get() -> list[int]: return Later.items\nvalue = get()\nclass Later:\n    items = [1]\n',
    "class_data_rebinding_changes_type": 'class C:\n    items = [1]\nC.items = "wrong"\n',
    "frozen_inherited_class_data": 'from dataclasses import dataclass\nclass Base:\n    items = [1]\n@dataclass(frozen=True)\nclass Child(Base):\n    n: int\nChild(1).items = [2]\n',
    "sibling_guard_field": SUBCLASS_LAYOUT + "def f(x: Base) -> str:\n    if isinstance(x, (A, B)): return x.label\n    return 'other'\n",
    "reassigned_subclass_field": SUBCLASS_LAYOUT + "def f(x: Base, replacement: Base) -> str:\n    if isinstance(x, A):\n        x = replacement\n        return x.label\n    return 'other'\n",
    "init_hook_skips_field": HOOK + "class Child(Build):\n    def prepare(self) -> None: pass\n",
    "init_hook_reads_before_write": HOOK + "class Child(Build):\n    def prepare(self) -> None: self.value = self.value + 1\n",
    "init_hook_leaks_receiver": HOOK + "def observe(value: Build) -> int: return value.value\nclass Child(Build):\n    def prepare(self) -> None: self.value = observe(self)\n",
    "init_hook_observes_pending_field": HOOK + "class Child(Build):\n    def prepare(self) -> None:\n        print(self.value)\n        super().prepare()\n",
    "init_hook_super_reads_pending_field": HOOK + "    def read(self) -> int: return self.value\nclass Child(Build):\n    def prepare(self) -> None: self.value = super().read()\n",
    "init_hook_duplicates_field": HOOK.replace("self.prepare()", "self.value = 2\n        self.prepare()"),
    "read_class_tuple_before_definition": 'def get() -> tuple[str]:\n    return Later.words\nvalue = get()\nclass Later:\n    words = ("late",)\n',
    "assign_to_class_string": 'class C:\n    label = "fixed"\nC.label = "new"\n',
    "instance_shadows_class_string": 'class C:\n    label = "fixed"\n    def __init__(self, value: str) -> None:\n        self.label = value\nC("instance")\n',
    "incompatible_subclass_class_string": 'class C:\n    label = "first"\nclass D(C):\n    label = 2\n',
    "inherited_name_in_class_expression": 'class C:\n    words = ("a",)\nclass D(C):\n    longer = words + ("b",)\n',
    "mutable_class_tuple": 'class C:\n    items = (["not fixed"],)\n',
    "copy_hook_wrong_arity": 'import copy\nclass C:\n    def __copy__(self, extra: int) -> int: return extra\ncopy.copy(C())\n',
    "inherited_copy_reduction": 'import copy\nclass Base:\n    def __reduce_ex__(self, protocol: int) -> int: return 0\nclass Child(Base): pass\ncopy.copy(Child())\n',
    "copy_custom_getstate": 'import copy\nclass C:\n    def __getstate__(self) -> int: return 0\ncopy.copy(C())\n',
    "overridden_static_binding": "import hashlib\nclass Base:\n    make = staticmethod(hashlib.sha256)\nclass Child(Base):\n    def make(self, x: int) -> int:\n        return x\n",
    "literal_unknown_value": 'from typing import Literal\ndef f(s: Literal["a", "b"]) -> str:\n    return s\nf("c")\n',
    "literal_unproven_string": 'from typing import Literal\ndef f(s: str) -> Literal["a"]:\n    return s\n',
    "literal_wider_domain": 'from typing import Literal\ndef f(s: Literal["a", "b"]) -> Literal["a"]:\n    return s\n',
    "literal_invalid_default": 'from typing import Literal\ndef f(s: Literal["a"] = "b") -> str:\n    return s\n',
    "literal_mutable_alias": 'from typing import Literal\na: list[Literal["a"]] = ["a"]\nb: list[str] = a\nb.append("b")\n',
    "literal_non_string": 'from typing import Literal\nx: Literal["a", 7] = "a"\n',
    "literal_invalidated_guard": 'from typing import Literal\ndef f(s: str) -> Literal["a"]:\n    if s == "a":\n        s = input()\n        return s\n    return "a"\n',
    "root_super_wrong_arity": CELL.replace("self.value = value", "super().__init__(value)\n        self.value = value"),
    "init_alias_partial_self": CELL.replace("self.value = value", "alias = self\n        self.value = value"),
    "init_rebind_self": CELL.replace("self.value = value", "self = self\n        self.value = value"),
    "init_assert_unset_field": CELL.replace("self.value = value", "assert self.value\n        self.value = value"),
    "init_eager_message_escape": CELL.replace("self.value = value", "assert value, self\n        self.value = value"),
    "wrong_field_type": CELL + 'value = Cell(1)\nvalue.value = "wrong"\n',
    "wrong_alias_type": CELL + "value = Cell(1)\nalias = value\nalias.value = True\n",
    "ignored_field_type": CELL + 'value = Cell(1)\nvalue.value = "wrong"  # type: ignore\n',
    "wrong_constructor_type": CELL + 'Cell("wrong")\n',
    "wrong_constructor_arity": CELL + "Cell()\n",
    "wrong_method_type": CELL + 'value = Cell(1)\nvalue.add("wrong")\n',
    "wrong_method_arity": CELL + "value = Cell(1)\nvalue.add()\n",
    "explicit_initializer_call": CELL + "value = Cell(1)\nvalue.__init__(2)\n",
    "missing_method_annotation": CELL.replace("amount: int", "amount"),
    "wrong_method_return": CELL.replace("return self.value", 'return "wrong"'),
    "wrong_self_name": CELL.replace("def __init__(self,", "def __init__(other,").replace("self.value = value", "other.value = value"),
    "wrong_self_annotation": CELL.replace("def __init__(self,", "def __init__(self: int,"),
    "missing_self": CELL.replace("def add(self, amount: int)", "def add(amount: int)"),
    "method_expression_default": CELL.replace("amount: int)", "amount: int = 1 + 1)"),
    "method_unknown_keyword": CELL + "Cell(1).add(unknown=2)\n",
    "recursive_method": CELL.replace("return self.value", "return self.add(amount)"),
    "mutual_method_recursion": CELL.replace("return self.value", "return self.other(amount)")
        + "    def other(self, amount: int) -> int:\n        return self.add(amount)\n",
    "constructor_factory_recursion": CELL.replace("self.value = value", "self.value = construct_value(value)")
        + "def construct_value(value: int) -> int:\n    return Cell(value).value\n",
    "untyped_constructor": CELL.replace("def __init__(self, value: int)", "def __init__(self, value)"),
    "dynamic_attribute": CELL + "value = Cell(1)\nvalue.extra = 2\n",
    "dynamic_lookup": CELL + 'print(getattr(Cell(1), "value"))\n',
    "instance_dictionary": CELL + "print(Cell(1).__dict__)\n",
    "field_default": CELL.replace("    value: int\n", "    value: int = 0\n", 1),
    "duplicate_field": CELL.replace("    value: int\n", "    value: int\n    value: int\n", 1),
    "field_method_collision": CELL.replace("def add(", "def value("),
    "missing_initializer": "class Cell:\n    value: int\n",
    "missing_field": CELL.replace("self.value = value", "pass"),
    "duplicate_initialization": CELL.replace("self.value = value", "self.value = value\n        self.value = value"),
    "undeclared_initialization": CELL.replace("self.value = value", "self.other = value"),
    "constructor_self_read": CELL.replace("self.value = value", "self.value = self.add(value)"),
    "constructor_self_escape": CELL.replace("self.value = value", "self.value = escape(self)")
        + "def escape(value: Cell) -> int:\n    return 0\n",
    "constructor_control_flow": CELL.replace("self.value = value", "if value > 0:\n            self.value = value\n        else:\n            self.value = 0"),
    "constructor_side_effect": CELL.replace("self.value = value", "print(value)\n        self.value = value"),
    "constructor_return": CELL.replace("self.value = value", "self.value = value\n        return"),
    "inheritance": CELL + "class Child(Cell):\n    def __init__(self, value: int) -> None:\n        self.value = value\n",
    "metaclass": CELL.replace("class Cell:", "class Cell(metaclass=type):"),
    "magic_method": CELL + '    def __bytes__(self) -> bytes:\n        return b"Cell"\n',
    "str_extra_parameter": CELL + '    def __str__(self, detail: bool = False) -> str:\n        return "Cell"\n',
    "str_static_method": CELL + '    @staticmethod\n    def __str__() -> str:\n        return "Cell"\n',
    "str_wrong_result": CELL + '    def __str__(self) -> int:\n        return self.value\n',
    "str_wrong_body": CELL + '    def __str__(self):\n        return self.value\n',
    "recursive_str": CELL + '    def __str__(self) -> str:\n        return str(self)\n',
    "finalizer": CELL + "    def __del__(self) -> None:\n        pass\n",
    "private_mangling": CELL.replace("value", "__value"),
    "class_surrogate_docstring": CELL.replace("class Cell:\n", 'class Cell:\n    "\\ud800"\n'),
    "staticmethod": CELL.replace("    def add(", "    @staticmethod\n    def add("),
    "classmethod": CELL.replace("    def add(", "    @classmethod\n    def add("),
    "property": CELL.replace("    def add(", "    @property\n    def add("),
    "read_only_assignment": "class C:\n    @property\n    def value(self) -> int: return 1\nC().value = 2\n",
    "orphan_setter": "class C:\n    @value.setter\n    def value(self, n: int) -> None: pass\n",
    "property_setter_arity": "class C:\n    def get(self) -> int: return 1\n    def put(self) -> None: pass\n    value = property(get, put)\n",
    "unused_dynamic_method": CELL.replace("return self.value", 'if False:\n            eval("1")\n        return self.value'),
    "same_shape_different_nominal_type": CELL
        + CELL.replace("Cell", "Other")
        + "def consume(value: Cell) -> int:\n    return value.value\nconsume(Other(1))\n",
    "direct_field_cycle": 'class Node:\n    next: "Node"\n    def __init__(self, next: "Node") -> None:\n        self.next = next\n',
    "mutual_field_cycle": 'class Left:\n    right: "Right"\n    def __init__(self, right: "Right") -> None:\n        self.right = right\n\nclass Right:\n    left: Left\n    def __init__(self, left: Left) -> None:\n        self.left = left\n',
}


IMPORTED_REJECTIONS = {
    "imported_nominal_mismatch": {
        "main.py": "from first import Cell as First\nfrom second import Cell as Second\ndef consume(value: First) -> int:\n    return value.value\nconsume(Second(1))\n",
        "first.py": CELL,
        "second.py": CELL,
    },
    "imported_wrong_field": {
        "main.py": 'from model import Cell\nvalue = Cell(1)\nvalue.value = "wrong"\n',
        "model.py": CELL,
    },
    "unused_imported_bad_class": {
        "main.py": "import model\nprint(1)\n",
        "model.py": CELL.replace("self.value = value", "self.value = escape(self)")
            + "def escape(value: Cell) -> int:\n    return 0\n",
    },
}


def verify_upstream_definitions() -> None:
    def definitions(path: Path, names: set[str]) -> dict[str, str]:
        source = path.read_text()
        lines = source.splitlines(keepends=True)
        tree = ast.parse(source)
        nodes = [(node.name, node) for node in ast.walk(tree)
                 if isinstance(node, (ast.ClassDef, ast.FunctionDef))]
        nodes.extend((f"{cls.name}.{method.name}", method)
                     for cls in tree.body if isinstance(cls, ast.ClassDef)
                     for method in cls.body if isinstance(method, ast.FunctionDef))
        # A surrounding function indents Python code but must preserve multiline string contents.
        literal_lines = set()
        for token in tokenize.generate_tokens(io.StringIO(source).readline):
            if tokenize.tok_name[token.type] in {"STRING", "FSTRING_MIDDLE"}:
                literal_lines.update(range(token.start[0] + 1, token.end[0] + 1))

        result = {}
        for name, node in nodes:
            if name in names:
                start = min([node.lineno, *(d.lineno for d in node.decorator_list)])
                indent = " " * node.col_offset
                result[name] = "".join(
                    lines[i - 1] if i in literal_lines else lines[i - 1].removeprefix(indent)
                    for i in range(start, node.end_lineno + 1)
                )
        return result

    reporter = PROJECT / "examples" / "conformance_report"
    checks = [
        (reporter / "upstream/conformance.py", reporter / "model.py",
         {"Source", "Classification", "Change", "compute_stats", "change", "diagnostics_by_source",
          "classify", "key", "diagnostics_are_equivalent", "group_diagnostics_by_key", "collect_file_stats",
          "TyDiagnostic.__str__", "TyDiagnostic.severity_for_display", "Statistics"}),
        (reporter / "upstream/conformance.py", reporter / "parse.py", {"collect_expected_diagnostics"}),
        (reporter / "upstream/conformance.py", reporter / "render.py",
         {"collect_diagnostic_entries", "render_file_stats_table", "render_test_cases", "render_html_diff_row", "render_diff_row",
          "diff_format", "render_summary"}),
        (PROJECT / "examples/python_catalog/upstream/fetch-download-metadata.py",
         FIXTURES / "uv_variant.py", {"Variant", "variant_sort_key"}),
    ]
    for upstream, actual, names in checks:
        expected = definitions(upstream, names)
        assert expected.keys() == names
        assert definitions(actual, names) == expected, actual


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        input="",
        text=True,
        capture_output=True,
        timeout=60,
        env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"},
    )


def write_files(root: Path, files: dict[str, str]) -> None:
    for name, source in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")


def require_success(result: subprocess.CompletedProcess[str]) -> None:
    assert result.returncode == 0 and not result.stderr, (result.args, result.stdout, result.stderr)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", nargs="+", choices=["0", "1", "2", "3"], default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    build_flags = ["--quiet"]
    if args.lto_linker is not None:
        build_flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.target is not None:
        build_flags += ["--target", args.target]
    compiler = str(args.compiler.resolve())
    runtime = str(args.runtime.resolve())
    python = str(Path(args.python).resolve()) if "/" in args.python else args.python
    clang = str(Path(args.clang).resolve()) if "/" in args.clang else args.clang
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-classes-") as directory:
        temporary = Path(directory)
        project = temporary / "project"
        shutil.copytree(FIXTURES, project, ignore=shutil.ignore_patterns("__pycache__"))
        verify_upstream_definitions()
        class_sources = write_class_data_sources(project, PROJECT / "benchmarks/cpython_stdlib/full")
        for relative in ["ownership.py", "record_reuse.py", "record_reuse_list.py", "imported/main.py", "string_literals.py", "constant_strings.py", "class_namespace.py", "class_data_setup.py", "string_enum_members.py", "string_enum_storage.py", "ordinary_enums.py", "enum_exhaustion.py", "conditional_lists.py", "uv_variant.py", "fixed_lineage.py", "object_base.py", "instance_layout.py", "subclass_guard_fields.py", "property_setters.py", "initializer_hook.py", "constructor_fields.py", "weak_proxy.py", "shallow_copy_protocol.py", "implicit_copy.py", "plain_shallow_copy.py", "mapping_abc.py", "mapping_alias.py", "mapping_class_values.py", *class_sources]:
            source = project / relative
            reference = run([python, "-E", str(source)], temporary)
            require_success(reference)
            require_success(run([compiler, "check", str(source)], temporary))
            for level in args.opt_levels:
                executable = temporary / f"{source.parent.name}-{source.stem}-O{level}"
                command = [compiler, "build", str(source), "-o", str(executable),
                           "--runtime", runtime, "--clang", clang, "--opt-level", level]
                command += build_flags
                if args.sanitize:
                    command.append("--sanitize")
                modes = (False, True) if relative == "weak_proxy.py" else (relative == "implicit_copy.py",)
                for reachable in modes:
                    require_success(run(command + (["--reachable-only"] if reachable else []), temporary))
                    actual = run([str(executable)], temporary)
                    require_success(actual)
                    assert actual.stdout == reference.stdout, (relative, level, reachable, actual.stdout, reference.stdout)
                    checks += 1

        rejected = {name: {"main.py": source} for name, source in REJECTED.items()}
        rejected.update(IMPORTED_REJECTIONS)
        for name, files in rejected.items():
            case = temporary / name
            write_files(case, files)
            result = run([compiler, "check", str(case / "main.py")], temporary)
            assert result.returncode > 0 and result.stderr, (name, result.stdout, result.stderr)
            assert "panicked" not in result.stderr and "AddressSanitizer" not in result.stderr, (name, result.stderr)
            if name in INVALID_OBJECT_BASES:
                reference = run([python, "-I", str(case / "main.py")], temporary)
                assert reference.returncode and not reference.stdout and "TypeError:" in reference.stderr, name
                reachable = run([compiler, "check", str(case / "main.py"), "--reachable-only"], temporary)
                for rejection in (result, reachable):
                    assert rejection.returncode and "the base must be a known fixed-layout class" in rejection.stderr, (name, rejection.stderr)
            if name in {"inherited_copy_reduction", "copy_custom_getstate"}:
                reachable = run([compiler, "check", str(case / "main.py"), "--reachable-only"], temporary)
                assert reachable.returncode > 0 and "unsupported reconstruction protocol" in reachable.stderr, (name, reachable.stderr)
            if name in {"direct_field_cycle", "mutual_field_cycle"}:
                assert "cyclic heap types are unsupported" in result.stderr, (name, result.stderr)
            if name in {"recursive_method", "mutual_method_recursion", "constructor_factory_recursion"}:
                assert "recursive calls are unsupported" in result.stderr, (name, result.stderr)
            checks += 1
    print(f"Passed {checks} class checks ({len(REJECTED) + len(IMPORTED_REJECTIONS)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})")


if __name__ == "__main__":
    main()
