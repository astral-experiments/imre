#!/usr/bin/env python3
"""Compare scoped workspaces and path mutations with CPython 3.14+."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile

if sys.flags.optimize:
    raise SystemExit("workspace validation requires assertions; do not use python -O")

sys.path.insert(0, str(Path(__file__).resolve().parent))
import files

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "workspaces"
IMPORTS = "from pathlib import Path\nfrom tempfile import TemporaryDirectory\n"
REJECTED = {
    "nullcontext_integer": "from contextlib import nullcontext\nwith nullcontext(1):\n    pass",
    "nullcontext_two_args": "from contextlib import nullcontext\nwith nullcontext('a', 'b'):\n    pass",
    "constructor_value": "factory = TemporaryDirectory",
    "manager_annotation": "def use(value: TemporaryDirectory) -> None:\n    pass",
    "manual_enter": "print(TemporaryDirectory().__enter__())",
    "manual_cleanup": "TemporaryDirectory().cleanup()",
    "manager_attribute": "print(TemporaryDirectory().name)",
    "positional_directory": "with TemporaryDirectory('suffix') as name:\n    print(name)",
    "delete_option": "with TemporaryDirectory(delete=False):\n    pass",
    "ignore_cleanup_option": "with TemporaryDirectory(ignore_cleanup_errors=True):\n    pass",
    "unknown_option": "with TemporaryDirectory(encoding='utf-8'):\n    pass",
    "wrong_directory": "with TemporaryDirectory(dir=1):\n    pass",
    "bytes_directory": "with TemporaryDirectory(dir=b'.'):\n    pass",
    "wrong_prefix": "with TemporaryDirectory(prefix=1):\n    pass",
    "nullable_prefix": "with TemporaryDirectory(prefix=None):\n    pass",
    "wrong_suffix": "with TemporaryDirectory(suffix=Path('tail')):\n    pass",
    "computed_keywords": "with TemporaryDirectory(**{str('prefix'): 'tmp'}):\n    pass",
    "unpacked_target": "with TemporaryDirectory() as (left, right):\n    pass",
    "changed_target_type": "name = Path('.')\nwith TemporaryDirectory() as name:\n    pass",
    "mkdir_wrong_mode": "Path('a').mkdir(mode='700')",
    "mkdir_bool_mode": "Path('a').mkdir(mode=True)",
    "mkdir_wrong_parents": "Path('a').mkdir(parents='yes')",
    "mkdir_wrong_exist_ok": "Path('a').mkdir(exist_ok=1)",
    "mkdir_extra_argument": "Path('a').mkdir(0o700, True, False, True)",
    "mkdir_duplicate": "Path('a').mkdir(0o700, mode=0o755)",
    "mkdir_unknown_keyword": "Path('a').mkdir(follow_symlinks=False)",
    "unlink_wrong_flag": "Path('a').unlink(missing_ok='yes')",
    "unlink_extra_argument": "Path('a').unlink(True, False)",
    "unlink_unknown_keyword": "Path('a').unlink(parents=True)",
    "rmdir_argument": "Path('a').rmdir(True)",
    "rmdir_keyword": "Path('a').rmdir(missing_ok=True)",
    "replace_missing_target": "Path('a').replace()",
    "replace_wrong_target": "Path('a').replace(1)",
    "replace_optional_target": "def move(target: Path | None) -> Path:\n    return Path('a').replace(target)",
    "replace_duplicate": "Path('a').replace('b', target='c')",
    "replace_unknown_keyword": "Path('a').replace(destination='b')",
    "unsupported_factory": "from tempfile import mkdtemp\nprint(mkdtemp())",
}

ENVIRONMENTS = {
    "default": {},
    "empty_primary": {"TMPDIR": ""},
    "missing_primary": {"TMPDIR": "missing-temp"},
    "file_primary": {"TMPDIR": "plain-file"},
    "denied_primary": {"TMPDIR": "denied"},
    "third_choice": {"TMPDIR": "", "TEMP": ""},
    "relative_primary": {"TMPDIR": "relative-temp"},
}


def seed(directory: Path) -> None:
    directory.mkdir()
    for name in ("scratch", "temp-default", "temp-second", "temp-third", "relative-temp", "existing", "empty", "nonempty", "outside", "readonly-tree", "readonly-tree/blocked", "denied"):
        (directory / name).mkdir()
    for name, content in {
        "plain-file": b"plain file",
        "sentinel.txt": b"preserve this file",
        "source.txt": "source\n\u03bb\x00tail".encode(),
        "destination.txt": b"old destination",
        "nonempty/child.txt": b"nonempty",
        "outside/keep.txt": b"outside the workspace",
        "readonly-tree/blocked/locked.txt": b"read-only descendant",
        "hard-a": b"shared inode",
    }.items():
        (directory / name).write_bytes(content)
    os.link(directory / "hard-a", directory / "hard-b")
    for name, target in {
        "file-link": "plain-file",
        "directory-link": "existing",
        "dangling-link": "absent-target",
        "nested-link": "../../outside",
        "root-symlink": "../outside",
    }.items():
        (directory / name).symlink_to(target)
    invalid = os.fsencode(directory / "readonly-tree" / "blocked") + b"/invalid-\xff"
    files.write_non_utf8_filename(invalid)
    (directory / "outside" / "keep.txt").chmod(0o400)
    (directory / "outside").chmod(0o500)
    (directory / "readonly-tree" / "blocked" / "locked.txt").chmod(0)
    (directory / "readonly-tree" / "blocked").chmod(0)
    (directory / "denied").chmod(0)


def snapshot(directory: Path) -> dict:
    """Read every entry without following links or hiding permission changes."""
    entries = {}
    inodes = {}

    def visit(path: Path, relative: str) -> None:
        info = path.lstat()
        mode = stat.S_IMODE(info.st_mode)
        if stat.S_ISLNK(info.st_mode):
            entries[relative] = ("link", mode, os.readlink(path))
            return
        changed = False
        try:
            if stat.S_ISDIR(info.st_mode):
                entries[relative] = ("directory", mode)
                if mode & 0o500 != 0o500:
                    path.chmod(mode | 0o700)
                    changed = True
                for name in sorted(os.listdir(path)):
                    visit(path / name, f"{relative}/{name}" if relative else name)
            else:
                assert stat.S_ISREG(info.st_mode), (relative, info.st_mode)
                if not mode & 0o400:
                    path.chmod(mode | 0o400)
                    changed = True
                entries[relative] = ("file", mode, path.read_bytes())
                inodes.setdefault((info.st_dev, info.st_ino), []).append(relative)
        finally:
            if changed:
                path.chmod(mode)

    visit(directory, "")
    return {"entries": entries, "hardlinks": sorted(sorted(names) for names in inodes.values() if len(names) > 1)}


def child_setup() -> None:
    files.descriptor_limit()
    os.umask(0o027)


def run(command: list[str], directory: Path, scenario: str = "default"):
    environment = files.environment()
    environment.update(
        TMPDIR=str(directory / "temp-default"),
        TEMP=str(directory / "temp-second"),
        TMP=str(directory / "temp-third"),
        PYTHONHASHSEED="0",
    )
    environment.update(ENVIRONMENTS[scenario])
    return subprocess.run(command, cwd=directory, input=b"", capture_output=True,
                          timeout=60, check=False, env=environment, preexec_fn=child_setup)


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
    version = files.run([args.python, "-I", "-c", "import sys; print(sys.version_info >= (3, 14))"], FIXTURES)
    files.check_result(version, None, "CPython version")
    assert version.stdout.strip() == b"True", "workspace validation requires a CPython 3.14+ oracle; pass --python"
    assert os.geteuid() != 0, "permission cases require an unprivileged user"
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-workspaces-") as name:
        temporary = Path(name)
        for label, source in REJECTED.items():
            path = temporary / f"reject-{label}.py"
            path.write_text(IMPORTS + source + "\n", encoding="utf-8")
            result = files.run([compiler, "check", str(path)], temporary)
            assert result.returncode > 0 and result.stderr, (label, result)
            assert b"panicked" not in result.stderr and b"Sanitizer" not in result.stderr, (label, result.stderr)
            checks += 1
        shadow = temporary / "shadow"
        shadow.mkdir()
        (shadow / "tempfile.py").write_text("def TemporaryDirectory() -> str:\n    return 'local'\n")
        (shadow / "main.py").write_text("from tempfile import TemporaryDirectory\nwith TemporaryDirectory() as name:\n    print(name)\n")
        result = files.run([compiler, "check", str(shadow / "main.py")], temporary)
        assert result.returncode > 0 and b"standard-library" in result.stderr, result.stderr
        checks += 1
        sources = [*sorted(FIXTURES.glob("*.py")), FIXTURES / "imported" / "main.py"]
        for source in sources:
            label = source.stem if source.parent == FIXTURES else "imported"
            project = temporary / label
            project.mkdir()
            scenarios = ENVIRONMENTS if label == "environment" else ["default"]
            references = {}
            for scenario in scenarios:
                reference_dir = project / f"cpython-{scenario}"
                seed(reference_dir)
                reference = run([args.python, "-E", "-B", str(source)], reference_dir, scenario)
                files.check_result(reference, None, f"CPython {label}/{scenario}")
                references[scenario] = (reference.stdout, snapshot(reference_dir))
            for level in args.opt_levels:
                binary = project / f"native-O{level}"
                built = files.run([compiler, "build", str(source), "--module-root", str(source.parent),
                                   "-o", str(binary), "--opt-level", level, *flags], temporary)
                assert built.returncode == 0 and not built.stderr, (label, level, built.stderr.decode())
                for scenario in scenarios:
                    native_dir = project / f"imre-O{level}-{scenario}"
                    seed(native_dir)
                    actual = run([str(binary)], native_dir, scenario)
                    files.check_result(actual, None, f"Imre {label}/{scenario} O{level}")
                    stdout, expected = references[scenario]
                    assert actual.stdout == stdout, (label, scenario, level, actual.stdout, stdout)
                    observed = snapshot(native_dir)
                    assert observed == expected, (label, scenario, level, "filesystem effects differ", observed, expected)
                    checks += 1
    print(f"Passed {checks} workspace checks ({len(REJECTED) + 1} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize}; CPython 3.14+ oracle)")


if __name__ == "__main__":
    main()
