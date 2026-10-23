"""Compare sealed POSIX paths, directory consumption, and filesystem effects."""

import argparse
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

if sys.flags.optimize:
    raise SystemExit("validation requires Python assertions")

sys.path.insert(0, str(Path(__file__).resolve().parent))
import files

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "paths"
IMPORT = "from pathlib import Path\n"
REJECTED = {
    "integer_segment": "Path(1)",
    "bytes_segment": "Path(b'file')",
    "keyword_segment": "Path(path='file')",
    "expanded_segments": "Path(*['a', 'b'])",
    "wrong_join": "Path('a') / 1",
    "wrong_annotation": "value: Path = 'a'",
    "wrong_list_element": "values: list[Path] = [Path('a'), 'b']",
    "wrong_field_write": "class Item:\n    path: Path\n    def __init__(self, path: Path) -> None:\n        self.path = path\nitem = Item(Path('a'))\nitem.path = 'b'",
    "optional_access": "value: Path | None = None\nprint(value.name)",
    "mutable_attribute": "value = Path('a')\nvalue.name = 'b'",
    "unknown_attribute": "print(Path('a').drive)",
    "path_index": "print(Path('a')[0])",
    "path_length": "print(len(Path('a')))",
    "path_order": "print(Path('a') < Path('b'))",
    "mixed_path_set": "values = {Path('a'), 'a'}",
    "mixed_path_dict_key": "values = {Path('a'): 1, 'a': 2}",
    "path_constructor_value": "constructor = Path\nprint(constructor('a'))",
    "subclass": "class Custom(Path):\n    pass",
    "escaped_iterdir": "values = Path('.').iterdir()",
    "escaped_glob": "values = Path('.').glob('*.json')",
    "reversed_iterator": "for value in reversed(Path('.').iterdir()):\n    print(value)",
    "iterdir_arguments": "for value in Path('.').iterdir(1):\n    print(value)",
    "glob_dynamic_pattern": "pattern = '*.json'\nfor value in Path('.').glob(pattern):\n    print(value)",
    "glob_empty": "for value in Path('.').glob(''):\n    print(value)",
    "glob_dot": "for value in Path('.').glob('.'):\n    print(value)",
    "glob_recursive": "for value in Path('.').glob('**'):\n    print(value)",
    "glob_nested": "for value in Path('.').glob('sub/*.json'):\n    print(value)",
    "glob_trailing_separator": "for value in Path('.').glob('*/'):\n    print(value)",
    "glob_absolute": "for value in Path('.').glob('/tmp'):\n    print(value)",
    "glob_options": "for value in Path('.').glob('*', case_sensitive=False):\n    print(value)",
    "predicate_options": "print(Path('.').exists(follow_symlinks=False))",
    "read_default_encoding": "Path('a').read_text()",
    "write_default_encoding": "Path('a').write_text('data')",
    "read_encoding": "Path('a').read_text(encoding='latin-1')",
    "read_dynamic_encoding": "encoding = 'utf-8'\nPath('a').read_text(encoding=encoding)",
    "write_bytes": "Path('a').write_text(b'data', encoding='utf-8')",
    "read_errors": "Path('a').read_text(encoding='utf-8', errors='ignore')",
    "write_newline": "Path('a').write_text('data', encoding='utf-8', newline='')",
    "read_argument": "Path('a').read_text(1, encoding='utf-8')",
    "unsupported_method": "Path('.').expanduser()",
    "resolve_integer_strict": "Path('.').resolve(strict=1)",
    "resolve_optional_strict": "strict: bool | None = None\nPath('.').resolve(strict=strict)",
    "resolve_extra_argument": "Path('.').resolve(True, False)",
    "resolve_extra_keyword": "Path('.').resolve(missing_ok=True)",
    "resolve_duplicate_strict": "Path('.').resolve(True, strict=False)",
    "absolute_argument": "Path('.').is_absolute(True)",
    "builtin_class_pattern": "match Path('a'):\n    case Path():\n        print(1)",
    "unsupported_import": "from pathlib import PureWindowsPath\nPureWindowsPath('a')",
}

# Native paths retain Relar's Unicode-scalar string domain. Unix surrogateescape
# names cannot be represented; exercise the explicit boundary and retry cleanup.
INVALID_NAMES = """from pathlib import Path
for turn in range(4):
    try:
        for path in Path('invalid-names').iterdir():
            print('unexpected path')
    except UnicodeDecodeError:
        print('invalid directory entry')
    try:
        for path in Path('invalid-names').glob('*.json'):
            print('unexpected glob path')
    except UnicodeDecodeError:
        print('invalid glob entry')
print(Path('input.txt').is_file())
"""


def seed(directory: Path) -> None:
    directory.mkdir()
    for name, data in {
        "input.txt": "alpha\r\néλ🙂\rzero\0tail\n".encode(),
        "empty.txt": b"",
        "invalid.txt": b"valid prefix\xffinvalid",
        "sentinel.txt": b"preserve this output",
        "output.txt": b"truncate old data completely",
        "denied.txt": b"secret",
    }.items():
        (directory / name).write_bytes(data)
    tree = directory / "tree"
    tree.mkdir()
    for name in ["a.json", "b.json", "c.json", "z.json", "λ.json", "α🙂.json", ".hidden.json", "[a].json", "literal[.json", "upper.JSON", "note.txt"]:
        (tree / name).write_text(name, encoding="utf-8")
    (tree / "nested.json").mkdir()
    (tree / "nested.json" / "inside.json").write_text("ignored descendant")
    (tree / "dangling.json").symlink_to("missing-target")
    (directory / "empty-directory").mkdir()
    (directory / "nested" / "only").mkdir(parents=True)
    (directory / "nested" / "only" / "child.json").write_text("nested")
    for name in ["eager", "lazy", "sorted-eager", "sorted-lazy", "mutate-eager", "mutate-lazy"]:
        (directory / name).mkdir()
        (directory / name / "old.json").write_text("old")
    (directory / "file-link").symlink_to("input.txt")
    (directory / "directory-link").symlink_to("tree", target_is_directory=True)
    (directory / "dangling-link").symlink_to("missing")
    (directory / "loop-a").symlink_to("loop-b")
    (directory / "loop-b").symlink_to("loop-a")
    for index in range(64):
        (directory / f"chain-{index}").symlink_to(
            "input.txt" if index == 63 else f"chain-{index + 1}"
        )
    (directory / "repeated-link").symlink_to("directory-link/../directory-link")
    (directory / "trailing-link").symlink_to("input.txt/")
    (directory / "absolute-link").symlink_to("/tmp", target_is_directory=True)
    (directory / "denied").mkdir()
    (directory / "denied" / "secret.json").write_text("secret")
    (directory / "denied").chmod(0)
    (directory / "denied.txt").chmod(0)


def snapshot(directory: Path) -> dict:
    """Capture exact bytes, directory membership, and symlink destinations."""
    # Restore only the fixture's deliberate access barriers after execution.
    (directory / "denied").chmod(0o700)
    (directory / "denied.txt").chmod(0o600)
    result = {}
    for parent, directories, names in os.walk(directory, followlinks=False):
        for name in directories + names:
            path = Path(parent) / name
            mode = path.lstat().st_mode
            value = ("link", os.readlink(path)) if stat.S_ISLNK(mode) else (
                ("directory", None) if stat.S_ISDIR(mode) else ("file", path.read_bytes())
            )
            result[str(path.relative_to(directory))] = value
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--opt-levels", nargs="+", choices=["0", "1", "2", "3"], default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    assert len(set(args.opt_levels)) == len(args.opt_levels)
    compiler = str(args.compiler.resolve())
    flags = ["--quiet", "--runtime", str(args.runtime.resolve()), "--clang", args.clang]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags += ["--sanitize"]
    checks = 0
    version = files.run([args.python, "-I", "-c", "import sys; print(sys.version_info >= (3, 14))"], FIXTURES)
    files.check_result(version, None, "CPython version")
    assert version.stdout.strip() == b"True", "path validation requires a CPython 3.14+ oracle; pass --python"
    with tempfile.TemporaryDirectory(prefix="relar-paths-") as directory:
        temporary = Path(directory)
        for name, source in REJECTED.items():
            path = temporary / f"reject-{name}.py"
            path.write_text(IMPORT + source + "\n", encoding="utf-8")
            result = files.run([compiler, "check", str(path)], temporary)
            assert result.returncode > 0 and result.stderr, (name, result)
            assert b"panicked" not in result.stderr and b"Sanitizer" not in result.stderr, (name, result.stderr)
            checks += 1
        shadow = temporary / "shadow"
        shadow.mkdir()
        (shadow / "pathlib.py").write_text("def value() -> int:\n    return 1\n")
        (shadow / "main.py").write_text(IMPORT + "print(Path('a'))\n")
        result = files.run([compiler, "check", str(shadow / "main.py")], temporary)
        assert result.returncode > 0 and b"standard-library" in result.stderr, result.stderr
        checks += 1
        sources = [*sorted(FIXTURES.glob("*.py")), FIXTURES / "imported/main.py"]
        for source in sources:
            name = source.stem if source.parent == FIXTURES else "imported"
            runner = files.run_with_failing_output if name == "failures" else subprocess.run
            project = temporary / name
            project.mkdir()
            reference_dir = project / "cpython"
            seed(reference_dir)
            reference = files.run([args.python, "-E", "-B", str(source)], reference_dir, limit_descriptors=name == "cleanup", runner=runner)
            files.check_result(reference, None, f"CPython {name}")
            stdout = reference.stdout
            expected_files = snapshot(reference_dir)
            for level in args.opt_levels:
                binary = project / f"native-O{level}"
                built = files.run([compiler, "build", str(source), "--module-root", str(source.parent), "-o", str(binary), "--opt-level", level, *flags], temporary)
                assert built.returncode == 0 and not built.stderr, (name, level, built.stderr.decode())
                native_dir = project / f"relar-O{level}"
                seed(native_dir)
                actual = files.run([str(binary)], native_dir, limit_descriptors=name == "cleanup", runner=runner)
                files.check_result(actual, None, f"Relar {name} O{level}")
                assert actual.stdout == stdout, (name, level, actual.stdout, stdout)
                assert snapshot(native_dir) == expected_files, (name, level, "filesystem effects differ")
                checks += 1
        source = temporary / "invalid_names.py"
        source.write_text(INVALID_NAMES)
        bad = temporary / "invalid-names"
        bad.mkdir()
        if files.write_non_utf8_filename(os.fsencode(bad) + b"/\xff.json"):
            (temporary / "input.txt").touch()
            for level in args.opt_levels:
                binary = temporary / f"invalid-names-O{level}"
                built = files.run([compiler, "build", str(source), "-o", str(binary), "--opt-level", level, *flags], temporary)
                assert built.returncode == 0 and not built.stderr, built.stderr.decode()
                actual = files.run([str(binary)], temporary, limit_descriptors=True)
                files.check_result(actual, None, "invalid filename boundary")
                assert actual.stdout == (b"invalid directory entry\ninvalid glob entry\n" * 4 + b"True\n"), actual.stdout
                checks += 1
    print(f"Passed {checks} path checks ({len(REJECTED) + 1} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize}; CPython 3.14+ oracle)")


if __name__ == "__main__":
    main()
