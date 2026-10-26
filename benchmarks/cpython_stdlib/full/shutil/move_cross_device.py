"""Differential control for the real EXDEV fallback, independent of the upstream rename mock."""
import argparse
import errno
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

PROGRAM = '''import os.path
import shutil
import sys
from pathlib import Path

def copy_file(src: str, dst: str) -> str:
    print(f"copy {os.path.basename(src)} {os.path.basename(dst)}")
    return shutil.copy2(src, dst)

def note(src: str, dst: str) -> None:
    print(f"note {os.path.basename(src)} {os.path.basename(dst)}")

def refuse(src: str, dst: str) -> None:
    print(f"refuse {os.path.basename(src)} {os.path.basename(dst)}")
    raise ValueError("stop before removing the source")

def path_types(src: str | Path, dst: str | Path) -> None:
    print(f"path types {isinstance(src, Path)} {isinstance(dst, Path)}")

def source_variant(src: str | Path, dst: str) -> None:
    print(f"source path {isinstance(src, Path)}")

def destination_variant(src: str, dst: str | Path) -> None:
    print(f"destination path {isinstance(dst, Path)}")

def byte_types(src: str | bytes, dst: str | bytes) -> None:
    print(f"bytes {isinstance(src, bytes)} {isinstance(dst, bytes)}")

def remove_source(src: str, dst: str) -> None:
    Path(src).unlink()

def invalid_copier() -> int:
    print("evaluated copier")
    return 42

def same_filename(value: str | bytes | Path | None, expected: str) -> bool:
    if isinstance(value, str):
        return value == expected
    return False

def error_number(value: int | None) -> int:
    if value is None:
        raise AssertionError("OS error has no errno")
    return value

try:
    if sys.argv[3] == "copy":
        shutil.move(sys.argv[1], sys.argv[2], copy_function=copy_file)
    elif sys.argv[3] == "note":
        shutil.move(sys.argv[1], sys.argv[2], copy_function=note)
    elif sys.argv[3] == "path" or sys.argv[3] == "into":
        target = Path(sys.argv[2])
        result = shutil.move(Path(sys.argv[1]), target, copy_function=path_types)
        print(f"returned original path {isinstance(result, Path) and result is target}")
    elif sys.argv[3] == "path-source":
        shutil.move(Path(sys.argv[1]), sys.argv[2], copy_function=source_variant)
    elif sys.argv[3] == "path-destination":
        shutil.move(sys.argv[1], Path(sys.argv[2]), copy_function=destination_variant)
    elif sys.argv[3] == "bytes":
        shutil.move(sys.argv[1].encode(), sys.argv[2].encode(), copy_function=byte_types)
    elif sys.argv[3] == "remove-source":
        shutil.move(sys.argv[1], sys.argv[2], copy_function=remove_source)
    elif sys.argv[3] == "invalid":
        shutil.move(sys.argv[1], sys.argv[2], copy_function=invalid_copier())
    elif sys.argv[3] == "none":
        shutil.move(sys.argv[1], sys.argv[2], copy_function=None)
    elif sys.argv[3] == "protected-parent":
        root = Path(sys.argv[1])
        try:
            shutil.move(root, sys.argv[2])
        except PermissionError as error:
            filename = error.filename
            print(f"root removal {error_number(error.errno)} {isinstance(filename, Path) and filename is root} {error.filename2 is None}")
    else:
        shutil.move(sys.argv[1], sys.argv[2], copy_function=refuse)
except ValueError as error:
    print(str(error))
except TypeError as error:
    print(str(error))
except FileNotFoundError as error:
    print(f"missing source {error_number(error.errno)} {same_filename(error.filename, sys.argv[1])} {error.filename2 is None}")
except FileExistsError as error:
    print(f"existing link target {error_number(error.errno)} {same_filename(error.filename, 'missing')} {same_filename(error.filename2, sys.argv[2])}")
'''

def snapshot(path):
    if path.is_symlink():
        return ("symlink", os.readlink(path))
    if path.is_dir():
        return ("directory", sorted((p.name, snapshot(p)) for p in path.iterdir()))
    if path.is_file():
        return ("file", path.read_bytes())
    return None

def run(program, source_mount, destination_mount, kind, action):
    with TemporaryDirectory(dir=source_mount, prefix="shutil-move-") as src_tmp, TemporaryDirectory(dir=destination_mount, prefix="shutil-move-") as dst_tmp:
        src = Path(src_tmp) / "source"
        dst = Path(dst_tmp) / "result"
        if kind == "file":
            src.write_bytes(b"payload\x00")
        elif kind in ("directory", "empty"):
            src.mkdir()
            if kind == "directory":
                (src/"child").write_bytes(b"child")
        else:
            src.symlink_to("missing")
        if action == "into":
            dst.mkdir()
        elif action == "existing-link-target":
            dst.write_bytes(b"original destination")
        # Demonstrate the kernel error before Imre/CPython gets this untouched pair.
        try:
            os.rename(src, dst / src.name if dst.is_dir() else dst)
        except OSError as e:
            assert e.errno == errno.EXDEV, e
        else:
            raise AssertionError("need two distinct mounts for a real EXDEV")
        if action == "protected-parent":
            os.chmod(src_tmp, 0o555)
        try:
            if action == "protected-parent" and os.access(src_tmp, os.W_OK):
                raise RuntimeError("root removal control requires an unprivileged process")
            p = subprocess.run([*program, src, dst, action], capture_output=True, check=True, timeout=30)
        finally:
            if action == "protected-parent":
                os.chmod(src_tmp, 0o700)
        assert not p.stderr, p.stderr
        return p.stdout.decode(), snapshot(src), snapshot(dst)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-mount", type=Path, default=Path("/tmp"))
    parser.add_argument("--destination-mount", type=Path, default=Path("/dev/shm"))
    parser.add_argument("--oracle-only", action="store_true")
    parser.add_argument("--imre", type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--clang", type=Path, default=Path("/usr/bin/clang-18"))
    args = parser.parse_args()
    assert args.source_mount.stat().st_dev != args.destination_mount.stat().st_dev
    with TemporaryDirectory() as work:
        root = Path(work)
        source = root/"move_callbacks.py"
        source.write_text(PROGRAM)
        cases = [(kind, action) for kind in ("file", "directory") for action in ("copy", "note", "refuse", "path")]
        cases.extend(("file", action) for action in ("into", "path-source", "path-destination", "bytes", "remove-source"))
        cases.extend((("symlink", "refuse"), ("symlink", "existing-link-target")))
        cases.append(("directory", "protected-parent"))
        cases.extend((kind, "invalid") for kind in ("file", "directory", "empty", "symlink"))
        cases.append(("file", "none"))
        def trials(command):
            return {case:run(command, args.source_mount, args.destination_mount, *case) for case in cases}
        reference = trials([sys.executable, "-IB", source])
        assert reference[("file","note")][1:] == (None,None)
        assert reference[("directory","note")][1:] == (None,("directory",[]))
        assert reference[("file","refuse")][1:] == (("file",b"payload\x00"),None)
        assert reference[("symlink","refuse")] == ("",None,("symlink","missing"))
        assert reference["file", "path"][0] == "path types True True\nreturned original path True\n"
        assert reference["file", "into"][0] == "path types True False\nreturned original path False\n"
        assert reference["directory", "path"][0] == "path types False False\nreturned original path True\n"
        assert reference["file", "bytes"][0] == "bytes True True\n"
        assert reference["file", "remove-source"] == (f"missing source {errno.ENOENT} True True\n", None, None)
        assert reference["directory", "protected-parent"] == (
            f"root removal {errno.EACCES} True True\n", ("directory", []),
            ("directory", [("child", ("file", b"child"))]))
        assert reference["symlink", "existing-link-target"] == (
            f"existing link target {errno.EEXIST} True True\n", ("symlink","missing"), ("file", b"original destination"))
        invalid = "evaluated copier\n'int' object is not callable\n"
        assert reference["file", "invalid"] == (invalid, ("file", b"payload\x00"), None)
        assert reference["directory", "invalid"] == (invalid, ("directory", [("child", ("file", b"child"))]), ("directory", []))
        assert reference["empty", "invalid"] == ("evaluated copier\n", None, ("directory", []))
        assert reference["symlink", "invalid"] == ("evaluated copier\n", None, ("symlink", "missing"))
        assert reference["file", "none"] == ("'NoneType' object is not callable\n", ("file", b"payload\x00"), None)
        if args.oracle_only:
            print(reference)
            return
        for level in ("0","3"):
            exe = root / ("native-O"+level)
            subprocess.run([args.imre, "build", source, "--no-cache", "--runtime", args.runtime, "--clang", args.clang, "--opt-level", level, "-o", exe], check=True)
            actual = trials([exe])
            assert actual == reference, [(case, actual[case], ref) for case,ref in reference.items() if actual[case] != ref]
            print(f"O{level}: {len(cases)} real EXDEV callback, failure and symlink controls matched")

if __name__ == "__main__":
    main()
