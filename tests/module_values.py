"""Compare fixed module values and ordered initialization with CPython."""

import argparse
import shutil
import sys
import tempfile
from pathlib import Path

# Validation also runs under -I; use only the trusted sibling harness directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assignments import reference, require_result
from comprehensions import run as run_build, snapshot
from imports import require_success, run, write_files

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "module_values"
ERRORS = {"initialization_error": "ValueError", "entry_error": "ValueError"}
REJECTED = {
    "early_namedtuple_imported": {
        "library.py": 'from collections import namedtuple\ndef early() -> str:\n    return Late("before").value\nVALUE = early()\nLate = namedtuple("Late", "value")\n',
    },
    "entry_call_before_definition": {"main.py": "VALUE = make()\ndef make() -> int:\n    return 1\n"},
    "entry_early_read": {"main.py": "def read() -> int:\n    return SECOND\nFIRST = read()\nSECOND = 2\ndef last() -> None:\n    pass\n"},
    "early_class_entry_enum": {
        "main.py": 'from enum import StrEnum\ndef read() -> str:\n    return Later.ITEM.value\nVALUE = read()\nclass Later(StrEnum):\n    ITEM = "item"\nprint(VALUE)\n',
    },
    "early_class_imported_comprehension": {
        "library.py": "from dataclasses import dataclass\ndef read() -> list[bool]:\n    return [isinstance(item, Later) for item in range(2)]\nVALUE = read()\n@dataclass\nclass Later:\n    value: int\n",
    },
    "early_class_imported_pattern": {
        "library.py": "from dataclasses import dataclass\ndef matches(item: int) -> bool:\n    match item:\n        case Later():\n            return True\n    return False\ndef relay() -> bool:\n    return matches(1)\nVALUE = relay()\n@dataclass\nclass Later:\n    value: int\n",
    },
    "entry_import_after_value": {"main.py": "VALUE = 1\nimport os\ndef last() -> None:\n    pass\n"},
    "duplicate_binding": {"library.py": "VALUE = 1\nVALUE = 2\n"},
    "annotation_without_initializer": {"library.py": "VALUE: int\n"},
    "direct_early_read": {"library.py": "FIRST = SECOND\nSECOND = 2\n"},
    "early_function_read": {
        "library.py": "def read() -> int:\n    return VALUE\nFIRST = read()\nVALUE = 2\n"
    },
    "transitive_early_read": {
        "library.py": "def read() -> int:\n    return VALUE\ndef relay() -> int:\n    return read()\nFIRST = relay()\nVALUE = 2\n"
    },
    "comprehension_early_read": {
        "library.py": "def read() -> list[int]:\n    return [VALUE for index in range(2)]\nFIRST = read()\nVALUE = 2\n"
    },
    "constructor_early_read": {
        "library.py": "class Reader:\n    value: int\n    def __init__(self) -> None:\n        self.value = VALUE\nFIRST = Reader()\nVALUE = 2\n"
    },
    "method_early_read": {
        "library.py": "class Reader:\n    value: int\n    def __init__(self) -> None:\n        self.value = 0\n    def read(self) -> int:\n        return VALUE\nINSTANCE = Reader()\nFIRST = INSTANCE.read()\nVALUE = 2\n"
    },
    "call_before_definition": {
        "library.py": "VALUE = make()\ndef make() -> int:\n    return 1\n"
    },
    "transitive_late_definition": {
        "library.py": "def first() -> int:\n    return second()\nVALUE = first()\ndef second() -> int:\n    return 1\n"
    },
    "wrong_annotation": {"library.py": 'VALUE: int = "wrong"\n'},
    "wrong_mutation": {
        "library.py": 'VALUES = {"a": 1}\ndef change() -> None:\n    VALUES["a"] = "wrong"\n'
    },
    "local_shadow": {
        "library.py": "VALUE = 1\ndef change() -> int:\n    VALUE = 2\n    return VALUE\n"
    },
    "global_rebinding": {
        "library.py": "VALUE = 1\ndef change() -> None:\n    global VALUE\n    VALUE = 2\n"
    },
    "qualified_rebinding": {
        "main.py": "import library\nlibrary.VALUE = 2\n",
        "library.py": "VALUE = 1\n",
    },
    "import_after_value": {
        "library.py": "VALUE = 1\nimport dependency\n",
        "dependency.py": "VALUE = 2\n",
    },
    "conditional_export": {
        "main.py": "from library import VALUE\n",
        "library.py": "if True:\n    VALUE = 1\n",
    },
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable, help="CPython 3.14 reference executable")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument(
        "--opt-levels", nargs="+", choices=["0", "1", "2", "3"], default=["0", "3"]
    )
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    version = run(
        [
            args.python, "-I", "-c",
            "import platform; print(platform.python_implementation(), platform.python_version())",
        ],
        Path.cwd(),
    )
    require_success(version)
    if not version.stdout.startswith("CPython 3.14."):
        parser.error(f"--python must select CPython 3.14; {args.python} reported {version.stdout.strip()}")
    compiler, runtime = str(args.compiler.resolve()), str(args.runtime.resolve())
    flags = []
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags.append("--sanitize")
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-module-values-") as directory:
        temporary = Path(directory)
        project = temporary / "project"
        shutil.copytree(FIXTURES, project, ignore=shutil.ignore_patterns("__pycache__"))
        for case in sorted(project.iterdir()):
            source = case / "main.py"
            error = ERRORS.get(case.name)
            expected_dir = temporary / case.name / "reference"
            expected_dir.mkdir(parents=True)
            expected = reference(args.python, source, expected_dir, error)
            expected_files = snapshot(expected_dir)
            for level in args.opt_levels:
                actual_dir = temporary / case.name / f"O{level}"
                actual_dir.mkdir()
                binary = actual_dir / "program"
                require_success(
                    run_build(
                        [
                            compiler,
                            "build",
                            str(source),
                            "--module-root",
                            str(case),
                            "--runtime",
                            runtime,
                            "--clang",
                            args.clang,
                            "--opt-level",
                            level,
                            "-o",
                            str(binary),
                            *flags,
                        ],
                        temporary,
                        timeout=300 if args.lto_linker else 60,
                    )
                )
                actual = run([str(binary)], actual_dir)
                require_result(actual, error)
                binary.unlink()
                assert actual.stdout == expected.stdout, (
                    case.name,
                    level,
                    expected.stdout,
                    actual.stdout,
                )
                assert snapshot(actual_dir) == expected_files, (
                    case.name,
                    level,
                    expected_files,
                    snapshot(actual_dir),
                )
                checks += 1
        for name, files in REJECTED.items():
            case = temporary / f"reject-{name}"
            write_files(case, {"main.py": "import library\n", **files})
            actual = run([compiler, "check", str(case / "main.py")], temporary)
            assert actual.returncode != 0, (name, actual.stdout, actual.stderr)
            assert "panicked" not in actual.stderr, (name, actual.stderr)
            if name.startswith("early_class_"):
                assert "references a class before it is available" in actual.stderr, actual.stderr
            checks += 1
    print(
        f"Passed {checks} module value checks ({len(REJECTED)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()
