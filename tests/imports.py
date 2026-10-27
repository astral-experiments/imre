"""Check closed module graphs against CPython and exercise rejected import boundaries."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


FIXTURES = Path(__file__).resolve().parent / "fixtures" / "imports"
HELPER = "def f(value: int) -> int:\n    return value + 1\n"
IMPORT = "from helper import f\nprint(f(2))\n"


REJECTED = {
    "unready_partial_function": ({
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "def ready() -> int:\n    return 1\nfrom . import child\ndef later() -> int:\n    return 42\n",
        "pkg/child.py": "from .parent import later\nprint(later())\n",
    }, "has no static binding \"later\""),
    "unready_partial_attribute": ({
        "main.py": "import pkg.parent\n", "pkg/__init__.py": "",
        "pkg/parent.py": "from . import child\ndef later(n: int) -> int:\n    return n + 1\n",
        "pkg/child.py": "from . import parent\nprint(parent.later(41))\n",
    }, "references a function before it is available"),
    "partial_child_custom_getattr": ({
        "main.py": "import pkg.http\n",
        "pkg/__init__.py": "def __getattr__(name: str) -> str:\n    print(name)\n    return \"custom\"\n",
        "pkg/http.py": "import pkg.child as child\n",
        "pkg/child.py": "from . import http as partial\nprint(\"child finished\")\n",
    }, "cyclic import through \"pkg.http\""),
    "decorated_partial_function": ({
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "from functools import cache\n@cache\ndef earlier() -> int:\n    return 42\nfrom . import child\n",
        "pkg/child.py": "from .parent import earlier\nprint(earlier())\n",
    }, "cyclic import through \"pkg.parent\""),
    "partial_source_value": ({
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "answer = 42\nfrom . import child\n",
        "pkg/child.py": "from .parent import answer\nprint(answer)\n",
    }, "cyclic import through \"pkg.parent\""),
    "private_textwrap_module": ({"main.py": "import __imre_textwrap_wrapper__\n"}, "private textwrap implementation"),
    "private_textwrap_helper": ({"main.py": "from textwrap import _chunk_text\n"}, "unsupported sealed standard-library export textwrap._chunk_text"),
    "unsupported_stdlib": ({"main.py": "import turtle\n"}, 'standard-library imports and shadowing are unsupported: "turtle"'),
    "missing": ({"main.py": "import missing_dependency\n"}, "cannot resolve first-party"),
    "wildcard": ({"main.py": "from helper import *\n", "helper.py": HELPER}, "wildcard"),
    "dynamic": ({"main.py": '__import__("helper")\n', "helper.py": HELPER}, "unsupported"),
    "nested": ({"main.py": "def f() -> None:\n    import math\nf()\n"}, "unsupported"),
    "conditional": ({"main.py": "if False:\n    import math\n"}, "unsupported"),
    "conditional_missing_export": ({
        "main.py": "from helper import chosen\n",
        "helper.py": "if False:\n    def chosen():\n        return 1\n",
    }, "no static binding"),
    "unbound_alias": ({"main.py": "import helper\n", "helper.py": "Pair = tuple[Missing, int]\n"}, "name is not a local value"),
    "unused_bad_type": ({"main.py": IMPORT, "helper.py": HELPER + 'def unused() -> int:\n    return "wrong"\n'}, "helper.py"),
    "unused_unsupported": ({"main.py": IMPORT, "helper.py": HELPER + 'def unused() -> int:\n    return eval("1")\n'}, "unsupported"),
    "inactive_guard": ({"main.py": IMPORT, "helper.py": HELPER + 'if __name__ == "__main__":\n    eval("1")\n'}, "unsupported"),
    "wrong_imported_argument": ({"main.py": 'from helper import f\nprint(f("wrong"))\n', "helper.py": HELPER}, "int"),
    "cycle": ({"main.py": "import first\n", "first.py": "import second\n", "second.py": "import first\n"}, "cyclic import"),
    "relative_beyond_root": ({"main.py": "from .helper import f\n", "helper.py": HELPER}, "relative import"),
    "parameter_module_shadow": ({"main.py": "import helper\ndef f(helper: int) -> int:\n    return helper\n", "helper.py": HELPER}, "rebinding"),
    "parameter_function_shadow": ({"main.py": "from helper import f\ndef g(f: int) -> int:\n    return f\n", "helper.py": HELPER}, "rebinding"),
    "local_module_shadow": ({"main.py": "import helper\ndef g() -> None:\n    helper = 1\n", "helper.py": HELPER}, "rebinding"),
    "local_function_shadow": ({"main.py": "from helper import f\ndef g() -> None:\n    f = 1\n", "helper.py": HELPER}, "rebinding"),
    "module_rebinding": ({"main.py": "from helper import f\ndef f(value: int) -> int:\n    return value\n", "helper.py": HELPER}, "cannot be rebound"),
    "cached_target_early_alias": ({
        "main.py": "import pkg.parent\n", "pkg/__init__.py": "", "pkg/child.py": HELPER,
        "pkg/parent.py": "from . import child\nvalue = late(1)\nfrom .child import f as late\n",
    }, "import before its binding is available"),
    "cached_target_early_alias_through_function": ({
        "main.py": "import pkg.parent\n", "pkg/__init__.py": "", "pkg/child.py": HELPER,
        "pkg/parent.py": ("from . import child\ndef read() -> int:\n    return late(1)\n"
                          "value = read()\nfrom .child import f as late\n"),
    }, "import before its binding is available"),
    "unpublished_child_attribute": ({
        "main.py": "import pkg.child\n", "pkg/__init__.py": "", "pkg/other.py": "",
        "pkg/child.py": "import pkg\n" + HELPER + "observed = pkg.child.f(1)\nfrom . import other\n",
    }, "import before its binding is available"),
    "imports_after_execution": ({"main.py": "print(1)\nimport helper\n", "helper.py": HELPER}, "imports must precede"),
    "guard_reads_later_import": ({"main.py": "import helper\n", "helper.py": 'if __name__ == "helper":\n    raise ValueError(message)\nfrom child import message\n', "child.py": 'message = "late"\n'}, "imports must precede"),
    "library_computed_before_import": ({"main.py": "import package\n", "package/__init__.py": "metadata = str(7)\nfrom .child import f\n", "package/child.py": HELPER}, "imports must precede"),
    "definition_after_execution": ({"main.py": "print(1)\n" + HELPER}, "definitions must precede"),
    "namespace_package": ({"main.py": "import package.child\n", "package/child.py": HELPER}, "namespace-package"),
    "child_overwrites_definition": ({"main.py": "import package.child\n", "package/__init__.py": "def child() -> int:\n    return 1\n", "package/child.py": HELPER}, "overwrite"),
    "entry_self_import": ({"main.py": "import main\n"}, "entry module"),
    "reserved_alias": ({"main.py": "from helper import f as int\n", "helper.py": HELPER}, "reserved"),
    "normalized_reserved_alias": ({"main.py": "from helper import f as ℕone\n", "helper.py": HELPER}, "reserved"),
    "stdlib_shadow": ({"main.py": "import sys\n", "sys.py": HELPER}, "standard-library"),
    "opcode_shadow": ({"main.py": "import _opcode\n", "_opcode.py": HELPER}, "shadowing"),
    "opcode_bytecode_operation": ({"main.py": "from _opcode import stack_effect\n"}, "stack_effect"),
    "opcode_typed_argument": ({"main.py": "import _opcode as op\ndef text(x: str) -> None:\n    print(x)\ntext(op.ENABLE_SPECIALIZATION)\n"}, "expected str, got int"),
    "unstubbed_builtin_shadow": ({"main.py": "import _types\n", "_types.py": HELPER}, "standard-library"),
    "frozen_shadow": ({"main.py": "import __hello__\n", "__hello__.py": HELPER}, "standard-library"),
    "imported_surrogate": ({"main.py": IMPORT, "helper.py": '"\\ud800"\n' + HELPER}, "surrogate"),
    "imported_encoding": ({"main.py": IMPORT, "helper.py": "# coding: latin1\n" + HELPER}, "encoding"),
    "imported_tabs": ({"main.py": IMPORT, "helper.py": "def f(value: int) -> int:\n\treturn value\n"}, "indentation"),
}

ACCEPTED = {
    "conditional_function_export": {
        "main.py": (
            "from helper import selected as chosen\nimport helper\n"
            "print(chosen(), helper.selected is chosen, helper.other())\n"
        ),
        "helper.py": (
            "import sys\n"
            "def guard(prefix):\n    print(prefix)\n    return sys.platform.startswith(prefix)\n"
            "if guard('win'):\n    def selected():\n        return 'windows'\n"
            "elif guard('linux'):\n    def selected():\n        return 'linux'\n"
            "else:\n    def selected():\n        return 'other'\n"
            "if guard(''):\n    def other():\n        return 'always'\n"
            "else:\n    def other():\n        return 'unreachable'\n"
        ),
    },
    "ready_function_in_cycle": {
        "main.py": "from pkg.parent import observed\nprint(observed)\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "def increment(n: int) -> int:\n    return n + 1\nfrom . import child\nobserved = child.result\n",
        "pkg/child.py": "from .parent import increment\nresult = increment(41)\n",
    },
    "partial_child_shared_storage": {
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "shared = [1]\nfrom . import child\nprint(shared[-1])\n",
        "pkg/child.py": "from . import parent as first\nfrom . import parent as second\nprint(first is second)\nfirst.shared.append(42)\nprint(second.shared[-1])\n",
    },
    "cast_mutable": {"main.py": 'from typing import cast as view\ndef fresh() -> list[int]:\n    print("create")\n    return [1]\nvalues = view("list[int]", fresh())\nview("list[int]", values).append(7)\nprint(values[1])\n'},
    "type_aliases": {
        "types_used.py": (
            "from typing import NewType, TYPE_CHECKING\n"
            "if TYPE_CHECKING:\n    import pathlib as p\n    from typing import Literal\n"
            '    Mode = Literal["read", "write"]\n'
            'Name = NewType("Name", str)\nPair = tuple[Name, int]\n'
            'def named(value: Name) -> Pair:\n    return value, 7\n'
            'def mode(value: "Mode") -> str:\n    return value\n'
            'def leaf(value: "p.Path") -> str:\n    return value.name\n'
        ),
        "main.py": (
            "from pathlib import Path\nfrom types_used import Name, named, mode, leaf\n"
            'data = named(Name("alias"))\nprint(data[0])\nprint(data[1])\nprint(mode("read"))\nprint(leaf(Path("one/two")))\n'
        ),
    },
    "inferred_signature": {"main.py": IMPORT, "helper.py": "def f(value):\n    return value\n"},
    "library_effect": {"main.py": IMPORT, "helper.py": "print(7)\n" + HELPER},
    "library_state": {"main.py": "from helper import f, values\nprint(f(values[0]))\n", "helper.py": "values = [1]\n" + HELPER},
    "package_effect": {"main.py": "import package\n", "package/__init__.py": "print(1)\n"},
    "active_library_else": {"main.py": IMPORT, "helper.py": HELPER + 'if __name__ == "__main__":\n    print(1)\nelse:\n    print(2)\n'},
    "ordered_library_effects": {
        "main.py": "from pkg.parent import value\nprint(value)\n", "pkg/__init__.py": "",
        "pkg/parent.py": 'print("parent prefix")\nfrom . import child\nprint("parent suffix")\nvalue = child.value\n',
        "pkg/child.py": 'print("child init")\nvalue = 42\n',
    },
    "delayed_cached_alias": {
        "main.py": "from pkg.parent import read\nprint(read())\n", "pkg/__init__.py": "",
        "pkg/child.py": 'print("child once")\n' + HELPER,
        "pkg/parent.py": ('from . import child\ndef read() -> int:\n    return late(41)\n'
                          'print("before alias")\nfrom .child import f as late\n'),
    },
    "guard_before_import": {
        "main.py": "import pkg.guard\n",
        "pkg/__init__.py": "",
        "pkg/guard.py": 'if __name__ != "pkg.guard":\n    raise ValueError("wrong module")\nfrom . import child\n',
        "pkg/child.py": 'print("child initialized")\n',
    },
}


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, text=True, capture_output=True, timeout=60,
                          env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"})


def write_files(root: Path, files: dict[str, str]) -> None:
    for name, source in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)


def require_success(result: subprocess.CompletedProcess[str]) -> None:
    assert result.returncode == 0, (result.args, result.stdout, result.stderr)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", type=int, nargs="+", default=[0, 3])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    build_flags = []
    if args.lto_linker is not None:
        build_flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.target is not None:
        build_flags += ["--target", args.target]
    compiler, runtime = str(args.compiler.resolve()), str(args.runtime.resolve())
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-imports-") as directory:
        temporary = Path(directory)
        root = temporary / "project"
        shutil.copytree(FIXTURES, root)
        for entry, module in [("entry.py", "entry"), ("package/program.py", "package.program"),
                              ("package/__main__.py", "package")]:
            baseline = run([args.python, "-E", "-m", module], root)
            require_success(baseline)
            require_success(run([compiler, "check", str(root / entry)], temporary))
            for level in args.opt_levels:
                executable = temporary / f"program-{module}-{level}"
                command = [compiler, "build", str(root / entry), "-o", str(executable), "--runtime", runtime,
                           "--opt-level", str(level), "--clang", args.clang]
                command += build_flags
                # Exercise explicit and inferred roots independently of the current directory.
                if level == args.opt_levels[-1]:
                    command += ["--module-root", str(root)]
                if args.sanitize:
                    command += ["--sanitize"]
                require_success(run(command, temporary))
                result = run([str(executable)], temporary)
                require_success(result)
                assert result.stdout == baseline.stdout, (entry, level, result.stdout, baseline.stdout)
                checks += 1

        for name, files in ACCEPTED.items():
            case = temporary / name
            write_files(case, files)
            baseline = run([args.python, "-E", "-m", "main"], case)
            require_success(baseline)
            for level in args.opt_levels:
                executable = temporary / f"accepted-{name}-{level}"
                command = [compiler, "build", str(case / "main.py"), "-o", str(executable),
                           "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                           *build_flags]
                if args.sanitize:
                    command.append("--sanitize")
                require_success(run(command, temporary))
                result = run([str(executable)], temporary)
                require_success(result)
                assert result.stdout == baseline.stdout, (name, level, result.stdout, baseline.stdout)
                checks += 1

        # These are build capabilities: CPython's flags depend on its build; Imre emits native code.
        source = root / "opcode_flags.py"
        for level in args.opt_levels:
            executable = temporary / f"opcode-flags-{level}"
            command = [compiler, "build", str(source), "-o", str(executable), "--runtime", runtime,
                       "--clang", args.clang, "--opt-level", str(level), *build_flags]
            if args.sanitize:
                command.append("--sanitize")
            require_success(run(command, temporary))
            result = run([str(executable)], temporary)
            require_success(result)
            assert result.stdout == "0\n0\nno bytecode specialization\n", result
            checks += 1

        # A failing guard must run before the next import initializes its child.
        guarded = temporary / "wrong_name"
        write_files(guarded, {"main.py": "import helper\n", "child.py": 'print("child initialized")\n',
                              "helper.py": 'if "helper" == __name__:\n    raise ValueError("wrong module")\nimport child\n'})
        baseline = run([args.python, "-E", "-m", "main"], guarded)
        assert baseline.returncode and not baseline.stdout
        for level in args.opt_levels:
            executable = temporary / f"wrong-name-{level}"
            require_success(run([compiler, "build", str(guarded / "main.py"), "-o", str(executable),
                                 "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                                 *(["--sanitize"] if args.sanitize else []), *build_flags], temporary))
            result = run([str(executable)], temporary)
            assert result.returncode and result.stdout == baseline.stdout, result
            assert result.stderr.splitlines()[-1] == baseline.stderr.splitlines()[-1], result
            checks += 1

        chain = temporary / "deep_imports"
        files = {f"m{i}.py": f"import m{i + 1}\n" for i in range(97)}
        write_files(chain, {**files, "m97.py": "pass\n"})
        require_success(run([compiler, "check", str(chain / "m1.py")], temporary))
        result = run([compiler, "check", str(chain / "m0.py")], temporary)
        assert result.returncode == 1 and "import nesting exceeds 96" in result.stderr, result
        checks += 2

        for name, (files, expected) in REJECTED.items():
            case = temporary / name
            write_files(case, files)
            result = run([compiler, "check", str(case / "main.py")], temporary)
            assert result.returncode != 0 and expected in result.stderr, (name, result.stdout, result.stderr)
            checks += 1

        collision = temporary / "collisions"
        write_files(collision, {"main.py": IMPORT, "helper.py": HELPER})
        dependency = collision / "helper.py"
        for alias in [dependency, collision / "symlink.py", collision / "hardlink.py"]:
            if alias.name == "symlink.py":
                alias.symlink_to(dependency)
            elif alias.name == "hardlink.py":
                os.link(dependency, alias)
            for output_flag in ["-o", "--emit-llvm"]:
                command = [compiler, "build", str(collision / "main.py"), "--runtime", runtime,
                           "-o", str(temporary / "collision-program"), output_flag, str(alias), "--clang", args.clang]
                command += build_flags
                result = run(command, temporary)
                assert result.returncode != 0 and "distinct paths" in result.stderr, result.stderr
                assert dependency.read_text() == HELPER
                checks += 1

        alias_case = temporary / "module_alias"
        write_files(alias_case, {"main.py": "import helper\nimport alias\n",
                                "helper.py": HELPER + "def label() -> str:\n    return __name__\n"})
        (alias_case / "alias.py").symlink_to(alias_case / "helper.py")
        result = run([compiler, "check", str(alias_case / "main.py")], temporary)
        assert result.returncode != 0 and "multiple module identities" in result.stderr, result.stderr
        checks += 1

        # A single internal symlink keeps its logical import name in both ty and codegen.
        (alias_case / "main.py").write_text("import alias\nprint(alias.label())\nprint(alias.f(4))\n")
        baseline = run([args.python, "-E", "-m", "main"], alias_case)
        require_success(baseline)
        for level in args.opt_levels:
            executable = temporary / f"symlink-program-{level}"
            command = [compiler, "build", str(alias_case / "main.py"), "-o", str(executable),
                       "--runtime", runtime, "--opt-level", str(level), "--clang", args.clang]
            command += build_flags
            if args.sanitize:
                command += ["--sanitize"]
            require_success(run(command, temporary))
            result = run([str(executable)], temporary)
            require_success(result)
            assert result.stdout == baseline.stdout, (result.stdout, baseline.stdout)
            checks += 1

        escape = temporary / "escape"
        write_files(escape, {"main.py": "import helper\n"})
        (escape / "helper.py").symlink_to(dependency)
        result = run([compiler, "check", str(escape / "main.py")], temporary)
        assert result.returncode != 0 and "outside" in result.stderr, result.stderr
        checks += 1
    print(f"{checks} import checks passed")


if __name__ == "__main__":
    main()
