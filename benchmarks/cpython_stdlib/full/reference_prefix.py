"""Make a disposable Linux CPython prefix whose own test package is the vendored tree."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import sysconfig

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from stage_tests import stage


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("prefix", type=Path, help="new or empty directory")
    args = parser.parse_args()
    tag = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())["tag"]
    if sys.platform != "linux" or sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != tag:
        parser.error(f"use a test-enabled Linux CPython {tag}")
    import _testcapi

    prefix = args.prefix.resolve()
    prefix.mkdir(parents=True, exist_ok=True)
    if any(prefix.iterdir()):
        parser.error("the disposable prefix must be empty")
    lib = prefix / sys.platlibdir
    count = stage(lib)
    suffix = "t" if sysconfig.get_config_var("Py_GIL_DISABLED") else ""
    version = f"python{sys.version_info.major}.{sys.version_info.minor}{suffix}"
    stdlib = lib / version
    (lib / "Lib").rename(stdlib)  # Keep Tools beside the stdlib as test.support expects.
    # Leave the caller's Python installation untouched, including when a child
    # can write bytecode. Never copy the host's tests or third-party packages.
    ignore = shutil.ignore_patterns("__pycache__", "*.pyc")
    for src in Path(os.__file__).resolve().parent.iterdir():
        if src.name in {"test", "site-packages", "dist-packages", "__pycache__", "lib-dynload"}:
            continue
        if src.is_dir():
            shutil.copytree(src, stdlib / src.name, ignore=ignore)
        elif src.is_file():
            shutil.copy2(src, stdlib / src.name)
    # A source build can symlink extension files into its import directory.
    # Copy that directory, including its generated sysconfig data.
    shutil.copytree(Path(_testcapi.__file__).parent.resolve(), stdlib / "lib-dynload", ignore=ignore)
    binary = prefix / "bin" / version
    binary.parent.mkdir()
    shutil.copy2(sys.executable, binary)
    if sysconfig.get_config_var("Py_ENABLE_SHARED"):
        for name in {sysconfig.get_config_var(k) for k in ("LDLIBRARY", "INSTSONAME")} - {None, ""}:
            for directory in (Path(sysconfig.get_config_var("LIBDIR")), Path(sys.executable).resolve().parent):
                if (source := directory / name).is_file():
                    shutil.copy2(source, lib / name)
                    break
    # -E is the flag used by an upstream unittest child: it ignores PYTHONPATH.
    check = ("import pathlib,sys,sysconfig,test,_testcapi; "
             "assert pathlib.Path(test.__file__).resolve() == pathlib.Path(sys.argv[1]); "
             "assert pathlib.Path(sysconfig.get_path('stdlib')).resolve() == pathlib.Path(sys.argv[1]).parents[1]")
    subprocess.run([binary, "-E", "-B", "-c", check, stdlib / "test/__init__.py"], check=True)
    print(f"Staged {count} verified test files. Run: {binary} -B -m test")


if __name__ == "__main__":
    main()
